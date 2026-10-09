import AppKit
import SwiftUI

struct OverlayTranscriptHistory: View {
  @Environment(\.colorScheme) private var colorScheme
  @Environment(\.locale) private var locale
  @State private var model = OverlayTranscriptHistoryModel()
  /// Why the canvas cannot take an archive right now (live take, unsaved
  /// edit, revision in flight). Nil when opening is allowed.
  var openRefusal: String?
  /// Hands the archive to the overlay canvas; false when the canvas refused.
  var onOpen: (OverlayArchivedTranscript) -> Bool = { _ in false }
  /// Dismisses the history popover after a successful open.
  var onOpened: () -> Void = {}

  var body: some View {
    VStack(alignment: .leading, spacing: 10) {
      HStack {
        Text("Transcription history").font(.headline)
        Spacer()
        Button("Refresh", systemImage: "arrow.clockwise") {
          Task { await model.load() }
        }
        .labelStyle(.iconOnly)
        .disabled(model.loading)
      }
      if let reason = openRefusal ?? model.error {
        Text(reason)
          .font(.caption)
          .foregroundStyle(.secondary)
          .fixedSize(horizontal: false, vertical: true)
          .accessibilityIdentifier("overlay-history-notice")
      }
      if model.loading && model.entries.isEmpty {
        ProgressView().frame(maxWidth: .infinity)
      } else if model.entries.isEmpty {
        Text("No saved transcriptions yet.").foregroundStyle(.secondary)
      } else {
        historyList(model.entries)
      }
    }
    .frame(width: 260)
    .task { await model.load() }
    .accessibilityIdentifier("overlay-transcription-history")
  }

  private func open(_ entry: CsHistoryEntry) {
    Task {
      guard let archived = await model.open(entry) else { return }
      if onOpen(archived) {
        onOpened()
      } else {
        model.refuseOpen(
          openRefusal
            ?? String(
              localized: "The overlay is busy. Try again when the current take is done.",
              comment: "History entry could not be placed on the overlay canvas"))
      }
    }
  }

  func historyList(_ entries: [TranscriptHistoryRecord]) -> some View {
    ScrollView {
      LazyVStack(alignment: .leading, spacing: 4) {
        ForEach(entries, id: \.path) { entry in
          Button {
            open(entry.entry)
          } label: {
            VStack(alignment: .leading, spacing: 4) {
              HStack(spacing: 4) {
                if model.opening == entry.path {
                  ProgressView().controlSize(.mini)
                }
                Text(date(entry.entry), format: .dateTime.month(.abbreviated).day().hour().minute())
                  .foregroundStyle(.secondary)
                Text(
                  verbatim: "· \(Self.characterCountLabel(entry.characterCount, locale: locale))"
                )
                .foregroundStyle(OverlayAppearancePalette.resolve(colorScheme).mutedText.color)
              }
              .font(.caption)
              Text(entry.entry.preview.isEmpty ? Self.untitledLabel : entry.entry.preview)
                .lineLimit(2)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(8)
            .contentShape(Rectangle())
          }
          .buttonStyle(.plain)
          .disabled(openRefusal != nil)
          .accessibilityHint(
            Text("Opens this transcript on the overlay to edit, format, transcribe again or insert"))
          .accessibilityLabel(
            "\(date(entry.entry).formatted(.dateTime.month(.abbreviated).day().hour().minute().locale(locale))), "
              + "\(Self.characterCountLabel(entry.characterCount, locale: locale)), "
              + (entry.entry.preview.isEmpty ? Self.untitledLabel : entry.entry.preview))
          Divider()
        }
      }
    }
    .frame(height: 280)
  }

  static func characterCountLabel(_ count: Int?, locale: Locale) -> String {
    OverlayTranscriptHistoryModel.formattedCharacterCount(count, locale: locale)
  }

  /// Stand-in for an archived take whose preview is empty.
  static var untitledLabel: String {
    String(localized: "Untitled transcript", comment: "Archived take with no preview text")
  }

  private func date(_ entry: CsHistoryEntry) -> Date {
    Date(timeIntervalSince1970: TimeInterval(entry.timestampMs) / 1000)
  }
}
