import AppKit
import OSLog
import SwiftUI

/// Agent-surface performance breadcrumbs (app bootstrap, first agent open,
/// thread index load, selected thread load, tool catalog load). Filter with:
///   log show --predicate 'category == "agent-perf"' --info
enum AgentPerf {
  static let logger = Logger(
    subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
    category: "agent-perf"
  )

  static func log(_ label: String, since start: Date, detail: String = "") {
    let ms = Int(Date().timeIntervalSince(start) * 1000)
    logger.info(
      "\(label, privacy: .public): \(ms, privacy: .public)ms \(detail, privacy: .public)"
    )
  }
}

/// Owns the app's long-lived view-models + engines so they can reference each
/// other without @StateObject init-order pain.
/// The menu-bar status item itself lives in the AppDelegate (proven reliable).
@MainActor
final class AppModel: ObservableObject {
  static let shared = AppModel()

  let chat: AgentChatStore
  let overlay: OverlayController
  let tray: TrayViewModel
  /// Independent text scale for the agent chat surface (⌘+/-/0 while the chat
  /// window is key). The overlay's scale lives on `OverlayController`.
  let chatTextScale = TextScaleController(key: "AgentChat.textScale.v1")

  init() {
    let bootstrapStart = Date()
    // Shell-first agent bootstrap: the store starts as a light event sink
    // (voice delivery works immediately); the persisted thread index loads
    // asynchronously OFF the main actor and merges in when ready. No disk
    // I/O or thread-history parsing happens on this MainActor init path.
    let chat = AgentChatStore(
      engine: RealChatEngine(),
      threadsProvider: RealThreadsEngine(),
      licenseService: LicenseService.shared,
      loadsThreadIndexEagerly: false
    )
    chat.paletteSource = RealComposerPaletteSource(
      settings: RealSettingsEngine(),
      mcpAdmin: RealMCPAdminEngine()
    )
    self.chat = chat
    self.overlay = OverlayController(engine: ControllerDictationEngine(), composer: chat)
    self.tray = TrayViewModel(engine: RealTrayEngine())
    // The composer is a gesture-only adapter over RecordingController. Right
    // Option, composer mic, Dictation, and Formatting share one recorder/STT.
    chat.dictation = RealComposerDictation(store: chat)
    AgentPerf.log("app bootstrap (AppModel init)", since: bootstrapStart)
  }
}

/// Owns the floating dictation NSPanel + its OverlayState.
/// Recording is owned by `CodescribeHotkeys`/`RecordingController`; this panel is
/// only the SwiftUI surface for that single controller.
@MainActor
final class OverlayController: ObservableObject {
  let state: OverlayState
  /// Independent text scale for the dictation overlay (⌘+/-/0 while the panel is
  /// key). Separate from the chat scale so a distance-readable transcript and an
  /// up-close chat can be tuned independently.
  let textScale = TextScaleController(key: "DictationOverlayPanel.textScale.v1")
  private var panel: NSPanel?
  private let overlayEnabledProvider: () -> Bool
  private let assistiveStatusProvider: () -> Bool
  private let panelFactory: @MainActor (OverlayState, TextScaleController) -> NSPanel
  private let orderPanelFront: @MainActor (NSPanel) -> Void
  private let orderPanelOut: @MainActor (NSPanel) -> Void
  /// Runs the agent-handoff fade and calls back when it finishes. Injected so a
  /// test can fire the completion at an exact moment in the lifecycle instead of
  /// racing a wall-clock animation; the default keeps the real 0.18 s fade.
  ///
  /// The completion is `@escaping` because that is the lifetime BOTH consumers
  /// actually take, not a precaution:
  ///
  /// - the default implementation below hands it to
  ///   `NSAnimationContext.runAnimationGroup`'s `completionHandler`, which AppKit
  ///   stores and calls after this call has returned;
  /// - the delayed-completion test stores it in a local `pendingFade` and fires
  ///   it after a successor capture has opened.
  ///
  /// A closure parameter of a function type is non-escaping by default exactly
  /// as it is in a function declaration, so without this attribute neither
  /// consumer may keep the callback past the call — which is the whole point of
  /// a fade completion. `@MainActor @Sendable` stays: the completion runs on the
  /// main actor and its captures are main-actor isolated.
  private let runHandoffFade:
    @MainActor (NSPanel, @escaping @MainActor @Sendable () -> Void) -> Void
  /// A direct edge resize is the user's size decision for the current session.
  /// The next recording may breathe again from that persisted starting point.
  private var automaticContentSizingEnabled = true
  /// Latched across the session (preparing → started → stopped) because the
  /// Rust controller clears its assistive flag right after the stop pipeline —
  /// a single read at finalize would race it. Mid-hold upgrades (Fn → Fn+Shift)
  /// flip the tray status while recording, so every lifecycle hook re-polls.
  private var sessionWasAssistive = false

