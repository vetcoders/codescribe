import AppKit
import SwiftUI

// Slim evidence-first dictation overlay.
//
// Layout (top → bottom):
//   header   brand · compact waveform · agent glyph · timer · status mic/Stop and
//            live-preview controls. Paste mode lives in Settings and the tray,
//            never here: the waveform keeps the width (Founder direction as
//            relayed in the Codex handoff, Annex A2, 2026-09-29).
//   body     transcript is the product surface (listening / formatted / terminal)
//   header and footer float above the full-height transcript viewport
//
// Removed on purpose: duplicate RECORDING/modeMeta row, full bottom Finish/Close
// action layer, and decorative body-top waveform competing with words.
//
// Authority: this view only visualizes OverlayState / projection receipts. It
// never invents transcript truth, seals, or a second recorder. Future AoT mode
// attaches to AgentChatStore (same thread owner) via existing sendToAgent — not
// a parallel chat window.
struct OverlayBottomChromeSlots: Equatable {
  enum Slot: Equatable { case rail, coverageWarning }

  let ordered: [Slot]

  init(
    mode: OverlayMode, hasPresentationStatus: Bool, isCollapsed: Bool,
    hasLowInputSignal: Bool = false
  ) {
    // Only measured quiet-mic advice shares the bar. Ledger coverage (seconds
    // to verify, ranges) lives in Voice Lab, not here (Maciej, 2026-10-19:
    // "jak ja sobie moge odpalić voice lab i mieć czytelniejsze i lepsze").
    let lowInputAdvisory = mode == .listening && hasLowInputSignal
    if isCollapsed {
      ordered = []
    } else if !hasPresentationStatus && lowInputAdvisory {
      ordered = [.rail, .coverageWarning]
    } else {
      ordered = [.rail]
    }
  }

  var showsCoverageWarning: Bool { ordered.contains(.coverageWarning) }
}

struct OverlayRecordingControls: View {
  @Environment(\.displayScale) private var displayScale
  @Environment(\.accessibilityReduceMotion) private var reduceMotion

  let canFinish: Bool
  let recordingLight: OverlayRecordingLight?
  let animates: Bool
  let isFinalizing: Bool
  let presentationMode: OverlayPresentationMode
  let compact: Bool
  let palette: OverlayAppearancePalette
  let onIntent: (OverlayIntent) -> Void
  let onPreviewToggle: () -> Void
  var showsRecordingButton = true
  var showsPreviewButton = true
  var onShowDictation: (() -> Void)?
  var onShowAgents: (() -> Void)?
  var onPresentationSelect: ((OverlayPresentationMode) -> Void)?

  /// Recording and preview keep fixed hairline circles, leaving the remaining
  /// width to the waveform (Founder, 2026-09-29: "ten stop jest olbrzymi").
  static let controlDiameter: CGFloat = 22

  init(
    canFinish: Bool, presentationMode: OverlayPresentationMode, compact: Bool,
    palette: OverlayAppearancePalette, onIntent: @escaping (OverlayIntent) -> Void,
    onPreviewToggle: @escaping () -> Void, isFinalizing: Bool = false,
    recordingLight: OverlayRecordingLight? = nil, animates: Bool = true
  ) {
    self.canFinish = canFinish
    self.recordingLight = recordingLight
    self.animates = animates
    self.isFinalizing = isFinalizing
    self.presentationMode = presentationMode
    self.compact = compact
    self.palette = palette
    self.onIntent = onIntent
    self.onPreviewToggle = onPreviewToggle
  }

  var recordingDisabled: Bool {
    recordingLight == .processing || (isFinalizing && !canFinish)
  }
  var recordingTint: Color {
    switch recordingLight {
    case .holdToTalk, .handsFree: palette.errorStatus.color
    case .silence: OverlayRecordingLight.silence.color
    case .processing: OverlayRecordingLight.processing.color
    case .agent: OverlayRecordingLight.agent.color
    case nil: canFinish || isFinalizing ? palette.errorStatus.color : palette.listeningStatus.color
    }
  }
  var recordingStatusValue: String {
    recordingLight?.name
      ?? (isFinalizing ? String(localized: "Transcribing") : String(localized: "Ready"))
  }
  var showsStop: Bool { canFinish }
  var recordingSymbol: String { canFinish || isFinalizing ? "stop.fill" : "mic.fill" }
  var recordingLabel: String {
    canFinish || isFinalizing
      ? String(localized: "Stop recording") : String(localized: "Start dictation")
  }
  var recordingIdentifier: String {
    canFinish || isFinalizing ? "overlay-stop-recording" : "overlay-start-recording"
  }
  var previewAccessibilityLabel: String {
    switch presentationMode {
    case .mini, .midi: String(localized: "Expand transcript")
    case .expanded: String(localized: "Collapse widget")
    }
  }
  /// Expanding the widget is always an explicit click.
  var previewSymbol: String {
    switch presentationMode {
    case .mini, .midi: OverlayControlSymbols.midiToTranscript
    case .expanded: OverlayControlSymbols.returnToMini
    }
  }

  var body: some View {
    HStack(spacing: compact ? 4 : 7) {
      if showsRecordingButton { recordingButton }
      if showsPreviewButton { previewButton }
    }
    .fixedSize()
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-recording-controls")
    .contextMenu {
      agentsMenuItem
      Button("My dictation", systemImage: "waveform") { onShowDictation?() }
    }
  }

  @ViewBuilder private var agentsMenuItem: some View {
    if let onShowAgents {
      Button(action: onShowAgents) {
        Label {
          Text("Agents")
        } icon: {
          Image(nsImage: OverlayHeaderGlyph.menuImage(OverlayHeaderGlyph.agents))
        }
      }
      .accessibilityIdentifier("overlay-recording-agents")
    }
  }

  func finishRecording() {
    guard showsStop else { return }
    onIntent(.finish)
  }

  func activateRecordingControl() {
    guard !recordingDisabled else { return }
    withAnimation(reduceMotion ? nil : .easeInOut(duration: CSMotion.Tempo.hover)) {
      if canFinish { finishRecording() } else { onIntent(.startRecording) }
    }
  }

  func togglePreview() {
    onPreviewToggle()
  }

  static func showsStop(for projectedIntents: [OverlayIntent]) -> Bool {
    projectedIntents.contains(.finish)
  }

  static func railIntents(from projectedIntents: [OverlayIntent]) -> [OverlayIntent] {
    projectedIntents.filter { $0 != .finish }
  }

  private var recordingButton: some View {
    Button(action: activateRecordingControl) {
      Group {
        if recordingLight?.spins == true {
          if animates && !reduceMotion {
            TimelineView(.animation(minimumInterval: 1.0 / 30.0)) { timeline in
              recordingGlyph.overlay {
                processingArc(
                  rotation: OverlayRecordingLight.arcRotation(
                    at: timeline.date.timeIntervalSinceReferenceDate))
              }
            }
          } else {
            // Still motion: the open arc keeps the shape that tells it from red.
            recordingGlyph.overlay { processingArc(rotation: 0) }
          }
        } else if recordingLight?.pulses == true && animates && !reduceMotion {
          TimelineView(.animation(minimumInterval: 1.0 / 30.0)) { timeline in
            recordingGlyph.opacity(
              OverlayRecordingLight.pulseOpacity(at: timeline.date.timeIntervalSinceReferenceDate))
          }
        } else {
          recordingGlyph
        }
      }
      .frame(width: Self.controlDiameter, height: Self.controlDiameter)
      .contentShape(Circle())
    }
    .buttonStyle(.plain)
    .disabled(recordingDisabled)
    .opacity(recordingDisabled ? 0.45 : 1)
    .animation(
      reduceMotion ? nil : .easeInOut(duration: CSMotion.Tempo.hover), value: recordingSymbol
    )
    .csFocusOutline()
    .modifier(
      OverlayHeaderControlChrome(
        title: recordingLabel + (recordingLight.map { ". " + $0.tooltip } ?? ""),
        accent: recordingDisabled ? nil : .state, palette: palette)
    )
    .accessibilityLabel(recordingLabel)
    .accessibilityValue(recordingStatusValue)
    .accessibilityIdentifier(recordingIdentifier)
    .background {
      GeometryReader { geometry in
        Color.clear.preference(
          key: OverlayHeaderControlFramesPreferenceKey.self,
          value: OverlayHeaderControlFrames(
            stop: geometry.frame(in: .named("overlay-header")), preview: nil
          )
        )
      }
      .allowsHitTesting(false)
      .accessibilityHidden(true)
    }
  }

  private var recordingGlyph: some View {
    OverlayMicrophoneGlyph(symbol: recordingSymbol, tint: recordingTint)
  }

  /// Thin open arc over the light's circle, drawn while the take transcribes.
  private func processingArc(rotation: Double) -> some View {
    Circle()
      .inset(by: Self.processingArcWidth / 2)
      .trim(from: 0, to: OverlayRecordingLight.arcFraction)
      .stroke(
        recordingTint,
        style: StrokeStyle(lineWidth: Self.processingArcWidth, lineCap: .round)
      )
      .rotationEffect(.radians(rotation))
      .frame(width: OverlayMicrophoneGlyph.diameter, height: OverlayMicrophoneGlyph.diameter)
      .accessibilityHidden(true)
  }

  static let processingArcWidth: CGFloat = 1.5

  private var previewButton: some View {
    Button(action: togglePreview) {
      Image(systemName: previewSymbol)
        .font(.system(size: 11, weight: .semibold))
        .foregroundStyle(palette.mutedText.color)
        .frame(width: Self.controlDiameter, height: Self.controlDiameter)
        .contentShape(Circle())
        .overlay {
          Circle()
            .strokeBorder(palette.border.color, lineWidth: 1 / max(displayScale, 1))
            .accessibilityHidden(true)
        }
    }
    .contextMenu {
      agentsMenuItem
      if let onPresentationSelect {
        Picker(
          "Widget size",
          selection: Binding(get: { presentationMode }, set: { onPresentationSelect($0) })
        ) {
          Text("Mini", comment: "Widget size picker: the small tool capsule")
            .tag(OverlayPresentationMode.mini)
          Text("Bar", comment: "Widget size picker: the bar with the channel map")
            .tag(OverlayPresentationMode.midi)
          Text("Window", comment: "Widget size picker: the transcription and agents window")
            .tag(OverlayPresentationMode.expanded)
        }
        .pickerStyle(.inline)
        .accessibilityIdentifier("overlay-presentation-picker")
      }
    }
    .buttonStyle(.plain)
    .csFocusOutline()
    .modifier(
      OverlayHeaderControlChrome(
        title: previewAccessibilityLabel, accent: .state, palette: palette)
    )
    .accessibilityLabel(previewAccessibilityLabel)
    .accessibilityIdentifier("overlay-live-preview-toggle")
    .background {
      GeometryReader { geometry in
        Color.clear.preference(
          key: OverlayHeaderControlFramesPreferenceKey.self,
          value: OverlayHeaderControlFrames(
            stop: nil, preview: geometry.frame(in: .named("overlay-header"))
          )
        )
      }
      .allowsHitTesting(false)
      .accessibilityHidden(true)
    }
  }
}

struct OverlayHeaderControlFrames: Equatable {
  var stop: CGRect? = nil
  var preview: CGRect? = nil
  var waveform: CGRect? = nil
}

