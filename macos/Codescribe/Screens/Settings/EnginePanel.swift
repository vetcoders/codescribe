import SwiftUI

// Engine panel: runtime truth + engine controls. The key/value runtime rows are
// READ-ONLY (sourced from the live CsSettings snapshot, not hardcoded) and the
// permission matrix reflects live status. "Engine controls" and "LLM lanes" are
// editable: STT/layered controls plus per-lane provider, endpoint, and model
// overrides, all persisted through the promoted-key config router.

struct EnginePanel: View {
    @ObservedObject var model: SettingsViewModel

    private let matrixOrder: [PermissionKind] = [
        .microphone, .accessibility, .inputMonitoring, .screenRecording
    ]
    private let columns = [
        GridItem(.flexible(), spacing: 8),
        GridItem(.flexible(), spacing: 8)
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                EyebrowLabel(text: "Settings · Engine")
                Text("RUNTIME TRUTH · READ-ONLY ROWS")
                    .font(CSFont.mono(9, .medium))
                    .foregroundStyle(CSColor.textMutedAlt)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 2)
                    .background(
                        RoundedRectangle(cornerRadius: 6, style: .continuous)
                            .fill(CSColor.surfaceRaised(0.04))
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: 6, style: .continuous)
                            .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
                    )
            }

            Text("What's actually running.")
                .font(CSFont.ui(26, .bold))
                .tracking(-0.5)
                .foregroundStyle(CSColor.textHigh)
                .padding(.top, 6)

            runtimeRows
                .padding(.top, 20)

            SettingsSectionLabel("Engine controls")
                .padding(.top, 22)
            engineControls
                .padding(.top, 11)

            SettingsSectionLabel("LLM lanes")
                .padding(.top, 22)
            LLMLanesSection(model: model)
                .padding(.top, 11)

            SettingsSectionLabel("Permission matrix")
                .padding(.top, 22)
            LazyVGrid(columns: columns, spacing: 8) {
                ForEach(matrixOrder) { kind in
                    PermissionMatrixCell(kind: kind, state: model.permissions.state(kind))
                }
            }
            .padding(.top, 11)

            HStack(spacing: 8) {
                Text("●").font(CSFont.mono(11, .medium)).foregroundStyle(CSColor.olive)
                Text("runtime rows reflect the live engine — changes apply on the next recording session or LLM request")
                    .font(CSFont.mono(11, .medium))
                    .foregroundStyle(CSColor.textFaint)
            }
            .padding(.top, 16)

            WorkspaceRootsSection(model: model)
                .padding(.top, 30)

            AgentStatusSection(model: model)
                .padding(.top, 30)

            MCPServersSection(model: model)
                .padding(.top, 26)

            ResetAppDataSection(model: model)
                .padding(.top, 30)
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 24)
    }

    // MARK: Runtime key/value rows

    private var runtimeRows: some View {
        VStack(spacing: 0) {
            RuntimeRow(key: "Active STT", value: model.activeSTT,
                       tint: true, trailing: .dot(model.sttHealthy ? CSColor.oliveLight : CSColor.amber))
            divider
            RuntimeRow(key: "STT model", value: model.sttModelDescription,
                       tint: false, mono: true, trailing: .none)
            divider
            RuntimeRow(key: "Whisper language", value: model.whisperLanguageCode,
                       tint: true, mono: true, trailing: .none)
            divider
            RuntimeRow(key: "AI formatting", value: model.formattingDescription,
                       tint: false, trailing: .none)
            divider
            ForEach(LLMLane.allCases) { lane in
                RuntimeRow(key: "\(lane.title) endpoint", value: model.resolvedLLMEndpoint(for: lane),
                           tint: false, mono: true, trailing: .none)
                divider
                RuntimeRow(key: "\(lane.title) model", value: model.resolvedLLMModel(for: lane),
                           tint: true, mono: true, trailing: .none)
                divider
            }
            RuntimeRow(key: "API keys", value: model.apiKeysDescription,
                       tint: true,
                       trailing: model.apiKeysStored ? .text("secure", CSColor.oliveLight) : .text("missing", CSColor.amber))
        }
        .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 13, style: .continuous)
                .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
        )
    }

    private var divider: some View {
        Rectangle().fill(CSColor.hairline(0.05)).frame(height: 1)
    }

    // MARK: Engine controls (editable — F1 layered transcription)

    /// Selectable engines. "onnx" is deliberately NOT exposed (experimental,
    /// frozen); "auto" defers to the core policy (Apple live when available).
    private static let sttEngineOptions: [(id: String, label: String)] = [
        ("auto", "Auto"),
        ("apple", "Apple (live)"),
        ("whisper", "Whisper (Candle)"),
    ]

    private var layeredBinding: Binding<Bool> {
        Binding(get: { model.layeredTranscriptionEnabled },
                set: { model.setLayeredTranscription($0) })
    }

    private var engineControls: some View {
        VStack(spacing: 8) {
            SettingsControlRow(title: "STT engine",
                               subtitle: "Auto prefers Apple live speech, else Whisper") {
                Menu {
                    ForEach(Self.sttEngineOptions, id: \.id) { option in
                        Button {
                            model.setSttEngine(option.id)
                        } label: {
                            if option.id == model.sttEngineId {
                                Label(option.label, systemImage: "checkmark")
                            } else {
                                Text(option.label)
                            }
                        }
                    }
                } label: {
                    EngineMenuLabel(text: model.sttEngineLabel)
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
            }
            SettingsControlRow(title: "Layered transcription",
                               subtitle: "Experimental: Apple live layer + Whisper tail patches") {
                Toggle("", isOn: layeredBinding)
                    .toggleStyle(.switch)
                    .labelsHidden()
                    .tint(CSColor.terracotta)
            }
        }
    }
}

