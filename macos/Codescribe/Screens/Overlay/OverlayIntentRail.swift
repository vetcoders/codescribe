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
  enum Phase: Equatable { case idle, hover, open }
  private(set) var phase: Phase = .idle
  private(set) var pointerInside = false
  private(set) var hideDeadline: ContinuousClock.Instant?

  mutating func pointerChanged(_ inside: Bool, at now: ContinuousClock.Instant = .now) {
    pointerInside = inside
    if phase != .open { phase = inside ? .hover : .idle }
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
    hideDeadline = phase == .open && !pointerInside ? now.advanced(by: .seconds(3)) : nil
  }

  mutating func expire(at now: ContinuousClock.Instant = .now) {
    guard phase == .open, !pointerInside, let hideDeadline, now >= hideDeadline else { return }
    dismiss()
  }

  mutating func dismiss() {
    phase = .idle
    hideDeadline = nil
  }

  mutating func reset() { self = Self() }

  static func captionSlot(
    hovered: String?, notice: String?, engineLabel: String?
  ) -> (text: String, dimmed: Bool)? {
    if let hovered, !hovered.isEmpty { return (hovered, false) }
    if let notice, !notice.isEmpty { return (notice, false) }
    if let engineLabel, !engineLabel.isEmpty { return (engineLabel, true) }
    return nil
  }

  static func pillLabel(phase: Phase, notice: String?) -> String? {
    guard phase != .open else { return nil }
    if let notice, !notice.isEmpty { return notice }
    return phase == .hover ? "Actions…" : nil
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

/// Measures the single caption at its ideal width, then yields to the tool row.
/// A maximum-width frame alone would reserve empty space after short labels.
struct OverlayCaptionLayout: Layout {
  static func width(natural: CGFloat, proposed: CGFloat?) -> CGFloat {
    max(0, min(natural, 200, proposed ?? .infinity))
  }

  func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
    guard let caption = subviews.first else { return .zero }
    let natural = caption.sizeThatFits(.unspecified)
    let width = Self.width(natural: natural.width, proposed: proposal.width)
    let fitted = caption.sizeThatFits(ProposedViewSize(width: width, height: proposal.height))
    return CGSize(width: width, height: fitted.height)
  }

  func placeSubviews(
    in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()
  ) {
    subviews.first?.place(
      at: CGPoint(x: bounds.minX, y: bounds.midY), anchor: .leading,
      proposal: ProposedViewSize(width: bounds.width, height: bounds.height))
  }
}

/// The overlay's sole action surface. The reducer owns action availability;
/// this view renders projected commands and one trailing caption slot inside
/// the capsule supplied by the parent.
@MainActor
struct OverlayIntentRail: View {
  @FocusState private var focusedControl: String?
  @State private var hoveredControl: String?
  let onFocusChange: (Bool) -> Void
  let onDismiss: () -> Void
  let onInteraction: () -> Void
  let phase: String
  let intents: [OverlayIntent]
  let palette: OverlayAppearancePalette
  let footerEngineLabel: String
  let footerNotice: String?
  let history: [CsDocumentHistoryEntry]
  let historyAvailable: Bool
  let currentRevision: UInt64
  let formatLevel: FormattingPolicyOption
  let onIntent: (OverlayIntent) -> Void
  let onRetranscribe: (OverlayRetranscribePass) -> Void
  let onRestore: (UInt64) -> Void
  let onHistoryRequest: () -> Void
  let onFormatOnce: (FormattingPolicyOption) -> Void

