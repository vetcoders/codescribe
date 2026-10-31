import SwiftUI

/// One place in the transcript-action column. The column keeps this order and
/// mounts a place only while `actionAvailability` says it can run. A dim
/// resting glyph is the rail's hover presentation, not a disabled placeholder.
/// `finish`, `startRecording` and `close` are not places: the header's
/// recording and brand controls are their only renders.
enum OverlayTranscriptActionSlot: Equatable, Hashable {
  case history
  case versions
  case undo
  case redo
  case previousTake
  case intent(OverlayIntent)
}

/// Whether one ordered action can run right now, derived from the projected
/// intents and the versions presentation. Pure, so availability can be
/// falsified without rendering. The column mounts `isEnabled` rows only.
struct OverlayTranscriptActionAvailability: Equatable {
  let slot: OverlayTranscriptActionSlot
  let isEnabled: Bool
}

/// One non-empty band of the rail, in the stable order `actionAvailability`
/// already uses. An empty band is absent, so a missing action leaves no
/// separator and no placeholder.
private struct OverlayIntentRailBand: Identifiable {
  let id: String
  let slots: [OverlayTranscriptActionSlot]
  let showsSeparator: Bool
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

/// Symbols shared by the rendered controls and their collision census.
enum OverlayControlSymbols {
  static let history = "clock.arrow.circlepath"
  static let versions = "square.stack"
  static let undo = "arrow.uturn.backward"
  static let redo = "arrow.uturn.forward"
  static let previousTake = "tray.and.arrow.up"
  static let actions = "ellipsis"
  static let closeActions = "xmark"
  static let pin = "pin"
  static let pinned = "pin.fill"
  static let miniToMidi = "chevron.left"
  static let midiToTranscript = "chevron.down"
  static let returnToMini = "arrow.up.right"
}

enum OverlayDockVisuals {
  static func hoverOpacity(isHovering: Bool) -> Double {
    isHovering ? 0.14 : 0
  }
}

/// Stable transcript-action inventory in a seven-slot scrolling palette.
/// Hover hints stay over the document, inside its window. The same control
/// remains the only render of each command, including those below the fold.
@MainActor
struct OverlayIntentRail: View {
  @State private var presented: String?
  let phase: String
  let intents: [OverlayIntent]
  let palette: OverlayAppearancePalette
  /// The automatic level, shown beside the one-take choices. Picking a level
  /// here formats this take only and never changes it.
  var formatLevel: FormattingPolicyOption = .correction
  var cloudRetranscribeConfigured = false
  /// Specific reason the shown take cannot be transcribed again (an archived
  /// take whose audio is gone). Nil keeps the engine buttons.
  var retranscribeUnavailableReason: String?
  /// Why history cannot be opened onto the canvas right now; nil allows it.
  var historyOpenRefusal: String?
  /// What the recovery action will do for the oldest retained work.
  var recoverSupersededLabel = OverlayIntent.recoverSuperseded.accessibilityLabel
  var admitHistoryOpen: () -> UInt64 = { 0 }
  var onOpenArchive: (OverlayArchivedTranscript, UInt64) -> OverlayArchiveOpenOutcome = { _, _ in
    .superseded
  }
  var onHistoryDismiss: () -> Void = {}
  /// Versions of the transcript on the canvas, with Undo and Redo over the
  /// same cursor; shown only when there is more than one version.
  var versions: OverlayTranscriptVersionsPresentation = .empty
  var onVersionsOpened: () -> Void = {}
  var onSelectVersion: (UInt64) -> Void = { _ in }
  let onIntent: (OverlayIntent) -> Void
  var onRetranscribe: (OverlayRetranscribePass) -> Void = { _ in }
  var onFormatOnce: (FormattingPolicyOption) -> Void = { _ in }
  var onDismiss: () -> Void = {}
  var onInteraction: () -> Void = {}
  var onPresentationChange: (Bool) -> Void = { _ in }