// MARK: - Editable LLM lanes

/// Three request lanes sharing one visual grammar while preserving their distinct
/// promoted config keys. Runtime rows above remain the effective read-only truth.
private struct LLMLanesSection: View {
    @ObservedObject var model: SettingsViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Set provider, endpoint, and model per request path. Leave an override empty to use the resolved fallback.")
                .font(CSFont.ui(11.5))
                .lineSpacing(2)
                .foregroundStyle(CSColor.textMutedAlt)

            ForEach(LLMLane.allCases) { lane in
                LLMLaneEditor(model: model, lane: lane)
                if lane != LLMLane.allCases.last {
                    Rectangle()
                        .fill(CSColor.hairline(0.05))
                        .frame(height: 1)
                }
            }
        }
    }
}

private struct LLMLaneEditor: View {
    @ObservedObject var model: SettingsViewModel
    let lane: LLMLane

    @State private var endpointDraft = ""
    @State private var modelDraft = ""

    private var modelOptions: [CsModelOption] { model.modelOptions(for: lane) }

    private var providerLabel: String {
        model.availableProviders.first { $0.id == model.assistiveProviderId }?.displayName
            ?? model.assistiveProviderId
    }

    private var endpointPlaceholder: String {
        model.resolvedLLMEndpoint(for: lane)
    }

    private var modelPlaceholder: String {
        model.resolvedLLMModel(for: lane)
    }

    private var currentModelLabel: String {
        let id = model.resolvedLLMModel(for: lane)
        return modelOptions.first { $0.id == id }?.displayName ?? id
    }

    private var resolvedLaneEndpoint: String { model.resolvedLLMEndpoint(for: lane) }

    /// The bridge currently discovers OpenAI models through the Assistive lane's
    /// endpoint. Formatting/Main may consume that catalog only when their own
    /// resolved endpoint is identical; otherwise the catalog belongs elsewhere.
    private var discoveryMatchesLaneEndpoint: Bool {
        lane == .assistive
            || resolvedLaneEndpoint == model.resolvedLLMEndpoint(for: .assistive)
    }

    /// Formatting/main share the current OpenAI Responses discovery contract.
    /// A non-OpenAI host must stay free-form because discovery is not lane-aware.
    private var hasCustomOpenAIEndpoint: Bool {
        let providerId = lane == .assistive ? model.assistiveProviderId : "openai-responses"
        guard providerId == "openai-responses" else { return false }
        guard let host = URL(string: resolvedLaneEndpoint)?.host?.lowercased() else { return true }
        return host != "api.openai.com"
    }

