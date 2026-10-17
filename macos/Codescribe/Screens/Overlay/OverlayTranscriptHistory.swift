import AppKit
import SwiftUI

struct OverlayTranscriptHistory: View {
  @Environment(\.colorScheme) private var colorScheme
  @Environment(\.locale) private var locale
  @State private var model = OverlayTranscriptHistoryModel()
  @State private var openTask: Task<Void, Never>?
  /// Why the canvas cannot take an archive right now (live take, unsaved
  /// edit, revision in flight). Nil when opening is allowed.
  var openRefusal: String?
  /// Issues the canvas admission ticket for one open request. The canvas
  /// owns the counter, so a ticket from a dismissed list stays stale even
  /// when a new list instance is showing.
  var admitOpen: () -> UInt64 = { 0 }
  /// Hands the archive to the overlay canvas under its ticket.
  var onOpen: (OverlayArchivedTranscript, UInt64) -> OverlayArchiveOpenOutcome = { _, _ in
    .superseded
  }
  /// The list went away: no request it started may land afterwards.
  var onDismiss: () -> Void = {}
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
    .onDisappear {
      openTask?.cancel()
      openTask = nil
      model.cancelOpen()
      onDismiss()
    }
    .accessibilityIdentifier("overlay-transcription-history")
  }

  private func open(_ entry: CsHistoryEntry) {
    openTask?.cancel()
    let admission = admitOpen()
    openTask = Task {
      guard let archived = await model.open(entry), !Task.isCancelled else { return }
      switch onOpen(archived, admission) {
      case .opened:
        onOpened()
      case .refused(let reason):
        model.refuseOpen(reason)
      case .superseded:
        // A newer request, a new take or a dismissal took over. Nothing here
        // may repaint the canvas or reopen the list.
        break
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