  /// Stable order of every transcript action this column knows. `isEnabled`
  /// is the existing availability: projected intents, the versions cursor,
  /// and history, which stays available. The column mounts enabled rows only.
  /// It does not keep a disabled placeholder for a row that cannot run.
  /// Recovery stays inside the previous-take menu. Undo and redo appear only
  /// while the versions cursor can move that way.
  static func actionAvailability(
    intents: [OverlayIntent], versions: OverlayTranscriptVersionsPresentation
  ) -> [OverlayTranscriptActionAvailability] {
    let projected = Set(intents)
    func slot(_ slot: OverlayTranscriptActionSlot, _ enabled: Bool)
      -> OverlayTranscriptActionAvailability
    {
      OverlayTranscriptActionAvailability(slot: slot, isEnabled: enabled)
    }
    return [
      slot(.history, true),
      slot(.versions, versions.isVisible),
      slot(.undo, versions.canUndo),
      slot(.redo, versions.canRedo),
      slot(
        .previousTake,
        projected.contains(.recoverSuperseded) || projected.contains(.discardSuperseded)),
      slot(.intent(.commitRevision), projected.contains(.commitRevision)),
      slot(.intent(.discardRevision), projected.contains(.discardRevision)),
      slot(.intent(.insertPaste), projected.contains(.insertPaste)),
      slot(.intent(.copy), projected.contains(.copy)),
      slot(.intent(.retranscribe), projected.contains(.retranscribe)),
      slot(.intent(.format), projected.contains(.format)),
      slot(.intent(.sendToAgent), projected.contains(.sendToAgent)),
    ]
  }

  /// Enabled rows, grouped in `actionAvailability` order. Empty bands drop
  /// out, so the separators only sit between actions that are actually there.
  private var visibleBands: [OverlayIntentRailBand] {
    let mounted = Self.actionAvailability(intents: intents, versions: versions)
      .filter(\.isEnabled)
      .map(\.slot)
    let order = ["cursor", "recovery", "document", "engine", "delivery"]
    var bands: [OverlayIntentRailBand] = []
    for id in order {
      let slots = mounted.filter { Self.bandID($0) == id }
      guard !slots.isEmpty else { continue }
      bands.append(
        OverlayIntentRailBand(id: id, slots: slots, showsSeparator: !bands.isEmpty))
    }
    return bands
  }

  private var visibleControlIDs: [String] {
    visibleBands.flatMap(\.slots).map(Self.controlID)
  }

  /// History, versions and the versions cursor share the first band. The
  /// previous take is its own band. Document, engine and delivery follow.
  /// Header-owned and menu-owned intents are not bands.
  private static func bandID(_ slot: OverlayTranscriptActionSlot) -> String? {
    switch slot {
    case .history, .versions, .undo, .redo:
      "cursor"
    case .previousTake:
      "recovery"
    case .intent(let intent):
      switch intent {
      case .commitRevision, .discardRevision, .insertPaste, .copy:
        "document"
      case .retranscribe, .format:
        "engine"
      case .sendToAgent:
        "delivery"
      case .startRecording, .finish, .undo, .redo, .recoverSuperseded, .discardSuperseded, .close:
        nil
      }
    }
  }

  private static func controlID(_ slot: OverlayTranscriptActionSlot) -> String {
    switch slot {
    case .history:
      "overlay-history-menu"
    case .versions:
      versionsControlID
    case .undo:
      "overlay-intent-\(OverlayIntent.undo.rawValue)"
    case .redo:
      "overlay-intent-\(OverlayIntent.redo.rawValue)"
    case .previousTake:
      "overlay-previous-take-menu"
    case .intent(let intent):
      "overlay-intent-\(intent.rawValue)"
    }
  }

