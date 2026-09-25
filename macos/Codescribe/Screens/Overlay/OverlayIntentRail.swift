import SwiftUI

/// Action availability stays projected; chrome visibility is local presentation only.
struct OverlayDockLayout: Equatable {
  static let minimumCanvasWidth: CGFloat = 320
  let projectedIntents: [OverlayIntent]
  var visibleIntents: [OverlayIntent] { projectedIntents.filter { $0 != .close } }
}

enum OverlayChromeVisibility {
  /// Chrome stays ephemeral (Founder cut): pointer, keyboard focus or VoiceOver
  /// reveal it, nothing else. `retainedWork` is the one standing exception and
  /// it defaults off, so every existing caller keeps the ephemeral contract.
  ///
  /// Unacknowledged superseded work has to be reachable without the user first
  /// guessing to hover a panel that is currently showing a NEW take. Revealing
  /// the rail exposes the labelled recover/discard commands only — never the
  /// previous words, which stay off the canvas.
  static func actionsVisible(
    pointerInside: Bool,
    keyboardFocus: Bool,
    voiceOver: Bool,
    retainedWork: Bool = false
  ) -> Bool {
    pointerInside || keyboardFocus || voiceOver || retainedWork
  }
}

enum OverlayDockVisuals {
  static func hoverOpacity(isHovering: Bool) -> Double {
    isHovering ? 0.14 : 0
  }
}

/// The overlay's sole action surface. The reducer owns action availability;
/// this view only renders the projected commands, the engine chip, the
/// transient notice and formatting level floating over the transcript.
@MainActor
struct OverlayIntentRail: View {
  @FocusState private var focusedControl: String?
  let onFocusChange: (Bool) -> Void
  let phase: String
  let intents: [OverlayIntent]
  let palette: OverlayAppearancePalette
  let footerEngineLabel: String
  let footerNotice: String?
  let footerEngineDot: Color
  let history: [CsDocumentHistoryEntry]
  let historyAvailable: Bool
  let currentRevision: UInt64
  let formatLevel: FormattingPolicyOption
  let onIntent: (OverlayIntent) -> Void
  let onRetranscribe: (OverlayRetranscribePass) -> Void
  let onRestore: (UInt64) -> Void
  let onHistoryRequest: () -> Void
  let onFormatLevel: (FormattingPolicyOption) -> Void

  init(
    phase: String,
    intents: [OverlayIntent],
    palette: OverlayAppearancePalette,
    footerEngineLabel: String = "",
    footerNotice: String? = nil,
    footerEngineDot: Color = .clear,
    history: [CsDocumentHistoryEntry] = [],
    historyAvailable: Bool? = nil,
    currentRevision: UInt64 = 0,
    formatLevel: FormattingPolicyOption = .correction,
    onIntent: @escaping (OverlayIntent) -> Void,
    onRetranscribe: @escaping (OverlayRetranscribePass) -> Void = { _ in },
    onRestore: @escaping (UInt64) -> Void = { _ in },
    onHistoryRequest: @escaping () -> Void = {},
    onFormatLevel: @escaping (FormattingPolicyOption) -> Void = { _ in },
    onFocusChange: @escaping (Bool) -> Void = { _ in }
  ) {
    self.onFocusChange = onFocusChange
    self.phase = phase
    self.intents = intents
    self.palette = palette
    self.footerEngineLabel = footerEngineLabel
    self.footerNotice = footerNotice
    self.footerEngineDot = footerEngineDot
    self.history = history
    self.historyAvailable = historyAvailable ?? !history.isEmpty
    self.currentRevision = currentRevision
    self.formatLevel = formatLevel
    self.onIntent = onIntent
    self.onRetranscribe = onRetranscribe
    self.onRestore = onRestore
    self.onHistoryRequest = onHistoryRequest
    self.onFormatLevel = onFormatLevel
  }

  var body: some View {
    VStack(spacing: 4) {
      HStack(spacing: CSSpace.xs) {
        engineChip
        footerNoticeText
      }
      .padding(.horizontal, 8)
      .background(.regularMaterial, in: Capsule())
      HStack(spacing: 4) {
        if historyAvailable {
          historyMenu
        }
        ForEach(intents, id: \.self) { intent in
          if intent == .retranscribe {
            retranscribeMenu
              .focused($focusedControl, equals: intent.rawValue)
          } else if intent == .format {
            formatLevelMenu
            OverlayDockButton(
              title: intent.accessibilityLabel,
              systemImage: intent.systemImage,
              hint: intent.accessibilityHint,
              identifier: "overlay-intent-\(intent.rawValue)",
              palette: palette
            ) {
              dispatch(intent)
            }
            .focused($focusedControl, equals: intent.rawValue)
          } else if intent != .close {
            OverlayDockButton(
              title: intent.accessibilityLabel,
              systemImage: intent.systemImage,
              hint: intent.accessibilityHint,
              identifier: "overlay-intent-\(intent.rawValue)",
              palette: palette
            ) {
              dispatch(intent)
            }
            .focused($focusedControl, equals: intent.rawValue)
          }
        }
      }
      .padding(6)
      .buttonStyle(.plain)
      .background(.regularMaterial, in: Capsule())
      .overlay { Capsule().strokeBorder(palette.border.color, lineWidth: 1) }
    }
    .fixedSize(horizontal: false, vertical: true)
    .frame(maxWidth: .infinity, alignment: .center)
    .onChange(of: focusedControl) { _, control in onFocusChange(control != nil) }
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Overlay actions")
    .accessibilityValue(Self.accessibilityValue(for: phase))
    .accessibilityIdentifier("overlay-intent-dock")
  }

