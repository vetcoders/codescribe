//! A derived ASR view of a quiet microphone, never capture evidence.
//!
//! Raw PCM remains authoritative for calibration, energy, archives and sample
//! identity. Only an admitted, quiet 48 kHz capture profile selects this view.
//! RNNoise runs on one CPU thread outside CoreAudio and the event-drain loop.
//! Its first 480-sample output is discarded; subsequent outputs correspond to
//! the preceding input frame. EOF flushes the real final frame, trimming all
//! analysis padding. The output clock is therefore the original capture clock,
//! with 10–20 ms of framing/lookahead latency and no synthetic occurrences.

use std::io::BufWriter;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::thread;

use nnnoiseless::DenoiseState;
use tokio::sync::mpsc;
use tracing::info;

use crate::config::RuntimeSettingsSnapshot;
use crate::pipeline::streaming::live_audio_buffer::ResolvedAudioWindow;

const FRAME: usize = DenoiseState::FRAME_SIZE;
const RATE: u32 = 48_000;
const QUEUE_FRAMES: usize = 16;
// Temporary projection storage has its own hard resource fence. Reaching it
// refuses projection explicitly and preserves the recorder-owned raw archive.
const MAX_SPOOL_SAMPLES: u64 = (2 * 1024 * 1024 * 1024) / 4;
const QUIET_PROFILE_DBFS: f32 = -42.0;
const TARGET_DBFS: f32 = -30.0;
const MAX_GAIN_DB: f32 = 18.0;
const PEAK_HEADROOM_DBFS: f32 = -6.0;
// At maximum boost, even a wholly suppressed band keeps approximately its
// original amplitude. No VAD probability is allowed to gate a syllable away.
const DRY_SHARE: f32 = 0.125;

/// A fixed session policy derived from the one immutable settings snapshot.
pub(crate) struct SpeechInputPlan {
    gain: Option<f32>,
    disposition: &'static str,
}

impl SpeechInputPlan {
    pub(crate) fn is_enhanced(&self) -> bool {
        self.gain.is_some()
    }

    pub(crate) fn for_capture(
        settings: &RuntimeSettingsSnapshot,
        device: Option<&str>,
        sample_rate: u32,
    ) -> Self {
        let bypass = |disposition| Self {
            gain: None,
            disposition,
        };
        if sample_rate != RATE {
            return bypass("raw_unsupported_rate");
        }
        let Some(device) = device else {
            return bypass("raw_unbound_capture");
        };
        if settings
            .energy_calibration_for_capture(device, sample_rate)
            .is_err()
        {
            return bypass("raw_unqualified_profile");
        }
        let Some(profile) = settings
            .energy_calibration()
            .artifact()
            .and_then(|artifact| artifact.profile_for_device(device))
        else {
            return bypass("raw_missing_profile");
        };
        let level = profile.measurement.active_speech_median_dbfs;
        let peak = profile.measurement.peak_dbfs;
        if !level.is_finite() || !peak.is_finite() {
            return bypass("raw_nonfinite_profile");
        }
        if level > QUIET_PROFILE_DBFS {
            return bypass("raw_healthy_level");
        }
        // Fix the gain for the whole take: pauses cannot pump background noise
        // upward, and callback partitioning cannot change the waveform.
        let gain_db = (TARGET_DBFS - level)
            .min(MAX_GAIN_DB)
            .min(PEAK_HEADROOM_DBFS - peak)
            .max(0.0);
        Self {
            gain: Some(10.0_f32.powf(gain_db / 20.0)),
            disposition: "quiet_capture_rnnoise_v1",
        }
    }
}

/// One original capture interval and its equal-length recognizer projection.
pub(crate) struct SpeechInputFrame {
    raw: Option<Vec<f32>>,
    pub(crate) samples: Vec<f32>,
}

impl SpeechInputFrame {
    pub(crate) fn raw_samples(&self) -> &[f32] {
        self.raw.as_deref().unwrap_or(&self.samples)
    }
}