  var body: some View {
    GeometryReader { geometry in
      let bands = visibleBands
      let rowCount = bands.reduce(0) { $0 + $1.slots.count }
      let groupGaps = max(0, bands.count - 1)
      let itemGaps = max(0, rowCount - bands.count)
      let availableSpacing = max(
        0, geometry.size.height - CGFloat(rowCount) * OverlayTranscriptActionsLayout.controlHeight
          - CGFloat(groupGaps) * OverlayTranscriptActionsLayout.separatorHeight)
      let naturalSpacing =
        2 * OverlayTranscriptActionsLayout.contentVerticalPadding
        + CGFloat(itemGaps) * OverlayTranscriptActionsLayout.itemSpacing
        + CGFloat(groupGaps) * 2 * OverlayTranscriptActionsLayout.groupSpacing
      let spacingScale = naturalSpacing > 0 ? min(1, availableSpacing / naturalSpacing) : 1
      ScrollView(.vertical, showsIndicators: false) {
        VStack(spacing: 0) {
          ForEach(bands) { band in
            VStack(spacing: 0) {
              if band.showsSeparator {
                groupSeparator
                  .padding(.vertical, OverlayTranscriptActionsLayout.groupSpacing * spacingScale)
              }
              VStack(spacing: OverlayTranscriptActionsLayout.itemSpacing * spacingScale) {
                ForEach(band.slots, id: \.self) { slot in
                  mountedControl(slot)
                }
              }
            }
          }
        }
        .frame(maxWidth: .infinity, alignment: .top)
        .padding(.vertical, OverlayTranscriptActionsLayout.contentVerticalPadding * spacingScale)
        .csAutohidingScrollers(.verticalHidden)
      }
      .csRailOverflowHints(color: palette.mutedText.color)
    }
    .focusable(false)
    .onChange(of: presented) { _, value in
      onPresentationChange(value != nil)
      if value != nil { onInteraction() }
      if value == Self.versionsControlID { onVersionsOpened() }
    }
    .onChange(of: visibleControlIDs) { _, ids in
      if let presented, !ids.contains(presented) {
        self.presented = nil
      }
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

  @ViewBuilder
  private func mountedControl(_ slot: OverlayTranscriptActionSlot) -> some View {
    switch slot {
    case .history:
      historyControl
    case .versions:
      versionsControl
    case .undo:
      undoRedoControl(.undo, title: versions.undoTitle)
    case .redo:
      undoRedoControl(.redo, title: versions.redoTitle)
    case .previousTake:
      previousTakeControl
    case .intent(let intent):
      intentControl(intent)
    }
  }

  private var groupSeparator: some View {
    Rectangle()
      .fill(palette.border.color.opacity(0.7))
      .frame(width: 18, height: OverlayTranscriptActionsLayout.separatorHeight)
      .accessibilityHidden(true)
  }

  private var historyControl: some View {
    OverlayHoverControl(
      id: "overlay-history-menu", title: String(localized: "Transcription history"),
      palette: palette,
      presented: $presented,
      emphasis: .transcriptRail,
      keepsHintInsideWindow: true,
      popoverArrow: .leading
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
  }

  private var versionsControl: some View {
    OverlayHoverControl(
      id: Self.versionsControlID, title: Self.versionsTitle(versions),
      palette: palette,
      presented: $presented,
      emphasis: .transcriptRail,
      keepsHintInsideWindow: true,
      popoverArrow: .leading
    ) {
      Image(systemName: OverlayControlSymbols.versions).frame(width: 24, height: 24)
    } detail: { close in
      OverlayTranscriptVersionsMenu(
        versions: versions,
        onSelect: { step in
          onInteraction()
          onSelectVersion(step)
        },
        close: close)
    }
    .accessibilityValue(versions.current?.title ?? "")
  }

  private var previousTakeControl: some View {
    OverlayHoverControl(
      id: "overlay-previous-take-menu", title: String(localized: "Previous take"),
      palette: palette,
      presented: $presented,
      emphasis: .transcriptRail,
      keepsHintInsideWindow: true,
      popoverArrow: .leading
    ) {
      Image(systemName: OverlayControlSymbols.previousTake).frame(width: 24, height: 24)
    } detail: { close in
      VStack(alignment: .leading, spacing: 8) {
        if intents.contains(.recoverSuperseded) {
          Button(recoverSupersededLabel) {
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

  private func intentControl(_ intent: OverlayIntent) -> some View {
    OverlayHoverControl(
      id: "overlay-intent-\(intent.rawValue)", title: intent.accessibilityLabel,
      palette: palette, presented: $presented,
      action: intent == .format || intent == .retranscribe ? nil : { dispatch(intent) },
      emphasis: .transcriptRail,
      keepsHintInsideWindow: true,
      popoverArrow: .leading,
      detailWidth: 240
    ) {
      Image(systemName: intent.systemImage).frame(width: 24, height: 24)
    } detail: { close in
      if intent == .format {
        VStack(alignment: .leading, spacing: 8) {
          HStack(spacing: 10) {
            ForEach(Self.formatOnceLevels, id: \.rawValue) { level in
              Button(level.visibleName) {
                close()
                formatOnce(level)
              }
              .conversationActionHover(
                restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color)
              .accessibilityIdentifier("overlay-format-level-\(level.rawValue)")
            }
          }
          .buttonStyle(.plain)
          .controlSize(.small)
          .font(CSFont.ui(11, .medium))
          // Max is the Agent with its history and tools, asked for this take
          // only; the automatic level below stays as it is.
          Text(
            String(
              localized: "overlay.format.maxHint",
              defaultValue: "Max asks the Agent for this take, with its history and tools.",
              comment: "Format menu: what the Max choice does")
          )
          .fixedSize(horizontal: false, vertical: true)
          Text(
            String(
              localized: "overlay.format.automaticLevel",
              defaultValue: "Automatic formatting stays \(formatLevel.automaticLevel.visibleName).",
              comment: "Format menu: one-take formats keep this automatic level (placeholder)"
            )
          )
          .fixedSize(horizontal: false, vertical: true)
          .accessibilityIdentifier("overlay-format-automatic-level")
        }
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
              .conversationActionHover(
                restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color)
              .accessibilityIdentifier("overlay-retranscribe-hq")
              if cloudRetranscribeConfigured {
                Button(OverlayRetranscribeCopy.cloud) {
                  close()
                  retranscribe(.cloud)
                }
                .conversationActionHover(
                  restingColor: palette.mutedText.color, hoverColor: palette.primaryText.color)
                .accessibilityIdentifier("overlay-retranscribe-cloud")
              }
            }
          }
        }
        .buttonStyle(.plain)
        .controlSize(.small)
        .font(CSFont.ui(11, .medium))
      } else {
        Text(intent.accessibilityLabel)
      }
    }
    .accessibilityHint(intent.accessibilityHint)
  }

  static func projectedIntents(for state: OverlayState) -> [OverlayIntent] {
    // The same guard as every other entry point, retranscription included.
    if state.documentOperationPending {
      return []
    }
    if state.archivedTranscript != nil {
      return archivedIntents(for: state)
    }
    if state.isRevisionDraftDirty {
      return recoveryIntents(for: state) + [.commitRevision, .discardRevision, .close]
    }
    return recoveryIntents(for: state)
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
  /// unavailability, and Send to Agent is the same explicit click. Undo and
  /// Redo sit beside the versions control and move the archive's own cursor.
  static func archivedIntents(for state: OverlayState) -> [OverlayIntent] {
    if state.isRevisionDraftDirty {
      return recoveryIntents(for: state) + [.commitRevision, .discardRevision, .close]
    }
    return recoveryIntents(for: state)
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
        + (canFormat ? [.format] : [])
        + (canSendToAgent ? [.sendToAgent] : [])
        + [.close]
    case .noSpeech:
      (canRetranscribe ? [.retranscribe] : []) + [.close]
    }
  }

  static let versionsControlID = "overlay-versions-menu"

  /// Undo and Redo mount only while the versions cursor can move. Their title
  /// still names the step they take.
  private func undoRedoControl(_ intent: OverlayIntent, title: String) -> some View {
    OverlayHoverControl(
      id: "overlay-intent-\(intent.rawValue)", title: title, palette: palette,
      presented: $presented, action: { dispatch(intent) },
      emphasis: .transcriptRail,
      keepsHintInsideWindow: true,
      popoverArrow: .leading
    ) {
      Image(systemName: intent.systemImage).frame(width: 24, height: 24)
    } detail: { _ in
      Text(title)
    }
    .accessibilityHint(intent.accessibilityHint)
  }

  /// Names the shown version, so the control says which one is active.
  static func versionsTitle(_ versions: OverlayTranscriptVersionsPresentation) -> String {
    guard let current = versions.current else {
      return String(
        localized: "overlay.versions.control", defaultValue: "Transcript versions",
        comment: "Overlay control listing the versions of the take on the canvas")
    }
    return String(
      localized: "overlay.versions.control.current",
      defaultValue: "Transcript versions · shown: \(current.title)",
      comment: "Overlay control title; the placeholder names the version on the canvas")
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

  /// One-take format choices. Max stays here whatever automatic level is
  /// selected; Off is not a format.
  static let formatOnceLevels: [FormattingPolicyOption] = [.correction, .smart, .max]

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
    case .undo:
      String(
        localized: "overlay.intent.undo", defaultValue: "Undo", comment: "Overlay Undo control")
    case .redo:
      String(
        localized: "overlay.intent.redo", defaultValue: "Redo", comment: "Overlay Redo control")
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
    case .undo:
      String(
        localized: "overlay.intent.undo.hint",
        defaultValue: "Shows the previous version again without running anything",
        comment: "Overlay Undo accessibility hint")
    case .redo:
      String(
        localized: "overlay.intent.redo.hint",
        defaultValue: "Shows the next version again without running anything",
        comment: "Overlay Redo accessibility hint")
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
    case .undo: OverlayControlSymbols.undo
    case .redo: OverlayControlSymbols.redo
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