  /// Serving-engine evidence, inert. Truncates first when the window sits at
  /// its 320 pt floor so the commands never do.
  private var engineChip: some View {
    HStack(spacing: CSSpace.xxs) {
      Text("●")
        .foregroundStyle(footerEngineDot)
      Text(footerEngineLabel)
        .foregroundStyle(palette.mutedText.color)
        .lineLimit(1)
        .truncationMode(.tail)
    }
    .csMono(10, .medium)
    .layoutPriority(-1)
    .allowsHitTesting(false)
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-footer-engine")
  }

  @ViewBuilder
  private var footerNoticeText: some View {
    if let footerNotice, !footerNotice.isEmpty {
      Text(footerNotice)
        .csMono(10, .medium)
        .foregroundStyle(palette.mutedText.color)
        .lineLimit(1)
        .truncationMode(.tail)
        .accessibilityIdentifier("overlay-footer-notice")
    }
  }

  /// Retranscribe is opt-in with the pass picked here: Full HQ (local
  /// Whisper file pass) or Cloud. The formatting level is not overlay chrome;
  /// the operator sets it in the tray quick settings (Founder 2026-09-09).
  private var retranscribeMenu: some View {
    Menu {
      ForEach(OverlayRetranscribePass.allCases) { pass in
        Button(pass.visibleName) { retranscribe(pass) }
          .help(pass.help)
          .accessibilityIdentifier("overlay-retranscribe-\(pass.rawValue)")
      }
    } label: {
      Label(
        OverlayIntent.retranscribe.accessibilityLabel,
        systemImage: OverlayIntent.retranscribe.systemImage
      )
      .labelStyle(.iconOnly)
      .frame(width: 32, height: 28)
      .contentShape(RoundedRectangle(cornerRadius: CSRadius.chip, style: .continuous))
      .foregroundStyle(palette.primaryText.color)
    }
    .menuStyle(.button)
    .buttonStyle(.plain)
    .menuIndicator(.hidden)
    .help(OverlayIntent.retranscribe.accessibilityHint)
    .accessibilityLabel(OverlayIntent.retranscribe.accessibilityLabel)
    .accessibilityHint(OverlayIntent.retranscribe.accessibilityHint)
    .accessibilityIdentifier("overlay-intent-\(OverlayIntent.retranscribe.rawValue)")
  }

  private var historyMenu: some View {
    Menu {
      Button("Refresh transcript history") { onHistoryRequest() }
        .accessibilityIdentifier("overlay-history-refresh")
      ForEach(history, id: \.revision) { entry in
        Button {
          onRestore(entry.revision)
        } label: {
          Text("Revision \(entry.revision) · \(entry.provenance) · \(entry.emittedAt)")
          Text(String(entry.renderedText.prefix(64)).replacingOccurrences(of: "\n", with: " "))
        }
        .disabled(entry.revision == currentRevision)
        .accessibilityIdentifier("overlay-history-revision-\(entry.revision)")
      }
    } label: {
      Label("Transcript history", systemImage: "clock.arrow.circlepath")
        .labelStyle(.iconOnly)
        .frame(width: 32, height: 28)
    }
    .menuStyle(.button)
    .buttonStyle(.plain)
    .menuIndicator(.hidden)
    .help("Restore an earlier transcript as a new revision")
    .accessibilityLabel("Transcript version history")
    .accessibilityIdentifier("overlay-history-menu")
  }

  private var formatLevelMenu: some View {
    Menu {
      ForEach(FormattingPolicyOption.editablePrompts) { level in
        Button {
          onFormatLevel(level)
        } label: {
          if level == formatLevel { Image(systemName: "checkmark") }
          Text(level.visibleName)
        }
        .accessibilityIdentifier("overlay-format-level-\(level.rawValue)")
      }
    } label: {
      Text(formatLevel.visibleName)
        .csMono(10, .medium)
        .frame(minWidth: 52, minHeight: 28)
    }
    .menuStyle(.button)
    .buttonStyle(.plain)
    .menuIndicator(.hidden)
    .help("Choose Correction, Smart, or Max formatting")
    .accessibilityLabel("Formatter level")
    .accessibilityIdentifier("overlay-format-level-picker")
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
    onIntent(intent)
  }

  func retranscribe(_ pass: OverlayRetranscribePass) {
    onRetranscribe(pass)
  }
}

@MainActor
private struct OverlayDockButton: View {
  @State private var isHovering = false

  let title: String
  let systemImage: String
  let hint: String
  let identifier: String
  let palette: OverlayAppearancePalette
  let action: () -> Void

  var body: some View {
    Button(title, systemImage: systemImage, action: action)
      .buttonStyle(.plain)
      .labelStyle(.iconOnly)
      .frame(width: 32, height: 28)
      .contentShape(RoundedRectangle(cornerRadius: CSRadius.chip, style: .continuous))
      .foregroundStyle(palette.primaryText.color)
      .background {
        RoundedRectangle(cornerRadius: CSRadius.chip, style: .continuous)
          .fill(
            palette.primaryText.color.opacity(
              OverlayDockVisuals.hoverOpacity(isHovering: isHovering)))
      }
      .onHover { isHovering = $0 }
      .help(title)
      .accessibilityLabel(title)
      .accessibilityHint(hint)
      .accessibilityIdentifier(identifier)
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
    case .recoverSuperseded: "Recover previous transcript"
    case .discardSuperseded: "Discard previous transcript"
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
    case .recoverSuperseded: "clock.arrow.circlepath"
    case .discardSuperseded: "trash"
    case .close: "circle.fill"
    }
  }
  var helpText: String { accessibilityLabel }
}