/// The enhanced queue holds at most 160 ms. Backpressure parks only its CPU
/// thread; it never blocks the async event loop or drops/substitutes a frame.
enum SpeechInputReceiver {
    Raw(mpsc::Receiver<Vec<f32>>),
    Enhanced(mpsc::Receiver<Result<SpeechInputFrame, String>>),
    Failed {
        input: mpsc::Receiver<Vec<f32>>,
        error: Option<String>,
    },
}

/// A temporary, exact f32 projection spool, independent of the raw WAV. It is
/// published only after all real frames have been flushed, before output EOF.
/// Lookups read bounded ranges; Stop never reruns denoising over a whole take.
#[derive(Clone, Default)]
pub(crate) struct SpeechInputArchive(Arc<Mutex<Option<ProjectionFile>>>);

struct ProjectionFile {
    file: tempfile::NamedTempFile,
    samples: u64,
}

impl SpeechInputArchive {
    pub(crate) fn window(
        &self,
        start: u64,
        end: u64,
    ) -> Option<Result<ResolvedAudioWindow, String>> {
        let archive = match self.0.lock() {
            Ok(archive) => archive,
            Err(_) => return Some(Err("speech input archive lock poisoned".into())),
        };
        let archive = archive.as_ref()?;
        Some((|| {
            if end < start || end > archive.samples {
                return Err("speech input archive range outside capture".into());
            }
            let mut reader = hound::WavReader::new(std::io::BufReader::new(
                archive.file.reopen().map_err(|e| e.to_string())?,
            ))
            .map_err(|e| e.to_string())?;
            let spec = reader.spec();
            if spec.channels != 1
                || spec.sample_rate != RATE
                || spec.sample_format != hound::SampleFormat::Float
                || spec.bits_per_sample != 32
                || u64::from(reader.duration()) != archive.samples
            {
                return Err("speech input archive format/count mismatch".into());
            }
            reader
                .seek(u32::try_from(start).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            let length = usize::try_from(end - start).map_err(|e| e.to_string())?;
            let samples = reader
                .samples::<f32>()
                .take(length)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            if samples.len() != length {
                return Err("speech input archive truncated".into());
            }
            Ok(ResolvedAudioWindow {
                samples,
                sample_start: start,
                sample_end: end,
            })
        })())
    }
}

pub(crate) struct SpeechInputStream {
    receiver: SpeechInputReceiver,
    archive: SpeechInputArchive,
}

impl SpeechInputStream {
    pub(crate) fn archive(&self) -> SpeechInputArchive {
        self.archive.clone()
    }

    pub(crate) fn open(
        input: mpsc::Receiver<Vec<f32>>,
        plan: SpeechInputPlan,
        session: &str,
    ) -> Self {
        info!(
            session,
            projection = plan.disposition,
            gain = plan.gain.unwrap_or(1.0),
            "speech input projection selected; capture evidence remains raw"
        );
        let Some(gain) = plan.gain else {
            return Self {
                receiver: SpeechInputReceiver::Raw(input),
                archive: SpeechInputArchive::default(),
            };
        };
        let (output, receiver) = mpsc::channel(QUEUE_FRAMES);
        let archive = SpeechInputArchive::default();
        let worker_archive = archive.clone();
        // Retain ownership if the OS refuses thread creation. The failed
        // stream reports the error once, then remains drainable to capture EOF.
        let owner = Arc::new(Mutex::new(Some(input)));
        let worker_owner = owner.clone();
        let session = session.to_owned();
        let started = thread::Builder::new()
            .name("speech-input".into())
            .spawn(move || {
                let mut input = worker_owner
                    .lock()
                    .expect("speech input receiver handoff lock")
                    .take()
                    .expect("speech input receiver handed off once");
                let result = catch_unwind(AssertUnwindSafe(|| {
                    run_conditioner(&mut input, &output, gain, &session, &worker_archive)
                }));
                match result {
                    Ok(Ok(())) => return,
                    Ok(Err(error)) => {
                        let _ = output.blocking_send(Err(error));
                    }
                    Err(_) => {
                        let _ = output.blocking_send(Err("speech input worker panicked".into()));
                    }
                }
                // Cancellation/failure must not strand the live capture sender.
                // Keep draining raw input until its owner closes the take.
                while input.blocking_recv().is_some() {}
            });
        let receiver = match started {
            Ok(_) => SpeechInputReceiver::Enhanced(receiver),
            Err(error) => SpeechInputReceiver::Failed {
                input: owner
                    .lock()
                    .expect("speech input receiver handoff lock")
                    .take()
                    .expect("failed spawn retains capture receiver"),
                error: Some(format!("speech input thread unavailable: {error}")),
            },
        };
        Self { receiver, archive }
    }