  init(
    phase: String,
    intents: [OverlayIntent],
    palette: OverlayAppearancePalette,
    footerEngineLabel: String = "",
    footerNotice: String? = nil,
    history: [CsDocumentHistoryEntry] = [],
    historyAvailable: Bool? = nil,
    currentRevision: UInt64 = 0,
    formatLevel: FormattingPolicyOption = .correction,
    onIntent: @escaping (OverlayIntent) -> Void,
    onRetranscribe: @escaping (OverlayRetranscribePass) -> Void = { _ in },
    onRestore: @escaping (UInt64) -> Void = { _ in },
    onHistoryRequest: @escaping () -> Void = {},
    onFormatOnce: @escaping (FormattingPolicyOption) -> Void = { _ in },
    onFocusChange: @escaping (Bool) -> Void = { _ in },
    onDismiss: @escaping () -> Void = {},
    onInteraction: @escaping () -> Void = {}
  ) {
    self.onFocusChange = onFocusChange
    self.onDismiss = onDismiss
    self.onInteraction = onInteraction
    self.phase = phase
    self.intents = intents
    self.palette = palette
    self.footerEngineLabel = footerEngineLabel
    self.footerNotice = footerNotice
    self.history = history
    self.historyAvailable = historyAvailable ?? !history.isEmpty
    self.currentRevision = currentRevision
    self.formatLevel = formatLevel
    self.onIntent = onIntent
    self.onRetranscribe = onRetranscribe
    self.onRestore = onRestore
    self.onHistoryRequest = onHistoryRequest
    self.onFormatOnce = onFormatOnce
  }

  var body: some View {
    HStack(spacing: 2) {
      if historyAvailable {
        historyMenu
          .focused($focusedControl, equals: "history")
          .onHover { setHovered("history", inside: $0) }
      }
      if intents.contains(.recoverSuperseded) || intents.contains(.discardSuperseded) {
        previousTakeMenu
          .focused($focusedControl, equals: "previous-take")
          .onHover { setHovered("previous-take", inside: $0) }
      }
      ForEach(intents, id: \.self) { intent in
        if intent == .retranscribe {
          retranscribeMenu
            .focused($focusedControl, equals: intent.rawValue)
            .onHover { setHovered(intent.rawValue, inside: $0) }
        } else if intent == .format {
          formatMenu
            .focused($focusedControl, equals: intent.rawValue)
            .onHover { setHovered(intent.rawValue, inside: $0) }
        } else if intent != .close && intent != .recoverSuperseded && intent != .discardSuperseded {
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
          .onHover { setHovered(intent.rawValue, inside: $0) }
        }
      }
      // Keep discovery in-panel: native tooltips may be suppressed while inactive.
      if let slot = OverlayActionsPresentation.captionSlot(
        hovered: caption(for: hoveredControl ?? focusedControl),
        notice: footerNotice, engineLabel: footerEngineLabel)
      {
        OverlayCaptionLayout {
          Text(slot.text)
            .csMono(10, .medium)
            .foregroundStyle(slot.dimmed ? palette.mutedText.color : palette.primaryText.color)
            .lineLimit(1)
            .truncationMode(.tail)
        }
        .padding(.leading, 4)
        .layoutPriority(-1)
        .allowsHitTesting(false)
        .accessibilityIdentifier("overlay-tool-caption")
      }
    }
    .buttonStyle(.plain)
    .fixedSize(horizontal: false, vertical: true)
    .onChange(of: focusedControl) { _, control in
      onFocusChange(control != nil)
      if control != nil { onInteraction() }
    }
    .onExitCommand {
      focusedControl = nil
      onFocusChange(false)
      onDismiss()
    }
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Overlay actions")
    .accessibilityValue(Self.accessibilityValue(for: phase))
    .accessibilityIdentifier("overlay-intent-dock")
  }

  /// Retranscribe is opt-in with the pass picked here: Full HQ (local
  /// Whisper file pass) or Cloud.
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
      .frame(width: 24, height: 24)
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

