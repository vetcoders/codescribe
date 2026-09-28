import AppKit
import SwiftUI

struct OverlayTranscriptHistory: View {
  @State private var model = OverlayTranscriptHistoryModel()
  @State private var copied = false

  var body: some View {
    VStack(alignment: .leading, spacing: 10) {
      if let selected = model.selected {
        HStack {
          Button("Back", systemImage: "chevron.left") { model.back() }
          Spacer()
          Text(date(selected), format: .dateTime.month(.abbreviated).day().hour().minute())
            .foregroundStyle(.secondary)
        }
        if model.reading {
          ProgressView().frame(maxWidth: .infinity)
        } else if let error = model.error {
          Text(error)
        } else if let text = model.text {
          ScrollView {
            Text(text)
              .textSelection(.enabled)
              .frame(maxWidth: .infinity, alignment: .leading)
          }
          .frame(height: 220)
          HStack {
            Button(copied ? "Copied" : "Copy transcript", systemImage: "doc.on.doc") {
              NSPasteboard.general.clearContents()
              copied = NSPasteboard.general.setString(text, forType: .string)
            }
            Spacer()
            Button("Show in Finder", systemImage: "folder") {
              NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: selected.path)])
            }
            .labelStyle(.iconOnly)
          }
        }
      } else {
        HStack {
          Text("Transcription history").font(.headline)
          Spacer()
          Button("Refresh", systemImage: "arrow.clockwise") {
            Task { await model.load() }
          }
          .labelStyle(.iconOnly)
          .disabled(model.loading)
        }
        if model.loading && model.entries.isEmpty {
          ProgressView().frame(maxWidth: .infinity)
        } else if model.entries.isEmpty {
          Text("No saved transcriptions yet.").foregroundStyle(.secondary)
        } else {
          ScrollView {
            LazyVStack(alignment: .leading, spacing: 4) {
              ForEach(model.entries, id: \.path) { entry in
                Button {
                  copied = false
                  Task { await model.select(entry) }
                } label: {
                  VStack(alignment: .leading, spacing: 4) {
                    Text(date(entry), format: .dateTime.month(.abbreviated).day().hour().minute())
                      .font(.caption)
                      .foregroundStyle(.secondary)
                    Text(entry.preview.isEmpty ? "Untitled transcript" : entry.preview)
                      .lineLimit(2)
                      .frame(maxWidth: .infinity, alignment: .leading)
                  }
                  .padding(8)
                  .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                Divider()
              }
            }
          }
          .frame(height: 280)
        }
      }
    }
    .frame(width: 260)
    .task { await model.load() }
    .accessibilityIdentifier("overlay-transcription-history")
  }

  private func date(_ entry: CsHistoryEntry) -> Date {
    Date(timeIntervalSince1970: TimeInterval(entry.timestampMs) / 1000)
  }
}