struct OverlayHeaderControlFramesPreferenceKey: PreferenceKey {
  static let defaultValue = OverlayHeaderControlFrames()

  static func reduce(
    value: inout OverlayHeaderControlFrames,
    nextValue: () -> OverlayHeaderControlFrames
  ) {
    let next = nextValue()
    value.stop = next.stop ?? value.stop
    value.preview = next.preview ?? value.preview
    value.waveform = next.waveform ?? value.waveform
  }
}

struct OverlayDrawerFramePreferenceKey: PreferenceKey {
  static let defaultValue = CGRect.zero
  static func reduce(value: inout CGRect, nextValue: () -> CGRect) {
    let frame = nextValue()
    if !frame.isEmpty { value = frame }
  }
}

/// The conversation's laid-out frame, in the canvas coordinate space. It keeps
/// the split width while the cell around it folds.
struct OverlayConversationFramePreferenceKey: PreferenceKey {
  static let defaultValue = CGRect.zero
  static func reduce(value: inout CGRect, nextValue: () -> CGRect) {
    let frame = nextValue()
    if !frame.isEmpty { value = frame }
  }
}

/// The folded list's rail, in the canvas coordinate space.
struct OverlayAgentRailFramePreferenceKey: PreferenceKey {
  static let defaultValue = CGRect.zero
  static func reduce(value: inout CGRect, nextValue: () -> CGRect) {
    let frame = nextValue()
    if !frame.isEmpty { value = frame }
  }
}

/// Header density, a pure function of the presentation mode. It drives one
/// header tree, so the layout never re-chooses a candidate per window frame.
enum OverlayHeaderDensity: Equatable {
  case mini, midi, expanded

  /// One row height keeps the fold control at the same offset in every form.
  static let rowHeight: CGFloat = 26

  /// Room around the row that the clip must not cut: the close dot's hover
  /// growth (10/7 of 9 pt) and the 1 pt focus outlines.
  static let clipInset: CGFloat = 4

  init(_ mode: OverlayPresentationMode) {
    switch mode {
    case .mini: self = .mini
    case .midi: self = .midi
    case .expanded: self = .expanded
    }
  }

  var brandSize: CGFloat { self == .expanded ? 15 : 13 }
  var waveformBars: Int {
    switch self {
    case .mini, .midi: return 24
    case .expanded: return 34
    }
  }
  var waveformMinWidth: CGFloat {
    switch self {
    // Mini is as wide as its content (bus brief 2026-11-04 §3): its centre
    // closes, so the brand sits as close to the first control as the
    // controls sit to one another.
    case .mini: return 0
    case .midi: return 56
    case .expanded: return 12
    }
  }
}

/// The expanded bar's shared measures (bus brief 2026-11-04 §4, §8, §11C). The
/// header lays its rows out with them and the window owner sizes the bar from
/// the same values (`DictationOverlayWindow.midiBarGeometry`).
@MainActor
enum OverlayBarMetrics {
  /// The gap between the bar's items, in every form but mini.
  static let itemSpacing: CGFloat = 10
  /// The bar's waveform keeps one width at rest and while recording, so a
  /// take starting never changes the bar's length: mini and compact's 24
  /// `WaveformView` compact bars of 1.5 pt with their 23 gaps of 2 pt.
  static let waveformWidth: CGFloat = 82
  /// Between the bar's first row and its chips: with the 2 pt the 22 pt tools
  /// leave inside the 26 pt row, 4 pt between a tool and a chip.
  static let rowGap: CGFloat = 2

  /// The single row: the header row, raised to a stacked pill's height.
  @MainActor
  static func singleRowHeight(textScale: CGFloat) -> CGFloat {
    max(
      OverlayHeaderDensity.rowHeight,
      OverlayChannelPill.stackedHeight(scale: OverlayChannelPill.headerScale(textScale)))
  }

  /// Two rows: the header row, the gap and a chip row.
  static let twoRowHeight =
    OverlayHeaderDensity.rowHeight + rowGap + OverlayChannelPill.headerHeight
}

/// The expanded bar with its channel map: brand, waveform and timer lead, the
/// tools close the row at its trailing edge, and nothing stretches between
/// them. With up to five agents the map's stacked pills sit after the timer
/// in one row; with more, or when the window owner gives the bar two rows'
/// height because the user narrowed it below the single row's floor, the map
/// is a second row of chips under the first. The window owns that choice
/// through the height it gives the bar, so the layout never picks a row
/// count the frame does not have. The map takes the width left between the
/// timer and the tools; the window's resize limits keep it within the map's
/// own floor and ceiling.
struct OverlayBarLayout: Layout {
  enum Part: Equatable {
    case leading, waveform, timer, map, tools
  }

  struct PartKey: LayoutValueKey {
    static let defaultValue: Part = .leading
  }

  let agentCount: Int
  let singleRowHeight: CGFloat
  let twoRowHeight: CGFloat
  /// The header row the brand, the clock and the tools sit in.
  let rowHeight: CGFloat
  let chipRowHeight: CGFloat
  let rowGap: CGFloat
  let spacing: CGFloat
  let mapGap: CGFloat

  /// The bar's layout for the map's agents at the overlay's text scale.
  @MainActor
  init(agentCount: Int, textScale: CGFloat) {
    self.agentCount = agentCount
    singleRowHeight = OverlayBarMetrics.singleRowHeight(textScale: textScale)
    twoRowHeight = OverlayBarMetrics.twoRowHeight
    rowHeight = OverlayHeaderDensity.rowHeight
    chipRowHeight = OverlayChannelPill.headerHeight
    rowGap = OverlayBarMetrics.rowGap
    spacing = OverlayBarMetrics.itemSpacing
    mapGap = OverlayChannelMapView.leadingGap
  }

  private func twoRows(_ proposal: ProposedViewSize) -> Bool {
    if agentCount > OverlayChannelMapModel.singleRowAgentLimit { return true }
    guard let height = proposal.height, height.isFinite else { return false }
    return height + 0.5 >= twoRowHeight
  }

  func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
    let height = twoRows(proposal) ? twoRowHeight : singleRowHeight
    if let width = proposal.width, width.isFinite { return CGSize(width: width, height: height) }
    // The ideal-size probe: the first row's items, the map in it or under it.
    let two = twoRows(proposal)
    var row: CGFloat = 0
    var map: CGFloat = 0
    for subview in subviews {
      switch subview[PartKey.self] {
      case .map:
        map = subview.sizeThatFits(
          ProposedViewSize(width: nil, height: two ? chipRowHeight : height)
        ).width
      case .leading, .waveform, .timer, .tools:
        row += subview.sizeThatFits(.unspecified).width + spacing
      }
    }
    row = max(0, row - spacing)
    let width = two ? max(row, map) : row + (map > 0 ? mapGap + map : 0)
    return CGSize(width: width, height: height)
  }

  func placeSubviews(
    in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()
  ) {
    let two = twoRows(proposal)
    let firstRowMid = two ? bounds.minY + rowHeight / 2 : bounds.midY
    var x = bounds.minX
    var map: LayoutSubview?
    var tools: LayoutSubview?
    for subview in subviews {
      switch subview[PartKey.self] {
      case .map: map = subview
      case .tools: tools = subview
      case .leading, .waveform, .timer:
        let size = subview.sizeThatFits(.unspecified)
        subview.place(
          at: CGPoint(x: x, y: firstRowMid), anchor: .leading, proposal: ProposedViewSize(size))
        x += size.width + (subview[PartKey.self] == .timer ? 0 : spacing)
      }
    }
    var trailing = bounds.maxX
    if let tools {
      let size = tools.sizeThatFits(.unspecified)
      tools.place(
        at: CGPoint(x: bounds.maxX, y: firstRowMid), anchor: .trailing,
        proposal: ProposedViewSize(size))
      trailing = bounds.maxX - size.width - spacing
    }
    guard let map else { return }
    if two {
      map.place(
        at: CGPoint(x: bounds.minX, y: bounds.minY + rowHeight + rowGap), anchor: .topLeading,
        proposal: ProposedViewSize(width: bounds.width, height: chipRowHeight))
    } else {
      x += mapGap
      let proposal = ProposedViewSize(width: max(0, trailing - x), height: singleRowHeight)
      map.place(at: CGPoint(x: x, y: bounds.midY), anchor: .leading, proposal: proposal)
    }
  }
}

/// Pointer paint owns its state outside the canvas's observation scope. Crossing
/// the window boundary changes these indicators without rebuilding its geometry.
private struct OverlayPointerHoverChrome: ViewModifier {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @State private var pointerInsideOverlay = false
  let palette: OverlayAppearancePalette
  let showsIndicators: Bool
  let onHover: (Bool) -> Void

  func body(content: Content) -> some View {
    content
      .overlay {
        if showsIndicators {
          HStack {
            Capsule().frame(width: 3, height: 28)
            Spacer()
            Capsule().frame(width: 3, height: 28)
          }
          .foregroundStyle(palette.primaryText.color)
          .padding(.horizontal, 5)
          .opacity(OverlayResizeChrome.sideIndicatorOpacity(pointerInside: pointerInsideOverlay))
          .animation(
            OverlayResizeChrome.sideIndicatorAnimation(reduceMotion: reduceMotion),
            value: pointerInsideOverlay
          )
          .transaction { transaction in
            if reduceMotion {
              transaction.animation = nil
              transaction.disablesAnimations = true
            }
          }
          .allowsHitTesting(false)
          .accessibilityHidden(true)
        }
      }
      .onHover { inside in
        guard pointerInsideOverlay != inside else { return }
        pointerInsideOverlay = inside
        onHover(inside)
      }
  }
}

/// The brand's hover changes only the close control, never the canvas layout.
private struct OverlayCloseButton: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @State private var closeDotHovered = false
  let palette: OverlayAppearancePalette
  let onClose: () -> Void

  var body: some View {
    Button(action: onClose) {
      ModeDot(color: CSColor.terracotta, size: 9)
        .overlay {
          if closeDotHovered {
            OverlayCloseCross()
              .stroke(palette.desktopBackground.color, style: StrokeStyle(lineWidth: 1))
              .accessibilityHidden(true)
          }
        }
        .scaleEffect(closeDotHovered ? 10.0 / 7.0 : 1)
        .contentShape(Circle().inset(by: -7.5))
    }
    .buttonStyle(.plain)
    .animation(
      reduceMotion ? nil : .easeOut(duration: CSMotion.Tempo.hover), value: closeDotHovered
    )
    .focusable(false)
    .modifier(
      OverlayHeaderControlChrome(
        title: OverlayIntent.close.helpText, accent: nil, palette: palette,
        onHoverChanged: { closeDotHovered = $0 })
    )
    .accessibilityLabel(OverlayIntent.close.accessibilityLabel)
    .accessibilityIdentifier("overlay-brand-close-dot")
  }
}

