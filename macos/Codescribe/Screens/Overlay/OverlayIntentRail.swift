import SwiftUI

/// Action availability stays projected; chrome visibility is local presentation only.
struct OverlayDockLayout: Equatable {
  static let minimumCanvasWidth: CGFloat = 320
  let projectedIntents: [OverlayIntent]
  var visibleIntents: [OverlayIntent] { projectedIntents.filter { $0 != .close } }
}

/// The overlay needs a quiet text-only state; tools are transient. Retained work
/// is a badge, never a reveal trigger. This state has no document authority.
struct OverlayActionsPresentation {
  enum Phase: Equatable { case idle, open }
  private(set) var phase: Phase = .idle
  var controlSymbol: String {
    phase == .open ? OverlayControlSymbols.closeActions : OverlayControlSymbols.actions
  }
  var controlTitle: String {
    phase == .open
      ? String(localized: "Close actions") : String(localized: "More actions")
  }
  private(set) var pointerInside = false
  private(set) var panelPresented = false
  private(set) var keyboardFocused = false
  private(set) var hideDeadline: ContinuousClock.Instant?

  mutating func pointerChanged(_ inside: Bool, at now: ContinuousClock.Instant = .now) {
    pointerInside = inside
    interact(at: now)
  }

  mutating func toggle(at now: ContinuousClock.Instant = .now) {
    if phase == .open {
      dismiss()
    } else {
      phase = .open
      interact(at: now)
    }
  }

  mutating func interact(at now: ContinuousClock.Instant = .now) {
    hideDeadline =
      phase == .open && !pointerInside && !panelPresented && !keyboardFocused
      ? now.advanced(by: .seconds(3)) : nil
  }

  mutating func expire(at now: ContinuousClock.Instant = .now) {
    guard phase == .open, !pointerInside, !panelPresented, !keyboardFocused, let hideDeadline,
      now >= hideDeadline
    else { return }
    dismiss()
  }

  mutating func dismiss() {
    phase = .idle
    hideDeadline = nil
  }

  mutating func reset() { self = Self() }

  mutating func focusChanged(_ focused: Bool) {
    keyboardFocused = focused
    interact()
  }

  mutating func panelChanged(_ presented: Bool) {
    panelPresented = presented
    interact()
  }

  static func finishingLabel(mode: OverlayMode, transcribing: Bool, terminal: Bool) -> String? {
    guard !terminal, transcribing || mode == .finalizing else { return nil }
    return String(
      localized: "Finishing…", comment: "Overlay cap while the take is transcribed")
  }
}

/// One capsule for the cap and tools. An opaque palette token replaces material
/// when transparency is reduced; no glass reaches the resize band's hit region.
struct OverlayActionsSurface: ViewModifier {
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  let palette: OverlayAppearancePalette
  let glassNamespace: Namespace.ID

  @ViewBuilder
  func body(content: Content) -> some View {
    if reduceTransparency {
      content
        .background { Capsule().fill(palette.desktopBackground.color) }
        .overlay { Capsule().strokeBorder(palette.border.color, lineWidth: 1) }
    } else if #available(macOS 26.0, *) {
      content
        .glassEffect(.regular.interactive(), in: Capsule())
        .glassEffectID("overlay-actions", in: glassNamespace)
        .glassEffectTransition(reduceMotion ? .identity : .matchedGeometry)
    } else {
      content
        .background { Capsule().fill(.regularMaterial) }
        .overlay { Capsule().strokeBorder(palette.border.color, lineWidth: 1) }
    }
  }
}

/// Symbols shared by the rendered controls and their collision census.
enum OverlayControlSymbols {
  static let history = "clock.arrow.circlepath"
  static let previousTake = "tray.and.arrow.up"
  static let actions = "ellipsis"
  static let closeActions = "xmark"
  static let placement = "location.viewfinder"
  static let miniToMidi = "arrow.right"
  static let midiToTranscript = "chevron.down"
  static let returnToMini = "arrow.left"
}

enum OverlayDockVisuals {
  static func hoverOpacity(isHovering: Bool) -> Double {
    isHovering ? 0.14 : 0
  }
}