  init(
    state: OverlayState? = nil,
    engine: DictationEngine? = nil,
    composer: AgentChatStore? = nil,
    overlayEnabledProvider: @escaping () -> Bool = {
      DictationOverlayGate.shouldShowOverlay(
        trayEnabled: CodescribeConfig().trayToggles().transcriptionOverlayEnabled
      )
    },
    assistiveStatusProvider: @escaping () -> Bool = {
      CodescribeTrayStatus().currentStatus().assistive
    },
    panelFactory: (@MainActor (OverlayState, TextScaleController) -> NSPanel)? = nil,
    orderPanelFront: (@MainActor (NSPanel) -> Void)? = nil,
    orderPanelOut: (@MainActor (NSPanel) -> Void)? = nil,
    runHandoffFade: (
      @MainActor (NSPanel, @escaping @MainActor @Sendable () -> Void) -> Void
    )? = nil
  ) {
    let state = state ?? OverlayState()
    self.state = state
    self.overlayEnabledProvider = overlayEnabledProvider
    self.assistiveStatusProvider = assistiveStatusProvider
    self.panelFactory =
      panelFactory ?? {
        DictationOverlayWindow.make(state: $0, textScale: $1)
      }
    self.orderPanelFront = orderPanelFront ?? { $0.orderFrontRegardless() }
    self.orderPanelOut = orderPanelOut ?? { $0.orderOut(nil) }
    self.runHandoffFade =
      runHandoffFade ?? { panel, completed in
        NSAnimationContext.runAnimationGroup { context in
          context.duration = 0.18
          panel.animator().alphaValue = 0
        } completionHandler: {
          MainActor.assumeIsolated { completed() }
        }
      }
    state.engine = engine
    // Drive the tray status off the SAME authoritative recording lifecycle the
    // overlay already receives. The tray view-model otherwise only polls on
    // appear (and the popover is built once), so it stayed "Recording" after
    // Finish. These hooks fire for every start/stop path (hotkey, tray, auto).
    // The composer's phase is DERIVED from the lane the session turned out to be
    // (`sessionWasAssistive`) — never left to whatever the optimistic gesture set.
    // `.preparing` and `.recording` are both non-actionable in the composer, so a
    // phase that is entered optimistically and only cleared on a latch that read
    // true is a mic that can die permanently. Non-assistive sessions therefore
    // push the composer back to `.idle` (it renders as `.blocked` off
    // `dictationBlocked`, which is the honest "busy elsewhere" state), and every
    // identity-less lifecycle paint cannot revoke an admitted composer request.
    state.onRecordingPreparing = { [weak self] in
      guard let self else { return }
      self.automaticContentSizingEnabled = true
      self.sessionWasAssistive = false
      self.refreshAssistiveLatch()
      self.showForRecording()
      if !AppModel.shared.chat.hasComposerCaptureRequest {
        AppModel.shared.chat.setDictationPhase(self.sessionWasAssistive ? .preparing : .idle)
      }
      AppModel.shared.tray.isStartingDictation = true
      // Block the composer mic while the shared recorder owns the microphone.
      AppModel.shared.chat.dictationBlocked = true
      Task.detached(priority: .utility) {
        await VoiceLabRuntime.shared.ensureListening()
      }
    }
    state.onRecordingStarted = { [weak self] in
      guard let self else { return }
      self.refreshAssistiveLatch()
      self.showForRecording()
      if !AppModel.shared.chat.hasComposerCaptureRequest {
        AppModel.shared.chat.setDictationPhase(self.sessionWasAssistive ? .recording : .idle)
      }
      AppModel.shared.tray.isRecording = true
      AppModel.shared.tray.isStartingDictation = false
      AppModel.shared.chat.dictationBlocked = true
    }
    // The composition root supplies its existing chat before the listener is
    // attached. Looking up AppModel.shared here re-enters its once initializer.
    // Standalone overlays have no implicit global composer; delivery and
    // ownership release still use connectComposer's authenticated session path.
    if let composer { state.connectComposer(to: composer) }
    state.onRecordingStopped = { [weak self] in
      guard let self else { return }
      // A pending/newer composer request cannot be released by identity-less
      // lifecycle paint. Its matching projection releases it via connectComposer.
      guard !AppModel.shared.chat.hasComposerCaptureRequest else { return }
      self.refreshAssistiveLatch()
      self.markStopped()
      AppModel.shared.tray.isRecording = false
      AppModel.shared.tray.isStartingDictation = false
      AppModel.shared.chat.finishDictationCapture(sessionID: nil)
      Task { await VoiceLabRuntime.shared.stopOwnedProcess() }
    }
    // Admission and calibration outcomes are product feedback even when the
    // transcript overlay preference is off. The typed status is passive; this
    // seam only brings its already-reduced card on screen.
    state.onPresentationStatus = { [weak self] in self?.show() }
    state.onTranscriptPresentationChanged = { [weak self] in
      self?.resizeForProjectedContent()
    }
    state.onSuccessfulDictation = {
      Task { @MainActor in
        _ = await ActivationPing.shared.recordFirstSuccessfulDictation()
      }
    }
    state.onClose = { [weak self] in self?.hide() }
    state.onSendToAgent = { [weak self] text in
      guard !text.isEmpty else { return }
      // Rust already persisted and streamed the turn. TurnStarted opened
      // the chat passively; do NOT activate here (focus-steal at Done was
      // the wave10 operator bug). Fallback is also passive in case the
      // delivery listener missed TurnStarted.
      AppModel.shared.tray.onIntent(.revealChat)
      self?.hide()
    }
    state.onPlacementChanged = { [weak self] in self?.applyPlacement(animated: true) }
    state.attach()
  }