struct DictationOverlayView: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.colorScheme) private var colorScheme
  @Environment(\.csTextScale) private var textScale
  @Environment(\.displayScale) private var displayScale
  @Environment(\.openWindow) private var openWindow
  @AppStorage(DictationOverlayGate.labModeDefaultsKey) private var labMode = false
  @AppStorage(OverlayAgentDrawerLayout.widthDefaultsKey) private var preferredAgentDrawerWidth =
    300.0
  @State private var agentDrawerResize: (start: CGFloat, width: CGFloat)?
  @State private var overlayVisible = false
  @State private var headerHeight: CGFloat = 48
  @State private var footerHeight: CGFloat = 0
  @State private var footerDetail: String?
  @Bindable var state: OverlayState

  // Geometry constants local to this surface. The window is user-resizable;
  // content fills the frame. The resting minimum width belongs to the window
  // (`DictationOverlayWindow`); the root imposes none, because mid-morph the
  // window is narrower than any resting minimum.
  // `DictationOverlayWindow.minSize.height` MUST stay ≥ chrome + `bodyMinHeight`
  // or the canvas paints past the window rect and squares the corners.
  private let bodyMinHeight: CGFloat = 0
  private let transcriptMinHeight: CGFloat = 32
  private var palette: OverlayAppearancePalette {
    OverlayAppearancePalette.resolve(colorScheme)
  }
  private var showsDiagnostics: Bool {
    DeveloperSurface.isPowerModeEnabled(labMode: labMode)
  }
  private var bottomChromeSlots: OverlayBottomChromeSlots {
    OverlayBottomChromeSlots(
      mode: state.mode, hasPresentationStatus: state.presentationStatus != nil,
      isCollapsed: state.isCollapsed, hasLowInputSignal: state.levelMeter.hasLowInputSignal)
  }
  private var projectedIntents: [OverlayIntent] {
    OverlayIntentRail.projectedIntents(for: state)
  }
  private var railIntents: [OverlayIntent] {
    OverlayRecordingControls.railIntents(from: projectedIntents)
  }

  var body: some View {
    OverlayCanvasSurface(palette: palette) {
      // The rail reads the live transcript (intents, versions, history
      // refusal). Scoped, a capture tick re-renders the rail, not the root.
      overlayGlassContainer(
        canvasStack(
          OverlayObservationScope {
            OverlayIntentRail(
              phase: state.statusText,
              intents: railIntents,
              palette: palette,
              formatLevel: state.autoFormatLevel,
              cloudRetranscribeConfigured: state.cloudRetranscribeConfigured,
              retranscribeUnavailableReason: state.retranscribeUnavailableReason,
              historyOpenRefusal: state.archiveOpenRefusal,
              recoverSupersededLabel: state.supersededRecoveryActionLabel,
              admitHistoryOpen: { state.admitHistoryOpen() },
              onOpenArchive: { state.openArchivedTranscript($0, admission: $1) },
              onHistoryDismiss: { state.invalidateHistoryOpens() },
              versions: state.transcriptVersions,
              onVersionsOpened: { state.refreshTranscriptVersions() },
              onSelectVersion: { state.selectDocumentVersion($0) },
              onIntent: state.relayIntent,
              onRetranscribe: { state.retranscribe(pass: $0) },
              onFormatOnce: { state.formatTranscript(at: $0) }
            )
          }))
    }
    .coordinateSpace(name: "overlay-canvas")
    .csFocusPolicy()
    // The root fills the window and never asks for more width than it gets.
    // A 320 pt minimum here made the root wider than the 236 pt window at the
    // start of a mini→midi morph (isMini drops at t=0); the hosting view then
    // centred it and pushed the header 42 pt left (dragon gate 2026-10-17,
    // OverlayHeaderMorphTests @25 %, x -26; stasiu's arithmetic). At rest the
    // window's own minimum sizes keep the content at or above that width.
    .frame(minWidth: 0, maxWidth: .infinity, maxHeight: .infinity)
    // Terminal corner clip (U22): the canvas paints its background from the
    // CONTENT column's size, not the window's. Whenever the column outgrows
    // the window frame — a mid-edge-drag beat, a stale persisted size below
    // the chrome+body sum — that background used to spill past the window
    // rect and surface as a SQUARE corner under the rounded glass. Clipping
    // the whole panel to the window-frame rounded rect closes that class of
    // regression regardless of the height arithmetic. The panel shadow
    // already falls outside the borderless window (never rendered), so this
    // clip costs nothing visually.
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.window, style: .continuous))
    .overlay {
      CSFocusOutline(
        isFocused: state.isTranscriptEditable && state.isEditingTranscript,
        cornerRadius: CSRadius.window
      )
    }
    .animation(reduceMotion ? nil : CSMotion.floatIn, value: state.toast)
    .modifier(
      OverlayPointerHoverChrome(
        palette: palette, showsIndicators: !state.isCollapsed,
        onHover: { state.setPointerHovering($0) }
      )
    )

    .onAppear {
      FontLoader.register()
    }
    .onChange(of: state.channelMapInHeader) { _, _ in
      state.channelMapPreferenceChanged()
    }
  }

  @ViewBuilder
  private func overlayGlassContainer<Content: View>(_ content: Content) -> some View {
    if #available(macOS 26.0, *) {
      // One sampling region for the header and both panes' floating controls.
      // Zero merge distance keeps the existing clear gaps between them open.
      GlassEffectContainer(spacing: 0) { content }
    } else {
      content
    }
  }

  private func canvasStack<IntentRail: View>(_ intentRail: IntentRail) -> some View {
    ZStack {
      GeometryReader { geometry in
        let docked = OverlayAgentDrawerLayout.isDocked(
          windowWidth: geometry.size.width, textScale: textScale)
        let agentsSurface = state.windowContentSurface == .agents
        let transcriptionSurface = state.windowContentSurface == .transcription
        // The roster is the Agents surface. A pinned list, a remembered
        // drawer width, or no selected conversation does not mount it on
        // transcription. Beside a conversation the list stays as its corner
        // buttons left it (Maciej, 2026-10-19).
        let listOnScreen =
          agentsSurface
          && !state.isCollapsed
          && (state.showsAgentMonitor
            || OverlayAgentDrawerLayout.listPinned(
              layout: state.agentsLayout, docked: docked,
              conversationSelected: state.selectedConversation != nil))
        let columnShown = listOnScreen
        // A folded list leaves its own rail. A folded conversation leaves a
        // separate edge control on the left; neither panel owns the other's toggle.
        let railShown = agentsSurface && !state.isCollapsed && !listOnScreen
        let railWidth = OverlayAgentDrawerLayout.railWidth(textScale: textScale)
        let transcriptActionsShown = transcriptionSurface && !state.isCollapsed
        let transcriptActionsWidth =
          transcriptActionsShown
          ? OverlayTranscriptActionsLayout.columnWidth(textScale: textScale) : 0
        let conversationFolded = listOnScreen && state.agentsLayout == .agents
        let conversationEdgeWidth = min(
          geometry.size.width,
          OverlayAgentDrawerLayout.conversationEdgeWidth(textScale: textScale))
        // A dragged divider may narrow the list down to its rail: past the
        // fold width it folds there on release.
        let dragMinimumWidth: CGFloat? = agentDrawerResize == nil ? nil : railWidth
        let drawerWidth =
          conversationFolded && agentDrawerResize == nil
          ? max(0, geometry.size.width - conversationEdgeWidth)
          : OverlayAgentDrawerLayout.drawerWidth(
            textScale: textScale,
            preferredWidth: agentDrawerResize?.width ?? CGFloat(preferredAgentDrawerWidth),
            windowWidth: geometry.size.width, minimumWidth: dragMinimumWidth)
        let canvasWidth = max(
          0,
          geometry.size.width
            - (columnShown ? drawerWidth : 0) - (railShown ? railWidth : 0)
            - transcriptActionsWidth)
        // The conversation keeps its split width while its cell folds or
        // unfolds: the pane slide animates a clip, never a reflow. Laying the
        // text out at the folding cell's width wrapped it one character per
        // line in the edge column, and a stalled frame kept it there
        // (Founder, 2026-10-26). The width follows the window, the preferred
        // drawer width, a drag of the divider and the text scale, never the slide.
        let splitDrawerWidth = OverlayAgentDrawerLayout.drawerWidth(
          textScale: textScale,
          preferredWidth: agentDrawerResize?.width ?? CGFloat(preferredAgentDrawerWidth),
          windowWidth: geometry.size.width, minimumWidth: dragMinimumWidth)
        let conversationContentWidth = max(
          0,
          geometry.size.width
            - (columnShown ? splitDrawerWidth : 0) - (railShown ? railWidth : 0)
            - transcriptActionsWidth)
        let canvasHidden = canvasWidth < 1 || conversationFolded
        HStack(alignment: .top, spacing: 0) {
          ZStack {
            // Both ends are numbers, so the fold interpolates with the window's
            // morph, over the same duration (it follows the distance since
            // 2026-10-15). A `nil` height has nothing to animate from: the body
            // snapped to zero on the first frame while the glass kept shrinking
            // (presentation ledger, 2026-10-14).
            let transcriptVisible = transcriptionSurface && !state.isCollapsed
            bodySection
              .frame(height: state.isCollapsed ? 0 : geometry.size.height)
              .opacity(transcriptVisible ? 1 : 0)
              .allowsHitTesting(transcriptVisible)
              .accessibilityHidden(!transcriptVisible)
            if agentsSurface, let conversation = state.selectedConversation {
              let resolvedTarget = state.resolvedComposerTarget(viewing: conversation)
              // The inbox composes with no recipient of its own: the agents
              // linked to it are the destination (Maciej, 2026-10-30 13:58).
              let composesText = resolvedTarget != nil || conversation.isDraftSyncSource
              OverlayConversationView(
                conversation: conversation, palette: palette,
                topInset: headerHeight + 8, bottomInset: 20,
                pendingControls: state.pendingReplyControls,
                controlErrors: state.replyControlErrors,
                onControl: { message, stop in
                  Task { await state.controlReply(message, stop: stop) }
                },
                focusRevision: state.conversationFocusRevision,
                isPresented: !state.isCollapsed,
                isCovered: canvasHidden,
                draft: Binding(
                  get: { state.conversationDraft(for: conversation.id) },
                  set: { value in
                    guard composesText else { return }
                    state.setConversationDraft(value, for: conversation.id)
                  }),
                sending: state.pendingTextMessages.contains(conversation.id),
                sendError: state.draftSyncSendError(for: conversation.id)
                  ?? (state.draftCapture.errorConversationID == conversation.id
                    ? state.draftCapture.error : nil),
                onSend: {
                  guard composesText else { return }
                  Task { await state.sendConversationText(conversation) }
                },
                microphoneOpen: state.conversationMicrophoneOpen(conversation),
                microphoneEnabled: state.canToggleConversationMicrophone(conversation)
                  && !state.pendingChannelToggles.contains(conversation.channel),
                playbackMuted: state.conversationPlaybackMuted(conversation),
                playbackEnabled: conversation.channel == "0"
                  ? !state.globalPlaybackPending
                  : !state.pendingPlaybackOwners.contains(conversation.owner?.id ?? ""),
                onMicrophone: { Task { await state.toggleConversationMicrophone(conversation) } },
                onPlayback: { Task { await state.toggleConversationPlayback(conversation) } },
                playbackError: state.playbackPreferenceError,
                agentDescriptor: state.conversationAgentDescriptor(conversation),
                onFocusAgentPanel: conversation.owner.flatMap { owner in
                  state.canFocusAgentPanel(owner) ? { state.focusAgentPanel(owner) } : nil
                },
                focusAgentPanelHint: conversation.owner.flatMap { state.agentPanelFocusHelp($0) },
                onOpenOwnerConversation: { state.openConversation(for: $0) },
                navigationMenu: conversationHeaderMenu(conversation),
                navigationActions: conversationHeaderActions(conversation),
                navigationIndicators: conversationHeaderIndicators(conversation),
                composerConversation: resolvedTarget,
                composerAvailable: composesText,
                composerMicrophoneOpen: state.conversationMicrophoneOpen(conversation),
                onPrepareHeldEdit: { message in
                  state.prepareHeldComposerEdit(
                    state.heldComposerTarget(message, viewing: conversation))
                },
                onReturnHeldText: { message, text, revision in
                  state.returnHeldComposerText(
                    text, to: state.heldComposerTarget(message, viewing: conversation),
                    revision: revision
                  )
                },
                conversationNavigationRequest: state.conversationNavigationRequest,
                onComposerEditorActive: { state.noteComposerEditorActive($0) },
                onComposerTypingActivity: { state.noteComposerTypingActivity() },
                replySelection: state.conversationReplies[conversation.id],
                onClearReply: { state.conversationReplies.removeValue(forKey: conversation.id) },
                draftMicrophoneRecording: state.draftCapture.conversationID == conversation.id,
                draftMicrophoneBusy: state.draftMicrophoneBusy(in: conversation),
                onToggleDraftMicrophone: {
                  guard composesText else { return }
                  Task { await state.toggleDraftMicrophone(in: conversation) }
                },
                onStageCaptureAttachments: { paths in
                  // Only a live take of this exact agent may adopt the files;
                  // the inbox keeps them in its draft for manual Send.
                  guard state.composerTargetIsAvailable(conversation) else { return false }
                  return state.stageConversationAttachments(paths, in: conversation)
                },
                pendingCaptureAttachments: state.captureAttachmentDrafts[conversation.id]?.paths
                  ?? [],
                onRemoveCaptureAttachment: {
                  state.removeCaptureAttachment($0, in: conversation.id)
                },
                captureAttachmentError: state.captureAttachmentErrors[conversation.id],
                onRetryCaptureAttachments: {
                  Task { await state.retryCaptureAttachments(in: conversation.id) }
                },
                onReply: { message in
                  state.conversationReplies[conversation.id] = OverlayUserReplyReference(
                    message: message)
                },
                // Every channel's composer carries the recipient chevron beside
                // Send with the draft-sync set menu; the "+" stays a plain
                // attach button (Maciej, 2026-10-31 01:13Z).
                draftSyncChoices: state.draftSyncChoices(for: conversation),
                draftSyncNames: state.draftSyncNames(for: conversation.id),
                draftSyncArmed: state.armedDraftSyncConversationID == conversation.id,
                draftSyncEnabled: state.canActivateDraftSync(for: conversation.id),
                onProseEdit: { state.applyConversationProseEdit($0, for: conversation.id) },
                onToggleDraftSync: { state.toggleDraftSync(from: conversation.id, with: $0) },
                onStopDraftSync: { state.stopDraftSync(for: conversation.id) }
              )
              .opacity(state.isCollapsed ? 0 : 1)
              .allowsHitTesting(!state.isCollapsed)
              .accessibilityHidden(state.isCollapsed)
              .frame(width: conversationContentWidth, alignment: .leading)
              .background {
                GeometryReader { body in
                  Color.clear.preference(
                    key: OverlayConversationFramePreferenceKey.self,
                    value: body.frame(in: .named("overlay-canvas")))
                }
                .allowsHitTesting(false)
              }
            }
          }
          .opacity(canvasHidden ? 0 : 1)
          .allowsHitTesting(!canvasHidden)
          .accessibilityHidden(canvasHidden)
          // The cell clips the split-width conversation while it slides. A
          // folded conversation leaves only its edge; the divider beside it
          // brings it back.
          .frame(width: canvasWidth, height: geometry.size.height, alignment: .leading)
          .clipped()
          .animation(
            reduceMotion ? nil : .easeInOut(duration: state.presentationMorphDuration),
            value: state.isCollapsed
          )
          if columnShown {
            agentDrawerColumn(
              maximumHeight: geometry.size.height, width: drawerWidth,
              windowWidth: geometry.size.width
            )
            .transition(.move(edge: .trailing).combined(with: .opacity))
            .onExitCommand { state.hideAgentSidebar() }
          }
          if railShown {
            agentRail(
              maximumHeight: geometry.size.height, width: railWidth,
              windowWidth: geometry.size.width
            )
            .transition(.move(edge: .trailing).combined(with: .opacity))
          }
          if transcriptActionsShown {
            transcriptActionsColumn(
              maximumHeight: geometry.size.height, width: transcriptActionsWidth, rail: intentRail
            )
            .transition(.move(edge: .trailing).combined(with: .opacity))
          }
        }
        // Animate the column's arrival, never the bare docked flag: that flag
        // flips on the first geometry pass and would animate the transcript
        // from its intrinsic width for 240 ms (OverlayChromeV3Tests, 900 wide).
        // A corner button's choice slides the panes the same way.
        .animation(
          reduceMotion ? nil : .easeInOut(duration: CSMotion.Tempo.enter), value: columnShown
        )
        .animation(
          reduceMotion ? nil : .easeInOut(duration: CSMotion.Tempo.enter), value: railShown
        )
        .animation(
          reduceMotion ? nil : .easeInOut(duration: CSMotion.Tempo.enter),
          value: transcriptActionsShown
        )
        .animation(
          reduceMotion ? nil : .easeInOut(duration: CSMotion.Tempo.enter),
          value: state.agentsLayout
        )
        .onAppear { state.setAgentDrawerDocked(docked) }
        .onChange(of: docked) { _, docked in
          state.setAgentDrawerDocked(docked)
          if !docked { agentDrawerResize = nil }
        }
        .onChange(of: columnShown) { _, shown in
          if !shown { agentDrawerResize = nil }
        }
        // The keyboard follows what is on screen: the composer under the list
        // lets it go, and the list's selected row takes it.
        .onChange(of: canvasHidden) { _, hidden in
          if hidden { state.focusAgentRoster() }
        }
      }
      VStack(spacing: 0) {
        header
        if !state.isCollapsed,
          let label = OverlayActionsPresentation.finishingLabel(
            mode: state.mode, transcribing: state.transcribing, terminal: state.terminal)
        {
          Text(label)
            .csFont(10, .medium)
            .foregroundStyle(palette.processingStatus.color)
            .accessibilityIdentifier("overlay-finishing")
            .allowsHitTesting(false)
        }
      }
      .onGeometryChange(for: CGFloat.self) {
        $0.size.height
      } action: {
        headerHeight = $0
      }
      .frame(maxHeight: .infinity, alignment: .top)
      VStack(spacing: 0) {
        if !state.isCollapsed && state.windowContentSurface == .transcription {
          VStack(spacing: CSSpace.sm) {
            if state.completedMaxConsultationID != nil {
              Button("Continue in chat", systemImage: "bubble.left") {
                state.continueMaxConsultationInChat()
              }
              .buttonStyle(.plain)
              .csFont(10, .medium)
              .foregroundStyle(palette.primaryText.color)
              .padding(.vertical, 2)
              .accessibilityIdentifier("overlay-continue-max-chat")
            }
            // The footer reads the draft's dirty state, which moves with
            // every capture tick; scoped, only this slot re-renders.
            OverlayObservationScope {
              if footerMessage != nil || bottomChromeSlots.showsCoverageWarning {
                footerMessageRow
                  .frame(height: 18)
                  .padding(.horizontal, 20)
              }
            }

          }
          .padding(.bottom, OverlayResizeChrome.actionsBottomInset)
        } else if let label = OverlayActionsPresentation.finishingLabel(
          mode: state.mode, transcribing: state.transcribing, terminal: state.terminal)
        {
          // The folded bar keeps its height; the passive wait label uses its
          // bottom center without touching the header timer or capture state.
          Text(label)
            .csFont(10, .medium)
            .foregroundStyle(palette.processingStatus.color)
            .padding(.horizontal, 4)
            .background(palette.desktopBackground.color, in: Capsule())
            .padding(.bottom, 2)
            .accessibilityIdentifier("overlay-finishing")
            .allowsHitTesting(false)
        }
      }
      .onGeometryChange(for: CGFloat.self) {
        $0.size.height
      } action: {
        footerHeight = $0
      }
      .frame(maxHeight: .infinity, alignment: .bottom)
    }
    // The canvas is pinned to the offered width and anchored leading, so a
    // child that is wider than the window mid-morph overflows to the trailing
    // side only, where the window clips it, the same contract as the header
    // row. (The −26 pt push itself came from the root's minimum width below.)
    .frame(minWidth: 0, maxWidth: .infinity, alignment: .topLeading)
    .overlay(alignment: .bottom) {
      if !state.isCollapsed {
        // The container claims this entire bar and its vertical margin before
        // SwiftUI hit testing, then tracks .bottom with the edge resize cursor.
        Capsule()
          .fill(palette.primaryText.color.opacity(0.3))
          .frame(
            width: OverlayResizeChrome.gripSize.width, height: OverlayResizeChrome.gripSize.height
          )
          .padding(.bottom, OverlayResizeChrome.gripBottomInset)
          .allowsHitTesting(false)
          .accessibilityHidden(true)
      }
    }
    .onChange(of: state.captureGeneration) { _, _ in
      if state.windowContentSurface == .transcription {
        state.refreshRetranscriptionAvailability()
      }
    }
  }

  /// Trailing transcript actions. This is the only mount of the rail, so each
  /// available command has one control. The column uses the height under the
  /// header. A short window scrolls the overflow; a tall window does not stop
  /// at the seven-control floor.
  private func transcriptActionsColumn<Rail: View>(
    maximumHeight: CGFloat, width: CGFloat, rail: Rail
  ) -> some View {
    let availableHeight = max(
      0,
      maximumHeight - headerHeight - OverlayTranscriptActionsLayout.columnTopGap
        - OverlayTranscriptActionsLayout.columnBottomPadding)

    return
      rail
      .frame(height: availableHeight)
      .padding(.top, headerHeight + OverlayTranscriptActionsLayout.columnTopGap)
      .padding(.bottom, OverlayTranscriptActionsLayout.columnBottomPadding)
      // Keep the complete action buttons outside the native resize band.
      .padding(.trailing, OverlayResizeHit.band)
      .frame(width: width, height: max(0, maximumHeight), alignment: .top)
      .frame(maxHeight: .infinity, alignment: .top)
      .focusable(false)
      .background { OverlayMenuCursorRegion() }
      .accessibilityElement(children: .contain)
      .accessibilityIdentifier("overlay-transcript-actions")
      .onAppear { state.refreshRetranscriptionAvailability() }
  }

  /// The roster is always a side column, sized by the user and capped to
  /// leave the conversation readable. Its divider is its only fold.
  private func agentDrawerColumn(
    maximumHeight: CGFloat, width: CGFloat, windowWidth: CGFloat
  ) -> some View {
    let contentMinimum = OverlayAgentDrawerLayout.contentMinimumWidth(textScale: textScale)
    return OverlayObservationScope {
      var roster = channelStatusView
      roster.gestureFocusID = state.agentGestureFocusID
      return roster.monitorBody(
        maximumHeight: max(0, maximumHeight - headerHeight - 8 - 16),
        focusRevision: state.agentDrawerFocusRevision
      )
    }
    .padding(.top, headerHeight + 8)
    .padding(.horizontal, 8)
    .padding(.bottom, 16)
    .frame(width: max(width, contentMinimum))
    // The list lays out at its new width at once and only the column's frame
    // slides over it, anchored to the window edge: a row never re-wraps its
    // title and preview inside a half-way frame. A
    // divider dragged toward the rail keeps the rows' leading pills in view.
    .animation(nil, value: width)
    .frame(width: width, alignment: width < contentMinimum ? .leading : .trailing)
    .frame(maxHeight: .infinity, alignment: .top)
    .clipped()
    .background { OverlayMenuCursorRegion() }
    .overlay(alignment: .leading) {
      agentDrawerResizeHandle(width: width, windowWidth: windowWidth)
    }
    .accessibilityIdentifier("overlay-agent-sidebar")
    .background {
      GeometryReader { drawer in
        Color.clear.preference(
          key: OverlayDrawerFramePreferenceKey.self,
          value: drawer.frame(in: .named("overlay-canvas")))
      }
      .allowsHitTesting(false)
    }
  }

  /// The divider folds both panes; no button does. Crossing the conversation
  /// floor folds the conversation; released under the rail fold width, the
  /// list folds to its pills. Ordinary window resizing keeps the split.
  private func commitAgentDrawerWidth(_ preferred: CGFloat, windowWidth: CGFloat) {
    if preferred * textScale < OverlayAgentDrawerLayout.railFoldWidth(textScale: textScale) {
      state.foldAgentList()
      return
    }
    guard
      let width = OverlayAgentDrawerLayout.rememberedWidth(
        textScale: textScale, preferredWidth: preferred, windowWidth: windowWidth)
    else {
      state.setAgentsLayout(.agents)
      return
    }
    preferredAgentDrawerWidth = width
    if state.agentsLayout == .agents { state.setAgentsLayout(.split) }
  }

  /// The folded list beside the conversation: its pills, the minimal view of
  /// the list. Its divider, dragged toward the conversation, unfolds it.
  private func agentRail(
    maximumHeight: CGFloat, width: CGFloat, windowWidth: CGFloat
  ) -> some View {
    OverlayObservationScope {
      var roster = channelStatusView
      roster.gestureFocusID = state.agentGestureFocusID
      return roster.railBody(maximumHeight: max(0, maximumHeight - headerHeight - 8 - 16))
    }
    .padding(.top, headerHeight + 8)
    .padding(.bottom, 16)
    // The native panel owns the outer resize band before SwiftUI hit testing.
    // Equal insets keep the buttons centered in the rail and outside that band.
    .padding(.horizontal, OverlayResizeHit.band)
    .frame(width: width)
    .frame(maxHeight: .infinity, alignment: .top)
    .clipped()
    .background { OverlayMenuCursorRegion() }
    .overlay(alignment: .leading) {
      agentRailUnfoldHandle(width: width, windowWidth: windowWidth)
    }
    .accessibilityIdentifier("overlay-agent-rail")
    .background {
      GeometryReader { rail in
        Color.clear.preference(
          key: OverlayAgentRailFramePreferenceKey.self,
          value: rail.frame(in: .named("overlay-canvas")))
      }
      .allowsHitTesting(false)
    }
  }

  /// The rail keeps the list's divider. Dragged toward the conversation and
  /// released, it unfolds the list; dragged past the rows' minimum, the list
  /// opens at the dragged width.
  private func agentRailUnfoldHandle(width: CGFloat, windowWidth: CGFloat) -> some View {
    Rectangle()
      .fill(palette.border.color)
      .frame(width: 1 / max(displayScale, 1))
      .frame(width: 10, alignment: .leading)
      .contentShape(Rectangle())
      .background { OverlayMenuCursorRegion(cursor: .resizeLeftRight) }
      .gesture(
        DragGesture(minimumDistance: 2, coordinateSpace: .named("overlay-canvas"))
          .onEnded { value in
            guard -value.translation.width >= OverlayAgentDrawerLayout.railUnfoldDrag else {
              return
            }
            let dragged = (width - value.translation.width) / textScale
            if dragged * textScale
              >= OverlayAgentDrawerLayout.contentMinimumWidth(textScale: textScale),
              let remembered = OverlayAgentDrawerLayout.rememberedWidth(
                textScale: textScale, preferredWidth: dragged, windowWidth: windowWidth)
            {
              preferredAgentDrawerWidth = remembered
            }
            state.unfoldAgentList()
          }
      )
      .accessibilityElement()
      .accessibilityLabel(String(localized: "Resize agents sidebar"))
      .accessibilityAdjustableAction { direction in
        if direction == .increment { state.unfoldAgentList() }
      }
      .accessibilityIdentifier("overlay-agent-rail-resize")
      .padding(.top, headerHeight)
  }

  private func agentDrawerResizeHandle(width: CGFloat, windowWidth: CGFloat) -> some View {
    Rectangle()
      .fill(palette.border.color)
      .frame(width: 1 / max(displayScale, 1))
      .frame(width: 10, alignment: .leading)
      .contentShape(Rectangle())
      .background { OverlayMenuCursorRegion(cursor: .resizeLeftRight) }
      .gesture(
        DragGesture(minimumDistance: 0, coordinateSpace: .named("overlay-canvas"))
          .onChanged { value in
            let start = agentDrawerResize?.start ?? width / textScale
            agentDrawerResize = (start, start - value.translation.width / textScale)
          }
          .onEnded { value in
            let start = agentDrawerResize?.start ?? width / textScale
            commitAgentDrawerWidth(
              start - value.translation.width / textScale, windowWidth: windowWidth)
            agentDrawerResize = nil
          }
      )
      .accessibilityElement()
      .accessibilityLabel(String(localized: "Resize agents sidebar"))
      .accessibilityValue(Text(verbatim: "\(Int(width))"))
      .accessibilityAdjustableAction { direction in
        let step: CGFloat
        switch direction {
        case .increment: step = 20
        case .decrement: step = -20
        @unknown default: return
        }
        // Narrowing a roster that covers the window leaves the snap zone at
        // once; a 20-point step would stay inside it.
        let fills = state.agentsLayout == .agents
        // At the rows' minimum one more step down folds the list to its rail.
        let atMinimum =
          width <= OverlayAgentDrawerLayout.contentMinimumWidth(textScale: textScale) + 0.5
        let next =
          fills && step < 0
          ? OverlayAgentDrawerLayout.widestSplitWidth(
            textScale: textScale, windowWidth: windowWidth)
          : atMinimum && step < 0 ? 0 : width / textScale + step
        commitAgentDrawerWidth(next, windowWidth: windowWidth)
      }
      .accessibilityIdentifier("overlay-agent-sidebar-resize")
      .padding(.top, headerHeight)
      .onDisappear { agentDrawerResize = nil }
  }

  // MARK: Header

  private var header: some View {
    // One header tree for every presentation mode. The density is a pure
    // function of `presentationMode`, so it changes in the same animated
    // transaction as the mode: the close dot and the recording controls keep
    // one identity and glide between positions while the AppKit frame steps
    // never re-choose a layout candidate (no ViewThatFits, no cross-fade of
    // two trees; Founder report 2026-10-17 01:44Z).
    justifiedHeader(density: OverlayHeaderDensity(state.presentationMode))
      // The row is offered the full width first, so the clip below trims the
      // window's width and not the row's own ideal width. `minWidth: 0` pins
      // this frame to the offered width even when the row is wider mid-morph:
      // without it the flexible frame adopts the child's wider size, the parent
      // centres it and the leading group lands at a negative x (dragon gate
      // 2026-10-17, OverlayHeaderMorphTests mini→midi @25 %). The overflow now
      // runs to the trailing side only, where the clip trims it.
      .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
      // Hover growth of the close dot and the focus outlines live in this
      // inset; the clip then only trims a transient overflow mid-morph.
      .padding(.horizontal, OverlayHeaderDensity.clipInset)
      .padding(.vertical, OverlayHeaderDensity.clipInset)
      .clipped()
      // One clock: the window's frame morph is the only animation. A second
      // SwiftUI interpolation of spacing, font and positions is retargeted on
      // every AppKit frame step and puts controls at fractional positions
      // that overlap their neighbours mid-morph.
      .transaction { $0.animation = nil }
      // Mini hugs its controls: 7 pt from a 22 pt control to the capsule's
      // edge on every side (`DictationOverlayWindow.miniInset`).
      .padding(
        .horizontal,
        (state.isMini ? DictationOverlayWindow.miniInset : 16) - OverlayHeaderDensity.clipInset
      )
      .padding(
        .vertical,
        (state.isMini ? DictationOverlayWindow.miniVerticalPadding : 10)
          - OverlayHeaderDensity.clipInset
      )
      .coordinateSpace(name: "overlay-header")
      // Keep the explicit drag region above the passive glass background.
      // OverlayResizeHitTests verifies header dragging across its width.
      .background { OverlayWindowDragRegion(identifier: "overlay-header-drag-region") }
      // Placement, the take-start preference and the panel routes live behind a
      // secondary click anywhere on the header, drag region included, in every form.
      .contextMenu { OverlayHeaderMenu(state: state) }
      // Mini and MIDI are the header itself: the canvas already supplies their
      // glass. Raising a second sheet here draws an inset rim and shadow.
      // Keep one header identity through morphs; only its material changes.
      .modifier(
        OverlayControlGlass(
          cornerRadius: CSRadius.window, isEnabled: state.presentationMode == .expanded)
      )
      // The cached panel survives orderOut. Observe its window beside the header
      // row so the visibility probe is installed exactly once.
      .background {
        OverlayRenderVisibility(onHidden: { state.clearPointerHover() }) { visible in
          state.setConversationVisible(visible)
          guard overlayVisible != visible else { return }
          var transaction = Transaction(animation: nil)
          transaction.disablesAnimations = true
          withTransaction(transaction) { overlayVisible = visible }
        }
        .frame(width: 0, height: 0)
      }
      .onChange(of: state.presentationMode) { _, _ in
        state.setConversationVisible(overlayVisible)
      }
  }

  private var closeButton: some View {
    OverlayCloseButton(palette: palette) { state.relayIntent(.close) }
  }

  private func recordingControls(
    compact: Bool, showsMicrophone: Bool = true, showsPreview: Bool = true
  ) -> some View {
    var controls = OverlayRecordingControls(
      canFinish: state.recording && !state.transcribing,
      presentationMode: state.presentationMode, compact: compact, palette: palette,
      onIntent: state.requestHeaderRecording,
      onPreviewToggle: state.toggleCollapsed,
      isFinalizing: state.isFinalizingDictation,
      recordingLight: state.recordingLight, animates: overlayVisible)
    controls.showsRecordingButton = showsMicrophone
    controls.showsPreviewButton = showsPreview
    controls.onShowDictation = state.showTranscription
    controls.onShowAgents = { state.showAgentMonitor() }
    controls.onPresentationSelect = state.setPresentationMode
    return controls
  }

  private func justifiedHeader(density: OverlayHeaderDensity) -> some View {
    let mini = density == .mini
    // Mini's row gaps are half the 4 pt between its controls, so the gap
    // across its closed centre matches them (`DictationOverlayWindow.miniFloorWidth`).
    // Every use below is mini's: one mini in both layouts.
    let mapEnabled = state.channelMapInMini
    let map = channelMap
    let horizontalMap = density == .midi && state.channelMapInHeader && map != nil
    // The vertical layout's stacked counter is the visible way to the Intercom
    // group, so it keeps the brand's side in compact and expanded. Mini has
    // no counter (Monika, 2026-11-04: the Agents list replaces it).
    let leadingCounter = !mini && state.channelMapCounterStacked
    // Mini while a take is live (Monika, 2026-11-04, bus brief §6, §11D): it
    // grows by the waveform, the clock and the addressees, and its gaps open
    // to the 4 pt its controls keep (`DictationOverlayWindow.miniTakeWidth`).
    // It never becomes the bar; after the take it folds back to rest.
    let miniTake = mini && state.miniTakeLive
    let fixedWaveform = horizontalMap || miniTake
    // The expanded bar with its map is as wide as its content (bus brief
    // 2026-11-04 §4, §8): `OverlayBarLayout` sets the items side by side with
    // no stretching centre. Every other header keeps its row. One child list
    // serves both layouts, so the close dot and the recording controls keep
    // their identity through every morph.
    let layout =
      horizontalMap
      ? AnyLayout(
        OverlayBarLayout(agentCount: map?.model.count ?? 0, textScale: textScale))
      : AnyLayout(
        HStackLayout(
          spacing: mini
            ? (miniTake
              ? DictationOverlayWindow.miniControlSpacing : DictationOverlayWindow.miniRowSpacing)
            : OverlayBarMetrics.itemSpacing))
    return layout {
      // The shared lockup, with the close button in the brand dot's place.
      // With the channel map enabled mini shows the dot alone (Monika,
      // 2026-10-22, bus intercom spec §13.4); its counter left mini on
      // 2026-11-04 for the Agents list (bus brief §3). Without the map mini
      // keeps the word.
      HStack(
        spacing: state.channelMapCounterStacked && !mini ? 12 : DictationOverlayWindow.miniBrandGap
      ) {
        Wordmark(size: density.brandSize, spacing: mini ? 6 : 5, showsWord: !mini || !mapEnabled) {
          closeButton
        }
        .fixedSize()
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier(mini ? "" : "overlay-header-leading")
        // The brand block sits on an inert drag region, so the dot answers
        // clicks and the word moves the window.
        .background {
          OverlayWindowDragRegion(
            identifier: mini
              ? "overlay-mini-brand-drag-region" : "overlay-header-inert-drag-region")
        }
        if leadingCounter, let map {
          refreshingChordModifier(map)
        }
      }
      .layoutValue(key: OverlayBarLayout.PartKey.self, value: .leading)

      // The flexible centre keeps one identity in every density; in mini at
      // rest it collapses to a spacer so the recording controls stay
      // trailing. The expanded bar with its map and mini during a take give
      // it the bar's one fixed width instead.
      chromeWaveform(barCount: density.waveformBars)
        .frame(
          minWidth: fixedWaveform ? OverlayBarMetrics.waveformWidth : density.waveformMinWidth,
          maxWidth: fixedWaveform ? OverlayBarMetrics.waveformWidth : .infinity
        )
        .layoutPriority(-1)
        .opacity(mini && !miniTake ? 0 : 1)
        .allowsHitTesting(!mini || miniTake)
        .accessibilityHidden(mini && !miniTake)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier(mini && !miniTake ? "" : "overlay-header-center")
        .background {
          GeometryReader { geometry in
            Color.clear.preference(
              key: OverlayHeaderControlFramesPreferenceKey.self,
              value: OverlayHeaderControlFrames(
                waveform: geometry.frame(in: .named("overlay-header"))
              )
            )
          }
          .allowsHitTesting(false)
          .accessibilityHidden(true)
        }
        .layoutValue(key: OverlayBarLayout.PartKey.self, value: .waveform)
      if !mini || miniTake {
        sessionTimer
          .allowsHitTesting(false)
          .layoutValue(key: OverlayBarLayout.PartKey.self, value: .timer)
      }
      if miniTake {
        miniAddressees
      }
      // The expanded bar's channel map (bus intercom spec §5.1): stacked pills
      // after the clock, or the bar's second row (`OverlayBarLayout`).
      if horizontalMap, let map {
        refreshingChordModifier(map)
          .layoutValue(key: OverlayBarLayout.PartKey.self, value: .map)
      }

      HStack(spacing: mini ? DictationOverlayWindow.miniControlSpacing : 8) {
        transcriptionPreviewButton
        if mini {
          // Mini at rest, from the brand (Monika, 2026-11-04, bus brief §3):
          // transcription, microphone, Agents, chevron. The Agents button
          // stays whether or not an agent records (Maciej, 2026-10-20): while
          // any channel records, a press is that capture's stop, never
          // channel 0 or the monitor.
          recordingControls(compact: false, showsPreview: false)
          channelStatusView
          recordingControls(compact: false, showsMicrophone: false)
        } else {
          if let error = state.expansionPreferenceError {
            OverlayMicrophoneGlyph(
              symbol: "exclamationmark.triangle.fill", tint: palette.primaryText.color
            )
            .help(error)
            .accessibilityLabel(error)
            .accessibilityIdentifier("overlay-preference-save-error")
          }
          channelStatusView
          OverlayPinButton(state: state, palette: palette)
          recordingControls(compact: false)
        }
      }
      .fixedSize()
      .accessibilityElement(children: .contain)
      .accessibilityIdentifier(mini ? "" : "overlay-header-trailing")
      .layoutValue(key: OverlayBarLayout.PartKey.self, value: .tools)
    }
    // The bar's layout answers its own one- or two-row height.
    .frame(height: horizontalMap ? nil : OverlayHeaderDensity.rowHeight)
    .accessibilityIdentifier(mini ? "overlay-mini-widget" : "")
  }

  private var transcriptionPreviewButton: some View {
    let transcriptionSelected =
      !state.isCollapsed && state.windowContentSurface == .transcription
    return Button(action: state.showTranscription) {
      OverlayMicrophoneGlyph(
        character: OverlayHeaderGlyph.transcription,
        tint: transcriptionSelected
          ? OverlayHeaderAccent.transcription.selectionTint(in: palette)
          : palette.mutedText.color,
        plateOpacity: transcriptionSelected ? 0 : 0.12)
    }
    .buttonStyle(.plain)
    .csFocusOutline()
    .modifier(
      OverlayHeaderControlChrome(
        title: transcriptPreviewHelp, accent: .transcription, palette: palette,
        selected: transcriptionSelected)
    )
    .accessibilityLabel("Show transcription")
    .accessibilityAddTraits(transcriptionSelected ? .isSelected : [])
    .accessibilityValue(transcriptPreviewAccessibilityValue)
    .accessibilityIdentifier("overlay-transcription-preview")
  }

  /// The header's channel map, or nil while no agent is bound and channel 0
  /// is closed. The expanded bar shows its pills; mini shows none (Monika,
  /// 2026-11-04: the Agents list replaces mini's counter). A pill selects its
  /// conversation through the roster, as a rail pill does. The vertical
  /// layout has no pills: its counter, as stacked cards that open the
  /// Intercom group, stays in the bar and the window.
  /// Internal so tests can drive the header's same values.
  var channelMap: OverlayChannelMapView? {
    guard state.presentationMode != .mini else { return nil }
    let counterOnly = state.channelMapCounterStacked
    guard counterOnly || state.channelMapInHeader else { return nil }
    let roster = channelStatusView
    let model = roster.channelMapModel
    guard !model.chips.isEmpty else { return nil }
    return OverlayChannelMapView(
      model: model, placement: counterOnly ? .counter : .strip,
      modifier: state.agentChannelModifier, palette: palette,
      textScale: textScale, animates: overlayVisible,
      onSelectChannel: { roster.selectChannelConversation($0) },
      onCounter: { state.activateChannelMapCounter() },
      agentPillMenu: { roster.agentPillMenu(for: $0, showsInlineError: false) },
      stacked: state.channelMapCounterStacked)
  }

  /// Mini's addressees while a take is live (Monika, 2026-11-04, bus brief
  /// §6, §11D): one circled digit per channel the microphone feeds, in digit
  /// order, drawn by the map's own pill in the form the counter used for open
  /// channels, with the violet open-channel dot; no names and no `+N`. For
  /// channel 0 the Agents button's left-facing megaphone stands in for the
  /// circles. A dictation alone has none. A circle selects its conversation,
  /// as a bar chip does; the megaphone is a sign, not a control. The window
  /// sizes mini from the same values (`DictationOverlayWindow.miniAddresseesWidth`).
  @ViewBuilder
  private var miniAddressees: some View {
    let roster = channelStatusView
    let open = roster.channelMapModel.chips.filter(\.microphoneOpen)
    let modifier = state.agentChannelModifier
    if let broadcast = open.first(where: \.isBroadcast) {
      OverlayMicrophoneGlyph(
        symbol: "megaphone", tint: OverlayRecordingLight.agent.color, mirrored: true)
        .allowsHitTesting(false)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(verbatim: broadcast.accessibilityLabel(modifier: modifier)))
        .accessibilityIdentifier("overlay-mini-addressee-0")
    } else if !open.isEmpty {
      HStack(spacing: DictationOverlayWindow.miniControlSpacing) {
        ForEach(open) { chip in
          OverlayChannelPill(
            form: .counter,
            display: OverlayChannelPill.Display(
              figure: chip.digit, disconnected: chip.disconnected, microphoneOpen: true,
              openDigits: [chip.digit], unread: chip.unread, selected: chip.selected,
              enabled: chip.selectable, animates: overlayVisible),
            hint: chip.hint(modifier: modifier),
            accessibilityText: chip.accessibilityLabel(modifier: modifier),
            identifier: "overlay-mini-addressee-\(chip.digit)",
            palette: palette, textScale: 1
          ) {
            roster.selectChannelConversation(chip.digit)
          }
          .fixedSize()
        }
      }
      .accessibilityElement(children: .contain)
      .accessibilityIdentifier("overlay-mini-addressees")
    }
  }

  /// Hints name the chord with the stored modifier, which no settings event
  /// brings to the overlay: the map re-reads it when it appears or is hovered.
  private func refreshingChordModifier(_ map: OverlayChannelMapView) -> some View {
    map
      .onAppear { state.refreshAgentChannelModifier() }
      .onHover { inside in
        if inside { state.refreshAgentChannelModifier() }
      }
  }

  /// Internal so a test can assert that two body passes share one search model.
  var channelStatusView: OverlayChannelStatusView {
    var roster = OverlayChannelStatusView.intercomRoster(
      state: state, palette: palette, animates: overlayVisible, includesGestureFocus: false)
    roster.onWriteConversation = { conversation in
      state.chooseConversation(conversation.id)
    }
    return roster
  }

  /// A digit may be reused; roster actions belong only to the exact live lease.
  private func conversationHeaderChannel(_ conversation: OverlayConversation)
    -> OverlayChannelDelivery?
  {
    guard let owner = conversation.owner else { return nil }
    return channelStatusView.channels.first { channel in
      guard channel.channel == conversation.channel, let identity = channel.broadcastIdentity else {
        return false
      }
      return identity.provider == owner.provider && identity.session == owner.providerSessionID
        && identity.leaseID == owner.leaseID
    }
  }

  private func conversationHeaderMenu(_ conversation: OverlayConversation)
    -> OverlayBroadcastPolicyMenu?
  {
    conversationHeaderChannel(conversation).map {
      channelStatusView.agentPillMenu(for: $0.channel, showsInlineError: false)
    }
  }

  /// The header card is itself a glass surface, like the composer under it:
  /// its pair paints flat on the card, not as glass circles on glass.
  private func conversationHeaderActions(_ conversation: OverlayConversation) -> AnyView? {
    conversationHeaderChannel(conversation).map {
      AnyView(channelStatusView.compactAgentActions(for: $0, chrome: false, panel: true))
    }
  }

  private func conversationHeaderIndicators(_ conversation: OverlayConversation) -> AnyView? {
    conversationHeaderChannel(conversation).map {
      AnyView(channelStatusView.compactReceptionIndicators(for: $0))
    }
  }

  private var transcriptPreviewHelp: String {
    String(localized: "Show transcription")
  }

  private var transcriptPreviewAccessibilityValue: String {
    showsDiagnostics && state.compactProjection?.degraded == true
      ? OverlayWarningCopy.liveTranscriptBehind.sentence : ""
  }

  /// Audio-evidence strip in the primary bar. Amplitude/VAD only — word/PCM
  /// synchronized scrolling needs authenticated sample spans from projection
  /// receipts and is intentionally not invented here.
  private func chromeWaveform(barCount: Int) -> some View {
    WaveformView(
      barCount: barCount,
      active: state.audioCaptureActive,
      transcribing: state.mode == .finalizing && !state.channelAudioCaptureActive,
      indicatorMode: state.indicatorMode,
      agentRouted: state.usesAgentAccent,
      meter: state.levelMeter,
      inactiveColor: palette.border.color,
      compact: true,
      stretches: true,
      isVisible: overlayVisible
    )
    .accessibilityIdentifier("overlay-chrome-waveform")
    .accessibilityLabel("Live audio level")
    .accessibilityValue(state.audioLevelAccessibilityValue)
    .allowsHitTesting(false)
  }

  /// Live `00:00` session counter — absolute reference for audio sync and lag.
  /// Lives in the primary chrome (not a second status row). Capture end freezes
  /// the stamp so the displayed value is the session's true length.
  @ViewBuilder
  private var sessionTimer: some View {
    if !state.isMini || state.miniTakeLive {
      TimelineView(
        .animation(minimumInterval: 1, paused: !overlayVisible || state.sessionTimerPaused)
      ) { _ in
        Text(state.sessionTimerText)
          .font(CSFont.mono(11, .semibold))
          .foregroundStyle(palette.mutedText.color)
          .monospacedDigit()
          // A clock is read in one line or not at all. Without this the HStack
          // squeezed the counter before the waveform and broke `00:20` into one
          // glyph per line at the window's floor (Founder screenshot,
          // 2026-10-18 11:10). The waveform's negative layout priority makes it
          // the item that yields instead.
          .lineLimit(1)
          .fixedSize()
      }
      .accessibilityIdentifier("overlay-session-timer")
      .accessibilityLabel("Recording time")
      .accessibilityValue(state.sessionTimerText)
    }
  }

  // MARK: Body

  private var bodySection: some View {
    transcriptScroll
      .frame(maxWidth: .infinity, maxHeight: .infinity)
      .padding(.horizontal, 20)
      .background { OverlayWindowDragRegion(identifier: "overlay-body-drag-region") }
  }

  /// One message slot below the floating tools; details never grow the footer.
  private var footerMessage: String? {
    if let error = state.transcriptStorageError ?? state.maxPreparationError {
      return error
    }
    if state.archivedTranscript != nil {
      if let error = state.archiveActionError ?? state.recoveryFailure { return error }
    } else if let error = state.revisionCommitError ?? state.formatterError ?? state.recoveryFailure
    {
      return error
    }
    if state.formatterCommitPending { return String(localized: "Formatting revision…") }
    if state.revisionCommitPending { return String(localized: "Committing revision…") }
    if state.isRevisionDraftDirty { return String(localized: "Draft · not committed") }
    if let seconds = state.agentSendCountdownSeconds {
      return String(
        localized: "To Agent in \(seconds) s · edit to hold",
        comment: "Overlay footer countdown while an armed Agent take waits to send itself")
    }
    if let notice = state.toast { return notice }
    if let origin = state.archivedTranscriptOrigin { return origin }
    if let status = state.presentationStatus { return status.headline }
    if state.errorDiagnosticDetail != nil { return state.errorFooterSummary }
    if state.mode == .error {
      return state.errorMessage
        ?? (state.activeText.isEmpty
          ? String(localized: "Transcription failed")
          : String(localized: "Delivery interrupted"))
    }
    if state.mode == .noSpeech { return state.noSpeechNotice }
    return nil
  }

  /// True only for passive, informational footer lines that carry no extra
  /// diagnostic detail and have no click action. These render as one static,
  /// non-interactive label (`overlay-footer-notice`) without a duplicate
  /// floating tooltip or popover.
  private var footerMessageIsPassiveNotice: Bool {
    guard state.transcriptStorageError == nil && state.maxPreparationError == nil else {
      return false
    }
    if state.archivedTranscript != nil {
      if state.archiveActionError != nil || state.recoveryFailure != nil { return false }
    } else if state.revisionCommitError != nil || state.formatterError != nil
      || state.recoveryFailure != nil
    {
      return false
    }
    return state.formatterCommitPending || state.revisionCommitPending
      || state.isRevisionDraftDirty || state.agentSendCountdownSeconds != nil
      || state.toast != nil || state.archivedTranscriptOrigin != nil
  }

  @ViewBuilder
  private var footerMessageRow: some View {
    if let message = footerMessage {
      let opensMaxSettings = state.transcriptStorageError == nil && state.maxPreparationError != nil
      if footerMessageIsPassiveNotice {
        footerNoticeLabel(message)
          .accessibilityLabel(message)
          .allowsHitTesting(false)
      } else {
        OverlayHoverControl(
          id: "overlay-footer-message",
          title: opensMaxSettings
            ? String(localized: "Click to open Max consultation settings.") : message,
          palette: palette, presented: $footerDetail,
          action: opensMaxSettings
            ? { openMaxSettings(.creator, anchor: .maxConsultation) } : nil,
          showsTooltip: opensMaxSettings
        ) {
          footerNoticeLabel(message)
        } detail: { _ in
          footerMessageDetail(message)
        }
        .accessibilityLabel(message)
        .accessibilityHint(
          opensMaxSettings ? Text("Click to open Max consultation settings.") : Text("")
        )
      }
    } else if bottomChromeSlots.showsCoverageWarning, let warning = state.footerWarning {
      OverlayCoverageStatus(
        warning: warning, palette: palette,
        canRetranscribe: state.terminal && state.canRetranscribe,
        cloudConfigured: state.cloudRetranscribeConfigured,
        onRetranscribe: { state.retranscribe(pass: $0) }
      )
    } else {
      Color.clear.accessibilityHidden(true)
    }
  }

  private func footerNoticeLabel(_ message: String) -> some View {
    Text(message)
      .csFont(10, .medium)
      .lineLimit(1)
      .truncationMode(.tail)
      .foregroundStyle(palette.primaryText.color)
      .accessibilityIdentifier("overlay-footer-notice")
  }

  private func footerMessageDetail(_ message: String) -> some View {
    ScrollView {
      VStack(alignment: .leading, spacing: 8) {
        if state.presentationStatus != nil && state.archivedTranscript == nil {
          transcriptStatus
        } else if state.archivedTranscript == nil
          && (state.errorDiagnosticDetail != nil || state.mode == .error)
        {
          errorBody
        } else if state.mode == .noSpeech {
          noSpeechBody
        } else {
          Text(message).fixedSize(horizontal: false, vertical: true)
        }
      }
      .csAutohidingScrollers()
    }
    .frame(maxHeight: 320)
  }

  private func openMaxSettings(_ section: SettingsSection, anchor: SettingsAnchor? = nil) {
    footerDetail = nil
    SettingsDeepLink.shared.present(section, anchor: anchor)
    openWindow.presentSettings()
  }

  @ViewBuilder
  private var transcriptStatus: some View {
    if let status = state.presentationStatus {
      presentationStatusBody(status)
        .transition(reduceMotion ? .identity : .opacity.combined(with: .offset(y: 8)))
    } else {
      switch state.mode {
      case .listening, .finalizing, .formatted, .coverageRefused:
        EmptyView()
      case .noSpeech:
        noSpeechBody
          .transition(reduceMotion ? .identity : .opacity.combined(with: .offset(y: 8)))
      case .error:
        errorBody
          .transition(reduceMotion ? .identity : .opacity.combined(with: .offset(y: 8)))
      }
    }
  }

  /// Native live transcript: follows the newest words until the user clicks or
  /// selects an older phrase. The `NSTextView` keeps that selection stable across
  /// ongoing stream updates, so drag selection, Cmd-C and context-menu Copy work
  /// during recording without stopping capture. A `minHeight` reserves ~2–3 lines
  /// at the window floor.
  private var transcriptScroll: some View {
    VStack(alignment: .leading, spacing: 0) {
      LiveTranscriptTextView(
        text: state.canvasText,
        uncertainWords: state.canvasUncertainWords,
        pendingRanges: state.canvasPendingRanges,
        isEditable: state.isTranscriptEditable && !state.isCollapsed,
        appearance: palette.appearance,
        showsDiagnostics: showsDiagnostics,
        showsActivityUnderline: overlayVisible && state.animatesTranscriptCaret,
        contentInsets: NSEdgeInsets(
          top: headerHeight + 4, left: 0, bottom: footerHeight + 10, right: 0),
        onEditingChanged: { editing in
          if editing { state.beginTranscriptEdit() } else { state.endTranscriptEdit() }
        },
        onTextChange: { state.updateRevisionDraft($0) },
        onCancelEdit: { state.discardRevisionDraft() },
        onPlayUncertainWord: { state.playUncertainWord($0) },
        onTeachUncertainWord: { state.teachUncertainWord($0, canonical: $1) }
      )
      .modifier(OverlayScrollEdgeEffects())
      .frame(minHeight: transcriptMinHeight)
      .accessibilityIdentifier("overlay-transcript-area")
      // Accepted formatting painted during the take, not a finished format.
      .accessibilityValue(
        state.showsLiveFormattedPresentation
          ? Text(
            String(
              localized: "overlay.transcript.liveFormatted",
              defaultValue: "Formatted while you speak",
              comment: "Transcript area while recording: it shows accepted formatting so far"
            ))
          : Text(verbatim: "")
      )
      // The empty branch is absence of a hint, not copy, so it stays verbatim.
      .accessibilityHint(
        state.archivedTranscript != nil
          ? Text(
            "Saved transcript from history. Click to edit; changes are saved as new versions and the original is kept."
          )
          : state.isTranscriptEditable
            ? Text("Click to edit. Edits stay local until committed to the transcript ledger.")
            : Text(verbatim: "")
      )
    }
    .frame(maxWidth: .infinity, alignment: .leading)
  }

  /// Terminal outcome for a session that captured no usable speech. Replaces
  /// the empty editable FINAL with a calm, non-alarming notice (mic glyph +
  /// message). No Copy/Insert/Send — there is nothing to act on; the intent
  /// rail follows the projection table and keeps Retranscribe/Close only.
  private var noSpeechBody: some View {
    HStack(spacing: 12) {
      CSIconView(icon: .mic, size: 18, weight: .regular)
        .foregroundStyle(palette.mutedText.color)
      VStack(alignment: .leading, spacing: 2) {
        Text(state.noSpeechNotice)
          .csFont(15, .medium)
          .foregroundStyle(palette.bodyText.color)
          .fixedSize(horizontal: false, vertical: true)
        Text("No transcript was produced for this take.")
          .csFont(11, .medium)
          .foregroundStyle(palette.mutedText.color)
          .fixedSize(horizontal: false, vertical: true)
        Text(state.currentTakeRecoveryDetail)
          .csFont(11, .medium)
          .foregroundStyle(palette.mutedText.color)
          .fixedSize(horizontal: false, vertical: true)
      }
      Spacer(minLength: 0)
    }
    .frame(maxWidth: .infinity, minHeight: bodyMinHeight, alignment: .leading)
  }

  /// Terminal outcome for a recording/transcription failure. Unlike a toast, this
  /// persists after the session aborts so the overlay does not falsely report
  /// "no speech" when the engine actually failed.
  private var errorBody: some View {
    VStack(alignment: .leading, spacing: 12) {
      HStack(spacing: 12) {
        CSIconView(icon: .error, size: 18, weight: .regular)
          .foregroundStyle(palette.errorStatus.color)
        VStack(alignment: .leading, spacing: 2) {
          // "Transcription failed" is only true when there is nothing to show.
          // A take whose words exist but whose handover did not land is a
          // delivery failure, and saying otherwise buries a recoverable
          // transcript under a verdict about the audio.
          Text(
            state.errorMessage
              ?? (state.retainedComposerDelivery != nil
                ? String(localized: "Delivery interrupted — the transcript is still here")
                : state.activeText.isEmpty
                  ? String(localized: "Transcription failed")
                  : String(localized: "Delivery interrupted"))
          )
          .csFont(15, .medium)
          .foregroundStyle(palette.bodyText.color)
          .fixedSize(horizontal: false, vertical: true)
          Text(state.errorLifecycleDetail)
            .csFont(11, .medium)
            .foregroundStyle(palette.mutedText.color)
            .fixedSize(horizontal: false, vertical: true)
        }
        Spacer(minLength: 0)
      }
      if state.errorDiagnosticDetail != nil {
        Text(state.currentTakeRecoveryDetail)
          .csFont(11, .medium)
          .foregroundStyle(palette.mutedText.color)
          .fixedSize(horizontal: false, vertical: true)
      }
      if let diagnostic = state.errorDiagnosticDetail {
        DisclosureGroup("Diagnostic details") {
          Text(verbatim: diagnostic)
            .csMono(10, .medium)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
        }
        .accessibilityIdentifier("overlay-error-diagnostics")
      }
    }
    .frame(maxWidth: .infinity, minHeight: bodyMinHeight, alignment: .leading)
  }

  /// Rust supplies every word and classification. The canvas only paints the
  /// status and intentionally exposes no repair button or Settings command.
  private func presentationStatusBody(_ status: OverlayPresentationStatus) -> some View {
    HStack(spacing: 12) {
      CSIconView(icon: status.isError ? .error : .success, size: 18, weight: .regular)
        .foregroundStyle(status.isError ? palette.errorStatus.color : palette.successStatus.color)
      VStack(alignment: .leading, spacing: 4) {
        Text(status.headline)
          .csFont(15, .medium)
          .foregroundStyle(palette.bodyText.color)
          .fixedSize(horizontal: false, vertical: true)
        Text(status.message)
          .csFont(11, .medium)
          .foregroundStyle(palette.mutedText.color)
          .fixedSize(horizontal: false, vertical: true)
      }
      Spacer(minLength: 0)
    }
    .frame(maxWidth: .infinity, minHeight: bodyMinHeight, alignment: .leading)
    .accessibilityElement(children: .combine)
    .accessibilityIdentifier("overlay-presentation-status")
  }
}