  private var previousTakeMenu: some View {
    Menu {
      if intents.contains(.recoverSuperseded) {
        Button(OverlayIntent.recoverSuperseded.accessibilityLabel) {
          dispatch(.recoverSuperseded)
        }
        .help(OverlayIntent.recoverSuperseded.accessibilityHint)
        .accessibilityIdentifier("overlay-intent-recover-superseded")
      }
      if intents.contains(.discardSuperseded) {
        Button(OverlayIntent.discardSuperseded.accessibilityLabel, role: .destructive) {
          dispatch(.discardSuperseded)
        }
        .help(OverlayIntent.discardSuperseded.accessibilityHint)
        .accessibilityIdentifier("overlay-intent-discard-superseded")
      }
    } label: {
      Label("Previous take", systemImage: OverlayControlSymbols.previousTake)
        .labelStyle(.iconOnly)
        .frame(width: 24, height: 24)
    }
    .menuStyle(.button)
    .buttonStyle(.plain)
    .menuIndicator(.hidden)
    .help("Previous take: copy to clipboard or discard retained work")
    .accessibilityLabel("Previous take")
    .accessibilityHint("Copy or discard the retained previous take")
    .accessibilityIdentifier("overlay-previous-take-menu")
  }

  private var historyMenu: some View {
    Menu {
      Button("Refresh transcript history") {
        onInteraction()
        onHistoryRequest()
      }
      .accessibilityIdentifier("overlay-history-refresh")
      ForEach(history, id: \.revision) { entry in
        Button {
          onInteraction()
          onRestore(entry.revision)
        } label: {
          Text("Revision \(entry.revision) · \(entry.provenance) · \(entry.emittedAt)")
          Text(String(entry.renderedText.prefix(64)).replacingOccurrences(of: "\n", with: " "))
        }
        .disabled(entry.revision == currentRevision)
        .accessibilityIdentifier("overlay-history-revision-\(entry.revision)")
      }
    } label: {
      Label("Transcript history", systemImage: OverlayControlSymbols.history)
        .labelStyle(.iconOnly)
        .frame(width: 24, height: 24)
    }
    .menuStyle(.button)
    .buttonStyle(.plain)
    .menuIndicator(.hidden)
    .help("Restore an earlier revision of this transcript")
    .accessibilityLabel("Transcript version history")
    .accessibilityIdentifier("overlay-history-menu")
  }

  private var formatMenu: some View {
    Menu {
      Text("Settings: \(formatLevel.visibleName)")
        .disabled(true)
      Divider()
      Button("Correction") { formatOnce(.correction) }
        .accessibilityIdentifier("overlay-format-level-correction")
      Button("Smart") { formatOnce(.smart) }
        .accessibilityIdentifier("overlay-format-level-smart")
      Button("Max") { formatOnce(.max) }
        .accessibilityIdentifier("overlay-format-level-max")
    } label: {
      Label(OverlayIntent.format.accessibilityLabel, systemImage: OverlayIntent.format.systemImage)
        .labelStyle(.iconOnly)
        .frame(width: 24, height: 24)
        .contentShape(RoundedRectangle(cornerRadius: CSRadius.chip, style: .continuous))
        .foregroundStyle(palette.primaryText.color)
    } primaryAction: {
      dispatch(.format)
    }
    .menuStyle(.button)
    .buttonStyle(.plain)
    .menuIndicator(.visible)
    .help(formatHelp)
    .accessibilityLabel(OverlayIntent.format.accessibilityLabel)
    .accessibilityHint(formatHelp)
    .accessibilityIdentifier("overlay-intent-format")
  }

  private var formatHelp: String {
    "Format (Settings: \(formatLevel.visibleName)) · menu: Correction, Smart or Max once"
  }

  private func setHovered(_ control: String, inside: Bool) {
    if inside {
      hoveredControl = control
      onInteraction()
    } else if hoveredControl == control {
      hoveredControl = nil
    }
  }

  func caption(for control: String?) -> String? {
    switch control {
    case "history": "History: earlier revisions"
    case "previous-take": "Previous take: copy or discard"
    case "format": formatHelp
    case .some(let identifier): OverlayIntent(rawValue: identifier)?.accessibilityLabel
    case .none: nil
    }
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
      .frame(width: 24, height: 24)
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