    private var requiresManualModelEntry: Bool {
        hasCustomOpenAIEndpoint || !discoveryMatchesLaneEndpoint
    }

    private var usesDiscoveredPicker: Bool {
        let status = model.modelDiscoveryStatus(for: lane)
        return !requiresManualModelEntry
            && !modelOptions.isEmpty
            && status == "fresh"
    }

    private var discoveryDotColor: Color {
        if requiresManualModelEntry { return CSColor.textFaint }
        switch model.modelDiscoveryStatus(for: lane) {
        case "fresh": return CSColor.olive
        case "cached": return CSColor.amber
        case "no_key", "loading": return CSColor.textFaint
        default: return CSColor.terracotta
        }
    }

    private var discoveryDescription: String {
        if hasCustomOpenAIEndpoint {
            return "Custom endpoint — enter its model ID manually"
        }
        if !discoveryMatchesLaneEndpoint {
            return "Endpoint differs from Assistive discovery — enter its model ID manually"
        }
        return model.modelDiscoveryDescription(for: lane)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text(lane.title)
                    .font(CSFont.ui(14.5, .bold))
                    .foregroundStyle(CSColor.textHigh)
                Text(lane.subtitle)
                    .font(CSFont.ui(11.5))
                    .foregroundStyle(CSColor.textMutedAlt)
            }

            if lane == .assistive {
                SettingsControlRow(title: "Provider", subtitle: "Assistive requests only") {
                    Menu {
                        ForEach(model.availableProviders, id: \.id) { provider in
                            Button {
                                model.setAssistiveProvider(provider.id)
                            } label: {
                                if provider.id == model.assistiveProviderId {
                                    Label(provider.displayName, systemImage: "checkmark")
                                } else {
                                    Text(provider.displayName)
                                }
                            }
                        }
                    } label: {
                        EngineMenuLabel(text: providerLabel)
                    }
                    .menuStyle(.borderlessButton)
                    .menuIndicator(.hidden)
                    .fixedSize()
                    .accessibilityLabel("Assistive provider")
                    .accessibilityValue(providerLabel)
                }
            }