/// Fixed icon row. Hover content lives above its anchor, outside row layout.
@MainActor
struct OverlayIntentRail: View {
  @State private var presented: String?
  let phase: String
  let intents: [OverlayIntent]
  let palette: OverlayAppearancePalette
  var formatLevel: FormattingPolicyOption = .correction
  var cloudRetranscribeConfigured = false
  /// Specific reason the shown take cannot be transcribed again (an archived
  /// take whose audio is gone). Nil keeps the engine buttons.
  var retranscribeUnavailableReason: String?
  /// Why history cannot be opened onto the canvas right now; nil allows it.
  var historyOpenRefusal: String?
  var admitHistoryOpen: () -> UInt64 = { 0 }
  var onOpenArchive: (OverlayArchivedTranscript, UInt64) -> OverlayArchiveOpenOutcome = { _, _ in
    .superseded
  }
  var onHistoryDismiss: () -> Void = {}
  let onIntent: (OverlayIntent) -> Void
  var onRetranscribe: (OverlayRetranscribePass) -> Void = { _ in }
  var onFormatOnce: (FormattingPolicyOption) -> Void = { _ in }
  var onDismiss: () -> Void = {}
  var onInteraction: () -> Void = {}
  var onPresentationChange: (Bool) -> Void = { _ in }

  var body: some View {
    HStack(spacing: 2) {
      OverlayHoverControl(
        id: "overlay-history-menu", title: String(localized: "Transcription history"),
        palette: palette,
        presented: $presented
      ) {
        Image(systemName: OverlayControlSymbols.history).frame(width: 24, height: 24)
      } detail: { close in
        OverlayTranscriptHistory(
          openRefusal: historyOpenRefusal,
          admitOpen: admitHistoryOpen,
          onOpen: { archived, admission in
            onInteraction()
            return onOpenArchive(archived, admission)
          },
          onDismiss: onHistoryDismiss,
          onOpened: close)
      }
      if intents.contains(.recoverSuperseded) || intents.contains(.discardSuperseded) {
        OverlayHoverControl(
          id: "overlay-previous-take-menu", title: String(localized: "Previous take"),
          palette: palette,
          presented: $presented
        ) {
          Image(systemName: OverlayControlSymbols.previousTake).frame(width: 24, height: 24)
        } detail: { close in
          VStack(alignment: .leading, spacing: 8) {
            if intents.contains(.recoverSuperseded) {
              Button(OverlayIntent.recoverSuperseded.accessibilityLabel) {
                close()
                dispatch(.recoverSuperseded)
              }
              .accessibilityIdentifier("overlay-intent-recover-superseded")
            }
            if intents.contains(.discardSuperseded) {
              Button(OverlayIntent.discardSuperseded.accessibilityLabel, role: .destructive) {
                close()
                dispatch(.discardSuperseded)
              }
              .accessibilityIdentifier("overlay-intent-discard-superseded")
            }
          }
        }
      }
      ForEach(OverlayDockLayout(projectedIntents: intents).visibleIntents, id: \.self) { intent in
        if intent != .recoverSuperseded && intent != .discardSuperseded {
          OverlayHoverControl(
            id: "overlay-intent-\(intent.rawValue)", title: intent.accessibilityLabel,
            palette: palette, presented: $presented,
            action: intent == .format || intent == .retranscribe ? nil : { dispatch(intent) }
          ) {
            Image(systemName: intent.systemImage).frame(width: 24, height: 24)
          } detail: { close in
            if intent == .format {
              HStack(spacing: 10) {
                ForEach([FormattingPolicyOption.correction, .smart, .max], id: \.rawValue) {
                  level in
                  Button(level.visibleName) {
                    close()
                    formatOnce(level)
                  }
                  .accessibilityIdentifier("overlay-format-level-\(level.rawValue)")
                }
              }
              .buttonStyle(.borderless)
              .controlSize(.small)
              .font(CSFont.ui(11, .medium))
            } else if intent == .retranscribe {
              VStack(alignment: .leading, spacing: 8) {
                Text(String(localized: "Transcribe this take again"))
                Text(
                  retranscribeUnavailableReason
                    ?? String(localized: "Uses audio from the take currently shown in the overlay.")
                )
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityIdentifier("overlay-retranscribe-source")
                if retranscribeUnavailableReason == nil {
                  HStack(spacing: 10) {
                    Button(OverlayRetranscribeCopy.local) {
                      close()
                      retranscribe(.fullHq)
                    }
                    .accessibilityIdentifier("overlay-retranscribe-hq")
                    if cloudRetranscribeConfigured {
                      Button(OverlayRetranscribeCopy.cloud) {
                        close()
                        retranscribe(.cloud)
                      }
                      .accessibilityIdentifier("overlay-retranscribe-cloud")
                    }
                  }
                }
              }
              .buttonStyle(.borderless)
              .controlSize(.small)
              .font(CSFont.ui(11, .medium))
            } else {
              Text(intent.accessibilityLabel)
            }
          }
          .accessibilityHint(intent.accessibilityHint)
        }
      }
    }
    .fixedSize(horizontal: true, vertical: true)
    .onChange(of: presented) { _, value in
      onPresentationChange(value != nil)
      if value != nil { onInteraction() }
    }
    .onDisappear { onPresentationChange(false) }
    .onExitCommand {
      presented = nil
      onDismiss()
    }
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Overlay actions")
    .accessibilityValue(Self.accessibilityValue(for: phase))
    .accessibilityIdentifier("overlay-intent-dock")
  }

