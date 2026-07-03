import SwiftUI
import Foundation

// 34-bar listening waveform — Canvas + per-bar `eq` animation with staggered delays.
//
// AMBIENT, NOT AMPLITUDE-DRIVEN: the FFI exposes no audio-level callback, so the bars
// breathe synthetically. The caller gates `active` on VAD activity (on_vad_active),
// so the bars pulse while speech is detected and settle to the rest scale otherwise.
// The per-bar period and phase offset reproduce the mock's formula exactly:
//   duration = 0.7 + ((i*7) % 9) / 10   seconds
//   delay    = ((i*13) % 11) / 14       seconds
// and the `eq` keyframe (scaleY .35 → 1 → .35) is modeled as a raised cosine so the
// motion reads identically to the CSS `@keyframes eq` without a discrete keyframe rig.
struct WaveformView: View {
    var barCount: Int = 34
    var active: Bool = true
    /// Post-capture "transcribing" phase. Overrides `active`: instead of the
    /// audio-suggestive per-bar `eq` stagger, the bars hold a FROZEN silhouette
    /// that breathes together on one slow synchronous cycle at reduced opacity —
    /// unmistakably "processing", not "listening", and not a hung freeze either.
    var transcribing: Bool = false

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
        return mid - amp * CGFloat(cos(phase * 2 * .pi))
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

#Preview("Waveform — active") {
    WaveformView(active: true)
        .padding(40)
        .background(CSColor.glassUnder)
}