/// Only reports the hosting window/view visibility; it owns no capture state.
struct OverlayRenderVisibility: NSViewRepresentable {
  let onHidden: () -> Void
  let onChange: (Bool) -> Void

  func makeNSView(context: Context) -> VisibilityView {
    let view = VisibilityView()
    view.onHidden = onHidden
    view.onChange = onChange
    return view
  }

  func updateNSView(_ nsView: VisibilityView, context: Context) {
    nsView.onHidden = onHidden
    nsView.onChange = onChange
  }

  static func dismantleNSView(_ nsView: VisibilityView, coordinator: ()) {
    NotificationCenter.default.removeObserver(nsView)
    nsView.onHidden = nil
    nsView.onChange = nil
  }

  final class VisibilityView: NSView {
    var onHidden: (() -> Void)?
    var onChange: ((Bool) -> Void)?

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func viewDidMoveToWindow() {
      super.viewDidMoveToWindow()
      NotificationCenter.default.removeObserver(self)
      if let window {
        NotificationCenter.default.addObserver(
          self, selector: #selector(visibilityChanged(_:)),
          name: NSWindow.didChangeOcclusionStateNotification, object: window)
      }
      publishVisibility()
    }

    override func viewDidHide() {
      super.viewDidHide()
      publishVisibility()
    }

