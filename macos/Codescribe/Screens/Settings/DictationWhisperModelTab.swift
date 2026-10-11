import SwiftUI

/// Dictation › Whisper model. The selected model and its install state share
/// one card of two rows; below it, the models on disk the loader refuses, the
/// space each installed model takes, and — collapsed — the diagnostic values
/// (Founder brief, round 13, 2026-10-10). Paths, sources and raw validation
/// errors live only under "Model details", each value printed once.
struct DictationWhisperModelTab: View {
  @ObservedObject var model: SettingsViewModel
  @State private var storedModels: [CsModelDirectory] = []
  @State private var storageError: String?
  @State private var showingModelDetails = false

  var body: some View {
    let status = model.localWhisperStatus
    let catalog = model.whisperModelCatalog
    let embedded = status.embedded || catalog?.overrideKind == "embedded"
    VStack(alignment: .leading, spacing: CSSpace.md) {
      if embedded {
        Text("This build embeds Whisper. The on-disk model selection is ignored.")
          .font(CSFont.ui(11.5))
          .foregroundStyle(Color.secondary)
          .fixedSize(horizontal: false, vertical: true)
      } else {
        selectedModelCard(status: status, catalog: catalog)
        selectionMessages(catalog: catalog)
      }

      if model.whisperDownloadInFlight {
        downloadProgress
      } else if !status.available && !embedded {
        if let error = model.whisperDownloadStore.error {
          Text(error)
            .font(CSFont.ui(11.5))
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
        otherDetectedModels(catalog)
      }

      diskSpaceSection

      DisclosureGroup(isExpanded: $showingModelDetails) {
        modelDetails(status: status, catalog: catalog)
          .padding(.top, CSSpace.xs)
      } label: {
        SettingsSectionLabel(
          String(
            localized: "Model details",
            comment: "Whisper tab disclosure: paths, sources and raw errors"))
      }
      .padding(.top, CSSpace.xs)
      .accessibilityIdentifier("whisper-model-details")
    }
    .onAppear {
      model.refreshWhisperModelCatalog()
      refreshStoredModels()
    }
    .onChange(of: model.whisperModelCatalog?.configured) { _, _ in
      // A selection change moves the removal lock between disk-space rows.
      refreshStoredModels()
    }
  }

  // MARK: - Selected model

  /// Two rows in one card: the selector, then the install state. The picker
  /// already shows the selected model's name, so the row no longer repeats it
  /// underneath, and install state and residency stay two separate facts on
  /// two lines — "installed" never stands in for "loaded".
  @ViewBuilder
  private func selectedModelCard(status: CsWhisperModelStatus, catalog: CsWhisperModelCatalog?)
    -> some View
  {
    VStack(spacing: 0) {
      if let catalog {
        cardRow {
          Text("Model", comment: "Whisper tab row: the model selector")
            .font(CSFont.ui(12.5, .semibold))
            .foregroundStyle(Color.primary)
          Spacer(minLength: CSSpace.sm)
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
          // The picker can only offer selectable options, so a saved selection
          // the loader refuses leaves it blank. VoiceOver still names what is
          // saved; on screen that state is the resolution failure below.
          .accessibilityLabel(Text("Whisper model"))
          .accessibilityValue(Text(verbatim: Self.selectedOptionLabel(catalog)))
        }
        divider
      }
      cardRow {
        VStack(alignment: .leading, spacing: 2) {
          Text(
            status.available
              ? String(localized: "Installed") : String(localized: "Not installed")
          )
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(status.available ? CSColor.oliveLight : CSColor.amber)
          Text(Self.residencyLabel(catalog))
            .font(CSFont.ui(11.5))
            .foregroundStyle(Color.secondary)
            .fixedSize(horizontal: false, vertical: true)
        }
        Spacer(minLength: CSSpace.sm)
        SettingsChipButton("Check model", tint: Color.secondary, action: checkModel)
          .accessibilityHint("Re-reads the install state, the model catalog and the disk")
      }
    }
    .settingsGroupedInset(padding: 0)
  }

  /// Everything that shadows or contradicts the selection above: an authority
  /// over the picker, the engine's own notice, and its last error. All three
  /// stay — they are the states that ask the user to do something.
  @ViewBuilder
  private func selectionMessages(catalog: CsWhisperModelCatalog?) -> some View {
    let overrideNote = selectionOverrideNote(catalog?.overrideKind)
    // Nothing to say means no container at all: an empty stack would still
    // take the parent's spacing and open a gap under the card.
    if overrideNote != nil || model.whisperModelNotice != nil || model.whisperModelError != nil {
      VStack(alignment: .leading, spacing: CSSpace.xs) {
        if let overrideNote {
          Text(overrideNote)
            .font(CSFont.ui(11.5))
            .foregroundStyle(CSColor.amber)
            .fixedSize(horizontal: false, vertical: true)
        }
        if let notice = model.whisperModelNotice {
          Text(notice)
            .font(CSFont.ui(11.5))
            .foregroundStyle(Color.secondary)
            .fixedSize(horizontal: false, vertical: true)
        }
        if let error = model.whisperModelError {
          Text(error)
            .font(CSFont.ui(11.5))
            .foregroundStyle(CSColor.amber)
            .fixedSize(horizontal: false, vertical: true)
        }
      }
    }
  }

  private var downloadProgress: some View {
    VStack(alignment: .leading, spacing: CSSpace.xs) {
      if let fraction = model.whisperDownloadFraction {
        ProgressView(value: fraction)
          .progressViewStyle(.linear)
          .tint(CSColor.chromeAccent)
      } else {
        ProgressView()
          .controlSize(.small)
      }
      Text(model.whisperDownloadDetail ?? String(localized: "Downloading…"))
        .font(CSFont.ui(11.5))
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

  // MARK: - Other detected models

  /// Models on disk the loader refuses: the name, and one short status on the
  /// trailing edge. The bridge's raw reason is not dropped — it is printed
  /// under Model details, where the rest of the diagnostics live.
  @ViewBuilder
  private func otherDetectedModels(_ catalog: CsWhisperModelCatalog) -> some View {
    let unavailable = catalog.options.filter { !$0.usable }
    if !unavailable.isEmpty {
      VStack(alignment: .leading, spacing: CSSpace.control) {
        SettingsSectionLabel(
          String(
            localized: "Other detected models",
            comment: "Whisper tab section: models found on disk that cannot be selected"))
        VStack(spacing: 0) {
          ForEach(Array(unavailable.enumerated()), id: \.element.id) { entry in
            if entry.offset > 0 { divider }
            cardRow {
              Text(verbatim: entry.element.label)
                .font(CSFont.ui(12.5, .semibold))
                .foregroundStyle(Color.primary)
              Spacer(minLength: CSSpace.sm)
              Text(Self.reasonTag(entry.element.reason))
                .font(CSFont.ui(11.5, .medium))
                .foregroundStyle(CSColor.amber)
                .multilineTextAlignment(.trailing)
                .fixedSize(horizontal: false, vertical: true)
            }
            .accessibilityElement(children: .combine)
          }
        }
        .settingsGroupedInset(padding: 0)
      }
    }
  }

  // MARK: - Disk space

  /// One row per installed model: the directory name, its size and state, and
  /// Remove. The selected model keeps its removal lock; the section disappears
  /// entirely when there is nothing installed and nothing went wrong.
  @ViewBuilder
  private var diskSpaceSection: some View {
    if !storedModels.isEmpty || storageError != nil {
      VStack(alignment: .leading, spacing: CSSpace.control) {
        SettingsSectionLabel(
          String(
            localized: "Disk space",
            comment: "Whisper tab section: installed model directories and what they take"))
        if !storedModels.isEmpty {
          VStack(spacing: 0) {
            ForEach(Array(storedModels.enumerated()), id: \.element.name) { entry in
              if entry.offset > 0 { divider }
              diskSpaceRow(entry.element)
            }
          }
          .settingsGroupedInset(padding: 0)
        }
        if let storageError {
          Text(storageError)
            .font(CSFont.ui(11.5))
            .foregroundStyle(CSColor.amber)
            .fixedSize(horizontal: false, vertical: true)
        }
      }
    }
  }

  private func diskSpaceRow(_ directory: CsModelDirectory) -> some View {
    let removable = Self.canRemoveModel(status: directory.status)
    return cardRow {
      VStack(alignment: .leading, spacing: 2) {
        // A directory name is an identifier, so it keeps monospace while the
        // line under it reads as ordinary copy.
        Text(verbatim: directory.name)
          .font(CSFont.mono(12, .medium))
          .foregroundStyle(Color.primary)
          .lineLimit(1)
          .truncationMode(.middle)
          .textSelection(.enabled)
        Text(modelFootprint(directory))
          .font(CSFont.ui(11.5))
          .foregroundStyle(Color.secondary)
          .fixedSize(horizontal: false, vertical: true)
      }
      Spacer(minLength: CSSpace.sm)
      if removable {
        SettingsChipButton("Remove", tint: CSColor.terracotta) {
          remove(directory)
        }
      } else {
        // Disabled, not hidden: the row must still show that removal is the
        // action this line would otherwise offer, and say why it cannot.
        SettingsChipButton("Remove", tint: CSColor.terracotta, enabled: false) {}
          .help("The selected model cannot be removed.")
          .accessibilityHint("The selected model cannot be removed.")
      }
    }
  }

  private func remove(_ directory: CsModelDirectory) {
    do {
      try removeModelDirectory(name: directory.name)
      refreshStoredModels()
    } catch {
      storageError = error.localizedDescription
    }
  }

  // MARK: - Details

  /// Collapsed by default, and the only place a path is printed in full. Three
  /// groups of label–value pairs: what is selected and what the engine does
  /// with it, every option the catalog found, and what sits on disk. The
  /// selected option's path is the "Loads from"/"Next recording loads" line,
  /// so its own catalog row below does not repeat the same string.
  @ViewBuilder
  private func modelDetails(status: CsWhisperModelStatus, catalog: CsWhisperModelCatalog?)
    -> some View
  {
    VStack(alignment: .leading, spacing: CSSpace.md) {
      detailGroup(
        String(
          localized: "Selected model", comment: "Model details group: the selection and its engine")
      ) {
        if let catalog {
          detailLine(
            String(localized: "Configured as", comment: "Model details: the saved selection"),
            value: catalog.configured)
          if let path = Self.selectedOption(catalog)?.path, path != catalog.resolvedPath {
            detailLine(String(localized: "Loads from", comment: "Model details: path"), value: path)
          }
          detailLine(
            String(localized: "Next recording loads", comment: "Model details: resolved path"),
            value: catalog.resolvedPath
              ?? String(localized: "unresolved", comment: "Whisper model row: resolution failed"))
          detailLine(
            String(localized: "Loaded in memory", comment: "Model details: resident weights"),
            value: catalog.loaded
              ?? String(localized: "nothing", comment: "Model details: no resident weights"))
        } else if let path = status.path {
          detailLine(String(localized: "Loads from", comment: "Model details: path"), value: path)
        }
        detailLine(
          String(localized: "Download source", comment: "Model details: Hugging Face repository"),
          value: "\(status.repo) · \(status.sizeHint)")
      }

      if let catalog, !catalog.options.isEmpty {
        detailGroup(
          String(
            localized: "Detected models",
            comment: "Model details group: every option the catalog found")
        ) {
          ForEach(catalog.options, id: \.id) { option in
            detailLine(option.label, value: optionDetail(option, in: catalog))
          }
        }
      }

      if !storedModels.isEmpty {
        detailGroup(
          String(localized: "On disk", comment: "Model details group: installed directories")
        ) {
          ForEach(storedModels, id: \.name) { directory in
            detailLine(directory.name, value: directory.detail)
          }
        }
      }
    }
  }

  /// Source and refusal reason for one catalog option, with its path only when
  /// the group above has not already printed it.
  private func optionDetail(_ option: CsWhisperModelOption, in catalog: CsWhisperModelCatalog)
    -> String
  {
    let printedAbove =
      option.path == catalog.resolvedPath || option.path == Self.selectedOption(catalog)?.path
    return [printedAbove ? nil : option.path, Self.sourceLabel(option.source), option.reason]
      .compactMap { $0 }.joined(separator: " · ")
  }

  private func detailGroup<Content: View>(
    _ title: String, @ViewBuilder content: () -> Content
  ) -> some View {
    VStack(alignment: .leading, spacing: CSSpace.xs) {
      Text(title)
        .font(CSFont.ui(11.5, .semibold))
        .foregroundStyle(Color.secondary)
        .accessibilityAddTraits(.isHeader)
      content()
    }
  }

  /// Label in the interface font, value in monospace: the value is the
  /// technical datum, the label is an ordinary word. The value wraps instead of
  /// truncating and stays selectable, so a long path is readable and copyable.
  private func detailLine(_ label: String, value: String) -> some View {
    VStack(alignment: .leading, spacing: 1) {
      Text(label)
        .font(CSFont.ui(11))
        .foregroundStyle(Color.secondary)
      Text(verbatim: value)
        .font(CSFont.mono(10.5))
        .foregroundStyle(Color.primary)
        .textSelection(.enabled)
        .fixedSize(horizontal: false, vertical: true)
    }
    .frame(maxWidth: .infinity, alignment: .leading)
  }

  // MARK: - Card chrome

  /// One row inside a bordered card. The shared `SettingsControlRow` is one
  /// card per row; this pane puts related rows in one card with a divider
  /// between them, which is the Founder's round-13 layout.
  private func cardRow<Content: View>(@ViewBuilder content: () -> Content) -> some View {
    HStack(spacing: CSSpace.md) {
      content()
    }
    .padding(.horizontal, CSSpace.card)
    .padding(.vertical, 11)
  }

  private var divider: some View {
    Rectangle().fill(Color.primary.opacity(0.12)).frame(height: 1)
  }

  // MARK: - Copy (pure, tested)

  static func selectedOption(_ catalog: CsWhisperModelCatalog) -> CsWhisperModelOption? {
    catalog.options.first { $0.id == catalog.configured }
  }

  /// The readable label of the saved selection, falling back to the raw
  /// reference when the catalog has no row for it. The picker's accessibility
  /// value, so a selection the picker cannot show still has a name.
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
      String(
        localized: "Selected model", comment: "Model storage state: the runtime loads this one")
    case "usable": String(localized: "Ready", comment: "Model storage state: loader accepts it")
    case "refused":
      String(localized: "Refused", comment: "Model storage state: loader refuses the format")
    case "broken": String(localized: "Broken", comment: "Model storage state: validation failed")
    default: status
    }
  }

  /// One short status for the trailing edge of an "Other detected models" row:
  /// which problem it is, in two or three words. The bridge sends the reason in
  /// English prose; the raw text stays under Model details, so the short status
  /// hides nothing a user would act on.
  static func reasonTag(_ reason: String?) -> String {
    guard let reason, !reason.isEmpty else {
      return String(localized: "Unavailable", comment: "Whisper model row: no detail")
    }
    let lowered = reason.lowercased()
    if lowered.contains("quantized") {
      return String(
        localized: "Unsupported",
        comment: "Whisper model status: quantized (Q8) weights the local engine cannot load")
    }
    if lowered.contains("tokenizer") {
      return String(
        localized: "Tokenizer problem",
        comment: "Whisper model status: tokenizer validation failed")
    }
    if lowered.contains("missing") || lowered.contains("incomplete") {
      return String(
        localized: "Incomplete files", comment: "Whisper model status: model files are missing")
    }
    return String(
      localized: "Failed validation", comment: "Whisper model status: validation failed")
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

  /// Disk-space row second line: size first, then what the state means, and a
  /// model sharing the same tokenizer. Why Remove is locked is on the button
  /// itself, so the common case reads as two short facts.
  private func modelFootprint(_ directory: CsModelDirectory) -> String {
    let size = ByteCountFormatter.string(
      fromByteCount: Int64(directory.bytesOnDisk), countStyle: .file)
    var parts = ["\(size) · \(Self.statusLabel(directory.status))"]
    if let duplicate = directory.duplicateTokenizerWith {
      parts.append(
        String(
          localized: "identical tokenizer: \(duplicate)",
          comment: "Whisper tab disk-space row. Placeholder: name of the twin model"))
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
