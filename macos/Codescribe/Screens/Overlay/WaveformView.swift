import AppKit
import Foundation
import Observation
import QuartzCore
import SwiftUI

// Width-adaptive listening waveform — per-bar `eq` animation with staggered delays.
// While it moves, Core Animation runs the motion (`WaveformMotionStrip`); a still
// waveform draws once in a Canvas.
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
/// blocks (`on_audio_level`). Only advisory transitions are observed. The
/// TimelineView reads the display gain without publishing every capture block.
/// Main-actor only: pushed from the hopped listener callback, read from body.
@MainActor
@Observable
final class AudioLevelMeter {
  /// Smoothed display gain in 0...1, or nil when no live signal has arrived.
  @ObservationIgnored private(set) var gain: Double?
  private(set) var hasLowInputSignal = false
  /// Measured capture blocks have stayed below speaking level for
  /// `silenceHoldSeconds`. Energy only: the engine emits no Silero
  /// `VadStart`/`VadEnd` today, so this claims "nothing at speaking level",
  /// never "no speech". Missing or delayed blocks never count as silence.
  private(set) var isSilent = false
  @ObservationIgnored private var lastBlockAt: TimeInterval?
  @ObservationIgnored private var lastSpeechAt: TimeInterval?
  @ObservationIgnored private var quietSince: TimeInterval?
  @ObservationIgnored private var speechSeconds = 0.0
  @ObservationIgnored private var speechEnergy = 0.0

  /// A block below this level counts toward silence.
  static let silenceFloorDB = -52.0
  /// A block above this level ends silence at once. The gap between the two
  /// levels keeps a soft syllable from flickering the light.
  static let speakingLevelDB = -48.0
  /// Ordinary pauses between words and sentences stay shorter than this.
  static let silenceHoldSeconds = 1.2

  /// Map one linear RMS block onto display gain: dB scale (speech at a normal
  /// mic distance lives around −45…−25 dBFS), fast attack / slow release so
  /// peaks land instantly and the decay reads naturally instead of flickering
  /// per block. The window is deliberately tight and the response curve
  /// perceptual (pow 0.7): ordinary speech must visibly move the bars, not
  /// hover just above the rest scale.
  func push(
    rms: Float, speechActive: Bool = false,
    now: TimeInterval = ProcessInfo.processInfo.systemUptime
  ) {
    guard rms.isFinite, rms >= 0 else { return }
    let db = 20 * log10(max(Double(rms), 1e-6))
    let linear = min(max((db + 55) / 30, 0), 1)
    let target = pow(linear, 0.7)
    let current = gain ?? 0
    let smoothing = target > current ? 0.6 : 0.15
    gain = current + (target - current) * smoothing
    assessInput(rms: Double(rms), db: db, speechActive: speechActive, now: now)
  }

  private func assessInput(rms: Double, db: Double, speechActive: Bool, now: TimeInterval) {
    guard now.isFinite else { return }
    defer { lastBlockAt = now }
    guard let lastBlockAt else { return }
    let elapsed = now - lastBlockAt
    // Missing or delayed callbacks are not measured speech duration, and
    // they are not measured silence either.
    guard elapsed > 0, elapsed <= 0.25 else {
      speechSeconds = 0
      speechEnergy = 0
      quietSince = nil
      if isSilent { isSilent = false }
      return
    }
    assessSilence(db: db, speechActive: speechActive, now: now)
    if let lastSpeechAt, now - lastSpeechAt > 1.5 {
      speechSeconds = 0
      speechEnergy = 0
    }
    // VAD identifies speech; digital silence and near-zero blocks provide no
    // evidence for an input-quality claim. Never turn pauses into warnings.
    guard speechActive, rms > pow(10, -70.0 / 20) else { return }
    lastSpeechAt = now
    speechSeconds += elapsed
    speechEnergy += rms * rms * elapsed
    guard speechSeconds >= 2 else { return }
    let speechDB = 10 * log10(speechEnergy / speechSeconds)
    // Advisory thresholds, not transcript admission or an accuracy score.
    // A recovery margin keeps normal syllable-level variation from flashing.
    if speechDB < -46 {
      hasLowInputSignal = true
    } else if speechDB > -42 {
      hasLowInputSignal = false
    }
    speechSeconds = 0
    speechEnergy = 0
  }

  /// Schmitt trigger over consecutive measured blocks: silence needs an
  /// unbroken quiet run of `silenceHoldSeconds`; one block at speaking level
  /// ends it immediately, so the light turns back the moment you speak. A VAD
  /// speech verdict, when the engine sends one, outranks the energy reading.
  private func assessSilence(db: Double, speechActive: Bool, now: TimeInterval) {
    if speechActive || db > Self.speakingLevelDB {
      quietSince = nil
      if isSilent { isSilent = false }
      return
    }
    guard db < Self.silenceFloorDB else {
      // Between the two levels: an ongoing silence holds, a quiet run that
      // has not yet become silence starts over.
      if !isSilent { quietSince = nil }
      return
    }
    let since = quietSince ?? now
    quietSince = since
    if !isSilent, now - since >= Self.silenceHoldSeconds {
      isSilent = true
    }
  }

