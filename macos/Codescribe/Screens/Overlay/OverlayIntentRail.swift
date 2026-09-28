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
  private(set) var pointerInside = false
  private(set) var panelPresented = false
  private(set) var keyboardFocused = false
  private(set) var hideDeadline: ContinuousClock.Instant?

  mutating func pointerChanged(_ inside: Bool, at now: ContinuousClock.Instant = .now) {
    pointerInside = inside
    if inside { phase = .open }
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
    if focused { phase = .open }
    interact()
  }

  mutating func panelChanged(_ presented: Bool) {
    panelPresented = presented
    interact()
  }

  static func finishingLabel(mode: OverlayMode, transcribing: Bool, terminal: Bool) -> String? {
    !terminal && (transcribing || mode == .finalizing) ? "Finishing…" : nil
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
  static let autoPasteOff = "arrow.down.to.line"
  static let autoPasteOn = "arrow.down.to.line.compact"
  static let placement = "location.viewfinder"
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
  var history: [CsDocumentHistoryEntry] = []
  var historyAvailable: Bool = false
  var currentRevision: UInt64 = 0
  var formatLevel: FormattingPolicyOption = .correction
  var cloudRetranscribeConfigured = false
  let onIntent: (OverlayIntent) -> Void
  var onRetranscribe: (OverlayRetranscribePass) -> Void = { _ in }
  var onRestore: (UInt64) -> Void = { _ in }
  var onHistoryRequest: () -> Void = {}
  var onFormatOnce: (FormattingPolicyOption) -> Void = { _ in }
  var onDismiss: () -> Void = {}
  var onInteraction: () -> Void = {}
  var onPresentationChange: (Bool) -> Void = { _ in }

  var body: some View {
    HStack(spacing: 2) {
      if historyAvailable || !history.isEmpty {
        OverlayHoverControl(
          id: "overlay-history-menu", title: "Transcript history", palette: palette,
          presented: $presented
        ) {
          Image(systemName: OverlayControlSymbols.history).frame(width: 24, height: 24)
        } detail: { close in
          historyContent(close: close)
        }
      }
      if intents.contains(.recoverSuperseded) || intents.contains(.discardSuperseded) {
        OverlayHoverControl(
          id: "overlay-previous-take-menu", title: "Previous take", palette: palette,
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
            } else if intent == .retranscribe {
              HStack(spacing: 10) {
                Button("Local") {
                  close()
                  retranscribe(.fullHq)
                }
                .accessibilityIdentifier("overlay-retranscribe-hq")
                if cloudRetranscribeConfigured {
                  Button("Cloud") {
                    close()
                    retranscribe(.cloud)
                  }
                  .accessibilityIdentifier("overlay-retranscribe-cloud")
                }
              }
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

  func historyContent(close: @escaping () -> Void) -> some View {
    VStack(alignment: .leading, spacing: 8) {
      Button("Refresh transcript history") { onHistoryRequest() }
      ScrollView {
        LazyVStack(alignment: .leading, spacing: 8) {
          ForEach(history.reversed(), id: \.revision) { entry in
            Button {
              close()
              onRestore(entry.revision)
            } label: {
              VStack(alignment: .leading, spacing: 3) {
                Text("Version \(entry.revision)")
                  .fontWeight(.semibold)
                Text(entry.renderedText)
                  .lineLimit(2)
                  .foregroundStyle(.secondary)
              }
              .frame(maxWidth: .infinity, alignment: .leading)
              .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(entry.revision == currentRevision)
          }
        }
      }
      .frame(height: 300)
    }
    .frame(width: 260)
  }

  static func projectedIntents(for state: OverlayState) -> [OverlayIntent] {
    if state.revisionCommitPending || state.formatterCommitPending {
      return []
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
    case .finish: "Finish recording"
    case .commitRevision: "Commit transcript revision"
    case .discardRevision: "Discard transcript draft"
    case .copy: "Copy transcript"
    case .insertPaste: "Insert transcript"
    case .retranscribe: "Retranscribe recording"
    case .undoRetranscribe: "Undo retranscribe"
    case .format: "Format transcript"
    case .sendToAgent: "Send transcript to Agent"
    case .recoverSuperseded: "Copy previous take to clipboard"
    case .discardSuperseded: "Discard previous take"
    case .close: "Close overlay"
    }
  }

  var accessibilityHint: String {
    switch self {
    case .finish: "Stops capture and requests the final projection"
    case .commitRevision: "Commits this draft through the transcript ledger"
    case .discardRevision: "Restores the latest projected transcript"
    case .copy: "Copies the projected transcript"
    case .insertPaste: "Sends the projected transcript to the selected destination"
    case .retranscribe: "Requests another transcription of this recording"
    case .undoRetranscribe: "Restores the transcript this retranscribe replaced, as a new revision"
    case .format: "Requests formatting between takes"
    case .sendToAgent: "Sends the accepted transcript to Agent"
    case .recoverSuperseded:
      "Copies the retained previous take, including any unsaved edit, to the clipboard"
    case .discardSuperseded: "Drops the retained previous take without recovering it"
    case .close: "Closes the dictation overlay"
    }
  }

  var systemImage: String {
    switch self {
    case .finish: "stop.circle"
    case .commitRevision: "checkmark.circle"
    case .discardRevision: "arrow.uturn.backward.circle"
    case .copy: "doc.on.doc"
    case .insertPaste: "arrow.down.doc"
    case .retranscribe: "arrow.clockwise"
    case .undoRetranscribe: "arrow.uturn.backward"
    case .format: "textformat"
    case .sendToAgent: "paperplane"
    case .recoverSuperseded: "arrow.up.doc"
    case .discardSuperseded: "trash"
    case .close: "circle.fill"
    }
  }
  var helpText: String { accessibilityLabel }
}
