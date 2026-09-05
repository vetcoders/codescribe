import SwiftUI

/// The overlay's sole action surface. Visibility is derived from the latest
/// reducer projection; the only state owned here is whether the rail is open.
@MainActor
struct OverlayIntentRail: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @State private var isExpanded = false

  let phase: String
  let intents: [OverlayIntent]
  let palette: OverlayAppearancePalette
  let onIntent: (OverlayIntent) -> Void

  var body: some View {
    VStack(spacing: CSSpace.xs) {
      Button(
        isExpanded ? "Hide overlay actions" : "Show overlay actions",
        systemImage: isExpanded ? "chevron.right" : "chevron.left",
        action: toggleExpanded
      )
      .labelStyle(.iconOnly)
      .frame(width: CSSpace.xl, height: CSSpace.xl)
      .contentShape(Rectangle())
      .accessibilityHint(isExpanded ? "Collapses the action rail" : "Expands the action rail")
      .accessibilityIdentifier("overlay-intent-rail-toggle")

      if isExpanded {
        ForEach(intents, id: \.self) { intent in
          Button(intent.accessibilityLabel, systemImage: intent.systemImage) {
            dispatch(intent)
          }
          .labelStyle(.iconOnly)
          .frame(width: CSSpace.xl, height: CSSpace.xl)
          .contentShape(Rectangle())
          .accessibilityHint(intent.accessibilityHint)
          .accessibilityIdentifier("overlay-intent-\(intent.rawValue)")
          .transition(reduceMotion ? .identity : .move(edge: .trailing).combined(with: .opacity))
        }
      }
    }
    .buttonStyle(.plain)
    .foregroundStyle(palette.primaryText.color)
    .padding(CSSpace.xs)
    .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: CSRadius.input))
    .overlay {
      RoundedRectangle(cornerRadius: CSRadius.input)
        .strokeBorder(palette.border.color, lineWidth: 1)
        .allowsHitTesting(false)
    }
    .animation(Self.revealAnimation(reduceMotion: reduceMotion), value: isExpanded)
    .onHover(perform: setExpanded)
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Overlay actions")
    .accessibilityValue(Self.accessibilityValue(for: phase))
    .accessibilityIdentifier("overlay-intent-rail")
  }

  static func projectedIntents(for state: OverlayState) -> [OverlayIntent] {
    projectedIntents(
      phase: state.mode,
      canPaste: state.canPaste,
      canInsert: state.canInsert,
      canCopy: state.canCopy,
      canRetranscribe: state.canRetranscribe,
      canFormat: state.canFormat
    )
  }

  /// Frozen `overlay-canvas-v1` projection table. A false bit omits its
  /// command; the rail never reconstructs delivery legality from local state.
  static func projectedIntents(
    phase: OverlayMode,
    canPaste: Bool,
    canInsert: Bool,
    canCopy: Bool,
    canRetranscribe: Bool,
    canFormat: Bool
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
        + [.close]
    case .noSpeech:
      (canRetranscribe ? [.retranscribe] : []) + [.close]
    case .error:
      [.close]
    }
  }

  static func revealAnimation(reduceMotion: Bool) -> Animation? {
    reduceMotion ? nil : CSMotion.floatIn
  }

  static func accessibilityValue(for phase: String) -> String {
    phase
  }

  func dispatch(_ intent: OverlayIntent) {
    onIntent(intent)
  }

  private func toggleExpanded() {
    isExpanded.toggle()
  }

  private func setExpanded(_ expanded: Bool) {
    isExpanded = expanded
  }
}

extension OverlayIntent {
  var accessibilityLabel: String {
    switch self {
    case .finish: "Finish recording"
    case .copy: "Copy transcript"
    case .insertPaste: "Insert transcript"
    case .retranscribe: "Retranscribe recording"
    case .format: "Format transcript"
    case .close: "Close overlay"
    }
  }

  var accessibilityHint: String {
    switch self {
    case .finish: "Stops capture and requests the final projection"
    case .copy: "Copies the projected transcript"
    case .insertPaste: "Sends the projected transcript to the selected destination"
    case .retranscribe: "Requests another transcription of this recording"
    case .format: "Requests formatting between takes"
    case .close: "Closes the dictation overlay"
    }
  }

  var systemImage: String {
    switch self {
    case .finish: "stop.circle"
    case .copy: "doc.on.doc"
    case .insertPaste: "arrow.down.doc"
    case .retranscribe: "arrow.clockwise"
    case .format: "textformat"
    case .close: "xmark"
    }
  }
}