  func reset() {
    gain = nil
    hasLowInputSignal = false
    isSilent = false
    lastBlockAt = nil
    lastSpeechAt = nil
    quietSince = nil
    speechSeconds = 0
    speechEnergy = 0
  }
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
  /// Controller-projected channel capture shares the built-in Agent's hue.
  var agentRouted: Bool = false
  /// Real capture level, when the engine streams it. nil → neutral flat bars.
  var meter: AudioLevelMeter? = nil
  /// Appearance-aware neutral track supplied by the owning surface.
  var inactiveColor: Color = CSColor.hairline(0.16)
  /// Chrome placement uses a tighter strip so the waveform can live in the
  /// primary bar without competing with transcript words.
  var compact: Bool = false
  var stretches: Bool = false
  var isVisible: Bool = true

  private var barWidth: CGFloat { compact ? 1.5 : 2 }
  private var gap: CGFloat { compact ? 2 : 3 }
  private var maxBarHeight: CGFloat { compact ? 18 : 24 }
  private var trackHeight: CGFloat { compact ? 22 : 28 }
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
        if reduceMotion, active {
          // Essential data feedback still updates, but at a calm 5 Hz with no
          // decorative phase sweep. Shape changes only with measured RMS.
          TimelineView(.periodic(from: .now, by: 0.2)) { timeline in
            waveform(
              at: timeline.date.timeIntervalSinceReferenceDate, reducedMotion: true, count: count)
          }
        } else if reduceMotion || !(active || transcribing) {
          waveform(at: 0, reducedMotion: reduceMotion, count: count)
        } else {
          WaveformMotionStrip(configuration: motion(count: count))
        }
      }
    }
    .frame(idealWidth: contentWidth, maxWidth: stretches ? .infinity : nil)
    .frame(height: trackHeight)
    .clipped()
  }

  /// The moving waveform's geometry, timing and colours, handed to Core Animation.
  private func motion(count: Int) -> WaveformMotionStrip.Configuration {
    // Per-bar timing from the bar's own index (the mock's formula), so a bar
    // keeps its motion when the window widens and new bars join at the end.
    WaveformMotionStrip.Configuration(
      bars: (0..<count).map { index in
        WaveformMotionStrip.Bar(
          sweepDuration: 0.7 + Double((index * 7) % 9) / 10.0,
          sweepDelay: Double((index * 13) % 11) / 14.0,
          silhouette: transcribingScale(index: index, now: 0, reducedMotion: true),
          color: transcribing ? CSColor.modeProcessing.opacity(0.55) : liveColor(for: index))
      },
      barWidth: barWidth, gap: gap, maxBarHeight: maxBarHeight, minScale: minScale,
      transcribing: transcribing, inactiveColor: inactiveColor, meter: meter, isVisible: isVisible)
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
    return liveColor(for: i)
  }

  /// A bar's colour while measured audio moves it.
  private func liveColor(for i: Int) -> Color {
    if agentRouted || indicatorMode == .assistive {
      return i % 5 == 0 ? CSColor.assistiveLight : CSColor.modeAgent
    }
    return i % 5 == 0 ? CSColor.terracottaTintBars : CSColor.modeRecording
  }
}

/// The moving waveform on the render server (UI-88). Silver samples showed a
/// 60 fps `TimelineView` re-laying out the whole overlay every display frame
/// through a take. Here the per-bar sweep and the transcribing breath are
/// repeating Core Animation animations: the main thread only writes the
/// measured gain, at most `gainPollInterval` apart, onto one container layer.
struct WaveformMotionStrip: NSViewRepresentable {
  struct Bar: Equatable {
    var sweepDuration: Double
    var sweepDelay: Double
    /// Resting height while transcribing, as a share of the full bar.
    var silhouette: CGFloat
    var color: Color
  }

  struct Configuration {
    var bars: [Bar]
    var barWidth: CGFloat
    var gap: CGFloat
    var maxBarHeight: CGFloat
    var minScale: CGFloat
    var transcribing: Bool
    var inactiveColor: Color
    var meter: AudioLevelMeter?
    var isVisible: Bool = true

    func drawsLike(_ other: Configuration) -> Bool {
      bars == other.bars && barWidth == other.barWidth && gap == other.gap
        && maxBarHeight == other.maxBarHeight && minScale == other.minScale
        && transcribing == other.transcribing && inactiveColor == other.inactiveColor
        && meter === other.meter && isVisible == other.isVisible
    }
  }

  var configuration: Configuration

