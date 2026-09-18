import Foundation
import SwiftUI

// Width-adaptive listening waveform — Canvas + per-bar `eq` animation with staggered delays.
//
// AMPLITUDE-DRIVEN when the engine provides it: `on_audio_level` streams the
// capture RMS per audio block into `AudioLevelMeter`, and the bars scale with the
// real voice. When no live level has arrived, the bars remain a neutral flat
// line. Motion without measured RMS would be decorative, not audio evidence.
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
  /// Smoothed display gain in 0...1, or nil when no live signal has arrived.
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
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  /// Minimum bar count and source silhouette resolution.
  var barCount: Int = 34
  var active: Bool = true
  /// Post-capture "transcribing" phase. Overrides `active`: instead of the
  /// audio-suggestive per-bar `eq` stagger, the bars hold a FROZEN silhouette
  /// that breathes together on one slow synchronous cycle at reduced opacity —
  /// unmistakably "processing", not "listening", and not a hung freeze either.
  var transcribing: Bool = false
  var indicatorMode: CsIndicatorMode = .hold
  /// Real capture level, when the engine streams it. nil → neutral flat bars.
  var meter: AudioLevelMeter? = nil
  /// Appearance-aware neutral track supplied by the owning surface.
  var inactiveColor: Color = CSColor.hairline(0.16)
  /// Chrome placement uses a tighter strip so the waveform can live in the
  /// primary bar without competing with transcript words.
  var compact: Bool = false
  var stretches: Bool = false

  private var barWidth: CGFloat { compact ? 1.5 : 2 }
  private var gap: CGFloat { compact ? 2 : 3 }
  private var maxBarHeight: CGFloat { compact ? 9 : 12 }
  private var trackHeight: CGFloat { compact ? 12 : 16 }
  private let minScale: CGFloat = 0.35

  private var contentWidth: CGFloat {
    CGFloat(barCount) * (barWidth + gap) - gap
  }

  /// Fit fixed-width bars and gaps; an unspecified width keeps the caller's minimum.
  static func effectiveBarCount(
    width: CGFloat, barWidth: CGFloat, gap: CGFloat, minimum: Int
  ) -> Int {
    let minimum = max(0, minimum)
    let step = barWidth + gap
    guard width.isFinite, barWidth > 0, gap >= 0, step.isFinite else { return minimum }
    let fitted = floor((width + gap) / step)
    guard fitted.isFinite, fitted > CGFloat(minimum), fitted < CGFloat(Int.max) else {
      return minimum
    }
    return Int(fitted)
  }

  /// Interpolate the existing silhouette, preserving both ends at any display density.
  static func resampledLevels(_ samples: [CGFloat], count: Int) -> [CGFloat] {
    guard count > 0 else { return [] }
    guard let first = samples.first else { return Array(repeating: 0, count: count) }
    guard samples.count > 1, count > 1 else { return Array(repeating: first, count: count) }
    if samples.count == count { return samples }
    return (0..<count).map { index in
      let position = CGFloat(index) / CGFloat(count - 1) * CGFloat(samples.count - 1)
      let lower = min(Int(position), samples.count - 1)
      let upper = min(lower + 1, samples.count - 1)
      return samples[lower] + (samples[upper] - samples[lower]) * (position - CGFloat(lower))
    }
  }

  var body: some View {
    GeometryReader { geometry in
      let count = Self.effectiveBarCount(
        width: geometry.size.width, barWidth: barWidth, gap: gap, minimum: barCount)
      Group {
        if reduceMotion, active, meter?.gain != nil {
          // Essential data feedback still updates, but at a calm 5 Hz with no
          // decorative phase sweep. Shape changes only with measured RMS.
          TimelineView(.periodic(from: .now, by: 0.2)) { timeline in
            waveform(
              at: timeline.date.timeIntervalSinceReferenceDate, reducedMotion: true, count: count)
          }
        } else if reduceMotion {
          waveform(at: 0, reducedMotion: true, count: count)
        } else {
          TimelineView(.animation(minimumInterval: 1.0 / 60.0, paused: !(active || transcribing))) {
            timeline in
            waveform(
              at: timeline.date.timeIntervalSinceReferenceDate, reducedMotion: false, count: count)
          }
        }
      }
    }
    .frame(idealWidth: contentWidth, maxWidth: stretches ? .infinity : nil)
    .frame(height: trackHeight)
    .clipped()
  }

  private func waveform(at now: TimeInterval, reducedMotion: Bool, count: Int) -> some View {
    Canvas { ctx, size in
      // The meter supplies one gain, not a sample buffer. Resample the existing
      // phase silhouette so resizing changes density without changing its motion.
      let samples = (0..<max(1, barCount)).map {
        barScale(index: $0, now: now, reducedMotion: reducedMotion)
      }
      let levels = Self.resampledLevels(samples, count: count)
      for (i, scale) in levels.enumerated() {
        let height = maxBarHeight * scale
        let x = CGFloat(i) * (barWidth + gap)
        let y = (size.height - height) / 2
        let rect = CGRect(x: x, y: y, width: barWidth, height: height)
        ctx.fill(
          Path(roundedRect: rect, cornerRadius: 2),
          with: .color(color(for: i))
        )
      }
    }
    .frame(height: trackHeight)
  }

  private func barScale(index i: Int, now: TimeInterval, reducedMotion: Bool) -> CGFloat {
    if transcribing {
      return transcribingScale(index: i, now: now, reducedMotion: reducedMotion)
    }
    guard active, let gain = meter?.gain else { return minScale }
    if reducedMotion {
      let silhouette = 0.55 + 0.45 * abs(sin(Double(i) * 0.9))
      return minScale + (CGFloat(silhouette) - minScale) * CGFloat(gain)
    }
    let duration = 0.7 + Double((i * 7) % 9) / 10.0
    let delay = Double((i * 13) % 11) / 14.0
    let phase = (now + delay) / duration
    // raised cosine: 0.675 - 0.325*cos → .35 at phase 0/1, 1.0 at phase 0.5
    let mid = (1 + minScale) / 2  // 0.675
    let amp = (1 - minScale) / 2  // 0.325
    let ambient = mid - amp * CGFloat(cos(phase * 2 * .pi))
    // Real signal: the per-bar sweep becomes the SHAPE and the live level the
    // AMPLITUDE. Without a measured level the guard above stays flat.
    return minScale + (ambient - minScale) * CGFloat(gain)
  }

  /// Frozen per-bar silhouette (deterministic, no audio input — the capture
  /// waveform is itself synthetic) modulated by ONE slow synchronous breath, so
  /// the whole shape rises and falls together instead of the per-bar sweep. The
  /// breath is subtle (~0.86–1.0) and never reaches the capture amplitude.
  private func transcribingScale(
    index i: Int,
    now: TimeInterval,
    reducedMotion: Bool
  ) -> CGFloat {
    let silhouette = 0.30 + 0.34 * abs(sin(Double(i) * 0.9))  // fixed, in ~0.30–0.64
    if reducedMotion { return CGFloat(silhouette) }
    let breathPeriod = 1.7
    let breath = 0.93 - 0.07 * cos(now * 2 * .pi / breathPeriod)  // ~0.86–1.0
    return CGFloat(silhouette * breath)
  }

  private func color(for i: Int) -> Color {
    // Muted terracotta so the phase reads as our brand "at work", clearly
    // dimmer than the live-capture bars.
    if transcribing { return CSColor.modeProcessing.opacity(0.55) }
    guard active, meter?.gain != nil else { return inactiveColor }
    if indicatorMode == .assistive {
      return i % 5 == 0 ? CSColor.assistiveLight : CSColor.modeAgent
    }
    return i % 5 == 0 ? CSColor.terracottaTintBars : CSColor.modeRecording
  }
}

#if DEBUG
  #Preview("Waveform — active") {
    WaveformView(active: true)
      .padding(40)
      .background(CSColor.glassUnder)
  }
#endif
