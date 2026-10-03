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
        title: String(localized: "Install state"),
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
          Text(model.whisperDownloadDetail ?? String(localized: "Downloading…"))
            .font(CSFont.mono(10.5, .medium))
            .foregroundStyle(Color.secondary)
            .lineLimit(2)
        }
      } else if !status.available {
        SettingsControlRow(
          title: String(localized: "Download Whisper"),
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
          title: String(localized: "Local path"),
          subtitle: status.path ?? status.modelId
        ) {
          Button("Recheck", action: model.refreshWhisperModelStatus)
            .buttonStyle(.bordered)
            .controlSize(.small)
        }
      } else {
        Text("This build embeds Whisper (fat SKU). Runtime download is not required.")
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(Color.secondary)
      }

      SettingsSectionLabel(String(localized: "Model"))
      if let catalog = model.whisperModelCatalog {
        if catalog.overrideKind == "embedded" {
          Text("This build embeds Whisper — the on-disk model selection is ignored.")
            .font(CSFont.mono(10.5, .medium))
            .foregroundStyle(Color.secondary)
        } else {
          SettingsControlRow(
            title: String(localized: "Whisper model"),
            subtitle: selectedModelPath(catalog)
          ) {
            Picker(
              selection: Binding(
                get: { model.whisperModelSelection },
                set: { model.selectWhisperModel($0) }
              )
            ) {
              ForEach(catalog.options.filter(\.usable), id: \.id) { option in
                Text(option.label).tag(option.id)
              }
            } label: {
              EmptyView()
            }
            .labelsHidden()
            .pickerStyle(.menu)
            .disabled(model.whisperModelSwitchPending)
          }

          SettingsControlRow(
            title: String(localized: "In use"),
            subtitle: catalog.resolvedPath
              ?? String(localized: "unresolved", comment: "Whisper model row: resolution failed")
          ) {
            Text(inUseLabel(catalog))
              .font(CSFont.mono(11, .medium))
              .foregroundStyle(catalog.loaded != nil ? CSColor.oliveLight : Color.secondary)
          }

          if let overrideNote = selectionOverrideNote(catalog.overrideKind) {
            Text(overrideNote)
              .font(CSFont.mono(10.5, .medium))
              .foregroundStyle(CSColor.amber)
          }
          if let notice = model.whisperModelNotice {
            Text(notice)
              .font(CSFont.mono(10.5, .medium))
              .foregroundStyle(Color.secondary)
          }
          if let error = model.whisperModelError {
            Text(error)
              .font(CSFont.mono(10.5, .medium))
              .foregroundStyle(CSColor.amber)
          }

          let unavailable = catalog.options.filter { !$0.usable }
          if !unavailable.isEmpty {
            ForEach(unavailable, id: \.id) { option in
              SettingsControlRow(
                title: option.label,
                subtitle: option.reason
                  ?? String(localized: "Unavailable", comment: "Whisper model row: no detail")
              ) {
                EmptyView()
              }
            }
          }
        }
      }

      SettingsSectionLabel(String(localized: "Data footprint"))
      ForEach(storedModels, id: \.name) { directory in
        SettingsControlRow(
          title: directory.name,
          subtitle: modelFootprint(directory)
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
      model.refreshWhisperModelCatalog()
      refreshStoredModels()
    }
    .onChange(of: model.whisperModelCatalog?.configured) { _, _ in
      // A selection change moves the removal lock between footprint rows.
      refreshStoredModels()
    }
  }

  /// Picker subtitle: where the saved selection loads from.
  private func selectedModelPath(_ catalog: CsWhisperModelCatalog) -> String {
    catalog.options.first { $0.id == catalog.configured }?.path ?? catalog.configured
  }

  /// Runtime truth for the "In use" row: the resident weights, or what the
  /// next take will load.
  private func inUseLabel(_ catalog: CsWhisperModelCatalog) -> String {
    if let loaded = catalog.loaded {
      return loaded
    }
    return String(
      localized: "loads on next take",
      comment: "Whisper model row: no weights resident; the next take loads the selected model"
    )
  }

  /// A shadowing authority above the picker (env override), or nil. The
  /// override is shown, never silently removed.
  private func selectionOverrideNote(_ overrideKind: String?) -> String? {
    switch overrideKind {
    case "env_model_path":
      return String(
        localized: "CODESCRIBE_MODEL_PATH overrides the selected model.",
        comment: "Whisper model picker: an environment variable shadows the saved selection"
      )
    case "env_local_model":
      return String(
        localized: "The LOCAL_MODEL environment variable overrides the selected model.",
        comment: "Whisper model picker: an environment variable shadows the saved selection"
      )
    default:
      return nil
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

  /// Storage row subtitle: state, size on disk, and a model sharing the same
  /// tokenizer. The state and the size are machine values, so only the
  /// tokenizer note is copy.
  private func modelFootprint(_ directory: CsModelDirectory) -> String {
    let size = ByteCountFormatter.string(
      fromByteCount: Int64(directory.bytesOnDisk), countStyle: .file)
    guard let duplicate = directory.duplicateTokenizerWith else {
      return "\(directory.status) · \(size)"
    }
    return String(
      localized: "\(directory.status) · \(size) · identical tokenizer: \(duplicate)",
      comment: "Placeholders: storage state, size on disk, name of the twin model"
    )
  }

  private func whisperInstallLabel(_ status: CsWhisperModelStatus) -> String {
    if status.embedded {
      String(localized: "Embedded")
    } else if status.available {
      String(localized: "Installed")
    } else {
      String(localized: "Not installed")
    }
  }

  private func whisperInstallSubtitle(_ status: CsWhisperModelStatus) -> String {
    if status.embedded {
      String(localized: "Baked into this fat build · \(status.modelId)")
    } else if status.available {
      String(localized: "Ready for Whisper engine · \(status.modelId)")
    } else if model.asrModeId == "local_power" {
      String(localized: "Missing or invalid FP16 bundle · Local power is not ready")
    } else {
      String(localized: "Required by the direct Whisper engine or Local power live refinement")
    }
  }

  private func whisperDownloadSubtitle(_ status: CsWhisperModelStatus) -> String {
    if model.asrModeId == "local_power" {
      String(
        localized:
          "Required local FP16 model (\(status.sizeHint)). Missing or invalid weights keep Local power not ready."
      )
    } else {
      String(
        localized:
          "Local FP16 model (\(status.sizeHint)) for direct Whisper or Local power refinement."
      )
    }
  }
}