  func makeNSView(context: Context) -> WaveformMotionView {
    WaveformMotionView()
  }

  func updateNSView(_ view: WaveformMotionView, context: Context) {
    view.apply(configuration)
  }

  static func dismantleNSView(_ view: WaveformMotionView, coordinator: ()) {
    view.stopGainPolling()
  }
}

final class WaveformMotionView: NSView {
  static let gainPollInterval: TimeInterval = 1.0 / 20.0
  /// One clock for every bar, so a rebuilt strip keeps its phase.
  private static let epoch: CFTimeInterval = 1
  /// Sine ease: an autoreversing tween with it is the mock's raised cosine.
  private static let sine = CAMediaTimingFunction(controlPoints: 0.37, 0, 0.63, 1)

  /// Bars at the resting height, never animated.
  let cores = CALayer()
  /// Bars that sweep; their container's `sublayerTransform` carries the gain.
  let movers = CALayer()
  private(set) var configuration: WaveformMotionStrip.Configuration?
  /// Sweep animations attached since creation; a resize adds only new bars'.
  private(set) var sweepsAdded = 0
  private var builtBars: [WaveformMotionStrip.Bar] = []
  private var builtTranscribing: Bool?
  private var laidOutSize: CGSize?
  private var shownGain: Double?
  private var hasShownGain = false
  private(set) var gainTimer: Timer?

  override init(frame frameRect: NSRect) {
    super.init(frame: frameRect)
    wantsLayer = true
    layer?.masksToBounds = true
    layer?.addSublayer(cores)
    layer?.addSublayer(movers)
    setAccessibilityElement(false)
  }

  @available(*, unavailable)
  required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

  override func hitTest(_ point: NSPoint) -> NSView? { nil }

  func apply(_ next: WaveformMotionStrip.Configuration) {
    if let configuration, configuration.drawsLike(next) { return }
    configuration = next
    sync()
    updateGainPolling()
  }

  override func layout() {
    super.layout()
    guard bounds.size != laidOutSize else { return }
    sync()
  }

  override func viewDidMoveToWindow() {
    super.viewDidMoveToWindow()
    updateGainPolling()
  }

  override func viewDidChangeEffectiveAppearance() {
    super.viewDidChangeEffectiveAppearance()
    paint()
  }

  /// Bring the bar layers in line with the configuration and size. A bar that
  /// already exists keeps its layer and its running animation: a window that
  /// grows or shrinks only adds or removes bars at the end (Silver 23 X, the
  /// expand animation rebuilt every bar each frame and flooded the render
  /// server). A switch between live and transcribing motion starts over.
  func sync() {
    guard let configuration, let host = layer else { return }
    CATransaction.begin()
    CATransaction.setDisableActions(true)
    defer { CATransaction.commit() }
    if builtTranscribing != configuration.transcribing {
      cores.sublayers = nil
      movers.sublayers = nil
      movers.removeAllAnimations()
      movers.sublayerTransform = CATransform3DIdentity
      builtBars = []
      builtTranscribing = configuration.transcribing
      if configuration.transcribing { movers.add(breath(), forKey: "breath") }
      hasShownGain = false
    }
    laidOutSize = host.bounds.size
    if cores.frame != host.bounds { cores.frame = host.bounds }
    if movers.frame != host.bounds { movers.frame = host.bounds }
    let bars = configuration.bars
    while (movers.sublayers?.count ?? 0) > bars.count {
      movers.sublayers?.last?.removeFromSuperlayer()
    }
    while (cores.sublayers?.count ?? 0) > (configuration.transcribing ? 0 : bars.count) {
      cores.sublayers?.last?.removeFromSuperlayer()
    }
    let radius = min(2, configuration.barWidth / 2)
    var recolor = builtBars.count != bars.count
    for (index, bar) in bars.enumerated() {
      let x = CGFloat(index) * (configuration.barWidth + configuration.gap)
      let full = CGRect(
        x: x, y: (host.bounds.height - configuration.maxBarHeight) / 2,
        width: configuration.barWidth, height: configuration.maxBarHeight)
      let built = index < builtBars.count ? builtBars[index] : nil
      let mover: CALayer
      if let existing = movers.sublayers?[safe: index] {
        mover = existing
      } else {
        mover = CALayer()
        movers.addSublayer(mover)
        sweepsAdded += 1
      }
      if mover.bounds.size != full.size { mover.bounds = CGRect(origin: .zero, size: full.size) }
      let center = CGPoint(x: full.midX, y: full.midY)
      if mover.position != center { mover.position = center }
      mover.cornerRadius = radius
      if built?.color != bar.color { recolor = true }
      if configuration.transcribing {
        mover.transform = CATransform3DMakeScale(1, bar.silhouette, 1)
        continue
      }
      mover.transform = CATransform3DMakeScale(1, configuration.minScale, 1)
      if built?.sweepDuration != bar.sweepDuration || built?.sweepDelay != bar.sweepDelay
        || mover.animation(forKey: "sweep") == nil
      {
        mover.add(sweep(for: bar, minScale: configuration.minScale), forKey: "sweep")
      }
      let core: CALayer
      if let existing = cores.sublayers?[safe: index] {
        core = existing
      } else {
        core = CALayer()
        cores.addSublayer(core)
      }
      let rest = full.insetBy(dx: 0, dy: full.height * (1 - configuration.minScale) / 2)
      if core.frame != rest { core.frame = rest }
      core.cornerRadius = radius
    }
    builtBars = bars
    if recolor || !hasShownGain { paint() }
    if !hasShownGain {
      showGain(configuration.transcribing ? nil : configuration.meter?.gain, animated: false)
    }
  }

