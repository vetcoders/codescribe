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
    hasLowInputSignal: Bool = false, showsDiagnostics: Bool = false
  ) {
    // Ledger diagnostics belong to developer power mode. Measured quiet-mic
    // advice remains available in production.
    let technicalCoverageRefused = mode == .coverageRefused && showsDiagnostics
    let lowInputAdvisory = mode == .listening && hasLowInputSignal
    if isCollapsed {
      ordered = []
    } else if !hasPresentationStatus && (technicalCoverageRefused || lowInputAdvisory) {
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
    case .mini: String(localized: "Expand to compact widget")
    case .midi: String(localized: "Expand transcript")
    case .expanded: String(localized: "Collapse widget")
    }
  }
  /// Expanding the widget is always an explicit click.
  var previewSymbol: String {
    switch presentationMode {
    case .mini: OverlayControlSymbols.miniToMidi
    case .midi: OverlayControlSymbols.midiToTranscript
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
    withAnimation(reduceMotion ? nil : .easeInOut(duration: 0.16)) {
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
        if recordingLight?.pulses == true && animates && !reduceMotion {
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
    .animation(reduceMotion ? nil : .easeInOut(duration: 0.16), value: recordingSymbol)
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
          "Live preview",
          selection: Binding(get: { presentationMode }, set: { onPresentationSelect($0) })
        ) {
          Text("Collapse widget").tag(OverlayPresentationMode.mini)
          Text("Compact").tag(OverlayPresentationMode.midi)
          Text("Transcription").tag(OverlayPresentationMode.expanded)
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

/// Header density, a pure function of the presentation mode. It drives one
/// header tree, so the layout never re-chooses a candidate per window frame.
enum OverlayHeaderDensity: Equatable {
  case mini, midi, expanded

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
    case .mini: return 4
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
  @AppStorage("Overlay.agentDrawerWidth.v1") private var preferredAgentDrawerWidth = 300.0
  @State private var agentDrawerResize: (start: CGFloat, width: CGFloat)?
  @State private var closeDotHovered = false
  @Namespace private var bottomChromeNamespace
  @State private var actions = OverlayActionsPresentation()
  @FocusState private var actionsFocused: Bool
  @State private var pointerInsideOverlay = false
  @State private var overlayVisible = false
  @State private var headerHeight: CGFloat = 48
  @State private var footerHeight: CGFloat = 64
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
      isCollapsed: state.isCollapsed, hasLowInputSignal: state.levelMeter.hasLowInputSignal,
      showsDiagnostics: showsDiagnostics)
  }
  private var projectedIntents: [OverlayIntent] {
    OverlayIntentRail.projectedIntents(for: state)
  }
  private var railIntents: [OverlayIntent] {
    OverlayRecordingControls.railIntents(from: projectedIntents)
  }

  var body: some View {
    OverlayCanvasSurface(palette: palette) {
      canvasStack(
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
          onFormatOnce: { state.formatTranscript(at: $0) },
          onDismiss: { actions.dismiss() },
          onInteraction: { actions.interact() },
          onPresentationChange: { actions.panelChanged($0) }
        )
      )
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
  }

  @ViewBuilder
  private func bottomChromeContainer<IntentRail: View>(
    _ intentRail: IntentRail
  ) -> some View {
    if #available(macOS 26.0, *) {
      GlassEffectContainer(spacing: 0) { intentRail }
    } else {
      intentRail
    }
  }

  private func canvasStack<IntentRail: View>(_ intentRail: IntentRail) -> some View {
    ZStack {
      GeometryReader { geometry in
        let docked = OverlayAgentDrawerLayout.isDocked(
          windowWidth: geometry.size.width, textScale: textScale)
        // The column is permanent beside a conversation; the transcription
        // keeps its full width until the Agents button asks for the roster.
        let columnShown =
          docked && !state.isCollapsed
          && (state.selectedConversation != nil || state.showsAgentMonitor)
        let drawerWidth = OverlayAgentDrawerLayout.drawerWidth(
          textScale: textScale,
          preferredWidth: agentDrawerResize?.width ?? CGFloat(preferredAgentDrawerWidth),
          windowWidth: geometry.size.width)
        let canvasWidth = max(
          0,
          geometry.size.width
            - (columnShown ? drawerWidth : 0))
        HStack(alignment: .top, spacing: 0) {
          ZStack {
            // Both ends are numbers, so the fold interpolates with the window's
            // morph, over the same duration (it follows the distance since
            // 2026-10-15). A `nil` height has nothing to animate from: the body
            // snapped to zero on the first frame while the glass kept shrinking
            // (presentation ledger, 2026-10-14).
            bodySection
              .frame(height: state.isCollapsed ? 0 : geometry.size.height)
              .opacity(state.isCollapsed || !state.showsMyDictation ? 0 : 1)
              .allowsHitTesting(!state.isCollapsed && state.showsMyDictation)
              .accessibilityHidden(state.isCollapsed || !state.showsMyDictation)
            if let conversation = state.selectedConversation {
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
                draft: Binding(
                  get: { state.conversationDrafts[conversation.id] ?? "" },
                  set: { state.conversationDrafts[conversation.id] = $0 }),
                sending: state.pendingTextMessages.contains(conversation.id),
                sendError: state.textMessageErrors[conversation.id],
                onSend: { Task { await state.sendConversationText(conversation) } },
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
                // The name pill toggles the agents list: with the clickable
                // scrim gone, a second click is the pointer's way back out.
                onShowAgents: {
                  if state.showsAgentMonitor {
                    state.hideAgentSidebar()
                  } else {
                    state.showAgentMonitor()
                  }
                },
                onOpenOwnerConversation: { state.openConversation(for: $0) },
                agentPillStack: state.showsAgentMonitor && !docked
                  ? channelStatusView.pillStack(
                    onSelect: { state.selectConversation($0) },
                    onDismiss: { state.hideAgentSidebar() })
                  : nil,
                hidesNavigation: columnShown,
                onComposerEditorActive: { state.noteComposerEditorActive($0) },
                onComposerTypingActivity: { state.noteComposerTypingActivity() }
              )
              .opacity(state.isCollapsed ? 0 : 1)
              .allowsHitTesting(!state.isCollapsed)
              .accessibilityHidden(state.isCollapsed)
            }
          }
          // The cell takes the geometry's own numbers: inside an HStack the
          // AppKit transcript would otherwise be sized to its text's intrinsic
          // width and centred (OverlayChromeV3Tests, 900 wide).
          .frame(width: canvasWidth, height: geometry.size.height)
          .animation(
            reduceMotion ? nil : .easeInOut(duration: state.presentationMorphDuration),
            value: state.isCollapsed
          )
          // Narrow + transcription has no name pill: the stack hangs top-right
          // under the header, where the conversation's pill sits (mockup).
          .overlay(alignment: .topTrailing) {
            if state.showsAgentMonitor && !docked && !state.isCollapsed
              && state.selectedConversation == nil
            {
              channelStatusView.pillStack(
                onSelect: { state.selectConversation($0) },
                onDismiss: { state.hideAgentSidebar() }
              )
              .frame(maxHeight: max(160, geometry.size.height - headerHeight - 120))
              .padding(.top, headerHeight + 12)
              .padding(.trailing, 20)
              .onExitCommand { state.hideAgentSidebar() }
            }
          }
          // The wide window docks the roster as a column beside the
          // conversation; nothing overlays the bubbles (mockup, Founder
          // 2026-10-16). Below the threshold the name pill opens the pill
          // stack instead. Esc on the transcription gives the width back.
          if columnShown {
            agentDrawerColumn(
              maximumHeight: geometry.size.height, width: drawerWidth,
              windowWidth: geometry.size.width
            )
            .transition(.move(edge: .trailing).combined(with: .opacity))
            .onExitCommand { state.hideAgentSidebar() }
          }
        }
        // Animate the column's arrival, never the bare docked flag: that flag
        // flips on the first geometry pass and would animate the transcript
        // from its intrinsic width for 240 ms (OverlayChromeV3Tests, 900 wide).
        .animation(reduceMotion ? nil : .easeInOut(duration: 0.24), value: columnShown)
        .onAppear { state.setAgentDrawerDocked(docked) }
        .onChange(of: docked) { _, docked in
          state.setAgentDrawerDocked(docked)
          if !docked { agentDrawerResize = nil }
        }
        .onChange(of: columnShown) { _, shown in
          if !shown { agentDrawerResize = nil }
        }
      }
      VStack(spacing: 0) {
        header
        if !state.isCollapsed,
          let label = OverlayActionsPresentation.finishingLabel(
            mode: state.mode, transcribing: state.transcribing, terminal: state.terminal)
        {
          Text(label)
            .csMono(10, .medium)
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
        if !state.isCollapsed && state.showsMyDictation && !state.showsAgentMonitor {
          VStack(spacing: CSSpace.sm) {
            bottomChromeContainer(
              HStack(spacing: 6) {
                OverlayEvidenceChip(
                  state: state, palette: palette, actionsOpen: actions.phase == .open,
                  glassNamespace: bottomChromeNamespace
                )
                .layoutPriority(-1)
                HStack(spacing: 2) {
                  Button {
                    actions.toggle()
                  } label: {
                    HStack(spacing: 4) {
                      Image(systemName: actions.controlSymbol)
                        .contentTransition(reduceMotion ? .identity : .symbolEffect(.replace))

                    }
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(palette.primaryText.color)
                    .padding(.horizontal, actions.phase == .open ? 0 : 10)
                    .frame(
                      minWidth: actions.phase == .open
                        ? nil : OverlayResizeChrome.actionsWidth(narrow: true)
                    )
                    .frame(height: OverlayResizeChrome.actionsHeight)
                    .fixedSize(horizontal: true, vertical: true)
                    .contentShape(Capsule())
                    .overlay(alignment: .topTrailing) {
                      if state.hasRecoverableSupersededWork && actions.phase != .open {
                        Circle()
                          .fill(palette.processingStatus.color)
                          .frame(width: 5, height: 5)
                          .accessibilityHidden(true)
                          .accessibilityIdentifier("overlay-retained-work-badge")
                      }
                    }
                  }
                  .buttonStyle(.plain)
                  .focusable()
                  .focused($actionsFocused)
                  .accessibilityLabel(actions.controlTitle)
                  .accessibilityValue(actions.phase == .open ? "Expanded" : "Collapsed")
                  .accessibilityHint(
                    state.hasRecoverableSupersededWork
                      ? "Previous take available. Open actions to copy or discard it."
                      : "Show or hide transcript tools"
                  )
                  .accessibilityIdentifier("overlay-tools-handle")
                  .modifier(OverlayMiniTooltip(title: actions.controlTitle, palette: palette))
                  if actions.phase == .open {
                    intentRail
                  }
                }
                .padding(.vertical, actions.phase == .open ? 2 : 0)
                .padding(.horizontal, actions.phase == .open ? 10 : 0)
                .fixedSize(horizontal: false, vertical: true)
                .modifier(
                  OverlayActionsSurface(palette: palette, glassNamespace: bottomChromeNamespace)
                )
                .contentShape(Capsule())
                .onHover { actions.pointerChanged($0) }
                .onChange(of: actionsFocused) { _, focused in
                  actions.focusChanged(focused)
                }
                .onExitCommand { actions.dismiss() }
                .animation(reduceMotion ? nil : .easeOut(duration: 0.15), value: actions.phase)
                .transaction { transaction in
                  if reduceMotion {
                    transaction.animation = nil
                    transaction.disablesAnimations = true
                  }
                }
                .task(id: actions.hideDeadline) {
                  guard let deadline = actions.hideDeadline else { return }
                  do {
                    try await ContinuousClock().sleep(until: deadline)
                  } catch { return }
                  guard !Task.isCancelled else { return }
                  actions.expire()
                }
              }
            )
            // Glass is confined to each capsule, before the bar's clear margins.
            // The AppKit edge intercept and existing header/body drag regions stay in place.
            .frame(maxWidth: .infinity, alignment: .center)
            .padding(.horizontal, OverlayResizeChrome.actionsSideInset)
            if state.completedMaxConsultationID != nil {
              Button("Continue in chat", systemImage: "bubble.left") {
                state.continueMaxConsultationInChat()
              }
              .buttonStyle(.plain)
              .csMono(10, .medium)
              .foregroundStyle(palette.primaryText.color)
              .padding(.vertical, 2)
              .accessibilityIdentifier("overlay-continue-max-chat")
            }
            if footerMessage != nil || bottomChromeSlots.showsCoverageWarning {
              footerMessageRow
                .frame(height: 18)
                .padding(.horizontal, 20)
            }

          }
          .padding(.bottom, OverlayResizeChrome.actionsBottomInset)
        } else if let label = OverlayActionsPresentation.finishingLabel(
          mode: state.mode, transcribing: state.transcribing, terminal: state.terminal)
        {
          // The folded bar keeps its height; the passive wait label uses its
          // bottom center without touching the header timer or capture state.
          Text(label)
            .csMono(10, .medium)
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
    .onChange(of: actions.phase) { _, phase in
      if phase == .open { state.refreshRetranscriptionAvailability() }
    }
    .onChange(of: state.isCollapsed) { _, collapsed in
      if collapsed { actions.reset() }
    }
    .onChange(of: state.captureGeneration) { _, _ in actions.reset() }
  }

  /// The docked roster column (wide windows): full height under the header,
  /// user-sized, with a draggable left edge, never overlaying the
  /// conversation. The name pill focuses the selected row via the revision.
  private func agentDrawerColumn(
    maximumHeight: CGFloat, width: CGFloat, windowWidth: CGFloat
  ) -> some View {
    channelStatusView.monitorBody(
      maximumHeight: max(0, maximumHeight - headerHeight - 8 - 16),
      focusRevision: state.agentDrawerFocusRevision
    )
    .padding(.top, headerHeight + 8)
    .padding(.horizontal, 10)
    .padding(.bottom, 16)
    .frame(width: width)
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
            preferredAgentDrawerWidth =
              OverlayAgentDrawerLayout.drawerWidth(
                textScale: textScale,
                preferredWidth: start - value.translation.width / textScale,
                windowWidth: windowWidth) / textScale
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
        preferredAgentDrawerWidth =
          OverlayAgentDrawerLayout.drawerWidth(
            textScale: textScale, preferredWidth: width / textScale + step,
            windowWidth: windowWidth) / textScale
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
      .modifier(OverlayControlGlass())
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
    .onHover {
      closeDotHovered = $0
    }
    .animation(reduceMotion ? nil : .easeOut(duration: 0.12), value: closeDotHovered)
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
      isFinalizing: !state.terminal
        && (state.transcribing || state.mode == .finalizing
          || (!state.recording && state.showsSessionTimer)),
      recordingLight: state.recordingLight, animates: overlayVisible)
    controls.showsRecordingButton = showsMicrophone
    controls.onShowDictation = state.showTranscription
    controls.onPresentationSelect = state.setPresentationMode
    return controls
  }

  private func justifiedHeader(density: OverlayHeaderDensity) -> some View {
    let mini = density == .mini
    return HStack(spacing: mini ? 6 : 10) {
      HStack(spacing: mini ? 6 : 5) {
        closeButton

        // The wordmark is the product name, never translated copy.
        Text(verbatim: "codescribe")
          .font(CSFont.ui(density.brandSize, .bold))
          .tracking(-0.3)
          .foregroundStyle(palette.primaryText.color)
          .allowsHitTesting(false)
          .accessibilityIdentifier(mini ? "overlay-mini-brand" : "")
          .background {
            if mini {
              OverlayWindowDragRegion(identifier: "overlay-mini-brand-drag-region")
            }
          }
      }
      .fixedSize()
      .accessibilityElement(children: .contain)
      .accessibilityIdentifier(mini ? "" : "overlay-header-leading")
      .background {
        if !mini {
          OverlayWindowDragRegion(identifier: "overlay-header-inert-drag-region")
        }
      }

      // The flexible centre keeps one identity in every density; in mini it
      // collapses to a spacer so the recording controls stay trailing.
      chromeWaveform(barCount: density.waveformBars)
        .frame(minWidth: density.waveformMinWidth, maxWidth: .infinity)
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
        sessionTimer
          .allowsHitTesting(false)
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
        }
        recordingControls(compact: false)
      }
      .fixedSize()
      .accessibilityElement(children: .contain)
      .accessibilityIdentifier(mini ? "" : "overlay-header-trailing")
    }
    .frame(height: mini ? 26 : nil)
    .accessibilityIdentifier(mini ? "overlay-mini-widget" : "")
  }

  private var transcriptionPreviewButton: some View {
    let transcriptionSelected =
      !state.isCollapsed && !state.showsAgentMonitor
      && state.selectedConversationID == nil
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

  /// Internal so a test can assert that two body passes share one search model.
  var channelStatusView: OverlayChannelStatusView {
    var view = OverlayChannelStatusView(
      channels: state.visibleChannelRows.filter { $0.channel != "0" },
      unavailable: state.channelStatusUnavailable,
      palette: palette, animates: overlayVisible,
      hudStates: state.channelHudStates,
      agentDescriptors: state.channelAgentDescriptors,
      onToggleChannel: { digit in
        Task { await state.toggleAgentChannel(digit, keepsAgentList: true) }
      },
      toggleError: state.channelToggleError,
      pendingToggleChannels: state.pendingChannelToggles,
      conversations: state.conversations,
      selectedConversationID: state.selectedConversationID,
      unreadCounts: Dictionary(
        uniqueKeysWithValues: state.conversations.map {
          ($0.id, state.unreadReplies(in: $0))
        }),
      onSelectConversation: { state.selectConversation($0) },
      onShowMonitor: state.toggleAgentSidebar,
      // The drawer value is rebuilt on every body pass and only its
      // `monitorBody(...)` is installed, so a `@State` on it never persists:
      // each HUD tick handed the drawer a fresh, empty search model and the
      // query vanished mid-typing. The overlay state owns the model.
      searchModel: state.savedConversationSearch,
      rosterReady: state.hasLoadedChannelRoster
    )
    view.mutedChannels = state.channelPlaybackMuted
    view.headerSelected =
      !state.isCollapsed
      && (state.showsAgentMonitor || state.selectedConversationID != nil)
    view.pendingMuteChannels = state.pendingPlaybackChannels
    view.onTogglePlayback = { channel in Task { await state.toggleChannelPlayback(channel) } }
    view.playbackError = state.playbackPreferenceError
    view.archiveCandidates = Dictionary(
      uniqueKeysWithValues: state.visibleChannelRows.compactMap {
        state.archiveCandidate(for: $0.channel).map { ($0.channel, $0) }
      })
    view.pendingArchives = state.pendingAgentArchives
    view.archivedOwners = state.archivedAgentOwners
    view.onArchiveAgent = { owner in Task { await state.archiveAgent(owner) } }
    view.archiveError = state.agentArchiveError
    return view
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
      stretches: true
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

  @ViewBuilder
  private var footerMessageRow: some View {
    if let message = footerMessage {
      let opensMaxSettings = state.transcriptStorageError == nil && state.maxPreparationError != nil
      OverlayHoverControl(
        id: "overlay-footer-message",
        title: opensMaxSettings
          ? String(localized: "Click to open Max consultation settings.") : message,
        palette: palette, presented: $footerDetail,
        action: opensMaxSettings
          ? { openMaxSettings(.creator, anchor: .maxConsultation) } : nil
      ) {
        Text(message)
          .csMono(10, .medium)
          .lineLimit(1)
          .truncationMode(.tail)
          .foregroundStyle(palette.primaryText.color)
          .accessibilityIdentifier("overlay-footer-notice")
      } detail: { _ in
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
        }
        .frame(maxHeight: 320)
      }
      .accessibilityLabel(message)
      .accessibilityHint(
        opensMaxSettings ? Text("Click to open Max consultation settings.") : Text("")
      )
    } else if bottomChromeSlots.showsCoverageWarning, let warning = state.footerWarning {
      OverlayCoverageStatus(
        warning: warning, palette: palette,
        canRetranscribe: state.terminal && state.canRetranscribe,
        cloudConfigured: state.cloudRetranscribeConfigured,
        diagnosticDetail: showsDiagnostics ? state.coverageRefusalDetail : nil,
        onRetranscribe: { state.retranscribe(pass: $0) }
      )
    } else {
      Color.clear.accessibilityHidden(true)
    }
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
      case .listening, .finalizing:
        EmptyView()
      case .formatted:
        revisionStatusRow
      case .coverageRefused:
        revisionStatusRow
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

  /// Ledger truth under the canvas: what the bytes on screen ARE — a local
  /// draft, a revision in flight, or the reducer's projection — plus the last
  /// commit failure. Same states as T15's editor status.
  private var revisionStatusRow: some View {
    VStack(alignment: .leading, spacing: CSSpace.xxs) {
      HStack(spacing: CSSpace.xs) {
        if state.formatterCommitPending {
          ProgressView()
            .controlSize(.small)
          Text("Formatting revision…")
        } else if state.revisionCommitPending {
          ProgressView()
            .controlSize(.small)
          Text("Committing revision…")
        } else if state.isRevisionDraftDirty {
          Image(systemName: "pencil.line")
          Text("Draft · not committed")
        } else if state.mode != .coverageRefused {
          Image(systemName: "checkmark.seal")
          Text("Ledger projection")
        }
      }
      .csMono(10, .semibold)
      .foregroundStyle(
        state.isRevisionDraftDirty ? CSColor.terracotta : palette.mutedText.color
      )
      .accessibilityElement(children: .combine)
      .accessibilityIdentifier("overlay-revision-status")

      // `recoveryFailure` joins the chain because a failed recovery keeps the
      // retained item: the user needs the full sentence, not just the footer
      // chip. This row is `.formatted`-only, so the persisting footer notice
      // remains the surface that covers a live capture.
      if let error = state.revisionCommitError ?? state.formatterError
        ?? state.recoveryFailure
      {
        Label(error, systemImage: "exclamationmark.triangle")
          .csMono(10, .medium)
          .foregroundStyle(CSColor.terracotta)
          .lineLimit(2)
          .accessibilityIdentifier("overlay-revision-error")
      }
    }
    .allowsHitTesting(false)
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
          .csMono(11, .medium)
          .foregroundStyle(palette.mutedText.color)
          .fixedSize(horizontal: false, vertical: true)
        Text(state.currentTakeRecoveryDetail)
          .csMono(11, .medium)
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
          .foregroundStyle(CSColor.terracotta)
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
            .csMono(11, .medium)
            .foregroundStyle(palette.mutedText.color)
            .fixedSize(horizontal: false, vertical: true)
        }
        Spacer(minLength: 0)
      }
      if state.errorDiagnosticDetail != nil {
        Text(state.currentTakeRecoveryDetail)
          .csMono(11, .medium)
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
        .foregroundStyle(status.isError ? CSColor.terracotta : CSColor.oliveLight)
      VStack(alignment: .leading, spacing: 4) {
        Text(status.headline)
          .csFont(15, .medium)
          .foregroundStyle(CSColor.textBody)
          .fixedSize(horizontal: false, vertical: true)
        Text(status.message)
          .csMono(11, .medium)
          .foregroundStyle(CSColor.textFaint)
          .fixedSize(horizontal: false, vertical: true)
      }
      Spacer(minLength: 0)
    }
    .frame(maxWidth: .infinity, minHeight: bodyMinHeight, alignment: .leading)
    .accessibilityElement(children: .combine)
    .accessibilityIdentifier("overlay-presentation-status")
  }
}

/// Only reports the hosting window's visibility; it owns no capture state.
private struct OverlayRenderVisibility: NSViewRepresentable {
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
        onChange?(window?.isVisible == true && window?.occlusionState.contains(.visible) == true)
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
      .background(CSColor.windowWash)
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
    overlayPreviewCanvas(width: 320, height: 260) {
      DictationOverlayView(state: light)
    }
    .preferredColorScheme(.light)
    overlayPreviewCanvas(width: 320, height: 260) {
      DictationOverlayView(state: dark)
    }
    .preferredColorScheme(.dark)
  }

  #Preview("Dock matrix · 320 pt") {
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
    overlayPreviewCanvas(width: 320, height: 260) {
      DictationOverlayView(state: .previewTranscribing())
    }
  }

  #Preview("No speech") {
    // Session ended without usable text: dedicated notice body, no
    // Copy/Format/Send, only Close. Pinned to the min content size so it also
    // guards the floor layout for this outcome.
    overlayPreviewCanvas(width: 320, height: 260) {
      DictationOverlayView(state: .previewNoSpeech())
    }
  }

  #Preview("Formatted") {
    overlayPreviewCanvas {
      DictationOverlayView(state: .previewFormatted())
    }
  }

  #Preview("Formatted · compact chrome") {
    overlayPreviewCanvas(width: 340, height: 260) {
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
