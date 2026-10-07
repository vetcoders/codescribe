import SwiftUI

/// Dictation › Whisper model: install state and the opt-in download (the public
/// DMG is slim — the model is not bundled).
struct DictationWhisperModelTab: View {
  @ObservedObject var model: SettingsViewModel
  @State private var storedModels: [CsModelDirectory] = []
  @State private var storageError: String?

  var body: some View {
    VStack(alignment: .leading, spacing: 10) {
      WhisperDownloadView(
        store: model.whisperDownloadStore, downloadEnabled: model.whisperDownloadEnabled)

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
    .onChange(of: model.whisperDownloadInFlight) { _, inFlight in
      if !inFlight {
        model.refreshWhisperModelCatalog()
        refreshStoredModels()
      }
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
}

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
