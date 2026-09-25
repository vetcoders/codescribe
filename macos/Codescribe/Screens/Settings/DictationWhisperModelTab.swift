import SwiftUI

/// Dictation › Whisper model: install state and the opt-in download (the public
/// DMG is slim — the model is not bundled).
struct DictationWhisperModelTab: View {
  @ObservedObject var model: SettingsViewModel
  @State private var storedModels: [CsModelDirectory] = []
  @State private var storageError: String?

  var body: some View {
    let status = model.localWhisperStatus
    VStack(alignment: .leading, spacing: 10) {
      SettingsControlRow(
        title: "Install state",
        subtitle: whisperInstallSubtitle(status)
      ) {
        Text(whisperInstallLabel(status))
          .font(CSFont.mono(11, .medium))
          .foregroundStyle(status.available ? CSColor.oliveLight : CSColor.amber)
      }

      if model.whisperDownloadInFlight {
        VStack(alignment: .leading, spacing: 6) {
          if let fraction = model.whisperDownloadFraction {
            ProgressView(value: fraction)
              .progressViewStyle(.linear)
              .tint(CSColor.chromeAccent)
          } else {
            ProgressView()
              .controlSize(.small)
          }
          Text(model.whisperDownloadDetail ?? "Downloading…")
            .font(CSFont.mono(10.5, .medium))
            .foregroundStyle(CSColor.textFaint)
            .lineLimit(2)
        }
      } else if !status.available {
        SettingsControlRow(
          title: "Download Whisper",
          subtitle: whisperDownloadSubtitle(status)
        ) {
          Button("Download", action: model.startWhisperDownload)
            .buttonStyle(.borderedProminent)
            .controlSize(.small)
            .tint(CSColor.chromeAccent)
        }
      } else if !status.embedded {
        // On-disk / cache — offer re-check, not re-download spam.
        SettingsControlRow(
          title: "Local path",
          subtitle: status.path ?? status.modelId
        ) {
          Button("Recheck", action: model.refreshWhisperModelStatus)
            .buttonStyle(.bordered)
            .controlSize(.small)
        }
      } else {
        Text("This build embeds Whisper (fat SKU). Runtime download is not required.")
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(CSColor.textFaint)
      }

      SettingsSectionLabel("Data footprint")
      ForEach(storedModels, id: \.name) { directory in
        SettingsControlRow(
          title: directory.name,
          subtitle: "\(directory.status) · \(ByteCountFormatter.string(fromByteCount: Int64(directory.bytesOnDisk), countStyle: .file))\(directory.duplicateTokenizerWith.map { " · identical tokenizer: \($0)" } ?? "")"
        ) {
          Button("Remove") {
            do {
              try removeModelDirectory(name: directory.name)
              refreshStoredModels()
            } catch {
              storageError = error.localizedDescription
            }
          }
          .disabled(!Self.canRemoveModel(status: directory.status))
          .buttonStyle(.bordered)
        }
      }
      if let storageError {
        Text(storageError)
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(CSColor.amber)
      }
    }
    .onAppear {
      refreshStoredModels()
    }
  }

  private func refreshStoredModels() {
    do {
      storedModels = try modelDirectories()
      storageError = nil
    } catch {
      storageError = error.localizedDescription
    }
  }

  static func canRemoveModel(status: String) -> Bool {
    status != "active"
  }

  private func whisperInstallLabel(_ status: CsWhisperModelStatus) -> String {
    if status.embedded {
      "Embedded"
    } else if status.available {
      "Installed"
    } else {
      "Not installed"
    }
  }

  private func whisperInstallSubtitle(_ status: CsWhisperModelStatus) -> String {
    if status.embedded {
      "Baked into this fat build · \(status.modelId)"
    } else if status.available {
      "Ready for Whisper engine · \(status.modelId)"
    } else if model.asrModeId == "local_power" {
      "Missing or invalid FP16 bundle · Local power is not ready"
    } else {
      "Required by the direct Whisper engine or Local power live refinement"
    }
  }

  private func whisperDownloadSubtitle(_ status: CsWhisperModelStatus) -> String {
    if model.asrModeId == "local_power" {
      "Required local FP16 model (\(status.sizeHint)). Missing or invalid weights keep Local power not ready."
    } else {
      "Local FP16 model (\(status.sizeHint)) for direct Whisper or Local power refinement."
    }
  }
}