  private func sweep(for bar: WaveformMotionStrip.Bar, minScale: CGFloat) -> CAAnimation {
    let sweep = CABasicAnimation(keyPath: "transform.scale.y")
    sweep.fromValue = minScale
    sweep.toValue = 1
    sweep.duration = bar.sweepDuration / 2
    sweep.autoreverses = true
    sweep.repeatCount = .infinity
    sweep.timingFunction = Self.sine
    sweep.beginTime = Self.epoch
    sweep.timeOffset = bar.sweepDelay
    sweep.isRemovedOnCompletion = false
    return sweep
  }

  /// One slow synchronous breath, ~0.86–1.0 over 1.7 s, for the whole strip.
  private func breath() -> CAAnimation {
    let breath = CABasicAnimation(keyPath: "sublayerTransform.scale.y")
    breath.fromValue = 0.86
    breath.toValue = 1
    breath.duration = 0.85
    breath.autoreverses = true
    breath.repeatCount = .infinity
    breath.timingFunction = Self.sine
    breath.beginTime = Self.epoch
    breath.isRemovedOnCompletion = false
    return breath
  }

  private func paint() {
    guard let configuration else { return }
    let measured = configuration.transcribing || shownGain != nil
    effectiveAppearance.performAsCurrentDrawingAppearance {
      let inactive = NSColor(configuration.inactiveColor).cgColor
      for (index, bar) in configuration.bars.enumerated() {
        let color = measured ? NSColor(bar.color).cgColor : inactive
        if let mover = movers.sublayers?[safe: index] { mover.backgroundColor = color }
        if let core = cores.sublayers?[safe: index] { core.backgroundColor = color }
      }
    }
  }

  /// Read the meter and scale the sweep: a bar peaks at the measured level and
  /// never drops below the resting core, so silence stays a flat line.
  func refreshGain() {
    guard let configuration, configuration.isVisible, !configuration.transcribing else { return }
    showGain(configuration.meter?.gain, animated: true)
  }

  private func showGain(_ gain: Double?, animated: Bool) {
    guard let configuration, !configuration.transcribing else {
      movers.isHidden = false
      return
    }
    if hasShownGain, let gain, let shownGain, abs(gain - shownGain) < 0.005 { return }
    if hasShownGain, gain == nil, shownGain == nil { return }
    let recolor = !hasShownGain || (gain == nil) != (shownGain == nil)
    shownGain = gain
    hasShownGain = true
    CATransaction.begin()
    CATransaction.setDisableActions(!animated)
    CATransaction.setAnimationDuration(Self.gainPollInterval)
    CATransaction.setAnimationTimingFunction(CAMediaTimingFunction(name: .linear))
    let level = CGFloat(gain ?? 0)
    let peak = configuration.minScale + level * (1 - configuration.minScale)
    movers.sublayerTransform = CATransform3DMakeScale(1, peak, 1)
    movers.isHidden = gain == nil
    CATransaction.commit()
    if recolor { paint() }
  }

  private func updateGainPolling() {
    let polls =
      window != nil && configuration?.isVisible == true && configuration?.meter != nil
        && configuration?.transcribing == false
    guard polls != (gainTimer != nil) else { return }
    if !polls {
      stopGainPolling()
      return
    }
    let timer = Timer(timeInterval: Self.gainPollInterval, repeats: true) { [weak self] _ in
      MainActor.assumeIsolated { self?.refreshGain() }
    }
    timer.tolerance = Self.gainPollInterval / 4
    RunLoop.main.add(timer, forMode: .common)
    gainTimer = timer
  }

  func stopGainPolling() {
    gainTimer?.invalidate()
    gainTimer = nil
  }
}

extension Array {
  fileprivate subscript(safe index: Int) -> Element? {
    indices.contains(index) ? self[index] : nil
  }
}

#if DEBUG
  #Preview("Waveform — active") {
    WaveformView(active: true)
      .padding(40)
      .background(CSColor.glassUnder)
  }
#endif