    pub(crate) async fn recv(&mut self) -> Option<Result<SpeechInputFrame, String>> {
        match &mut self.receiver {
            SpeechInputReceiver::Raw(input) => input
                .recv()
                .await
                .map(|samples| Ok(SpeechInputFrame { raw: None, samples })),
            SpeechInputReceiver::Enhanced(input) => input.recv().await,
            SpeechInputReceiver::Failed { input, error } => {
                if let Some(error) = error.take() {
                    Some(Err(error))
                } else {
                    input
                        .recv()
                        .await
                        .map(|samples| Ok(SpeechInputFrame { raw: None, samples }))
                }
            }
        }
    }
}

struct SpeechInputConditioner {
    denoise: Box<DenoiseState<'static>>,
    gain: f32,
    pending: Vec<f32>,
    held: Option<Vec<f32>>,
    input_samples: u64,
    output_samples: u64,
}

impl SpeechInputConditioner {
    fn new(gain: f32) -> Self {
        Self {
            denoise: DenoiseState::new(),
            gain,
            pending: Vec::with_capacity(FRAME),
            held: None,
            input_samples: 0,
            output_samples: 0,
        }
    }

    fn frame(&mut self, raw: Vec<f32>) -> Option<SpeechInputFrame> {
        let mut input = [0.0; FRAME];
        for (dst, sample) in input.iter_mut().zip(&raw) {
            // RNNoise uses i16-scale floating point, unlike capture's [-1,1].
            *dst = (*sample * self.gain * 32768.0).clamp(-32768.0, 32767.0);
        }
        let mut wet = [0.0; FRAME];
        self.denoise.process_frame(&mut wet, &input);
        let previous = self.held.replace(raw)?;
        let samples = previous
            .iter()
            .zip(wet)
            .map(|(dry, wet)| {
                let mixed = DRY_SHARE * dry * self.gain + (1.0 - DRY_SHARE) * wet / 32768.0;
                // Unity below the knee; a continuous soft limiter above it.
                let magnitude = mixed.abs();
                if magnitude <= 0.8 {
                    mixed
                } else {
                    mixed.signum() * (0.8 + 0.2 * ((magnitude - 0.8) / 0.2).tanh())
                }
            })
            .collect::<Vec<_>>();
        self.output_samples += previous.len() as u64;
        Some(SpeechInputFrame {
            raw: Some(previous),
            samples,
        })
    }

    fn push(
        &mut self,
        samples: &[f32],
        output: &mpsc::Sender<Result<SpeechInputFrame, String>>,
        spool: &mut hound::WavWriter<BufWriter<std::fs::File>>,
    ) -> Result<(), String> {
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err("nonfinite capture PCM; projection refused".into());
        }
        self.input_samples = self
            .input_samples
            .checked_add(samples.len() as u64)
            .ok_or_else(|| "speech input sample count overflow".to_string())?;
        if self.input_samples > MAX_SPOOL_SAMPLES {
            return Err(
                "speech input projection exceeds 2 GiB spool budget; raw capture retained".into(),
            );
        }
        for sample in samples {
            self.pending.push(*sample);
            if self.pending.len() == FRAME {
                let frame = std::mem::replace(&mut self.pending, Vec::with_capacity(FRAME));
                if let Some(frame) = self.frame(frame) {
                    publish_frame(frame, output, spool)?;
                }
            }
        }
        Ok(())
    }

