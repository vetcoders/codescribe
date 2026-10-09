import SwiftUI

/// Versions of the take on the canvas. Distinct from Transcription history
/// (other takes) and Previous take (a retained text buffer). A row only asks
/// for a restore; the canvas changes when the reducer's projection arrives.
struct OverlayTranscriptVersionsMenu: View {
  let versions: OverlayTranscriptVersionsPresentation
  let onRestore: (UInt64) -> Void
  let close: () -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      Text(
        String(
          localized: "overlay.versions.title", defaultValue: "Versions of this transcript",
          comment: "Header of the versions list for the take on the overlay canvas"))
      if let reason = versions.blockedReason {
        Text(reason)
          .font(CSFont.ui(11))
          .foregroundStyle(.secondary)
          .fixedSize(horizontal: false, vertical: true)
          .accessibilityIdentifier("overlay-versions-unavailable")
      }
      ForEach(versions.options) { option in
        row(option)
      }
      if versions.currentMissing {
        Text(
          String(
            localized: "overlay.versions.currentMissing",
            defaultValue: "The shown version is not in the history yet.",
            comment: "Versions list: the canvas shows a revision the journal has not returned")
        )
        .font(CSFont.ui(10))
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
      }
      if !versions.options.isEmpty {
        Text(
          String(
            localized: "overlay.versions.footer",
            defaultValue: "Choosing a version adds it as a new one. Nothing is deleted.",
            comment: "Versions list footer: restore is a new revision, history stays")
        )
        .font(CSFont.ui(10))
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
      }
    }
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("overlay-versions-list")
  }

  private func row(_ option: OverlayTranscriptVersion) -> some View {
    let enabled = versions.selectableRevisions.contains(option.revision)
    return VStack(alignment: .leading, spacing: 2) {
      Button {
        close()
        onRestore(option.revision)
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
                  .foregroundStyle(.secondary)
              }
            }
            Text(verbatim: option.text)
              .font(CSFont.ui(10))
              .foregroundStyle(.secondary)
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
          : option.refusal ?? ""
      )
      .accessibilityHint(
        enabled
          ? String(
            localized: "overlay.versions.restoreHint",
            defaultValue: "Shows this version as the newest one",
            comment: "Accessibility hint of a version that can be chosen")
          : ""
      )
      .accessibilityIdentifier("overlay-version-\(option.revision)")
      if let refusal = option.refusal, versions.blockedReason == nil {
        Text(refusal)
          .font(CSFont.ui(10))
          .foregroundStyle(.secondary)
          .fixedSize(horizontal: false, vertical: true)
          .padding(.leading, 15)
          .accessibilityHidden(true)
      }
    }
  }
}