            SettingsControlRow(title: "Endpoint", subtitle: lane.endpointKey) {
                HStack(spacing: 8) {
                    TextField(endpointPlaceholder, text: $endpointDraft)
                        .textFieldStyle(.plain)
                        .font(CSFont.mono(11.5, .regular))
                        .foregroundStyle(CSColor.textBody)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 7)
                        .background(
                            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                                .fill(CSColor.surfaceRaised(0.03))
                        )
                        .overlay(
                            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                                .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
                        )
                        .onSubmit(saveEndpoint)
                        .accessibilityLabel("\(lane.title) LLM endpoint")

                    Button("Save", action: saveEndpoint)
                        .font(CSFont.ui(11.5, .semibold))
                        .foregroundStyle(endpointDraft.isEmpty ? CSColor.textFaint : CSColor.terracottaLight)
                        .buttonStyle(.plain)
                        .disabled(endpointDraft.isEmpty)
                        .accessibilityLabel("Save \(lane.title) endpoint")

                    Button("Reset") {
                        endpointDraft = ""
                        model.setLLMEndpoint("", for: lane)
                    }
                    .font(CSFont.ui(11.5, .semibold))
                    .foregroundStyle(CSColor.textMutedAlt)
                    .buttonStyle(.plain)
                    .help("Clear this endpoint override")
                    .accessibilityLabel("Reset \(lane.title) endpoint")
                }
                .frame(width: 380)
            }

            SettingsControlRow(title: "Model", subtitle: lane.modelKey) {
                HStack(spacing: 8) {
                    if model.modelDiscoveryStatus(for: lane) == "loading"
                        && !requiresManualModelEntry
                    {
                        HStack(spacing: 7) {
                            ProgressView()
                                .controlSize(.small)
                            Text("Discovering models…")
                                .font(CSFont.mono(10.5, .medium))
                                .foregroundStyle(CSColor.textFaint)
                        }
                        .frame(maxWidth: .infinity, alignment: .trailing)
                        .accessibilityLabel("Discovering \(lane.title) models")
                    } else if usesDiscoveredPicker {
                        Menu {
                            ForEach(modelOptions, id: \.id) { option in
                                Button {
                                    model.setLLMModel(option.id, for: lane)
                                } label: {
                                    if option.id == model.resolvedLLMModel(for: lane) {
                                        Label(option.displayName, systemImage: "checkmark")
                                    } else {
                                        Text(option.displayName)
                                    }
                                }
                            }
                        } label: {
                            EngineMenuLabel(text: currentModelLabel)
                        }
                        .menuStyle(.borderlessButton)
                        .menuIndicator(.hidden)
                        .frame(maxWidth: .infinity, alignment: .trailing)
                        .accessibilityLabel("\(lane.title) model")
                        .accessibilityValue(currentModelLabel)
                    } else {
                        TextField(modelPlaceholder, text: $modelDraft)
                            .textFieldStyle(.plain)
                            .font(CSFont.mono(11.5, .regular))
                            .foregroundStyle(CSColor.textBody)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 7)
                            .background(
                                RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                                    .fill(CSColor.surfaceRaised(0.03))
                            )
                            .overlay(
                                RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                                    .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
                            )
                            .onSubmit(saveModel)
                            .accessibilityLabel("\(lane.title) model ID")

                        Button("Save", action: saveModel)
                            .font(CSFont.ui(11.5, .semibold))
                            .foregroundStyle(modelDraft.isEmpty ? CSColor.textFaint : CSColor.terracottaLight)
                            .buttonStyle(.plain)
                            .disabled(modelDraft.isEmpty)
                            .accessibilityLabel("Save \(lane.title) model")
                    }

                    Button("Reset") {
                        modelDraft = ""
                        model.setLLMModel("", for: lane)
                    }
                    .font(CSFont.ui(11.5, .semibold))
                    .foregroundStyle(CSColor.textMutedAlt)
                    .buttonStyle(.plain)
                    .help("Clear this model override")
                    .accessibilityLabel("Reset \(lane.title) model")
                }
                .frame(width: 380)
            }

            HStack(spacing: 8) {
                Circle()
                    .fill(discoveryDotColor.opacity(0.85))
                    .frame(width: 7, height: 7)
                Text(discoveryDescription)
                    .font(CSFont.mono(10.5, .medium))
                    .foregroundStyle(CSColor.textFaint)
                    .lineLimit(2)
            }
            .padding(.leading, 2)
        }
    }

    private func saveEndpoint() {
        model.setLLMEndpoint(endpointDraft, for: lane)
        endpointDraft = ""
    }

    private func saveModel() {
        model.setLLMModel(modelDraft, for: lane)
        modelDraft = ""
    }
}

// MARK: - Engine dropdown label (mirrors the KeysPanel MenuLabel shape)

private struct EngineMenuLabel: View {
    let text: String

    var body: some View {
        HStack(spacing: 6) {
            Text(text)
                .font(CSFont.ui(12.5, .semibold))
                .foregroundStyle(CSColor.textHigh)
                .lineLimit(1)
            CSIconView(icon: .chevronUpDown, size: 9, weight: .semibold, color: CSColor.textFaint)
        }
    }
}

// MARK: - Agent workspace roots editor

/// Editable list of workspace roots the agent's `list_projects` tool scans to
/// resolve project names to absolute paths. Rows are edited locally and committed
/// through `SettingsViewModel.setAgentWorkspaceRoots` (colon-joined ->
/// `AGENT_WORKSPACE_ROOTS`). Each row shows a live "directory exists" indicator.
struct WorkspaceRootsSection: View {
    @ObservedObject var model: SettingsViewModel

    @State private var rows: [String] = []
    @State private var loaded = false