  func prepareForRecordingStart() {
    state.prepareForExternalStart()
  }

  /// Show the overlay for a dictation session, honouring the "Transcription
  /// Overlay" toggle. When disabled, dictation runs headless — hold the hotkey,
  /// dictate, and the text lands at the cursor (+ clipboard) with no window.
  /// Delivery is engine-side (LocalFinalPass), independent of this window, so
  /// hiding the overlay never suppresses the paste.
  func showForRecording() {
    refreshAssistiveLatch()
    guard !sessionWasAssistive else {
      hide()
      return
    }
    guard overlayEnabledProvider() else {
      DictationOverlayGate.logger.info("overlay suppressed: tray toggle off")
      if panel != nil { hide() }
      return
    }
    show()
  }

  func show() {
    let panel = panel ?? panelFactory(state, textScale)
    self.panel = panel
    if let floating = panel as? FloatingOverlayPanel {
      floating.startPresence()
      floating.onUserMove = { [weak self] in
        guard let self, !Self.isApplyingFrame, let panel = self.panel else { return }
        if self.state.freeMotion {
          OverlayPlacement.persistOrigin(panel.frame.origin)
        }
        self.state.userDraggedOverlay()
      }
      floating.onUserResize = { [weak self] in
        guard let self, !Self.isApplyingFrame else { return }
        self.automaticContentSizingEnabled = false
        self.state.userResizedOverlay()
      }
    }
    // A pending fade-out must not leave a freshly shown panel invisible.
    panel.alphaValue = 1
    applyPlacement(animated: false)
    orderPanelFront(panel)
    resizeForProjectedContent()
  }

  /// True while we `setFrame` from prefs. AppKit still fires `windowDidMove`
  /// for those writes; those must not count as a user drag.
  static var isApplyingFrame = false

  /// Derive and apply the panel's frame from the placement prefs: free motion
  /// restores the last dragged origin, anchored derives from the anchor —
  /// in ONE setFrame so there is no transient mismatched frame. Clamping the
  /// size here covers programmatic sizing, which AppKit's minSize does not.
  private func applyPlacement(animated: Bool) {
    guard let panel else { return }
    Self.isApplyingFrame = true
    defer { Self.isApplyingFrame = false }
    let screen = NSScreen.main
    let clamped = DictationOverlayWindow.clamp(panel.frame.size, to: screen)
    let size = NSSize(
      width: clamped.width,
      height: state.isCollapsed ? DictationOverlayWindow.collapsedHeight : clamped.height)
    let origin: NSPoint?
    if state.freeMotion {
      origin = OverlayPlacement.restoredOrigin(size: size, on: screen) ?? panel.frame.origin
    } else {
      origin = OverlayPlacement.origin(for: state.placementAnchor, size: size, on: screen)
    }
    guard let origin else {
      panel.setContentSize(size)
      return
    }
    let frame = NSRect(origin: origin, size: size)
    if animated, panel.isVisible {
      panel.animator().setFrame(frame, display: true)
    } else {
      panel.setFrame(frame, display: false)
    }
  }

