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
      Button("My dictation", systemImage: "waveform") { onShowDictation?() }
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
      if let onPresentationSelect {
        Picker(
          "Widget size",
          selection: Binding(get: { presentationMode }, set: { onPresentationSelect($0) })
        ) {
          Text("Collapse widget").tag(OverlayPresentationMode.mini)
          Text("Compact").tag(OverlayPresentationMode.midi)
          Text("Expanded").tag(OverlayPresentationMode.expanded)
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
    // Mini is as wide as its content (spec §13.4): its centre closes, so the
    // counter sits as close to the agents button as the trailing controls
    // sit to one another.
    case .mini: return 0
    case .midi: return 56
    case .expanded: return 12
    }
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
  @State private var closeDotHovered = false
  @State private var pointerInsideOverlay = false
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
        })
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
    .onHover { inside in
      pointerInsideOverlay = inside
      state.setPointerHovering(inside)
    }

    .onAppear {
      FontLoader.register()
    }
    .onChange(of: state.channelMapInHeader) { _, _ in
      state.channelMapPreferenceChanged()
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
        let drawerWidth =
          conversationFolded && agentDrawerResize == nil
          ? max(0, geometry.size.width - conversationEdgeWidth)
          : OverlayAgentDrawerLayout.drawerWidth(
            textScale: textScale,
            preferredWidth: agentDrawerResize?.width ?? CGFloat(preferredAgentDrawerWidth),
            windowWidth: geometry.size.width)
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
          windowWidth: geometry.size.width)
        let conversationContentWidth = max(
          0,
          geometry.size.width
            - (columnShown ? splitDrawerWidth : 0) - (railShown ? railWidth : 0)
            - transcriptActionsWidth)
        let canvasHidden = canvasWidth < 1 || conversationFolded
        // The conversation toggle has a reserved row, clear of message text.
        let conversationControlsHeight: CGFloat = columnShown ? 30 * textScale : 0
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
              OverlayConversationView(
                conversation: conversation, palette: palette,
                topInset: headerHeight + 8 + conversationControlsHeight, bottomInset: 20,
                pendingControls: state.pendingReplyControls,
                controlErrors: state.replyControlErrors,
                onControl: { message, stop in
                  Task { await state.controlReply(message, stop: stop) }
                },
                focusRevision: state.conversationFocusRevision,
                isPresented: !state.isCollapsed,
                isCovered: canvasHidden,
                draft: Binding(
                  get: { resolvedTarget.map { state.conversationDraft(for: $0.id) } ?? "" },
                  set: { value in
                    guard let target = resolvedTarget, state.composerTargetIsAvailable(target)
                    else { return }
                    state.setConversationDraft(value, for: target.id)
                  }),
                sending: resolvedTarget.map { state.pendingTextMessages.contains($0.id) } ?? false,
                sendError: resolvedTarget.flatMap { target in
                  state.draftSyncSendError(for: target.id)
                    ?? (state.draftCapture.errorConversationID == target.id
                      ? state.draftCapture.error : nil)
                },
                onSend: {
                  guard let target = resolvedTarget, state.composerTargetIsAvailable(target) else {
                    return
                  }
                  Task {
                    guard state.composerTargetIsAvailable(target) else { return }
                    await state.sendConversationText(target)
                  }
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
                navigationActions: conversationHeaderActions(conversation),
                navigationIndicators: conversationHeaderIndicators(conversation),
                composerConversation: resolvedTarget,
                composerAvailable: resolvedTarget != nil,
                composerMicrophoneOpen: resolvedTarget.map { state.conversationMicrophoneOpen($0) },
                composerRouting: AnyView(composerRouting(viewing: conversation)),
                onPrepareHeldEdit: { message in
                  state.prepareHeldComposerEdit(heldComposerTarget(message, viewing: conversation))
                },
                onReturnHeldText: { message, text, revision in
                  state.returnHeldComposerText(
                    text, to: heldComposerTarget(message, viewing: conversation), revision: revision
                  )
                },
                onNavigateComposerReply: resolvedTarget?.id == conversation.id
                  ? nil
                  : {
                    guard let target = resolvedTarget, state.composerTargetIsAvailable(target),
                      let messageID = state.conversationReplies[target.id]?.messageID
                    else { return }
                    state.navigateToConversationMessage(target, messageID: messageID)
                  },
                conversationNavigationRequest: state.conversationNavigationRequest,
                onComposerEditorActive: { state.noteComposerEditorActive($0) },
                onComposerTypingActivity: { state.noteComposerTypingActivity() },
                replySelection: resolvedTarget.flatMap { state.conversationReplies[$0.id] },
                onClearReply: {
                  guard let target = resolvedTarget, state.composerTargetIsAvailable(target) else {
                    return
                  }
                  state.conversationReplies.removeValue(forKey: target.id)
                },
                draftMicrophoneRecording: resolvedTarget.map {
                  state.draftCapture.conversationID == $0.id
                } ?? false,
                draftMicrophoneBusy: resolvedTarget.map { state.draftMicrophoneBusy(in: $0) }
                  ?? true,
                onToggleDraftMicrophone: {
                  guard let target = resolvedTarget, state.composerTargetIsAvailable(target) else {
                    return
                  }
                  Task {
                    guard state.composerTargetIsAvailable(target) else { return }
                    await state.toggleDraftMicrophone(in: target)
                  }
                },
                onStageCaptureAttachments: { paths in
                  guard let target = resolvedTarget, state.composerTargetIsAvailable(target) else {
                    return false
                  }
                  return state.stageConversationAttachments(paths, in: target)
                },
                pendingCaptureAttachments: resolvedTarget.flatMap {
                  state.captureAttachmentDrafts[$0.id]?.paths
                } ?? [],
                onRemoveCaptureAttachment: { path in
                  guard let target = resolvedTarget, state.composerTargetIsAvailable(target) else {
                    return
                  }
                  state.removeCaptureAttachment(path, in: target.id)
                },
                captureAttachmentError: resolvedTarget.flatMap {
                  state.captureAttachmentErrors[$0.id]
                },
                onRetryCaptureAttachments: {
                  guard let target = resolvedTarget, state.composerTargetIsAvailable(target) else {
                    return
                  }
                  Task {
                    guard state.composerTargetIsAvailable(target) else { return }
                    await state.retryCaptureAttachments(in: target.id)
                  }
                },
                onReply: { message in
                  state.chooseComposerTarget(conversation)
                  state.conversationReplies[conversation.id] = OverlayUserReplyReference(
                    message: message)
                },
                draftSyncChoices: resolvedTarget.map { state.draftSyncChoices(for: $0) } ?? [],
                draftSyncNames: resolvedTarget.map { state.draftSyncNames(for: $0.id) } ?? [],
                draftSyncArmed: resolvedTarget.map {
                  state.armedDraftSyncConversationID == $0.id
                } ?? false,
                draftSyncEnabled: resolvedTarget.map { state.canActivateDraftSync(for: $0.id) }
                  ?? false,
                onActivateDraftSync: {
                  guard let target = resolvedTarget, state.composerTargetIsAvailable(target) else {
                    return
                  }
                  state.activateDraftSync(for: target.id)
                },
                onProseEdit: { edit in
                  guard let target = resolvedTarget, state.composerTargetIsAvailable(target) else {
                    return
                  }
                  state.applyConversationProseEdit(edit, for: target.id)
                },
                onToggleDraftSync: { id in
                  guard let target = resolvedTarget, state.composerTargetIsAvailable(target) else {
                    return
                  }
                  state.toggleDraftSync(from: target.id, with: id)
                },
                onStopDraftSync: {
                  guard let target = resolvedTarget, state.composerTargetIsAvailable(target) else {
                    return
                  }
                  state.stopDraftSync(for: target.id)
                }
              )
              .onAppear { state.initializeComposerTarget(conversation) }
              .onChange(of: conversation.id) { _, _ in
                state.initializeComposerTarget(conversation)
              }
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
          // Keep the conversation and its reveal edge in the same cell. Its
          // control stays usable when the conversation itself is folded. The
          // cell clips the split-width conversation while it slides.
          .frame(width: canvasWidth, height: geometry.size.height, alignment: .leading)
          .clipped()
          .overlay(alignment: .topLeading) {
            if columnShown {
              conversationEdgeControls(canvasShown: !canvasHidden)
                .padding(.top, headerHeight + 16)
            }
          }
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
            agentRail(maximumHeight: geometry.size.height, width: railWidth)
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
    .overlay {
      if !state.isCollapsed {
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
  /// leave the conversation readable. Its header owns only its own fold.
  private func agentDrawerColumn(
    maximumHeight: CGFloat, width: CGFloat, windowWidth: CGFloat
  ) -> some View {
    var roster = channelStatusView
    roster.sidebarControls = agentsSidebarControls(canvasShown: state.agentsLayout != .agents)
    return roster.monitorBody(
      maximumHeight: max(0, maximumHeight - headerHeight - 8 - 16),
      focusRevision: state.agentDrawerFocusRevision
    )
    .padding(.top, headerHeight + 8)
    .padding(.horizontal, 8)
    .padding(.bottom, 16)
    .frame(width: width)
    // The list lays out at its new width at once and only the column's frame
    // slides over it, anchored to the window edge: a row never re-wraps its
    // title and preview inside a half-way frame (Founder, 2026-10-26).
    .animation(nil, value: width)
    .frame(width: width, alignment: .trailing)
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

  /// A real slot on the conversation's own edge, with equal insets on both
  /// sides. The row above the messages is reserved by their top inset.
  private func conversationEdgeControls(canvasShown: Bool) -> some View {
    let controls = agentsSidebarControls(canvasShown: canvasShown)
    return HStack(spacing: 0) {
      OverlaySidebarToggleButton(
        symbol: "sidebar.left", title: controls.canvasTitle,
        identifier: "overlay-agent-sidebar-toggle-canvas", palette: palette,
        textScale: textScale, action: controls.onToggleCanvas)
      Spacer(minLength: 0)
      if canvasShown {
        Color.clear
          .frame(width: 22 * textScale, height: 22 * textScale)
          .allowsHitTesting(false)
          .accessibilityHidden(true)
      }
    }
    .padding(.horizontal, 8 * textScale)
  }

  private func agentsSidebarControls(canvasShown: Bool) -> OverlayAgentsSidebarControls {
    OverlayAgentsSidebarControls(
      canvasShown: canvasShown,
      canvasIsTranscription: state.windowContentSurface == .transcription,
      onToggleCanvas: {
        if canvasShown {
          state.hideAgentsCanvas()
        } else {
          state.showAgentsCanvas()
        }
      },
      onFoldList: { state.foldAgentList() })
  }

  /// Preserve the existing explicit divider gesture: crossing the conversation
  /// floor folds that pane, while ordinary window resizing keeps the split.
  private func commitAgentDrawerWidth(_ preferred: CGFloat, windowWidth: CGFloat) {
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

  /// The folded list beside the conversation: its pills, under the
  /// `sidebar.right` that unfolds it, where the list's own button was.
  private func agentRail(maximumHeight: CGFloat, width: CGFloat) -> some View {
    channelStatusView.railBody(
      maximumHeight: max(0, maximumHeight - headerHeight - 8 - 16),
      onUnfold: { state.unfoldAgentList() }
    )
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
      Rectangle()
        .fill(palette.border.color)
        .frame(width: 1 / max(displayScale, 1))
        .padding(.top, headerHeight)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
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
        let next =
          fills && step < 0
          ? OverlayAgentDrawerLayout.widestSplitWidth(
            textScale: textScale, windowWidth: windowWidth)
          : width / textScale + step
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
      .padding(.horizontal, 16 - OverlayHeaderDensity.clipInset)
      .padding(.vertical, 10 - OverlayHeaderDensity.clipInset)
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
    Button {
      state.relayIntent(.close)
    } label: {
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
    .background {
      OverlayMenuCursorRegion { closeDotHovered = $0 }
    }
    .animation(
      reduceMotion ? nil : .easeOut(duration: CSMotion.Tempo.hover), value: closeDotHovered
    )
    .focusable(false)
    .modifier(
      OverlayHeaderControlChrome(
        title: OverlayIntent.close.helpText, accent: nil, palette: palette)
    )
    .accessibilityLabel(OverlayIntent.close.accessibilityLabel)
    .accessibilityIdentifier("overlay-brand-close-dot")
  }

  private func recordingControls(compact: Bool, showsMicrophone: Bool = true) -> some View {
    var controls = OverlayRecordingControls(
      canFinish: state.recording && !state.transcribing,
      presentationMode: state.presentationMode, compact: compact, palette: palette,
      onIntent: state.requestHeaderRecording,
      onPreviewToggle: state.toggleCollapsed,
      isFinalizing: state.isFinalizingDictation,
      recordingLight: state.recordingLight, animates: overlayVisible)
    controls.showsRecordingButton = showsMicrophone
    controls.onShowDictation = state.showTranscription
    controls.onPresentationSelect = state.setPresentationMode
    return controls
  }

  private func justifiedHeader(density: OverlayHeaderDensity) -> some View {
    let mini = density == .mini
    // Mini's row gaps are half the 8 pt between its trailing controls, so the
    // gap across its closed centre matches them (`DictationOverlayWindow.miniFloorWidth`).
    // Every use below is mini's: one mini in both layouts.
    let mapEnabled = state.channelMapInMini
    let horizontalMap = density == .midi && state.channelMapInHeader && channelMap != nil
    // The vertical layout's stacked counter is the visible way to the Intercom
    // group, so it keeps the brand's side in compact and expanded as in mini.
    let leadingCounter = mini || state.channelMapCounterStacked
    return HStack(spacing: mini ? (mapEnabled ? DictationOverlayWindow.miniRowSpacing : 6) : 10) {
      // The shared lockup, with the close button in the brand dot's place.
      // Monika, 2026-10-22 (bus intercom spec §13.4): "cs" leaves mini. Its
      // brand dot leads, and the channel counter takes the word's place: the
      // agent count, the agents on hover, the open channel's digit with the
      // violet open-channel dot (spec §14). This replaces Maciej's 2026-10-20 rule that mini
      // always reads "cs"; his agents button and its stop meaning stay.
      HStack(spacing: DictationOverlayWindow.miniBrandGap) {
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
        if leadingCounter, let map = channelMap {
          refreshingChordModifier(map)
        }
      }

      // The flexible centre keeps one identity in every density; in mini it
      // collapses to a spacer so the recording controls stay trailing.
      chromeWaveform(barCount: density.waveformBars)
        .frame(
          minWidth: mini && !mapEnabled ? 4 : density.waveformMinWidth,
          maxWidth: horizontalMap ? 120 : .infinity
        )
        .layoutPriority(-1)
        .opacity(mini ? 0 : 1)
        .allowsHitTesting(!mini)
        .accessibilityHidden(mini)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier(mini ? "" : "overlay-header-center")
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
      if !mini {
        // The compact strip carries the channel map between the clock and the
        // actions (bus intercom spec §5.1). The map brings its own leading
        // gap, so where it yields its place the strip keeps today's spacing.
        HStack(spacing: 0) {
          sessionTimer
            .allowsHitTesting(false)
          if density == .midi, state.channelMapInHeader, let map = channelMap {
            refreshingChordModifier(map)
          }
        }
        // Names and their separate actions take useful width before the
        // audio meter expands. The map itself degrades when the window is
        // genuinely narrow; global controls retain their fixed width.
        .layoutPriority(horizontalMap ? 1 : 0)
      }
      if horizontalMap {
        Spacer(minLength: 0)
          .layoutPriority(-2)
      }

      HStack(spacing: 8) {
        if !mini {
          transcriptionPreviewButton
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
        } else {
          // Maciej, 2026-10-20: mini always carries the agents button beside
          // the ordinary microphone, whether or not an agent records. Single-agent
          // capture uses a microphone; only All uses the left-facing megaphone
          // (Founder, 2026-10-22). While any channel records, a press here is
          // that capture's stop, never channel 0 or the monitor.
          channelStatusView
        }
        recordingControls(compact: false)
      }
      .fixedSize()
      .accessibilityElement(children: .contain)
      .accessibilityIdentifier(mini ? "" : "overlay-header-trailing")
    }
    .frame(height: OverlayHeaderDensity.rowHeight)
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

  /// The collapsed header's channel map, or nil while no agent is bound and
  /// channel 0 is closed. Mini shows its counter, compact its chips. A chip
  /// selects its conversation through the roster, as a rail pill does; the
  /// counter switches mini to compact, or returns a narrowed compact strip to
  /// its natural width (spec §13.4). Wide named chips have separate audio and
  /// panel controls. The vertical layout has no chips: its counter, as stacked
  /// cards that open the Intercom group, stays in every form, so widening the
  /// overlay never removes the way to the group.
  /// Internal so tests can drive the header's same values.
  var channelMap: OverlayChannelMapView? {
    let counterOnly = state.presentationMode == .mini || state.channelMapCounterStacked
    guard counterOnly ? state.channelMapInMini : state.channelMapInHeader else { return nil }
    let roster = channelStatusView
    let model = roster.channelMapModel
    guard !model.chips.isEmpty else { return nil }
    var controls: [String: OverlayChannelMapControls] = [:]
    for channel in roster.channels {
      let digit = channel.channel
      let focusState = roster.panelFocusStates[digit]
      let showsFocus = roster.showsPanelFocusButton(for: channel)
      controls[digit] = OverlayChannelMapControls(
        microphoneEnabled: !roster.unavailable && roster.onToggleChannel != nil
          && !roster.pendingToggleChannels.contains(digit)
          && OverlayChannelStatusView.toggleDigit(for: digit) != nil
          && !roster.hasDeadFollower(channel),
        muted: roster.mutedChannels[digit],
        playbackEnabled: roster.onTogglePlayback != nil
          && !roster.pendingMuteChannels.contains(digit),
        speaking: roster.conversation(for: channel)?.replySpeaking == true,
        focusSymbol: showsFocus
          ? roster.panelFocusSymbol(
            focusState,
            target: roster.panelFocusTarget(for: channel)
              ?? (roster.hasCodexCloudThread(channel) ? .codexThread : nil)) : nil,
        focusLabel: showsFocus ? roster.panelFocusHelp(for: channel, state: focusState) : "",
        focusEnabled: !roster.panelFocusUnreachable(channel),
        focusPending: focusState?.isRequesting == true,
        focusSucceeded: focusState?.isSucceeded == true,
        focusFailed: focusState?.isFailed == true)
    }
    return OverlayChannelMapView(
      model: model, placement: counterOnly ? .mini : .strip,
      modifier: state.agentChannelModifier, palette: palette,
      textScale: textScale, animates: overlayVisible,
      onSelectChannel: { roster.selectChannelConversation($0) },
      onCounter: { state.activateChannelMapCounter() },
      controls: controls,
      onToggleMicrophone: { digit in
        guard let channel = roster.channels.first(where: { $0.channel == digit }) else { return }
        roster.toggle(channel)
      },
      onTogglePlayback: { roster.onTogglePlayback?($0) },
      onFocusChannel: { digit in
        guard let channel = roster.channels.first(where: { $0.channel == digit }),
          roster.showsPanelFocusButton(for: channel),
          !roster.panelFocusUnreachable(channel),
          roster.panelFocusStates[digit]?.isRequesting != true,
          let owner = roster.archiveCandidates[digit]
        else { return }
        roster.onFocusAgentPanel?(owner)
      },
      agentPillMenu: { roster.agentPillMenu(for: $0, showsInlineError: false) },
      stacked: state.channelMapCounterStacked)
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
      state: state, palette: palette, animates: overlayVisible)
    roster.onWriteConversation = { conversation in
      state.chooseComposerTarget(conversation)
      state.chooseConversation(conversation.id)
    }
    return roster
  }

  private func heldComposerTarget(
    _ message: OverlayConversationMessage, viewing conversation: OverlayConversation
  ) -> OverlayConversation {
    message.sourceChannel == "0"
      ? OverlayConversation(id: "0", channel: "0", name: "All", owner: nil, messages: [])
      : conversation
  }

  private func composerRouting(viewing conversation: OverlayConversation) -> some View {
    let target = state.composerTarget
    let selected = state.resolvedComposerTarget(viewing: conversation)
    let choices = state.availableComposerRecipients
    let openChannels = state.channelHudStates.filter { $0.value.open }.keys.sorted()
    let openChannelTitles = openChannels.map { digit in
      if digit == "0" { return String(localized: "0 · All") }
      if let row = state.visibleChannelRows.first(where: { $0.channel == digit }) {
        return "\(digit) · \(row.agent)"
      }
      return digit
    }
    return VStack(alignment: .leading, spacing: 4) {
      HStack(spacing: 6) {
        Text("To:")
        Menu {
          ForEach(choices) { choice in
            Button(choice.displayTitle) { state.chooseComposerTarget(choice) }
          }
        } label: {
          Text(verbatim: selected?.displayTitle ?? target?.title ?? "—")
            .lineLimit(1).truncationMode(.middle)
        }
        .accessibilityIdentifier("overlay-composer-recipient")
        if conversation.owner != nil || conversation.channel == "0",
          selected?.id != conversation.id || selected?.owner?.id != conversation.owner?.id
        {
          Button {
            state.chooseComposerTarget(conversation)
          } label: {
            Text("Write to \(conversation.displayTitle)")
          }
          .buttonStyle(.plain)
        }
        Spacer(minLength: 0)
      }
      if target != nil && selected == nil {
        Text("Recipient unavailable")
          .accessibilityIdentifier("overlay-composer-recipient-unavailable")
      }
      HStack(spacing: 4) {
        Image(systemName: "waveform.badge.mic")
        if openChannels.isEmpty {
          Text("Voice channels closed")
        } else {
          Text("Voice channels:")
          Text(verbatim: openChannelTitles.joined(separator: ", "))
            .lineLimit(1).truncationMode(.middle)
        }
      }
    }
    .font(.system(size: 11 * textScale, weight: .medium))
    .foregroundStyle(palette.mutedText.color)
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

  private func conversationHeaderActions(_ conversation: OverlayConversation) -> AnyView? {
    conversationHeaderChannel(conversation).map {
      AnyView(channelStatusView.compactAgentActions(for: $0))
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
    if !state.isMini {
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