  static func projectedIntents(for state: OverlayState) -> [OverlayIntent] {
    if state.revisionCommitPending || state.formatterCommitPending || state.archiveActionPending {
      return []
    }
    if state.archivedTranscript != nil {
      return archivedIntents(for: state)
    }
    if state.isRevisionDraftDirty {
      return recoveryIntents(for: state) + [.commitRevision, .discardRevision, .close]
    }
    return recoveryIntents(for: state)
      + (state.canUndoRetranscribe ? [.undoRetranscribe] : [])
      + projectedIntents(
        phase: state.mode,
        canPaste: state.canPaste,
        canInsert: state.canInsert,
        canCopy: state.canCopy,
        canRetranscribe: state.canRetranscribe,
        canFormat: state.canFormat,
        canSendToAgent: state.canSendToAgent
      )
  }

  /// An archive reopened from history has no reducer projection, so its rail
  /// is the formatted table with the archive's own facts: Insert still passes
  /// the Rust paste route's target checks, Retranscribe states its own
  /// unavailability, Send to Agent is the same explicit click, and Undo
  /// restores the version the archive's last format or retranscription
  /// replaced in its own revision chain.
  static func archivedIntents(for state: OverlayState) -> [OverlayIntent] {
    if state.isRevisionDraftDirty {
      return recoveryIntents(for: state) + [.commitRevision, .discardRevision, .close]
    }
    return recoveryIntents(for: state)
      + (state.archivedUndoIntent.map { [$0] } ?? [])
      + [.insertPaste, .copy, .retranscribe]
      + (state.engine == nil ? [] : [.format])
      + (state.canSendToAgent ? [.sendToAgent] : [])
      + [.close]
  }

  /// The two commands the reducer does not project, and the only ones sourced
  /// from local presentation state. They LEAD the rail because the phase table
  /// below is allowed to be nearly empty — `.error` projects `[.close]` and a
  /// live `.listening` capture projects `[.finish, .close]` — and neither an
  /// error nor a new take may be the reason an unacknowledged edit becomes
  /// unreachable. They carry no delivery legality: recovery copies retained
  /// bytes to the pasteboard, discard drops them, and neither touches the
  /// reducer, the current canvas or focus.
  static func recoveryIntents(for state: OverlayState) -> [OverlayIntent] {
    state.hasRecoverableSupersededWork ? [.recoverSuperseded, .discardSuperseded] : []
  }

  /// Frozen `overlay-canvas-v1` projection table. A false bit omits its
  /// command; the dock never reconstructs delivery legality from local state.
  ///
  /// The two terminal outcomes that are not `formatted` still route through
  /// the producer's bits rather than through a fixed list. A refused take and
  /// a failed handover both leave real words on the canvas, and the whole
  /// reason they end up here is that their destination did not work — which
  /// is exactly when a user needs Copy, Insert or Retranscribe most. Printing
  /// only Close would have hidden the recovery behind the word "error" while
  /// the producer was saying, bit by bit, that recovery was available.
  ///
  /// Format remains available for refused coverage when the reducer projects
  /// permission. The request still passes through the terminal revision CAS;
  /// any reducer refusal is shown to the user without changing the seal verdict.
  static func projectedIntents(
    phase: OverlayMode,
    canPaste: Bool,
    canInsert: Bool,
    canCopy: Bool,
    canRetranscribe: Bool,
    canFormat: Bool,
    canSendToAgent: Bool = false
  ) -> [OverlayIntent] {
    switch phase {
    case .listening:
      [.finish] + (canCopy ? [.copy] : []) + [.close]
    case .finalizing:
      (canCopy ? [.copy] : []) + [.close]
    case .formatted:
      ((canPaste || canInsert) ? [.insertPaste] : [])
        + (canCopy ? [.copy] : [])
        + (canRetranscribe ? [.retranscribe] : [])
        + (canFormat ? [.format] : [])
        + (canSendToAgent ? [.sendToAgent] : [])
        + [.close]
    case .coverageRefused:
      ((canPaste || canInsert) ? [.insertPaste] : [])
        + (canCopy ? [.copy] : [])
        + (canRetranscribe ? [.retranscribe] : [])
        + (canFormat ? [.format] : [])
        + (canSendToAgent ? [.sendToAgent] : [])
        + [.close]
    case .error:
      ((canPaste || canInsert) ? [.insertPaste] : [])
        + (canCopy ? [.copy] : [])
        + (canRetranscribe ? [.retranscribe] : [])
        + (canSendToAgent ? [.sendToAgent] : [])
        + [.close]
    case .noSpeech:
      (canRetranscribe ? [.retranscribe] : []) + [.close]
    }
  }

