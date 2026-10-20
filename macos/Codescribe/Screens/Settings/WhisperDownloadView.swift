import SwiftUI

/// The same opt-in control observes the same retained store in both surfaces.
/// Model selection and removal remain in Settings, outside this shared control.
struct WhisperDownloadView: View {
  /// `.full` lists the source, size and location under the name (the setup
  /// model step). `.summary` keeps the name and state up front and moves that
  /// technical block under a collapsed Details disclosure (the setup summary).
  /// Both keep the download, progress and error controls visible.
  enum Presentation {
    case full
    case summary
  }

  @ObservedObject var store: WhisperDownloadStore
  var downloadEnabled = true
  var presentation: Presentation = .full
  @Environment(\.locale) private var locale
  /// A multi-gigabyte transfer starts on a second, deliberate press.
  @State private var confirmingDownload = false
  @State private var detailsExpanded = false

  var body: some View {
    VStack(alignment: .leading, spacing: 10) {
      // The state belongs to the model named beside it, so it sits next to the
      // heading instead of drifting to the far edge of whatever hosts this control.
      HStack(spacing: 10) {
        Text(
          String(
            localized: LocalizedStringResource(
              "Whisper model", locale: locale, comment: "Shared Whisper download control heading"))
        )
        .font(SettingsScale.settingName)
        Text(installLabel)
          .font(.caption.weight(.medium))
          .foregroundStyle(store.status.available ? CSColor.oliveLight : Color.secondary)
        Spacer(minLength: 0)
      }
      Text(store.status.modelId).font(SettingsScale.helper)
      switch presentation {
      case .full:
        technicalDetails
      case .summary:
        DisclosureGroup(isExpanded: $detailsExpanded) {
          technicalDetails
            .padding(.top, 6)
        } label: {
          Text(
            String(
              localized: LocalizedStringResource(
                "Details", locale: locale,
                comment: "Setup summary: disclosure with the Whisper model's source, size and path"))
          )
          .font(SettingsScale.helper)
          .foregroundStyle(.secondary)
        }
        .accessibilityIdentifier("whisper-download-details")
      }

      if store.inFlight {
        if let fraction = store.fraction {
          ProgressView(value: fraction, total: 1)
            .progressViewStyle(.linear)
            .controlSize(.large)
            .tint(CSColor.chromeAccent)
            .frame(maxWidth: .infinity)
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
        .font(SettingsScale.helper)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
        Button(cancelDownloadTitle, action: store.cancel)
          .csAction()
          .disabled(store.cancelling)
      } else if !store.status.available {
        if let error = store.error {
          Text(error).font(SettingsScale.helper).foregroundStyle(CSColor.terracotta)
            .fixedSize(horizontal: false, vertical: true)
        } else if store.interruption == .cancelled {
          Text(
            String(
              localized: LocalizedStringResource(
                "Download stopped. The model is not installed.", locale: locale,
                comment: "The user cancelled the Whisper download; the partial file is removed"))
          )
          .font(SettingsScale.helper)
          .foregroundStyle(.secondary)
          .fixedSize(horizontal: false, vertical: true)
          .accessibilityIdentifier("whisper-download-stopped")
        }
        Button(downloadTitle) { confirmingDownload = true }
          .csAction(prominent: true)
          .disabled(!downloadEnabled)
      }
    }
    .accessibilityIdentifier("whisper-download")
    .alert(confirmTitle, isPresented: $confirmingDownload) {
      Button(confirmActionTitle, action: store.start)
      Button(confirmCancelTitle, role: .cancel) {}
    } message: {
      Text(confirmMessage)
    }
    .onAppear {
      if downloadEnabled { store.refresh() }
    }
  }

  /// Source, size and on-disk location of the model.
  private var technicalDetails: some View {
    VStack(alignment: .leading, spacing: 10) {
      Text(
        String(
          localized: LocalizedStringResource(
            "Hugging Face · \(store.status.repo) · \(store.status.sizeHint)", locale: locale,
            comment:
              "Download source: first placeholder is a repository id, second is approximate disk size"
          ))
      )
      .font(SettingsScale.helper)
      .foregroundStyle(.secondary)
      .fixedSize(horizontal: false, vertical: true)
      if let path = store.status.path {
        Text(path).font(.caption).textSelection(.enabled)
      } else if !store.status.embedded {
        Text(
          String(
            localized: LocalizedStringResource(
              "Default location: \(store.status.modelsDir)", locale: locale,
              comment: "Default model storage folder; the placeholder is a directory path"))
        )
        .font(.caption)
        .foregroundStyle(.secondary)
      }
    }
  }

  private var downloadTitle: String {
    store.error == nil
      ? String(
        localized: LocalizedStringResource(
          "Download model (\(store.status.sizeHint))", locale: locale,
          comment: "Opt in to download Whisper; the placeholder is the approximate size"))
      : String(
        localized: LocalizedStringResource(
          "Try again", locale: locale, comment: "Retry the failed Whisper model download"))
  }

  private var cancelDownloadTitle: String {
    String(
      localized: LocalizedStringResource(
        "Cancel download", locale: locale, comment: "Stop the running Whisper model transfer"))
  }

  private var confirmTitle: String {
    String(
      localized: LocalizedStringResource(
        "Download the Whisper model?", locale: locale,
        comment: "Confirmation before a multi-gigabyte transfer starts"))
  }

  private var confirmMessage: String {
    String(
      localized: LocalizedStringResource(
        "\(store.status.sizeHint) will be saved in \(store.status.modelsDir). The download finishes in the background.",
        locale: locale,
        comment:
          "Download confirmation: first placeholder is the approximate size, second is a directory path"
      ))
  }

  /// Both verbs already exist in the catalog without a translator comment;
  /// adding one here would only rewrite a shared row.
  private var confirmActionTitle: String {
    String(localized: LocalizedStringResource("Download", locale: locale))
  }

  private var confirmCancelTitle: String {
    String(localized: LocalizedStringResource("Cancel", locale: locale))
  }

  private var installLabel: String {
    if store.inFlight {
      guard let fraction = store.fraction else {
        return String(
          localized: LocalizedStringResource(
            "Downloading…", locale: locale,
            comment: "Whisper download progress with no file detail yet"))
      }
      let percent = fraction.formatted(.percent.precision(.fractionLength(0)).locale(locale))
      return String(
        localized: LocalizedStringResource(
          "Downloading… \(percent)", locale: locale,
          comment: "Model row status while the transfer runs; the placeholder is a percentage"))
    }
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
