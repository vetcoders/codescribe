import SwiftUI

/// Dictation › Whisper model. The selected model and its state share one card
/// of two rows; under it, in Local power, the live refinement card; below
/// that, the models on disk the loader refuses and — collapsed — the
/// diagnostic values (Founder brief, round 13, 2026-10-10; Founder review,
/// round 2, 2026-10-20). Paths, sources, sizes, raw validation errors and the
/// Remove action for installed directories live only under "Model details",
/// each value printed once.
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
          .font(SettingsScale.helper)
          .foregroundStyle(CSColor.secondaryInk)
          .fixedSize(horizontal: false, vertical: true)
      } else {
        selectedModelCard(status: status, catalog: catalog)
        selectionMessages(catalog: catalog)
        if catalog?.discoveryPending == true {
          Text("Checking other model locations…")
            .font(SettingsScale.helper)
            .foregroundStyle(CSColor.secondaryInk)
        }
      }

      // Readiness of the Local power refinement belongs next to the model it
      // depends on; the other modes do not use it, so nothing renders there.
      if model.asrModeId == "local_power" {
        liveRefinementCard
      }

      if model.whisperDownloadInFlight {
        downloadProgress
      } else if !status.available && !embedded {
        if let error = model.whisperDownloadStore.error {
          Text(error)
            .font(SettingsScale.helper)
            .foregroundStyle(CSColor.dangerLight)
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
      // Install state and catalog together: the model card and the refinement
      // card both read the install state, so neither opens on a stale one.
      model.recheckWhisperModel()
      refreshStoredModels()
    }
    .onChange(of: model.whisperModelCatalog?.configured) { _, _ in
      // A selection change moves the removal lock between "On disk" rows.
      refreshStoredModels()
    }
  }

  // MARK: - Selected model

  /// Two equal rows in one card: the selector, then one status line beside
  /// Check model. The picker already shows the selected model's name, so the
  /// row does not repeat it; the status line names the most specific true
  /// state (see `modelStatus`). Neither "Installed" nor "Loaded in memory" is
  /// recognition readiness — that is the refinement card's fact.
  @ViewBuilder
  private func selectedModelCard(status: CsWhisperModelStatus, catalog: CsWhisperModelCatalog?)
    -> some View
  {
    VStack(spacing: 0) {
      cardRow {
        Text("Model", comment: "Whisper tab row: the model selector")
          .font(SettingsScale.settingName)
          .foregroundStyle(Color.primary)
        Spacer(minLength: CSSpace.sm)
        if let catalog {
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
        } else {
          ProgressView()
            .controlSize(.small)
          Text("Checking the model catalog…")
            .font(SettingsScale.helper)
            .foregroundStyle(CSColor.secondaryInk)
        }
      }
      .accessibilityIdentifier("whisper-model-selector")
      divider
      cardRow {
        let line = Self.modelStatus(available: status.available, catalog: catalog)
        let statusText = String(
          localized: "Status: \(line.label)",
          comment:
            "Whisper tab row: the selected model's state. Placeholder: a state such as Installed or Loaded in memory"
        )
        Group {
          if line.tone == .ready {
            CSStatusBadge(text: statusText, mark: .verified)
          } else {
            Text(statusText)
          }
        }
        .font(SettingsScale.settingName)
        .foregroundStyle(line.tone.color)
        .fixedSize(horizontal: false, vertical: true)
        Spacer(minLength: CSSpace.sm)
        SettingsChipButton("Check model", tint: CSColor.secondaryInk, action: checkModel)
          .accessibilityHint("Re-reads the install state, the model catalog and the disk")
      }
    }
    .settingsGroupedInset(padding: 0)
  }

  /// Everything that shadows or contradicts the selection above: an authority
  /// over the picker, the engine's own notice, and its last error. Informational
  /// overrides stay neutral; a failed model selection still needs attention.
  @ViewBuilder
  private func selectionMessages(catalog: CsWhisperModelCatalog?) -> some View {
    let overrideNote = selectionOverrideNote(catalog?.overrideKind)
    // Nothing to say means no container at all: an empty stack would still
    // take the parent's spacing and open a gap under the card.
    if overrideNote != nil || model.whisperModelNotice != nil || model.whisperModelError != nil {
      VStack(alignment: .leading, spacing: CSSpace.xs) {
        if let overrideNote {
          Text(overrideNote)
            .font(SettingsScale.helper)
            .foregroundStyle(CSColor.secondaryInk)
            .fixedSize(horizontal: false, vertical: true)
        }
        if let notice = model.whisperModelNotice {
          Text(notice)
            .font(SettingsScale.helper)
            .foregroundStyle(CSColor.secondaryInk)
            .fixedSize(horizontal: false, vertical: true)
        }
        if let error = model.whisperModelError {
          Text(error)
            .font(SettingsScale.helper)
            .foregroundStyle(CSColor.amberLight)
            .fixedSize(horizontal: false, vertical: true)
        }
      }
    }
  }

  // MARK: - Live refinement (Local power)

  /// Whether Local power's live Whisper correction can run, with Recheck.
  /// Moved here from the Engine tab (Founder review, round 2, 2026-10-20);
  /// this is its one owner. The state is computed from the settings snapshot
  /// and the install state the `onAppear` above re-reads.
  private var liveRefinementCard: some View {
    SettingsControlRow(
      title: String(localized: "Live Whisper refinement"),
      subtitle: localWhisperRuntimeSubtitle
    ) {
      HStack(spacing: 8) {
        // Readiness is an ordinary word, so it reads in the interface
        // font; monospace stays for technical values on this pane
        // (Founder brief, round 13, 2026-10-10). A ready runtime is the
        // olive capsule with a check (Founder, 2026-10-26).
        if model.localWhisperRuntimeState == .livePatchingConfigured {
          CSStatusBadge(text: localWhisperRuntimeLabel, mark: .verified)
        } else {
          Text(localWhisperRuntimeLabel)
            .font(CSFont.ui(12, .semibold))
            .foregroundStyle(localWhisperRuntimeColor)
        }
        Button("Recheck", action: model.recheckLocalWhisperRuntime)
          .buttonStyle(.bordered)
          .controlSize(.small)
      }
    }
  }

  private var localWhisperRuntimeLabel: String {
    switch model.localWhisperRuntimeState {
    case .livePatchingNotReady: String(localized: "Not ready")
    case .livePatchingConfigured: String(localized: "Ready")
    case .degradedEnvOverride: String(localized: "Degraded (env override)")
    case .notSelected: String(localized: "Not selected")
    }
  }

  private var localWhisperRuntimeSubtitle: String {
    switch model.localWhisperRuntimeState {
    case .livePatchingConfigured:
      String(
        localized:
          "Apple recognizes speech and Whisper corrects the text during the recording.",
        comment: "Live Whisper refinement card: Local power is ready")
    case .livePatchingNotReady:
      String(
        localized:
          "The local Whisper model files are missing or invalid. Local power is not ready until they are valid.",
        comment: "Live Whisper refinement card: the model files failed the check")
    case .degradedEnvOverride:
      String(
        localized:
          "CODESCRIBE_LAYERED_TRANSCRIPTION disables live refinement. Remove the diagnostic override from .env and restart.",
        comment: "CODESCRIBE_LAYERED_TRANSCRIPTION is an environment variable name"
      )
    case .notSelected:
      String(localized: "Local Whisper is not the selected provider.")
    }
  }

  private var localWhisperRuntimeColor: Color {
    switch model.localWhisperRuntimeState {
    case .livePatchingConfigured:
      CSColor.oliveLight
    case .livePatchingNotReady:
      CSColor.amberLight
    case .degradedEnvOverride, .notSelected:
      CSColor.secondaryInk
    }
  }

  private var downloadProgress: some View {
    VStack(alignment: .leading, spacing: CSSpace.xs) {
      if let fraction = model.whisperDownloadFraction {
        CSProgressBar(value: fraction)
      } else {
        ProgressView()
          .controlSize(.small)
      }
      Text(model.whisperDownloadDetail ?? String(localized: "Downloading…"))
        .font(SettingsScale.helper)
        .foregroundStyle(CSColor.secondaryInk)
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
      VStack(alignment: .leading, spacing: SettingsScale.rowGap) {
        SettingsSectionLabel(
          String(
            localized: "Other detected models",
            comment: "Whisper tab section: models found on disk that cannot be selected"))
        VStack(spacing: 0) {
          ForEach(Array(unavailable.enumerated()), id: \.element.id) { entry in
            if entry.offset > 0 { divider }
            cardRow {
              Text(verbatim: entry.element.label)
                .font(SettingsScale.settingName)
                .foregroundStyle(Color.primary)
              Spacer(minLength: CSSpace.sm)
              Text(Self.reasonTag(entry.element.reason))
                .font(CSFont.ui(12, .medium))
                .foregroundStyle(CSColor.secondaryInk)
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

  // MARK: - On disk

  /// One installed directory under Model details › On disk: its name and
  /// location, its size and state, and Remove. This was the Disk space
  /// section's row; the section is gone (Founder review, round 2,
  /// 2026-10-20), and this is still the one place a size or Remove is shown.
  private func storedModelLine(_ directory: CsModelDirectory) -> some View {
    let removable = Self.canRemoveModel(status: directory.status)
    return HStack(alignment: .top, spacing: CSSpace.md) {
      VStack(alignment: .leading, spacing: 1) {
        detailLine(directory.name, value: directory.detail)
        Text(modelFootprint(directory))
          .font(CSFont.ui(11))
          .foregroundStyle(CSColor.secondaryInk)
          .fixedSize(horizontal: false, vertical: true)
      }
      Spacer(minLength: CSSpace.sm)
      if removable {
        SettingsChipButton("Remove", tint: CSColor.dangerLight) {
          remove(directory)
        }
      } else {
        // Disabled, not hidden: the line must still show that removal is the
        // action it would otherwise offer, and say why it cannot.
        SettingsChipButton("Remove", tint: CSColor.dangerLight, enabled: false) {}
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
  /// with it, every option the catalog found, and what sits on disk with its
  /// size and Remove. The
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

      if !storedModels.isEmpty || storageError != nil {
        detailGroup(
          String(localized: "On disk", comment: "Model details group: installed directories")
        ) {
          ForEach(storedModels, id: \.name) { directory in
            storedModelLine(directory)
          }
          if let storageError {
            Text(storageError)
              .font(SettingsScale.helper)
              .foregroundStyle(CSColor.amberLight)
              .fixedSize(horizontal: false, vertical: true)
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
        .font(CSFont.ui(12, .semibold))
        .foregroundStyle(CSColor.secondaryInk)
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
        .foregroundStyle(CSColor.secondaryInk)
      Text(verbatim: value)
        .font(CSFont.mono(11))
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
    .padding(.horizontal, SettingsScale.cardPadding)
    .padding(.vertical, 11)
  }

  private var divider: some View {
    Rectangle().fill(Color.primary.opacity(CSAlpha.stroke)).frame(height: 1)
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

  /// How the selected-model status line reads: ready-ish (olive), missing
  /// (amber), or still being read (secondary).
  enum ModelStatusTone: Equatable {
    case ready, missing, pending

    var color: Color {
      switch self {
      case .ready: CSColor.oliveLight
      case .missing: CSColor.amberLight
      case .pending: CSColor.secondaryInk
      }
    }
  }

  /// The one status line of the selected-model card: the most specific true
  /// state. Not installed wins; then the catalog has to be read and the
  /// selection resolved; then residency decides between "Loaded in memory"
  /// and "Installed". `catalog.loaded` (resident weights) and
  /// `catalog.resolvedPath` (what the next recording loads) are different
  /// facts; the line names both when they differ and never calls the next
  /// load "in use".
  static func modelStatus(available: Bool, catalog: CsWhisperModelCatalog?)
    -> (label: String, tone: ModelStatusTone)
  {
    guard available else {
      return (
        String(localized: "Not installed", comment: "Whisper tab status: no usable model on disk"),
        .missing
      )
    }
    guard let catalog else {
      return (
        String(
          localized: "Checking the model catalog…",
          comment: "Whisper tab: catalog not loaded yet"),
        .pending
      )
    }
    guard let resolved = catalog.resolvedPath else {
      return (
        String(
          localized:
            "The selected model cannot be resolved; the next recording has no model to load.",
          comment: "Whisper tab: resolution failed"),
        .missing
      )
    }
    guard let loaded = catalog.loaded else {
      return (
        String(
          localized: "Installed", comment: "Whisper tab status: on disk, not in memory yet"),
        .ready
      )
    }
    if loaded == resolved || loaded == "embedded" {
      return (
        String(
          localized: "Loaded in memory", comment: "Whisper tab: the selected model is resident"),
        .ready
      )
    }
    return (
      String(
        localized:
          "Loaded in memory: \(label(forPath: loaded, in: catalog)) · next recording loads \(label(forPath: resolved, in: catalog))",
        comment:
          "Whisper tab: resident model differs from the next load. Placeholders: two model names"
      ),
      .ready
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

  /// "On disk" line under the location: size first, then what the state means, and a
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
          comment: "Whisper tab On disk line. Placeholder: name of the twin model"))
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