    fn finish(
        &mut self,
        output: &mpsc::Sender<Result<SpeechInputFrame, String>>,
        spool: &mut hound::WavWriter<BufWriter<std::fs::File>>,
    ) -> Result<(), String> {
        if !self.pending.is_empty() {
            let frame = std::mem::take(&mut self.pending);
            if let Some(frame) = self.frame(frame) {
                publish_frame(frame, output, spool)?;
            }
        }
        if self.held.is_some() {
            // Empty raw means analysis padding, not captured samples. This
            // releases the preceding real frame, including a one-sample tail.
            if let Some(frame) = self.frame(Vec::new()) {
                publish_frame(frame, output, spool)?;
            }
            self.held = None;
        }
        if self.input_samples != self.output_samples {
            return Err(format!(
                "speech input count mismatch: {} in / {} out",
                self.input_samples, self.output_samples
            ));
        }
        Ok(())
    }
}

fn publish_frame(
    frame: SpeechInputFrame,
    output: &mpsc::Sender<Result<SpeechInputFrame, String>>,
    spool: &mut hound::WavWriter<BufWriter<std::fs::File>>,
) -> Result<(), String> {
    for sample in &frame.samples {
        spool.write_sample(*sample).map_err(|e| e.to_string())?;
    }
    output
        .blocking_send(Ok(frame))
        .map_err(|_| "speech input consumer closed".to_string())
}

fn run_conditioner(
    input: &mut mpsc::Receiver<Vec<f32>>,
    output: &mpsc::Sender<Result<SpeechInputFrame, String>>,
    gain: f32,
    session: &str,
    archive: &SpeechInputArchive,
) -> Result<(), String> {
    let file = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut spool = hound::WavWriter::new(
        BufWriter::new(file.reopen().map_err(|e| e.to_string())?),
        spec,
    )
    .map_err(|e| e.to_string())?;
    let mut conditioner = SpeechInputConditioner::new(gain);
    while let Some(samples) = input.blocking_recv() {
        conditioner.push(&samples, output, &mut spool)?;
    }
    conditioner.finish(output, &mut spool)?;
    spool.finalize().map_err(|e| e.to_string())?;
    *archive
        .0
        .lock()
        .map_err(|_| "speech input archive lock poisoned".to_string())? = Some(ProjectionFile {
        file,
        samples: conditioner.output_samples,
    });
    info!(
        session,
        input_samples = conditioner.input_samples,
        output_samples = conditioner.output_samples,
        padding_published = 0,
        "speech input projection complete"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn condition(input: &[f32], partition: usize) -> (Vec<f32>, Vec<f32>) {
        let (tx, mut rx) = mpsc::channel(4096);
        let file = tempfile::NamedTempFile::new().unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut spool =
            hound::WavWriter::new(BufWriter::new(file.reopen().unwrap()), spec).unwrap();
        let mut conditioner = SpeechInputConditioner::new(4.0);
        for part in input.chunks(partition) {
            conditioner.push(part, &tx, &mut spool).unwrap();
        }
        conditioner.finish(&tx, &mut spool).unwrap();
        spool.finalize().unwrap();
        let mut raw = Vec::new();
        let mut projected = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            let frame = frame.unwrap();
            raw.extend_from_slice(frame.raw_samples());
            projected.extend_from_slice(&frame.samples);
        }
        let archived = hound::WavReader::open(file.path())
            .unwrap()
            .samples::<f32>()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        assert_eq!(projected, archived);
        (raw, projected)
    }

    #[test]
    fn speech_input_keeps_every_raw_sample_and_eof_tail_under_repartitioning() {
        for count in [1, FRAME - 1, FRAME, FRAME + 1, FRAME * 5 + 17] {
            let input = (0..count)
                .map(|i| (i as f32 * 0.13).sin() * 0.2)
                .collect::<Vec<_>>();
            let (raw, projected) = condition(&input, count);
            assert_eq!(raw, input);
            assert_eq!(projected.len(), count);
            assert!(
                projected
                    .iter()
                    .all(|value| value.is_finite() && value.abs() <= 1.0)
            );
            for partition in [1, 37, FRAME] {
                assert_eq!(
                    condition(&input, partition),
                    (raw.clone(), projected.clone())
                );
            }
        }
    }
}