    private var isDirty: Bool {
        cleaned(rows) != cleaned(model.agentWorkspaceRoots)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SettingsSectionLabel("Agent workspace roots")

            Text("Directories the assistant scans to resolve a project name to a path (list_projects). One level deep; git checkouts only.")
                .font(CSFont.ui(11.5))
                .lineSpacing(2)
                .foregroundStyle(CSColor.textMutedAlt)
                .padding(.top, 8)

            VStack(spacing: 8) {
                ForEach(rows.indices, id: \.self) { index in
                    rootRow(index: index)
                }
            }
            .padding(.top, 12)

            HStack(spacing: 10) {
                Button {
                    rows.append("")
                } label: {
                    Label("Add root", systemImage: "plus")
                        .font(CSFont.ui(12, .semibold))
                }
                .buttonStyle(.plain)
                .foregroundStyle(CSColor.textBody)

                Spacer()

                Button {
                    model.setAgentWorkspaceRoots(rows)
                    syncFromModel()
                } label: {
                    Text("Save roots")
                        .font(CSFont.ui(12, .semibold))
                        .foregroundStyle(isDirty ? CSColor.textHigh : CSColor.textFaint)
                }
                .buttonStyle(.plain)
                .disabled(!isDirty)
            }
            .padding(.top, 12)
        }
        .onAppear {
            guard !loaded else { return }
            loaded = true
            syncFromModel()
        }
    }

    private func rootRow(index: Int) -> some View {
        HStack(spacing: 10) {
            existsDot(for: rows[index])
            TextField("~/Git", text: Binding(
                get: { index < rows.count ? rows[index] : "" },
                set: { if index < rows.count { rows[index] = $0 } }
            ))
            .textFieldStyle(.plain)
            .font(CSFont.mono(12, .regular))
            .foregroundStyle(CSColor.textBody)
            .frame(maxWidth: .infinity, alignment: .leading)

            Button {
                rows.remove(at: index)
            } label: {
                CSIconView(icon: .remove, size: 13, weight: .semibold, color: CSColor.textFaint)
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 11)
        .padding(.vertical, 9)
        .background(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                .fill(CSColor.surfaceRaised(0.03))
        )
        .overlay(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
        )
    }

    /// Green when the (tilde-expanded) path is an existing directory, amber
    /// otherwise — the tool will silently skip a root that does not resolve.
    private func existsDot(for path: String) -> some View {
        let trimmed = path.trimmingCharacters(in: .whitespaces)
        let valid = Self.directoryExists(trimmed)
        return Circle()
            .fill(valid ? CSColor.oliveLight : CSColor.amber)
            .frame(width: 7, height: 7)
    }

    private func syncFromModel() {
        rows = model.agentWorkspaceRoots
        if rows.isEmpty { rows = ["~/Git"] }
    }

    private func cleaned(_ input: [String]) -> [String] {
        input
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
    }

    private static func directoryExists(_ path: String) -> Bool {
        guard !path.isEmpty else { return false }
        let expanded = (path as NSString).expandingTildeInPath
        var isDir: ObjCBool = false
        let exists = FileManager.default.fileExists(atPath: expanded, isDirectory: &isDir)
        return exists && isDir.boolValue
    }
}

// MARK: - Runtime row

private struct RuntimeRow: View {
    enum Trailing {
        case none
        case dot(Color)
        case text(String, Color)
    }

    let key: String
    let value: String
    var tint: Bool = false
    var mono: Bool = false
    var trailing: Trailing = .none

    var body: some View {
        HStack(spacing: 12) {
            Text(key)
                .font(CSFont.mono(12, .medium))
                .foregroundStyle(CSColor.textMutedAlt)
                .frame(width: 160, alignment: .leading)
            Text(value)
                .font(mono ? CSFont.mono(12.5, .semibold) : CSFont.ui(12.5, .semibold))
                .foregroundStyle(mono ? CSColor.textBodyAlt : CSColor.textHigh)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: .infinity, alignment: .leading)
            trailingView
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 13)
        .background(tint ? CSColor.surfaceRaised(0.02) : Color.clear)
    }

    @ViewBuilder
    private var trailingView: some View {
        switch trailing {
        case .none:
            EmptyView()
        case .dot(let color):
            Circle().fill(color).frame(width: 7, height: 7)
        case .text(let label, let color):
            Text(label)
                .font(CSFont.mono(10, .semibold))
                .foregroundStyle(color)
        }
    }
}