    override func viewDidUnhide() {
      super.viewDidUnhide()
      publishVisibility()
    }

    @objc private func visibilityChanged(_ notification: Notification) {
      publishVisibility()
    }

    private func publishVisibility() {
      // Window attachment can happen during a SwiftUI update. Read the current
      // window on the next actor turn, so an old notification cannot revive it.
      Task { @MainActor [weak self] in
        guard let self else { return }
        // Occlusion during a frame morph suppresses painting, but does not
        // dismiss the widget or cancel the hover that is growing it.
        if window?.isVisible != true { onHidden?() }
        onChange?(
          window?.isVisible == true && window?.occlusionState.contains(.visible) == true
            && !isHiddenOrHasHiddenAncestor)
      }
    }
  }
}

private struct OverlayCloseCross: Shape {
  func path(in rect: CGRect) -> Path {
    let inset = min(rect.width, rect.height) * 0.28
    var path = Path()
    path.move(to: CGPoint(x: rect.minX + inset, y: rect.minY + inset))
    path.addLine(to: CGPoint(x: rect.maxX - inset, y: rect.maxY - inset))
    path.move(to: CGPoint(x: rect.maxX - inset, y: rect.minY + inset))
    path.addLine(to: CGPoint(x: rect.minX + inset, y: rect.maxY - inset))
    return path
  }
}

