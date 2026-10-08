import SwiftUI

/// Dictation › Whisper model, in three sections: the selected model (picker,
/// install state, what is resident), other detected models (why they are not
/// selectable), and disk space (installed directories and removal). Paths,
/// sources and raw validation errors live in one collapsed "Model details"
/// disclosure so the same path is not repeated on every row.
struct DictationWhisperModelTab: View {
  @ObservedObject var model: SettingsViewModel
  @State private var storedModels: [CsModelDirectory] = []
  @State private var storageError: String?
  @State private var showingModelDetails = false

  var body: some View {
    let status = model.localWhisperStatus
    let catalog = model.whisperModelCatalog
    let embedded = status.embedded || catalog?.overrideKind == "embedded"
    VStack(alignment: .leading, spacing: 10) {
      SettingsSectionLabel(
        String(
          localized: "Selected model", comment: "Whisper tab section: the picker and its state"))
      if embedded {
        Text("This build embeds Whisper. The on-disk model selection is ignored.")
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(Color.secondary)
      } else {
        selectedModelRows(status: status, catalog: catalog)
      }

      if model.whisperDownloadInFlight {
        downloadProgress
      } else if !status.available && !embedded {
        if let error = model.whisperDownloadStore.error {
          Text(error)
            .font(CSFont.mono(10.5, .medium))
            .foregroundStyle(CSColor.terracotta)
            .fixedSize(horizontal: false, vertical: true)
        }
        SettingsControlRow(
          title: String(localized: "Download Whisper"),
          subtitle: whisperDownloadSubtitle(status)
        ) {
          Button("Download", action: model.whisperDownloadStore.start)
            .buttonStyle(.borderedProminent)
            .controlSize(.small)
            .tint(CSColor.chromeAccent)
            .disabled(!model.whisperDownloadEnabled)
        }
      }

      if let catalog, !embedded {
        let unavailable = catalog.options.filter { !$0.usable }
        if !unavailable.isEmpty {
          SettingsSectionLabel(
            String(
              localized: "Other detected models",
              comment: "Whisper tab section: models found on disk that cannot be selected"))
          ForEach(unavailable, id: \.id) { option in
            SettingsControlRow(title: option.label, subtitle: Self.reasonSummary(option.reason)) {
              EmptyView()
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

      DisclosureGroup(isExpanded: $showingModelDetails) {
        modelDetails(status: status, catalog: catalog)
          .padding(.top, 6)
      } label: {
        Text("Model details", comment: "Whisper tab disclosure: paths, sources and raw errors")
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(Color.primary)
      }
      .padding(.top, CSSpace.xs)
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

  // MARK: - Selected model

  @ViewBuilder
  private func selectedModelRows(status: CsWhisperModelStatus, catalog: CsWhisperModelCatalog?)
    -> some View
  {
    if let catalog {
      SettingsControlRow(
        title: String(localized: "Whisper model"),
        subtitle: Self.selectedOptionLabel(catalog)
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
    }

    SettingsControlRow(
      title: String(
        localized: "Install state", comment: "Whisper tab row: are the weights on disk"),
      subtitle: Self.residencyLabel(catalog)
    ) {
      HStack(spacing: 8) {
        Text(
          status.available
            ? String(localized: "Installed") : String(localized: "Not installed")
        )
        .font(CSFont.mono(11, .medium))
        .foregroundStyle(status.available ? CSColor.oliveLight : CSColor.amber)
        Button(action: checkModel) {
          Text(
            "Check model", comment: "Whisper tab button: re-read install state, catalog and disk")
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
      }
    }

    if let overrideNote = selectionOverrideNote(catalog?.overrideKind) {
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
  }

  private var downloadProgress: some View {
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
  }

  /// Install state, option catalog, resident-engine truth and the on-disk
  /// directories, re-read together.
  private func checkModel() {
    model.recheckWhisperModel()
    refreshStoredModels()
  }

  // MARK: - Details

  @ViewBuilder
  private func modelDetails(status: CsWhisperModelStatus, catalog: CsWhisperModelCatalog?)
    -> some View
  {
    VStack(alignment: .leading, spacing: 6) {
      if let catalog {
        detailLine(
          String(localized: "Selected", comment: "Model details: the saved selection"),
          value: "\(catalog.configured)")
        if let path = Self.selectedOption(catalog)?.path {
          detailLine(String(localized: "Loads from", comment: "Model details: path"), value: path)
        }
        detailLine(
          String(localized: "Loaded in memory", comment: "Model details: resident weights"),
          value: catalog.loaded
            ?? String(localized: "nothing", comment: "Model details: no resident weights"))
        detailLine(
          String(localized: "Next recording loads", comment: "Model details: resolved path"),
          value: catalog.resolvedPath
            ?? String(localized: "unresolved", comment: "Whisper model row: resolution failed"))
        ForEach(catalog.options, id: \.id) { option in
          detailLine(
            option.label,
            value: [option.path, Self.sourceLabel(option.source), option.reason]
              .compactMap { $0 }.joined(separator: " · "))
        }
      } else if let path = status.path {
        detailLine(String(localized: "Loads from", comment: "Model details: path"), value: path)
      }
      detailLine(
        String(localized: "Download source", comment: "Model details: Hugging Face repository"),
        value: "\(status.repo) · \(status.sizeHint)")
      ForEach(storedModels, id: \.name) { directory in
        detailLine(directory.name, value: directory.detail)
      }
    }
  }

  private func detailLine(_ label: String, value: String) -> some View {
    VStack(alignment: .leading, spacing: 1) {
      Text(label)
        .font(CSFont.mono(10, .semibold))
        .foregroundStyle(Color.secondary)
      Text(verbatim: value)
        .font(CSFont.mono(10, .medium))
        .foregroundStyle(Color.secondary)
        .textSelection(.enabled)
        .fixedSize(horizontal: false, vertical: true)
    }
  }

  // MARK: - Copy (pure, tested)

  static func selectedOption(_ catalog: CsWhisperModelCatalog) -> CsWhisperModelOption? {
    catalog.options.first { $0.id == catalog.configured }
  }

  /// Picker subtitle: the readable label of the saved selection, falling back
  /// to the raw reference when the catalog has no row for it.
  static func selectedOptionLabel(_ catalog: CsWhisperModelCatalog) -> String {
    selectedOption(catalog)?.label ?? catalog.configured
  }

  /// `catalog.loaded` (resident weights) and `catalog.resolvedPath` (what the
  /// next recording loads) are different facts; the row names both when they
  /// differ and never calls the next load "in use".
  static func residencyLabel(_ catalog: CsWhisperModelCatalog?) -> String {
    guard let catalog else {
      return String(
        localized: "Checking the model catalog…",
        comment: "Whisper tab: catalog not loaded yet")
    }
    guard let resolved = catalog.resolvedPath else {
      return String(
        localized:
          "The selected model cannot be resolved; the next recording has no model to load.",
        comment: "Whisper tab: resolution failed")
    }
    guard let loaded = catalog.loaded else {
      return String(
        localized: "Not loaded yet · loads on the next recording",
        comment: "Whisper tab: no resident weights")
    }
    if loaded == resolved || loaded == "embedded" {
      return String(
        localized: "Loaded in memory", comment: "Whisper tab: the selected model is resident")
    }
    return String(
      localized:
        "Loaded in memory: \(label(forPath: loaded, in: catalog)) · next recording loads \(label(forPath: resolved, in: catalog))",
      comment:
        "Whisper tab: resident model differs from the next load. Placeholders: two model names"
    )
  }

  static func label(forPath path: String, in catalog: CsWhisperModelCatalog) -> String {
    catalog.options.first { $0.path == path }?.label ?? path
  }

  /// `models_dir` | `hf_cache` | `configured_path` | `env_override` → copy.
  static func sourceLabel(_ source: String) -> String {
    switch source {
    case "models_dir": String(localized: "models folder", comment: "Model source")
    case "hf_cache": String(localized: "Hugging Face cache", comment: "Model source")
    case "configured_path": String(localized: "configured path", comment: "Model source")
    case "env_override": String(localized: "environment override", comment: "Model source")
    default: source
    }
  }

  /// Storage state code from the bridge → copy. Unknown codes stay verbatim.
  static func statusLabel(_ status: String) -> String {
    switch status {
    case "active":
      String(localized: "Selected", comment: "Model storage state: the runtime loads it")
    case "usable": String(localized: "Ready", comment: "Model storage state: loader accepts it")
    case "refused":
      String(localized: "Refused", comment: "Model storage state: loader refuses the format")
    case "broken": String(localized: "Broken", comment: "Model storage state: validation failed")
    default: status
    }
  }

  /// One plain sentence for a refusal reason the bridge sends in English; the
  /// raw text stays available under "Model details".
  static func reasonSummary(_ reason: String?) -> String {
    guard let reason, !reason.isEmpty else {
      return String(localized: "Unavailable", comment: "Whisper model row: no detail")
    }
    let lowered = reason.lowercased()
    if lowered.contains("quantized") {
      return String(
        localized: "The local engine does not support quantized (Q8) models.",
        comment: "Whisper model refusal: quantized weights")
    }
    if lowered.contains("tokenizer") {
      return String(
        localized: "The model's tokenizer is invalid. See Model details.",
        comment: "Whisper model refusal: tokenizer validation failed")
    }
    if lowered.contains("missing") || lowered.contains("incomplete") {
      return String(
        localized: "The model files are incomplete. See Model details.",
        comment: "Whisper model refusal: files missing")
    }
    return String(
      localized: "The model failed validation. See Model details.",
      comment: "Whisper model refusal: generic")
  }

  static func canRemoveModel(status: String) -> Bool {
    status != "active"
  }

  // MARK: - Storage

  private func refreshStoredModels() {
    do {
      storedModels = try modelDirectories()
      storageError = nil
    } catch {
      storageError = error.localizedDescription
    }
  }

  /// Storage row subtitle: state and size, the removal lock when selected, and
  /// a model sharing the same tokenizer.
  private func modelFootprint(_ directory: CsModelDirectory) -> String {
    let size = ByteCountFormatter.string(
      fromByteCount: Int64(directory.bytesOnDisk), countStyle: .file)
    var parts = ["\(Self.statusLabel(directory.status)) · \(size)"]
    if !Self.canRemoveModel(status: directory.status) {
      parts.append(
        String(
          localized: "The selected model cannot be removed.",
          comment: "Whisper tab storage row: why Remove is disabled"))
    }
    if let duplicate = directory.duplicateTokenizerWith {
      parts.append(
        String(
          localized: "identical tokenizer: \(duplicate)",
          comment: "Whisper tab storage row. Placeholder: name of the twin model"))
    }
    return parts.joined(separator: " · ")
  }

  /// A shadowing authority above the picker (env override), or nil. The
  /// override is shown, never silently removed.
  private func selectionOverrideNote(_ overrideKind: String?) -> String? {
    switch overrideKind {
    case "env_model_path":
      return String(
        localized: "CODESCRIBE_MODEL_PATH overrides the selected model.",
        comment: "Whisper model picker: an environment variable shadows the saved selection")
    case "env_local_model":
      return String(
        localized: "The LOCAL_MODEL environment variable overrides the selected model.",
        comment: "Whisper model picker: an environment variable shadows the saved selection")
    default:
      return nil
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