// MARK: - Permission matrix cell

private struct PermissionMatrixCell: View {
    let kind: PermissionKind
    let state: PermissionState

    private var granted: Bool { state.isGranted }
    private var accent: Color { granted ? CSColor.olive : CSColor.terracotta }
    private var accentLight: Color { granted ? CSColor.oliveLight : CSColor.terracottaLight }

    var body: some View {
        HStack(spacing: 10) {
            CSIconView(icon: granted ? .success : .warning, size: 11, weight: .semibold, color: accentLight)
            Text(kind.rawValue)
                .font(CSFont.ui(12.5, .medium))
                .foregroundStyle(CSColor.textBodyAlt)
                .frame(maxWidth: .infinity, alignment: .leading)
            Text(granted ? "granted" : state.label)
                .font(CSFont.mono(10, .semibold))
                .foregroundStyle(accentLight)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(accent.opacity(0.08))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(accent.opacity(0.2), lineWidth: 1)
        )
        .contentShape(Rectangle())
        .onTapGesture { if !granted { kind.openSystemSettings() } }
    }
}

// MARK: - Reset app data (destructive privacy action)

/// Danger-zone control at the foot of the Engine panel: a two-step, destructive
/// "Reset app data" flow. The checkbox opts into removing Keychain API keys too;
/// the button arms a confirmation alert that spells out exactly what disappears
/// before the wipe + relaunch runs.
private struct ResetAppDataSection: View {
    @ObservedObject var model: SettingsViewModel
    @State private var includeKeys = false
    @State private var confirming = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SettingsSectionLabel("Reset app data")

            Text("Permanently delete all local codescribe data on this Mac: "
                + "conversation history, transcription history, logs, preferences, "
                + "and MCP configuration. This cannot be undone.")
                .font(CSFont.mono(11, .medium))
                .foregroundStyle(CSColor.textFaint)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 5)

            Toggle(isOn: $includeKeys) {
                Text("Also remove API keys from Keychain")
                    .font(CSFont.ui(12.5, .medium))
                    .foregroundStyle(CSColor.textBody)
            }
            .toggleStyle(.checkbox)
            .padding(.top, 13)

            Button {
                confirming = true
            } label: {
                Text("Reset app data…")
                    .font(CSFont.ui(12, .semibold))
                    .foregroundStyle(CSColor.terracottaLight)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 8)
                    .background(
                        RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                            .fill(CSColor.terracotta.opacity(0.14))
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                            .strokeBorder(CSColor.terracotta.opacity(0.30), lineWidth: 1)
                    )
            }
            .buttonStyle(.plain)
            .padding(.top, 13)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 16)
        .padding(.vertical, 16)
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(CSColor.terracotta.opacity(0.04))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(CSColor.terracotta.opacity(0.16), lineWidth: 1)
        )
        .alert("Reset app data?", isPresented: $confirming) {
            Button("Cancel", role: .cancel) {}
            Button("Reset & Relaunch", role: .destructive) {
                model.resetAppData(includeKeys: includeKeys)
            }
        } message: {
            Text(confirmMessage)
        }
    }

    private var confirmMessage: String {
        var text = "This permanently deletes conversation history, transcription "
            + "history, logs, preferences, and MCP configuration."
        if includeKeys {
            text += " Your API keys will also be removed from the Keychain."
        }
        text += " codescribe will then relaunch as a fresh install."
        return text
    }
}

// MARK: - Shared section label (mono, muted, wide tracking) — used by all panels.

struct SettingsSectionLabel: View {
    let text: String
    init(_ text: String) { self.text = text }
    var body: some View {
        Text(text.uppercased())
            .font(CSFont.mono(12, .semibold))
            .tracking(0.5)
            .foregroundStyle(CSColor.textMuted)
    }
}

#if DEBUG
#Preview("Engine panel") {
    ScrollView { EnginePanel(model: .preview(.engine)) }
        .frame(width: 720, height: 620)
        .background(SettingsView.windowGradient)
        .preferredColorScheme(.dark)
}
#endif