  /// Grow only: short projections keep the user's/restored resting size, long
  /// projections reveal more lines until 60% of the visible screen, then the
  /// native transcript scroll view takes over. Window-frame writes are direct
  /// and unanimated; content keeps its existing reveal transition instead of
  /// morphing the glass panel or exporting hosting constraints.
  private func resizeForProjectedContent() {
    guard automaticContentSizingEnabled, !state.isCollapsed, let panel else { return }
    let screen = panel.screen ?? NSScreen.main
    let targetHeight = OverlayContentSizePolicy.preferredHeight(
      for: state.activeText,
      width: panel.frame.width,
      textScale: textScale.scale,
      screen: screen
    )
    guard targetHeight > panel.frame.height + 0.5 else { return }

    Self.isApplyingFrame = true
    defer { Self.isApplyingFrame = false }
    let size = NSSize(width: panel.frame.width, height: targetHeight)
    let origin: NSPoint
    if state.freeMotion, let visible = screen?.visibleFrame {
      origin = OverlayPlacement.clampOrigin(
        NSPoint(x: panel.frame.minX, y: panel.frame.maxY - targetHeight),
        size: size,
        in: visible
      )
    } else {
      origin =
        OverlayPlacement.origin(for: state.placementAnchor, size: size, on: screen)
        ?? NSPoint(x: panel.frame.minX, y: panel.frame.maxY - targetHeight)
    }
    panel.setFrame(NSRect(origin: origin, size: size), display: true)
  }

  func markStopped() {
    state.finishControllerRecording()
  }

  /// Called by the live TrayStatusStore listener. Assistive uses the shared
  /// controller but keeps the Dictation overlay closed in favor of Agent UI.
  /// Format / Retranscribe (and the post-take review they run on) own the
  /// panel: an Assistive tray tick must not hide it, steal focus, or arm
  /// Agent auto-send.
  func handleIndicatorModeChange(_ mode: CsIndicatorMode) {
    if mode == .assistive, state.blocksAssistiveOverlayHide {
      return
    }
    if mode == .assistive {
      sessionWasAssistive = true
      hide()
    }
    state.setAutoPasteControlAvailable(!sessionWasAssistive)
    state.applyIndicatorMode(mode)
  }

  func handleAssistiveStatusChange(_ assistive: Bool) {
    handleIndicatorModeChange(assistive ? .assistive : .hold)
  }

  private func refreshAssistiveLatch() {
    handleAssistiveStatusChange(assistiveStatusProvider())
  }

  func hide() {
    // Persist the user's chosen size for next launch (replaces frame autosave,
    // which used to write back the old feedback loop's runaway sizes) — and,
    // in free motion, the dragged origin.
    if let panel {
      DictationOverlayWindow.persist(
        size: (panel as? FloatingOverlayPanel)?.sizeForPersistence ?? panel.frame.size)
      if state.freeMotion {
        OverlayPlacement.persistOrigin(panel.frame.origin)
      }
      (panel as? FloatingOverlayPanel)?.invalidatePresence()
    }
    if let panel { orderPanelOut(panel) }
  }

  /// The dictated transcript was handed to the agent (voice turn opened in the
  /// chat window). The overlay's job is done — fade it out immediately instead
  /// of lingering over the conversation it just fed.
  func hideForAgentHandoff() {
    guard let panel, panel.isVisible else { return }
    DictationOverlayWindow.persist(
      size: (panel as? FloatingOverlayPanel)?.sizeForPersistence ?? panel.frame.size)
    if state.freeMotion {
      OverlayPlacement.persistOrigin(panel.frame.origin)
    }
    // Bind the completion to the exact panel AND the capture it is fading out.
    // The panel object is cached and reused, so the old completion re-read
    // `self.panel` and ordered out whichever window was current — a take that
    // started inside the 0.18 s fade lost its overlay to its predecessor's
    // handoff. Alpha is always restored: refusing the order-out while leaving a
    // reused window at alpha 0 would trade a hidden panel for an invisible one.
    let fadedPanel = panel
    let generation = state.captureGeneration
    runHandoffFade(
      fadedPanel,
      { [weak self] in
        defer { fadedPanel.alphaValue = 1 }
        guard let self else { return }
        // Still the live window, but a successor capture owns it now: this fade
        // has no authority over the take that replaced its own. A panel that is
        // no longer current is an orphan and is ordered out either way.
        if self.panel === fadedPanel, self.state.captureGeneration != generation {
          return
        }
        self.orderPanelOut(fadedPanel)
      }
    )
  }
}