private struct OverlayScrollEdgeEffects: ViewModifier {
  @ViewBuilder
  func body(content: Content) -> some View {
    if #available(macOS 26.0, *) {
      content.scrollEdgeEffectStyle(.soft, for: .top)
    } else {
      content
    }
  }
}

#if DEBUG
  @ViewBuilder
  private func overlayPreviewCanvas<Content: View>(
    width: CGFloat? = nil,
    height: CGFloat? = nil,
    @ViewBuilder content: () -> Content
  ) -> some View {
    content()
      .frame(width: width, height: height)
      .padding(CSSpace.previewInset)
      .background(CSColor.sheet)
  }

  @MainActor
  @ViewBuilder
  private func dockPreviewRow(
    _ title: String,
    light: OverlayState,
    dark: OverlayState
  ) -> some View {
    Text(title)
      .font(.headline)
    overlayPreviewCanvas(width: 360, height: DictationOverlayWindow.minSize.height) {
      DictationOverlayView(state: light)
    }
    .preferredColorScheme(.light)
    overlayPreviewCanvas(width: 360, height: DictationOverlayWindow.minSize.height) {
      DictationOverlayView(state: dark)
    }
    .preferredColorScheme(.dark)
  }

  #Preview("Dock matrix · 360 pt") {
    ScrollView {
      VStack(spacing: CSSpace.section) {
        dockPreviewRow("Listening", light: .previewListening(), dark: .previewListening())
        dockPreviewRow(
          "Listening · evidence", light: .previewListeningWithEvidence(),
          dark: .previewListeningWithEvidence())
        dockPreviewRow(
          "Finalizing", light: .previewTranscribing(), dark: .previewTranscribing())
        dockPreviewRow("Formatted", light: .previewFormatted(), dark: .previewFormatted())
        dockPreviewRow("No speech", light: .previewNoSpeech(), dark: .previewNoSpeech())
        dockPreviewRow("Error", light: .previewError(), dark: .previewError())
      }
      .padding()
    }
  }

  #Preview("Listening") {
    Group {
      overlayPreviewCanvas {
        DictationOverlayView(state: .previewListening())
      }
      .preferredColorScheme(.light)

      overlayPreviewCanvas {
        DictationOverlayView(state: .previewListening())
      }
      .preferredColorScheme(.dark)
    }
  }

  #Preview("Listening · evidence") {
    // A refused Whisper alternative sits beside the canvas as secondary,
    // non-committed text; the canvas keeps the committed words only.
    overlayPreviewCanvas(width: 360, height: 300) {
      DictationOverlayView(state: .previewListeningWithEvidence())
    }
  }

  #Preview("Transcribing") {
    // Pinned to the window's min content size so this preview doubles as the
    // min-size regression check: "transcribing…" fills the main status slot and
    // the transcript reserves ~2–3 lines instead of collapsing at the floor.
    overlayPreviewCanvas(width: 360, height: DictationOverlayWindow.minSize.height) {
      DictationOverlayView(state: .previewTranscribing())
    }
  }

  #Preview("No speech") {
    // Session ended without usable text: dedicated notice body, no
    // Copy/Format/Send, only Close. Pinned to the min content size so it also
    // guards the floor layout for this outcome.
    overlayPreviewCanvas(width: 360, height: DictationOverlayWindow.minSize.height) {
      DictationOverlayView(state: .previewNoSpeech())
    }
  }

  #Preview("Formatted") {
    overlayPreviewCanvas {
      DictationOverlayView(state: .previewFormatted())
    }
  }

  #Preview("Formatted · compact chrome") {
    overlayPreviewCanvas(width: 340, height: DictationOverlayWindow.minSize.height) {
      DictationOverlayView(state: .previewFormatted())
    }
  }

  #Preview("Listening · scaled 1.4x") {
    // Exercises `\.csTextScale`: transcript + status render 40% larger while the
    // window chrome and paddings keep their intrinsic geometry (transcript scrolls
    // rather than forcing the panel taller).
    overlayPreviewCanvas(width: 470, height: 280) {
      DictationOverlayView(state: .previewListening())
        .environment(\.csTextScale, 1.4)
    }
  }
#endif

/// Evaluates its content in a body of its own. Observation attributes the
/// content's reads to this node, so a value that moves with every capture tick
/// re-renders this slot instead of the whole overlay. Unscoped, each tick re-ran
/// the root body, header, channel map and intercom included: the main thread
/// sat 94-98% busy through a take (UI-88, Silver samples, 23 X).
struct OverlayObservationScope<Content: View>: View {
  @ViewBuilder let content: () -> Content

  var body: some View { content() }
}
