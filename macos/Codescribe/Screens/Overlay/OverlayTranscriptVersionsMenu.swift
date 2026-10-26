import SwiftUI

/// Versions of the transcript on the canvas. Distinct from Transcription
/// history (other takes) and Previous take (a retained text buffer). A row
/// only asks Rust to select that version; the canvas changes when the
/// projection arrives. Undo and Redo move through the same list.
struct OverlayTranscriptVersionsMenu: View {
  let versions: OverlayTranscriptVersionsPresentation
  let onSelect: (UInt64) -> Void
  let close: () -> Void
  @Environment(\.colorScheme) private var colorScheme

  /// Secondary lines take the overlay's muted rung, as Transcription history
  /// does. An explicit colour does not fade with a disabled row by itself,
  /// so a version that cannot be chosen dims its secondary lines with it.
  private var mutedText: Color { OverlayAppearancePalette.resolve(colorScheme).mutedText.color }

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      Text(
        String(
          localized: "overlay.versions.title", defaultValue: "Versions of this transcript",
          comment: "Header of the versions list for the take on the overlay canvas"))
      if let reason = versions.blockedReason {
        Text(reason)
          .font(CSFont.ui(11))
          .foregroundStyle(mutedText)
          .fixedSize(horizontal: false, vertical: true)
          .accessibilityIdentifier("overlay-versions-unavailable")
      }
      ForEach(versions.options) { option in
        row(option)
      }
      if !versions.options.isEmpty {
        Text(
          String(
            localized: "overlay.versions.footer.linear",
            defaultValue:
              "Choosing a version works like Undo and Redo. A new change after it replaces the versions above it.",
            comment: "Versions list footer: selection moves the cursor, a new change ends redo")
        )
        .font(CSFont.ui(10))
        .foregroundStyle(mutedText)
        .fixedSize(horizontal: false, vertical: true)
      }
    }
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-versions-list")
  }

  private func row(_ option: OverlayTranscriptVersion) -> some View {
    let enabled = versions.selectableSteps.contains(option.step)
    return Button {
      close()
      onSelect(option.step)
    } label: {
      HStack(alignment: .firstTextBaseline, spacing: 6) {
        Image(systemName: option.isCurrent ? "checkmark" : "circle")
          .font(CSFont.ui(9, .semibold))
          .opacity(option.isCurrent ? 1 : 0.35)
          .accessibilityHidden(true)
        VStack(alignment: .leading, spacing: 1) {
          HStack(spacing: 6) {
            Text(option.title)
            if let time = option.timeLabel {
              Text(verbatim: time)
                .font(CSFont.ui(10))
                .foregroundStyle(mutedText.opacity(enabled ? 1 : 0.5))
            }
          }
          Text(verbatim: option.text)
            .font(CSFont.ui(10))
            .foregroundStyle(mutedText.opacity(enabled ? 1 : 0.5))
            .lineLimit(2)
            .multilineTextAlignment(.leading)
        }
      }
      .contentShape(Rectangle())
    }
    .buttonStyle(.borderless)
    .disabled(!enabled)
    .accessibilityLabel(option.title)
    .accessibilityValue(
      option.isCurrent
        ? String(
          localized: "overlay.versions.current", defaultValue: "Current version",
          comment: "Accessibility value of the version the canvas shows")
        : ""
    )
    .accessibilityHint(
      enabled
        ? String(
          localized: "overlay.versions.selectHint",
          defaultValue: "Shows this version without running anything again",
          comment: "Accessibility hint of a version that can be chosen")
        : ""
    )
    .accessibilityIdentifier("overlay-version-\(option.step)")
  }
}
