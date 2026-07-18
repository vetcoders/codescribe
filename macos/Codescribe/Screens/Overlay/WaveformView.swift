import SwiftUI
import Foundation

// 34-bar listening waveform — Canvas + per-bar `eq` animation with staggered delays.
//
// AMPLITUDE-DRIVEN when the engine provides it: `on_audio_level` streams the
// capture RMS per audio block into `AudioLevelMeter`, and the bars scale with the
// real voice. When no live level has arrived (older engine, previews, warm-up)
// the bars fall back to the original ambient breathing, gated on VAD activity
// (`on_vad_active`) via `active`.
// The per-bar period and phase offset reproduce the mock's formula exactly:
//   duration = 0.7 + ((i*7) % 9) / 10   seconds
//   delay    = ((i*13) % 11) / 14       seconds
// and the `eq` keyframe (scaleY .35 → 1 → .35) is modeled as a raised cosine so the
// motion reads identically to the CSS `@keyframes eq` without a discrete keyframe rig.

/// Live input-level meter driving the waveform when the engine streams real RMS
/// blocks (`on_audio_level`). Deliberately NOT an ObservableObject: the
/// TimelineView already redraws every frame while active, so the Canvas simply
/// reads the latest smoothed value on each tick — republishing every ~21ms
/// block through @Published would only add invalidation churn on the host view.
/// Main-actor only: pushed from the hopped listener callback, read from body.
@MainActor
final class AudioLevelMeter {
    /// Smoothed display gain in 0...1, or nil when no live signal has arrived —
    /// callers fall back to the ambient animation.
    private(set) var gain: Double?

    /// Map one linear RMS block onto display gain: dB scale (speech at a normal
    /// mic distance lives around −45…−25 dBFS), fast attack / slow release so
    /// peaks land instantly and the decay reads naturally instead of flickering
    /// per block. The window is deliberately tight and the response curve
    /// perceptual (pow 0.7): ordinary speech must visibly move the bars, not
    /// hover just above the rest scale.
    func push(rms: Float) {
        guard rms.isFinite, rms >= 0 else { return }
        let db = 20 * log10(max(Double(rms), 1e-6))
        let linear = min(max((db + 55) / 30, 0), 1)
        let target = pow(linear, 0.7)
        let current = gain ?? 0
        let smoothing = target > current ? 0.6 : 0.15
        gain = current + (target - current) * smoothing
    }

    func reset() { gain = nil }
}

struct WaveformView: View {
    var barCount: Int = 34
    var active: Bool = true
    /// Post-capture "transcribing" phase. Overrides `active`: instead of the
    /// audio-suggestive per-bar `eq` stagger, the bars hold a FROZEN silhouette
    /// that breathes together on one slow synchronous cycle at reduced opacity —
    /// unmistakably "processing", not "listening", and not a hung freeze either.
    var transcribing: Bool = false
    /// Real capture level, when the engine streams it. nil → ambient animation.
    var meter: AudioLevelMeter? = nil

    private let barWidth: CGFloat = 3
    private let gap: CGFloat = 4
    private let maxBarHeight: CGFloat = 26
    private let trackHeight: CGFloat = 34
    private let minScale: CGFloat = 0.35

    private var contentWidth: CGFloat {
        CGFloat(barCount) * (barWidth + gap) - gap
    }

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 60.0, paused: !(active || transcribing))) { timeline in
            Canvas { ctx, size in
                let now = timeline.date.timeIntervalSinceReferenceDate
                for i in 0..<barCount {
                    let scale = barScale(index: i, now: now)
                    let h = maxBarHeight * scale
                    let x = CGFloat(i) * (barWidth + gap)
                    let y = (size.height - h) / 2  // transform-origin: center
                    let rect = CGRect(x: x, y: y, width: barWidth, height: h)
                    ctx.fill(
                        Path(roundedRect: rect, cornerRadius: 2),
                        with: .color(color(for: i))
                    )
                }
            }
            .frame(width: contentWidth, height: trackHeight)
        }
        .frame(width: contentWidth, height: trackHeight, alignment: .leading)
    }

    private func barScale(index i: Int, now: TimeInterval) -> CGFloat {
        if transcribing { return transcribingScale(index: i, now: now) }
        guard active else { return minScale }
        let duration = 0.7 + Double((i * 7) % 9) / 10.0
        let delay = Double((i * 13) % 11) / 14.0
        let phase = (now + delay) / duration
        // raised cosine: 0.675 - 0.325*cos → .35 at phase 0/1, 1.0 at phase 0.5
        let mid = (1 + minScale) / 2          // 0.675
        let amp = (1 - minScale) / 2          // 0.325
        let ambient = mid - amp * CGFloat(cos(phase * 2 * .pi))
        // Real signal: the per-bar sweep becomes the SHAPE and the live level
        // the AMPLITUDE — full voice reads like the original animation, silence
        // settles the bars near the rest scale. No signal → pure ambient.
        if let gain = meter?.gain {
            return minScale + (ambient - minScale) * CGFloat(gain)
        }
        return ambient
    }

    /// Frozen per-bar silhouette (deterministic, no audio input — the capture
    /// waveform is itself synthetic) modulated by ONE slow synchronous breath, so
    /// the whole shape rises and falls together instead of the per-bar sweep. The
    /// breath is subtle (~0.86–1.0) and never reaches the capture amplitude.
    private func transcribingScale(index i: Int, now: TimeInterval) -> CGFloat {
        let silhouette = 0.30 + 0.34 * abs(sin(Double(i) * 0.9)) // fixed, in ~0.30–0.64
        let breathPeriod = 1.7
        let breath = 0.93 - 0.07 * cos(now * 2 * .pi / breathPeriod) // ~0.86–1.0
        return CGFloat(silhouette * breath)
    }

    private func color(for i: Int) -> Color {
        // Muted terracotta so the phase reads as our brand "at work", clearly
        // dimmer than the live-capture bars.
        if transcribing { return CSColor.terracotta.opacity(0.4) }
        guard active else { return CSColor.hairline(0.16) }
        // every 5th bar uses the lighter terracotta tint, per the mock.
        return i % 5 == 0 ? CSColor.terracottaTintBars : CSColor.terracotta
    }
}

#if DEBUG
#Preview("Waveform — active") {
    WaveformView(active: true)
        .padding(40)
        .background(CSColor.glassUnder)
}
#endif
