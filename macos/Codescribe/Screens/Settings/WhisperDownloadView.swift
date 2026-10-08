import SwiftUI

/// The same opt-in control observes the same retained store in both surfaces.
/// Model selection and removal remain in Settings, outside this shared control.
struct WhisperDownloadView: View {
  @ObservedObject var store: WhisperDownloadStore
  var downloadEnabled = true
  @Environment(\.locale) private var locale

  var body: some View {
    VStack(alignment: .leading, spacing: 10) {
      HStack(spacing: 12) {
        Text(
          String(
            localized: LocalizedStringResource(
              "Whisper model", locale: locale, comment: "Shared Whisper download control heading"))
        )
        .font(.body.weight(.semibold))
        Spacer(minLength: 0)
        Text(installLabel)
          .font(.caption.weight(.medium))
          .foregroundStyle(store.status.available ? CSColor.oliveLight : Color.secondary)
      }
      Text(store.status.modelId).font(.callout)
      Text(
        String(
          localized: LocalizedStringResource(
            "Hugging Face · \(store.status.repo) · \(store.status.sizeHint)", locale: locale,
            comment:
              "Download source: first placeholder is a repository id, second is approximate disk size"
          ))
      )
      .font(.callout)
      .foregroundStyle(.secondary)
      .fixedSize(horizontal: false, vertical: true)
      if let path = store.status.path {
        Text(path).font(.caption).textSelection(.enabled)
      } else if !store.status.embedded {
        Text(
          String(
            localized: LocalizedStringResource(
              "Default location: ~/.codescribe/models", locale: locale,
              comment: "Default model storage folder; keep ~/.codescribe/models verbatim"))
        )
        .font(.caption)
        .foregroundStyle(.secondary)
      }

      if store.inFlight {
        if let fraction = store.fraction {
          ProgressView(value: fraction).progressViewStyle(.linear).tint(CSColor.chromeAccent)
        } else {
          ProgressView().controlSize(.small)
        }
        Text(
          store.detail
            ?? String(
              localized: LocalizedStringResource(
                "Downloading…", locale: locale,
                comment: "Whisper download progress with no file detail yet"))
        )
        .font(.callout)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
      } else if !store.status.available {
        if let error = store.error {
          Text(error).font(.callout).foregroundStyle(CSColor.terracotta)
            .fixedSize(horizontal: false, vertical: true)
        }
        Button(downloadTitle, action: store.start)
          .csAction(prominent: true)
          .disabled(!downloadEnabled)
      } else {
        Button(
          String(
            localized: LocalizedStringResource(
              "Recheck", locale: locale,
              comment: "Recheck local Whisper availability without downloading")),
          action: store.refresh
        )
        .csAction()
        .disabled(!downloadEnabled)
      }
    }
    .accessibilityIdentifier("whisper-download")
    .onAppear {
      if downloadEnabled { store.refresh() }
    }
  }

  private var downloadTitle: String {
    store.error == nil
      ? String(
        localized: LocalizedStringResource(
          "Download", locale: locale, comment: "Opt in to download Whisper from Hugging Face"))
      : String(
        localized: LocalizedStringResource(
          "Try again", locale: locale, comment: "Retry the failed Whisper model download"))
  }

  private var installLabel: String {
    if store.status.embedded {
      return String(
        localized: LocalizedStringResource(
          "Embedded", locale: locale, comment: "Whisper is included in this app build"))
    }
    if store.status.available {
      return String(
        localized: LocalizedStringResource(
          "Installed", locale: locale, comment: "Whisper weights are available locally"))
    }
    return String(
      localized: LocalizedStringResource(
        "Not installed", locale: locale, comment: "Whisper weights are not yet available locally"))
  }
}