  static func accessibilityValue(for phase: String) -> String {
    phase
  }

  func dispatch(_ intent: OverlayIntent) {
    onInteraction()
    onIntent(intent)
  }

  func retranscribe(_ pass: OverlayRetranscribePass) {
    onInteraction()
    onRetranscribe(pass)
  }

  func formatOnce(_ level: FormattingPolicyOption) {
    onInteraction()
    onFormatOnce(level)
  }
}

extension OverlayIntent {
  var accessibilityLabel: String {
    switch self {
    case .startRecording: String(localized: "Start dictation")
    case .finish: String(localized: "Finish recording")
    case .commitRevision: String(localized: "Commit transcript revision")
    case .discardRevision: String(localized: "Discard transcript draft")
    case .copy: String(localized: "Copy transcript")
    case .insertPaste: String(localized: "Insert transcript")
    case .retranscribe: String(localized: "Transcribe this take again")
    case .undoRetranscribe: String(localized: "Undo retranscribe")
    case .undoFormat: String(localized: "Undo format")
    case .format: String(localized: "Format transcript")
    case .sendToAgent: String(localized: "Send transcript to Agent")
    case .recoverSuperseded: String(localized: "Copy previous take to clipboard")
    case .discardSuperseded: String(localized: "Discard previous take")
    case .close: String(localized: "Close overlay")
    }
  }

  var accessibilityHint: String {
    switch self {
    case .startRecording:
      String(localized: "Starts a new take in the current dictation mode")
    case .finish: String(localized: "Stops capture and requests the final projection")
    case .commitRevision: String(localized: "Commits this draft through the transcript ledger")
    case .discardRevision: String(localized: "Restores the latest projected transcript")
    case .copy: String(localized: "Copies the projected transcript")
    case .insertPaste:
      String(localized: "Sends the projected transcript to the selected destination")
    case .retranscribe: String(localized: "Requests another transcription of this recording")
    case .undoRetranscribe:
      String(localized: "Restores the transcript this retranscribe replaced, as a new revision")
    case .undoFormat:
      String(localized: "Restores the transcript this format replaced, as a new revision")
    case .format: String(localized: "Requests formatting between takes")
    case .sendToAgent: String(localized: "Sends the accepted transcript to Agent")
    case .recoverSuperseded:
      String(
        localized:
          "Copies the retained previous take, including any unsaved edit, to the clipboard")
    case .discardSuperseded:
      String(localized: "Drops the retained previous take without recovering it")
    case .close: String(localized: "Closes the dictation overlay")
    }
  }

  var systemImage: String {
    switch self {
    case .startRecording: "mic.fill"
    case .finish: "stop.circle"
    case .commitRevision: "checkmark.circle"
    case .discardRevision: "arrow.uturn.backward.circle"
    case .copy: "doc.on.doc"
    case .insertPaste: "arrow.down.doc"
    case .retranscribe: "arrow.clockwise"
    case .undoRetranscribe: "arrow.uturn.backward"
    case .undoFormat: "arrow.uturn.backward"
    case .format: "textformat"
    case .sendToAgent: "paperplane"
    case .recoverSuperseded: "arrow.up.doc"
    case .discardSuperseded: "trash"
    case .close: "circle.fill"
    }
  }
  var helpText: String { accessibilityLabel }
}

/// Engine buttons under "Transcribe this take again", shared by the intent rail and the
/// coverage popover. They have their own keys: here the word answers "where
/// should it run", while `Cloud` elsewhere names an engine.
enum OverlayRetranscribeCopy {
  static var local: String {
    String(
      localized: "overlay.retranscribe.local", defaultValue: "Local",
      comment: "Button under Transcribe again: run the transcription on this Mac")
  }

  static var cloud: String {
    String(
      localized: "overlay.retranscribe.cloud", defaultValue: "Cloud",
      comment: "Button under Transcribe again: run the transcription in the cloud")
  }
}
