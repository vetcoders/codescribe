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
  var onShowDictation: (() -> Void)?

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
    case .mini, .midi: String(localized: "Expand widget")
    case .expanded: String(localized: "Collapse widget")
    }
  }
  /// Full view is always an explicit click; hover reveals only the midi strip.
  var previewSymbol: String {
    switch presentationMode {
    case .mini: OverlayControlSymbols.miniToTranscript
    case .midi: OverlayControlSymbols.midiToTranscript
    case .expanded: OverlayControlSymbols.returnToMini
    }
  }

  var body: some View {
    HStack(spacing: compact ? 4 : 7) {
      if showsRecordingButton { recordingButton }
      previewButton
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
    .help(recordingLabel + (recordingLight.map { ". " + $0.tooltip } ?? ""))
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
    .buttonStyle(.plain)
    .csFocusOutline()
    .help(previewAccessibilityLabel)
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

struct DictationOverlayView: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.colorScheme) private var colorScheme
  @AppStorage(DictationOverlayGate.labModeDefaultsKey) private var labMode = false
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
  // content fills the frame and never goes narrower than `windowMinWidth`.
  // `DictationOverlayWindow.minSize.height` MUST stay ≥ chrome + `bodyMinHeight`
  // or the canvas paints past the window rect and squares the corners.
  private let windowMinWidth: CGFloat = 320
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
    .frame(
      minWidth: state.isMini ? DictationOverlayWindow.collapsedSize.width : windowMinWidth,
      maxWidth: .infinity, maxHeight: .infinity
    )
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
    .task(id: state.widgetHoverDeadline) {
      guard let deadline = state.widgetHoverDeadline else { return }
      do { try await ContinuousClock().sleep(until: deadline) } catch { return }
      guard !Task.isCancelled else { return }
      state.expireWidgetHover()
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
        ZStack {
          bodySection
            .frame(height: state.isCollapsed ? 0 : nil)
            .opacity(state.isCollapsed || !state.showsMyDictation ? 0 : 1)
            .allowsHitTesting(!state.isCollapsed && state.showsMyDictation)
            .accessibilityHidden(state.isCollapsed || !state.showsMyDictation)
          if let conversation = state.selectedConversation {
            OverlayConversationView(
              conversation: conversation, palette: palette,
              topInset: headerHeight + 8, bottomInset: 20,
              pendingControls: state.pendingReplyControls, controlErrors: state.replyControlErrors,
              onControl: { message, stop in
                Task { await state.controlReply(message, stop: stop) }
              },
              focusRevision: state.conversationFocusRevision,
              followsLiveChannel: state.channelHudStates[conversation.channel]?.open == true,
              draft: Binding(
                get: { state.conversationDrafts[conversation.id] ?? "" },
                set: { state.conversationDrafts[conversation.id] = $0 }),
              sending: state.pendingTextMessages.contains(conversation.id),
              sendError: state.textMessageErrors[conversation.id],
              onSend: { Task { await state.sendConversationText(conversation) } },
              microphoneOpen: state.conversationMicrophoneOpen(conversation),
              microphoneEnabled: state.canToggleConversationMicrophone(conversation),
              playbackMuted: state.conversationPlaybackMuted(conversation),
              playbackEnabled: !state.pendingPlaybackOwners.contains(conversation.owner?.id ?? ""),
              onMicrophone: { Task { await state.toggleConversationMicrophone(conversation) } },
              onPlayback: { Task { await state.toggleConversationPlayback(conversation) } },
              playbackError: state.playbackPreferenceError
            )
            .opacity(state.isCollapsed ? 0 : 1)
            .allowsHitTesting(!state.isCollapsed)
            .accessibilityHidden(state.isCollapsed)
          }
        }
        .animation(reduceMotion ? nil : .easeInOut(duration: 0.28), value: state.isCollapsed)
        .overlay(alignment: .trailing) {
          if state.showsAgentMonitor && !state.isCollapsed {
            ZStack(alignment: .trailing) {
              Button {
                state.hideAgentSidebar()
              } label: {
                Color.clear
                  .frame(maxWidth: .infinity, maxHeight: .infinity)
                  .contentShape(Rectangle())
              }
              .buttonStyle(.plain)
              .accessibilityLabel("Close agent sidebar")
              .accessibilityIdentifier("overlay-agent-drawer-dismiss")
              channelStatusView.monitorBody
                .padding(12)
                .frame(width: min(360, max(0, (geometry.size.width - 16) * 0.78)))
                .frame(maxHeight: .infinity)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14))
                .clipShape(RoundedRectangle(cornerRadius: 14))
                .padding(.trailing, 8)
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
            .padding(.top, headerHeight + 8)
            .padding(.bottom, 16)
            .transition(.move(edge: .trailing).combined(with: .opacity))
            .onExitCommand { state.hideAgentSidebar() }
          }
        }
        .animation(reduceMotion ? nil : .easeInOut(duration: 0.24), value: state.showsAgentMonitor)
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
            .padding(.horizontal, OverlayResizeChrome.actionsBottomInset)
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

  // MARK: Header

  private var header: some View {
    VStack(spacing: 6) {
      if state.isMini {
        miniHeader
      } else {
        ViewThatFits(in: .horizontal) {
          fullHeader
          narrowHeader
        }
      }

    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .padding(.horizontal, 16)
    .padding(.vertical, 10)
    .coordinateSpace(name: "overlay-header")
    // Keep the explicit drag region above the passive glass background.
    // OverlayResizeHitTests verifies header dragging across its width.
    .background { OverlayWindowDragRegion(identifier: "overlay-header-drag-region") }
    .modifier(OverlayControlGlass())
    // The cached panel survives orderOut. Observe its window outside
    // ViewThatFits so hidden header candidates cannot compete for visibility.
    .background {
      OverlayRenderVisibility(onHidden: { state.clearWidgetHover() }) { visible in
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

  private var fullHeader: some View { justifiedHeader(compact: false) }

  private var narrowHeader: some View { justifiedHeader(compact: true) }

  private var miniHeader: some View {
    HStack(spacing: 6) {
      closeButton
      Text(verbatim: "codescribe")
        .font(CSFont.ui(13, .bold))
        .tracking(-0.3)
        .foregroundStyle(palette.primaryText.color)
        .fixedSize()
        .accessibilityIdentifier("overlay-mini-brand")
      Spacer(minLength: 4)
      recordingControls(compact: false)
    }
    .frame(height: 26)
    .accessibilityIdentifier("overlay-mini-widget")
    .contextMenu {
      Button("Agents", systemImage: "sidebar.right") { state.showAgentMonitor() }
      Button("Transcription", systemImage: "text.alignleft", action: state.showTranscription)
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
        .scaleEffect(closeDotHovered ? 1.15 : 1)
        .contentShape(Circle().inset(by: -7.5))
    }
    .buttonStyle(.plain)
    .onHover {
      closeDotHovered = $0
      state.setWidgetInteraction(.closeControl, held: $0)
    }
    .animation(reduceMotion ? nil : .easeOut(duration: 0.12), value: closeDotHovered)
    .focusable(false)
    .help(OverlayIntent.close.helpText)
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
    return controls.onHover { state.setWidgetInteraction(.primaryControls, held: $0) }
  }

  private func justifiedHeader(compact: Bool) -> some View {
    HStack(spacing: compact ? 6 : 10) {
      HStack(spacing: 5) {
        closeButton

        // The wordmark is the product name, never translated copy.
        Text(verbatim: "codescribe")
          .font(CSFont.ui(state.presentationMode == .midi ? 13 : compact ? 12 : 15, .bold))
          .tracking(-0.3)
          .foregroundStyle(palette.primaryText.color)
          .allowsHitTesting(false)
      }
      .fixedSize()
      .accessibilityElement(children: .contain)
      .accessibilityIdentifier("overlay-header-leading")
      .background {
        OverlayWindowDragRegion(identifier: "overlay-header-inert-drag-region")
      }

      chromeWaveform(barCount: state.presentationMode == .midi ? 24 : compact ? 10 : 34)
        .frame(
          minWidth: state.presentationMode == .midi ? 56 : compact ? 12 : 100, maxWidth: .infinity
        )
        .layoutPriority(-1)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("overlay-header-center")
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
      sessionTimer
        .allowsHitTesting(false)

      HStack(spacing: compact ? 4 : 8) {
        Button(action: state.showTranscription) {
          OverlayMicrophoneGlyph(
            symbol: "waveform.badge.magnifyingglass",
            tint: showsDiagnostics && state.compactProjection?.degraded == true
              ? palette.processingStatus.color : palette.mutedText.color)
        }
        .buttonStyle(.plain)
        .csFocusOutline()
        .help(transcriptPreviewHelp)
        .accessibilityLabel("Show transcription")
        .accessibilityValue(transcriptPreviewHelp)
        .accessibilityIdentifier("overlay-transcription-preview")
        if let error = state.expansionPreferenceError {
          OverlayMicrophoneGlyph(
            symbol: "exclamationmark.triangle.fill", tint: palette.processingStatus.color
          )
          .help(error)
          .accessibilityLabel(error)
          .accessibilityIdentifier("overlay-preference-save-error")
        }
        channelStatusView
        OverlayPlacementMenu(state: state, palette: palette)
        recordingControls(compact: compact)
      }
      .fixedSize()
      .accessibilityElement(children: .contain)
      .accessibilityIdentifier("overlay-header-trailing")
    }
  }

  private var channelStatusView: OverlayChannelStatusView {
    var view = OverlayChannelStatusView(
      channels: state.visibleChannelRows.filter { $0.channel != "0" },
      unavailable: state.channelStatusUnavailable,
      palette: palette, animates: overlayVisible,
      hudStates: state.channelHudStates,
      onToggleChannel: { digit in
        Task { await state.toggleAgentChannel(digit) }
      },
      toggleError: state.channelToggleError,
      conversations: state.conversations,
      selectedConversationID: state.selectedConversationID,
      unreadCounts: Dictionary(
        uniqueKeysWithValues: state.conversations.map {
          ($0.id, state.unreadReplies(in: $0))
        }),
      onSelectConversation: { state.selectConversation($0) },
      onShowMonitor: state.toggleAgentSidebar
    )
    view.mutedChannels = state.channelPlaybackMuted
    view.pendingMuteChannels = state.pendingPlaybackChannels
    view.onTogglePlayback = { channel in Task { await state.toggleChannelPlayback(channel) } }
    view.onDismissMonitor = state.hideAgentSidebar
    view.onShowTranscription = state.showTranscription
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
    let action = String(localized: "Show transcription")
    return showsDiagnostics && state.compactProjection?.degraded == true
      ? action + ". " + OverlayWarningCopy.liveTranscriptBehind.sentence : action
  }

  /// Audio-evidence strip in the primary bar. Amplitude/VAD only — word/PCM
  /// synchronized scrolling needs authenticated sample spans from projection
  /// receipts and is intentionally not invented here.
  private func chromeWaveform(barCount: Int) -> some View {
    WaveformView(
      barCount: barCount,
      active: state.mode == .listening && (state.audioReady || state.vadActive),
      transcribing: state.mode == .finalizing,
      indicatorMode: state.indicatorMode,
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
          .csMono(11, .semibold)
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
    if let error = state.revisionCommitError ?? state.formatterError ?? state.recoveryFailure {
      return error
    }
    if state.formatterCommitPending { return String(localized: "Formatting revision…") }
    if state.revisionCommitPending { return String(localized: "Committing revision…") }
    if state.isRevisionDraftDirty { return String(localized: "Draft · not committed") }
    if let notice = state.toast { return notice }
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
      OverlayHoverControl(
        id: "overlay-footer-message", title: message, palette: palette, presented: $footerDetail
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
            if state.presentationStatus != nil {
              transcriptStatus
            } else if state.errorDiagnosticDetail != nil || state.mode == .error {
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

  /// The accepted capture snapshot paints the same canvas while listening.
  /// It never changes the committed text or the human revision draft. Session,
  /// epoch and sequence admission remain in OverlayState's existing consumer.
  private var livePaint: CsCompactProjection? {
    guard !state.finalized, !state.terminal,
      state.mode == .listening || state.mode == .finalizing,
      !state.isEditingTranscript, !state.isRevisionDraftDirty,
      let paint = state.compactProjection
    else { return nil }
    if let document = state.latestTranscriptProjection,
      document.sessionId != paint.sessionId
    {
      return nil
    }
    // Exact committed bytes keep their confidence styling. Only a differing
    // ephemeral snapshot needs uncommitted paint and invalidates those ranges.
    guard !paint.text.utf8.elementsEqual(state.canvasText.utf8) else { return nil }
    return paint
  }

  /// Native live transcript: follows the newest words until the user clicks or
  /// selects an older phrase. The `NSTextView` keeps that selection stable across
  /// ongoing stream updates, so drag selection, Cmd-C and context-menu Copy work
  /// during recording without stopping capture. A `minHeight` reserves ~2–3 lines
  /// at the window floor.
  private var transcriptScroll: some View {
    VStack(alignment: .leading, spacing: 0) {
      LiveTranscriptTextView(
        text: livePaint?.text ?? state.canvasText,
        // Committed confidence ranges cannot index an ephemeral snapshot.
        uncertainWords: livePaint == nil ? state.canvasUncertainWords : [],
        isEditable: state.isTranscriptEditable && !state.isCollapsed,
        appearance: palette.appearance,
        showsDiagnostics: showsDiagnostics,
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
      // Muted live paint is explicitly uncommitted; it is not a confidence
      // warning and gains no editing or delivery capability from its display.
      .opacity(livePaint == nil ? 1 : 0.65)
      .modifier(OverlayScrollEdgeEffects())
      .overlay(alignment: .bottomTrailing) {
        // The decorative caret yields to the real insertion point while the
        // canvas is being edited.
        if !state.isEditingTranscript {
          BlinkingCaret(animating: overlayVisible && state.animatesTranscriptCaret)
            .padding(.trailing, 3)
            .allowsHitTesting(false)
        }
      }
      .frame(minHeight: transcriptMinHeight)
      .accessibilityIdentifier("overlay-transcript-area")
      // The empty branch is absence of a hint, not copy, so it stays verbatim.
      .accessibilityHint(
        livePaint != nil
          ? Text("Live preview. Uncommitted words may change.")
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
