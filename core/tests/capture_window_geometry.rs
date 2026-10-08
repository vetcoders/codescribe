//! Integrator-owned tests of the actual planner, retention ring and provider
//! request contract. Synthetic PCM proves geometry, not ASR accuracy or latency.
mod stt {
    pub mod tail_provider {
        pub use codescribe_core::stt::tail_provider::TailSampleRange;
    }
}

#[path = "../pipeline/streaming/layer1_window.rs"]
mod capture_window_plan;
#[path = "../pipeline/streaming/live_audio_buffer.rs"]
mod live_audio_buffer;

use capture_window_plan::CaptureWindowPlan;
use codescribe_core::stt::tail_provider::{TailProviderRequest, TailRequestIdentity};
use live_audio_buffer::LiveAudioBuffer;

#[test]
fn planned_frames_keep_original_pcm_including_silence_at_every_capture_rate() {
    for rate in [100_u32, 16_000, 44_100, 48_000] {
        let r = rate as usize;
        let pcm = (0..25 * r)
            .map(|i| {
                if (5 * r..7 * r).contains(&i) {
                    0.0
                } else {
                    i as f32 / r as f32
                }
            })
            .collect::<Vec<_>>();
        let mut ring = LiveAudioBuffer::new(rate, 30.0);
        let mut plan = CaptureWindowPlan::new("sample-clock".into(), 4, rate);
        let mut requests = Vec::new();
        for chunk in pcm.chunks(1024) {
            ring.push(chunk);
            while let Some(range) = plan.next_due(ring.session_sample_end(), false) {
                let window = ring
                    .window_by_samples(range.sample_start, range.sample_end)
                    .unwrap();
                assert_eq!(window.sample_start, range.sample_start);
                assert_eq!(window.sample_end, range.sample_end);
                assert_eq!(
                    window.samples,
                    pcm[range.sample_start as usize..range.sample_end as usize]
                );
                let request = TailProviderRequest {
                    identity: TailRequestIdentity {
                        request_id: requests.len() as u64 + 1,
                        range: range.clone(),
                    },
                    sample_rate: rate,
                    language: None,
                };
                request.validate_pcm(&window.samples).unwrap();
                assert!(plan.account(&range));
                requests.push(request);
            }
        }
        while let Some(range) = plan.next_due(ring.session_sample_end(), true) {
            let window = ring
                .window_by_samples(range.sample_start, range.sample_end)
                .unwrap();
            assert_eq!(
                window.samples,
                pcm[range.sample_start as usize..range.sample_end as usize]
            );
            let request = TailProviderRequest {
                identity: TailRequestIdentity {
                    request_id: requests.len() as u64 + 1,
                    range: range.clone(),
                },
                sample_rate: rate,
                language: None,
            };
            request.validate_pcm(&window.samples).unwrap();
            assert!(plan.account(&range));
            requests.push(request);
        }
        assert_eq!(requests.len(), 7);
        assert!(plan.is_finished());
        for i in [0, 5 * r, 6 * r, 9 * r, 18 * r, 25 * r - 1] {
            let visits = requests
                .iter()
                .filter(|request| {
                    let range = &request.identity.range;
                    range.sample_start <= i as u64 && (i as u64) < range.sample_end
                })
                .count();
            assert!((1..=3).contains(&visits));
        }
    }
}

#[test]
fn evicted_pcm_is_refused_without_cropping_the_outstanding_plan() {
    let mut ring = LiveAudioBuffer::new(100, 9.0);
    let mut plan = CaptureWindowPlan::new("eviction".into(), 1, 100);
    ring.push(&vec![0.25; 900]);
    let offered = plan.next_due(ring.session_sample_end(), false).unwrap();
    ring.push(&vec![0.25; 300]);
    assert_eq!(ring.retained_start_sample(), 300);
    assert!(
        ring.window_by_samples(offered.sample_start, offered.sample_end)
            .is_none()
    );
    assert_eq!(
        plan.next_due(ring.session_sample_end(), false),
        Some(offered.clone())
    );
    assert!(plan.account(&offered)); // Explicit unavailable-work receipt consumes the offer.
    let next = plan.next_due(ring.session_sample_end(), false).unwrap();
    assert_eq!((next.sample_start, next.sample_end), (300, 1200));
    let window = ring
        .window_by_samples(next.sample_start, next.sample_end)
        .unwrap();
    assert_eq!(window.samples.len(), 900);
}
