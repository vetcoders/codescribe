import AppKit
import SwiftUI

let defaultTranscriptTagTemplate = "<codescribe mode=\"{mode}\" lang=\"{lang}\">\n{text}\n</codescribe>"
let transcriptTagTemplatePlaceholders = ["{mode}", "{lang}", "{text}", "{conf}", "{flags}"]

func transcriptTagTemplatePreview(
    _ template: String,
    mode: String = "dictation",
    lang: String = "pl",
    text: String = "…",
    conf: String = "medium",
    flags: String = "possible_hallucination_logprob"
) -> String {
    var rendered = template
        .replacingOccurrences(of: "{mode}", with: mode)
        .replacingOccurrences(of: "{lang}", with: lang)
        .replacingOccurrences(of: "{conf}", with: conf)
        .replacingOccurrences(of: "{flags}", with: flags)
    if rendered.contains("{text}") {
        return rendered.replacingOccurrences(of: "{text}", with: text)
    }
    if !rendered.isEmpty, !rendered.hasSuffix("\n") {
        rendered.append("\n")
    }
    rendered.append(text)
    return rendered
}

func transcriptTagTemplateAppendWarning(_ template: String) -> String? {
    template.contains("{text}")
        ? nil
        : "Missing {text}; delivered transcript will be appended after the template."
}

/// Runtime serving truth for the Active STT row (not configured preference).
struct LastServingVerdict: Equatable {
    let engine: String
    let routingMode: String
    let disposition: String?
    let fallbackUsed: Bool
}

/// Format Active STT from the last serving verdict. Config projection is forbidden.
func formatActiveSTT(lastServing: LastServingVerdict?) -> String {
    guard let verdict = lastServing else {
        return "Not yet served"
    }
    let engine: String
    switch verdict.engine {
    case "local_apple":
        engine = "Apple on-device"
    case "local_whisper":
        engine = verdict.fallbackUsed ? "Whisper (fallback)" : "Whisper"
    case "streaming_whisper":
        engine = "Streaming Whisper"
    case "cloud_stt":
        engine = "Cloud"
    default:
        engine = verdict.engine.isEmpty ? "Unknown" : verdict.engine
    }
    let mode: String
    switch verdict.routingMode.lowercased() {
    case "always":
        mode = "Always final pass"
    case "off":
        mode = "Off final pass"
    default:
        mode = "Smart final pass"
    }
    if let disposition = verdict.disposition, !disposition.isEmpty {
        return "\(engine) · \(mode) · \(disposition)"
    }
    return "\(engine) · \(mode)"
}

enum SettingsSectionAvailability: Equatable {
    case available
    case hidden
}

enum FormattingPolicyOption: String, CaseIterable, Identifiable {
    case off
    case correction
    case smart
    case max

    var id: String { rawValue }
    var visibleName: String { rawValue.capitalized }

    init?(storedValue: String?) {
        switch storedValue {
        case "off", "raw": self = .off
        case "correction", "medium", nil: self = .correction
        case "smart": self = .smart
        case "max", "creative": self = .max
        default: return nil
        }
    }

    static let editablePrompts: [Self] = [.correction, .smart, .max]

    /// Next level in the tray's cycling control: Off → Correction → Smart → Max → Off.
    var next: Self {
        let all = Self.allCases
        let index = all.firstIndex(of: self) ?? all.startIndex
        return all[(index + 1) % all.count]
    }
}

enum HoldBadgeOption: CaseIterable, Identifiable, Equatable {
    case off
    case four
    case eight
    case twelve

    var id: String { visibleName }
    var visibleName: String {
        guard let size else { return "Off" }
        return "\(size)px"
    }
    var size: UInt32? {
        switch self {
        case .off: return nil
        case .four: return 4
        case .eight: return 8
        case .twelve: return 12
        }
    }

    init(indicatorEnabled: Bool, size: UInt32) {
        guard indicatorEnabled else {
            self = .off
            return
        }
        switch size {
        case 4: self = .four
        case 8: self = .eight
        default: self = .twelve
        }
    }

    var next: Self {
        let all = Self.allCases
        let index = all.firstIndex(of: self) ?? all.startIndex
        return all[(index + 1) % all.count]
    }
}

/// Deferred-insert delivery chord (`CODESCRIBE_DEFERRED_INSERT_SHORTCUT`).
/// Mirrors core `DeferredInsertShortcut` (core/config/types.rs) — the same
/// closed set, the same wire ids, the same modifier-glyph labels. Disabled is
/// the product default: the CGEventTap is listen-only, so a host app bound to
/// the same chord would see BOTH actions fire (types.rs, review P1-04).
enum DeferredInsertShortcutOption: String, CaseIterable, Identifiable, Equatable {
    case disabled = "disabled"
    case commandOptionV = "command_option_v"
    case commandShiftV = "command_shift_v"
    case commandControlV = "command_control_v"

    var id: String { rawValue }

    /// Canonical wire identifier persisted through `update_config` — must stay
    /// lockstep with `DeferredInsertShortcut::wire_id()`.
    var wireId: String { rawValue }

    /// Reads the canonical bridge value defensively. The core always emits one
    /// of these four ids, while an unknown legacy/corrupt value renders as the
    /// safe opt-in default instead of causing a passive write.
    init(wireId: String) {
        self = Self(rawValue: wireId) ?? .disabled
    }

    /// Chord rendered with macOS modifier glyphs (matches the Rust `label()`).
    var visibleName: String {
        switch self {
        case .disabled: return "Disabled"
        case .commandOptionV: return "⌘⌥V"
        case .commandShiftV: return "⌘⇧V"
        case .commandControlV: return "⌘⌃V"
        }
    }
}

/// Panel a rail section routes to. `SettingsView`'s detail switch consumes this
/// map exhaustively, so routing stays testable without rendering.
enum SettingsPanelDestination: Equatable {
    case creator
    case shortcuts
    case providers
    case agent
    case prompts
    case dictation
    case audio
    case dictionary
    case license
    case user
}

/// Testable ownership contract for the two settings surfaces that used to be
/// mixed together. This is UI metadata only; it never participates in storage.
enum SettingsPanelCapability: Hashable {
    case apiKeys
    case llmLanes
    case workspaceRoots
    case agentStatus
    case mcpServers
    case toolPermissions
}

// Every rail section declares its product truth explicitly. The raw value is a
// stable route id (focus targets, SwiftUI identity); `title` is the ONE owner of
// the user-visible name — rail, eyebrows, help copy, and dictionary-supporting
// copy all derive from it, so renaming a tab is a one-line change.
enum SettingsSection: String, CaseIterable, Identifiable {
    case creator
    case shortcuts
    case keys
    case agent
    case prompts
    case engine
    case audio
    case voiceLab
    case license
    case user

    var id: String { rawValue }

    var title: String {
        switch self {
        case .creator: return "Creator"
        case .shortcuts: return "Hotkeys"
        case .keys: return "Providers"
        case .agent: return "Agent"
        case .prompts: return "Prompts"
        case .engine: return "Dictation"
        case .audio: return "Audio"
        case .voiceLab: return "Dictionary"
        case .license: return "License"
        case .user: return "User"
        }
    }

    var destination: SettingsPanelDestination {
        switch self {
        case .creator: return .creator
        case .shortcuts: return .shortcuts
        case .keys: return .providers
        case .agent: return .agent
        case .prompts: return .prompts
        case .engine: return .dictation
        case .audio: return .audio
        case .voiceLab: return .dictionary
        case .license: return .license
        case .user: return .user
        }
    }

    var availability: SettingsSectionAvailability {
        switch self {
        case .creator, .shortcuts, .keys, .agent, .prompts, .engine, .audio, .voiceLab, .license, .user:
            return .available
        }
    }

    var isInteractive: Bool { availability == .available }

    /// Sidebar grouping. A flat ten-item list forces the user to read every row;
    /// native sidebars carry `Section` headers for free, so the rail states what
    /// each area is FOR instead of relying on the reader's memory.
    var group: SettingsSectionGroup {
        switch self {
        case .creator, .shortcuts, .audio: return .setup
        case .keys, .agent, .prompts, .engine, .voiceLab: return .intelligence
        case .license, .user: return .account
        }
    }

    /// SF Symbol for the sidebar row. System symbols follow the user's theme,
    /// accent and accessibility sizes; the previous hand-drawn 7pt dots did not.
    var symbol: String {
        switch self {
        case .creator: return "wand.and.stars"
        case .shortcuts: return "keyboard"
        case .keys: return "key.horizontal"
        case .agent: return "cpu"
        case .prompts: return "text.bubble"
        case .engine: return "waveform"
        case .audio: return "mic"
        case .voiceLab: return "character.book.closed"
        case .license: return "checkmark.seal"
        case .user: return "person.crop.circle"
        }
    }

    /// Extra terms the settings search matches beyond the visible title, so the
    /// user can look for what a panel DOES ("api key", "mikrofon") instead of
    /// having to guess the tab name.
    var searchKeywords: [String] {
        switch self {
        case .creator: return ["setup", "onboarding", "permissions", "quick start", "language"]
        case .shortcuts: return ["hotkey", "keyboard", "shortcut", "trigger", "hold", "toggle"]
        case .keys: return ["api key", "provider", "openai", "anthropic", "endpoint", "model", "token"]
        case .agent: return ["mcp", "tools", "workspace", "permissions", "server"]
        case .prompts: return ["system prompt", "persona", "assistive", "instructions"]
        case .engine: return ["stt", "whisper", "apple", "speech", "transcription", "final pass"]
        case .audio: return ["microphone", "mikrofon", "input", "device", "levels"]
        case .voiceLab: return ["lexicon", "dictionary", "vocabulary", "corrections", "słownik"]
        case .license: return ["subscription", "activation", "trial", "billing"]
        case .user: return ["account", "profile", "sign in", "identity"]
        }
    }

    /// Sections a query should reveal. An empty query keeps the full rail; a
    /// query matches the visible title first, then the keyword aliases. Pure so
    /// the search contract is testable without rendering the window.
    static func matching(query: String) -> [SettingsSection] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let visible = allCases.filter { $0.availability != .hidden }
        guard !needle.isEmpty else { return visible }
        return visible.filter { section in
            section.title.lowercased().contains(needle)
                || section.searchKeywords.contains { $0.contains(needle) }
        }
    }
}

/// A page inside a section. Long panels (Agent stacks five independent
/// subsystems; Dictation grew four hand-rolled collapsibles) become one page per
/// subsystem instead of one endless scroll.
///
/// Pages live in the SIDEBAR TREE rather than in tabs on purpose: a tab is
/// invisible to the settings search and to deep links, so "mcp" or a health
/// footer pointing at a broken lane could never land on it. As tree children
/// they are addressable by exactly the same mechanisms as a top-level section.
enum SettingsPage: String, CaseIterable, Identifiable {
    // Agent
    case agentLanes
    case agentWorkspace
    case agentStatus
    case agentTools
    case agentMcp
    // Prompts — one page per prompt file; the stacked editor was four
    // TextEditors in one scroll ("scrollowany potworek", operator 2026-08-09).
    case promptCorrection
    case promptSmart
    case promptMax
    case promptAssistive

    var id: String { rawValue }

    var section: SettingsSection {
        switch self {
        case .agentLanes, .agentWorkspace, .agentStatus, .agentTools, .agentMcp:
            return .agent
        case .promptCorrection, .promptSmart, .promptMax, .promptAssistive:
            return .prompts
        }
    }

    var title: String {
        switch self {
        case .agentLanes: return "LLM lanes"
        case .agentWorkspace: return "Workspace roots"
        case .agentStatus: return "Capabilities"
        case .agentTools: return "Tool permissions"
        case .agentMcp: return "MCP servers"
        case .promptCorrection: return "Correction"
        case .promptSmart: return "Smart"
        case .promptMax: return "Max"
        case .promptAssistive: return "Assistive"
        }
    }

    var symbol: String {
        switch self {
        case .agentLanes: return "arrow.triangle.branch"
        case .agentWorkspace: return "folder"
        case .agentStatus: return "checklist"
        case .agentTools: return "lock.shield"
        case .agentMcp: return "server.rack"
        case .promptCorrection: return "text.badge.checkmark"
        case .promptSmart: return "wand.and.stars"
        case .promptMax: return "text.alignleft"
        case .promptAssistive: return "person.wave.2"
        }
    }

    var searchKeywords: [String] {
        switch self {
        case .agentLanes: return ["provider", "model", "endpoint", "assistive", "formatting"]
        case .agentWorkspace: return ["roots", "directory", "repo", "path"]
        case .agentStatus: return ["capability", "native", "enhanced", "readiness"]
        case .agentTools: return ["permission", "allow", "ask", "deny", "tool"]
        case .agentMcp: return ["mcp", "server", "stdio", "transport"]
        case .promptCorrection: return ["prompt", "formatting", "correction", "formatting.txt"]
        case .promptSmart: return ["prompt", "smart", "formatting-smart"]
        case .promptMax: return ["prompt", "max", "prose", "formatting-max"]
        case .promptAssistive: return ["prompt", "assistive", "assistant", "system"]
        }
    }

    static func pages(in section: SettingsSection) -> [SettingsPage] {
        allCases.filter { $0.section == section }
    }

    /// Pages a query should surface, so search reaches inside a long section
    /// instead of stopping at its title.
    static func matching(query: String) -> [SettingsPage] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !needle.isEmpty else { return allCases }
        return allCases.filter { page in
            page.title.lowercased().contains(needle)
                || page.searchKeywords.contains { $0.contains(needle) }
        }
    }
}

/// One selectable row in the rail: a section, or a page inside it.
enum SettingsRoute: Hashable, Identifiable {
    case section(SettingsSection)
    case page(SettingsPage)

    var id: String {
        switch self {
        case .section(let section): return "section:\(section.rawValue)"
        case .page(let page): return "page:\(page.rawValue)"
        }
    }

    var section: SettingsSection {
        switch self {
        case .section(let section): return section
        case .page(let page): return page.section
        }
    }

    var page: SettingsPage? {
        switch self {
        case .section: return nil
        case .page(let page): return page
        }
    }
}

/// Sidebar sections. Order is the rail's top-to-bottom order.
enum SettingsSectionGroup: String, CaseIterable, Identifiable {
    case setup
    case intelligence
    case account

    var id: String { rawValue }

    var title: String {
        switch self {
        case .setup: return "Setup"
        case .intelligence: return "Intelligence"
        case .account: return "Account"
        }
    }
}

enum SettingsKeyState: Equatable {
    case available
    case missing
    case unknown
}

enum SettingsHealthLevel: Equatable {
    case healthy
    case degraded
    case offline
    case unknown
}

struct SettingsHealthState: Equatable {
    let level: SettingsHealthLevel
    let message: String
    let targetSection: SettingsSection?
}

/// Pure aggregate used by the rail footer and its XCTest matrix. Known failures
/// beat unknown inputs so the footer never hides a concrete problem behind a
/// muted "unknown" state.
func healthState(
    stt: Bool?,
    keys: SettingsKeyState,
    agent: Bool?
) -> SettingsHealthState {
    if stt == false {
        return SettingsHealthState(
            level: .offline,
            message: "speech engine: unavailable",
            targetSection: .engine
        )
    }
    if keys == .missing {
        return SettingsHealthState(
            level: .degraded,
            message: "assistive lane: no key",
            targetSection: .keys
        )
    }
    if agent == false {
        return SettingsHealthState(
            level: .offline,
            message: "assistive lane: not ready",
            targetSection: .engine
        )
    }
    if stt == nil || keys == .unknown || agent == nil {
        return SettingsHealthState(
            level: .unknown,
            message: "system health: unknown",
            targetSection: .engine
        )
    }
    return SettingsHealthState(
        level: .healthy,
        message: "systems ready",
        targetSection: nil
    )
}

struct AppBuildInfo: Equatable {
    let version: String
    let build: String
    let commit: String
    let builtAt: String

    static func current(bundle: Bundle = .main) -> AppBuildInfo {
        let info = bundle.infoDictionary ?? [:]
        return AppBuildInfo(
            version: info["CFBundleShortVersionString"] as? String ?? "unknown",
            build: info["CFBundleVersion"] as? String ?? "unknown",
            commit: info["CSBuildCommit"] as? String ?? "unknown",
            builtAt: info["CSBuiltAt"] as? String ?? "unknown"
        )
    }
}

func resetConfirmationMatches(_ text: String) -> Bool {
    text == "RESET"
}

/// Rust marks failures that occurred after the first irreversible data move.
/// Those are errors for audit/recovery, but staying in the current process is
/// no longer safe because its app-data plane is deliberately latched.
func resetFailureRequiresRelaunch(_ description: String) -> Bool {
    description.contains("CODESCRIBE_RESET_RELAUNCH_REQUIRED")
}

func resetImpactSummary(_ preview: CsResetPreview) -> String {
    let recordings = preview.audioFiles == 1 ? "recording" : "recordings"
    let days = preview.transcriptDays == 1 ? "day" : "days"
    let threads = preview.threads == 1 ? "thread" : "threads"
    let megabytes = Double(preview.totalBytes) / 1_048_576.0
    return "\(preview.audioFiles) \(recordings) from \(preview.transcriptDays) \(days), "
        + "\(preview.threads) \(threads) (\(String(format: "%.1f", megabytes)) MB)"
}

/// One-shot deep-link target for the Settings window. A surface outside Settings
/// (e.g. the onboarding wizard routing the user to MCP setup) sets this before
/// opening or focusing the window; `SettingsView` consumes it once on appear and
/// whenever an already-open window receives a new target. Nil means "open on the
/// last/default section".
@MainActor
enum SettingsDeepLink {
    static let pendingSectionDidChange = Notification.Name("codescribe.settingsDeepLink.pendingSectionDidChange")
    static let agentConfigurationSection: SettingsSection = .agent

    static var pendingSection: SettingsSection? {
        didSet {
            guard pendingSection != nil else { return }
            NotificationCenter.default.post(name: pendingSectionDidChange, object: nil)
        }
    }

    /// Take the pending target (if any), clearing it so a later open is unaffected.
    static func consume() -> SettingsSection? {
        guard let target = pendingSection else { return nil }
        pendingSection = nil
        return target
    }
}

enum LLMLane: String, CaseIterable, Identifiable {
    case assistive
    case formatting
    case main

    var id: String { rawValue }

    var bridgeLane: CsLlmLane {
        switch self {
        case .assistive: return .assistive
        case .formatting: return .formatting
        case .main: return .main
        }
    }

    var title: String {
        switch self {
        case .assistive: return "Assistive"
        case .formatting: return "Formatting"
        case .main: return "Main"
        }
    }

    var subtitle: String {
        switch self {
        case .assistive: return "Agent and voice-assistant requests"
        case .formatting: return "Transcript cleanup and formatting"
        case .main: return "Default LLM fallback lane"
        }
    }

    var endpointKey: String {
        switch self {
        case .assistive: return "LLM_ASSISTIVE_ENDPOINT"
        case .formatting: return "LLM_FORMATTING_ENDPOINT"
        case .main: return "LLM_ENDPOINT"
        }
    }

    var modelKey: String {
        switch self {
        case .assistive: return "LLM_ASSISTIVE_MODEL"
        case .formatting: return "LLM_FORMATTING_MODEL"
        case .main: return "LLM_MODEL"
        }
    }

    var endpointPath: WritableKeyPath<CsSettings, String?> {
        switch self {
        case .assistive: return \CsSettings.llmAssistiveEndpoint
        case .formatting: return \CsSettings.llmFormattingEndpoint
        case .main: return \CsSettings.llmEndpoint
        }
    }

    var modelPath: WritableKeyPath<CsSettings, String?> {
        switch self {
        case .assistive: return \CsSettings.llmAssistiveModel
        case .formatting: return \CsSettings.llmFormattingModel
        case .main: return \CsSettings.llmModel
        }
    }
}

/// One read model for a request lane. Both Settings panels consume this snapshot,
/// so provider resolution, model discovery, and manual-entry rules cannot drift.
struct LLMLaneModel {
    let lane: LLMLane
    let providerId: String
    let provider: CsProviderOption?
    let resolvedEndpoint: String
    let configuredModel: String
    let resolvedModel: String
    let discoveryEndpoint: String
    let discovery: CsModelDiscovery

    var modelOptions: [CsModelOption] { discovery.models }

    var manualModelReason: String? {
        guard providerId == "openai-responses" else { return nil }
        guard URL(string: resolvedEndpoint)?.host?.lowercased() == "api.openai.com" else {
            return "Custom endpoint — enter its model ID manually"
        }
        guard lane == .assistive || resolvedEndpoint == discoveryEndpoint else {
            return "Endpoint differs from OpenAI discovery — enter its model ID manually"
        }
        return nil
    }

    var usesDiscoveredPicker: Bool {
        manualModelReason == nil && !modelOptions.isEmpty && discovery.status == "fresh"
    }

    var discoveryDescription: String {
        if let manualModelReason { return manualModelReason }
        switch discovery.status {
        case "fresh":
            let count = modelOptions.count
            return count == 0
                ? "no models returned by provider"
                : "\(count) \(count == 1 ? "model" : "models") discovered from provider"
        case "cached":
            if let message = discovery.message, !message.isEmpty {
                return "using cached models — \(message)"
            }
            return "using cached models"
        case "no_key": return "Add API key to discover models"
        case "loading": return "discovering models…"
        default:
            if let message = discovery.message, !message.isEmpty {
                return "model discovery failed — \(message)"
            }
            return "model discovery failed"
        }
    }
}

// MARK: - Preview timing domain (Dictation-owned; model + panel both consume)

enum PreviewTimingPreset: String, CaseIterable, Identifiable, Equatable {
    case smooth = "Smooth"
    case snappy = "Snappy"
    case relaxed = "Relaxed"
    case off = "Off"
    case custom = "Custom"

    var id: String { rawValue }
}

struct PreviewTimingValues: Equatable {
    let bufferDelayMs: UInt64
    let typingCps: Float
    let emitWordsMax: UInt64
    let interimSeconds: Float

    // Source: operator-tested C5b values (2026-06-11). Smooth is the
    // recommended default; Snappy/Relaxed retain the original values without
    // the optional +/-20% retuning because all are inside current clamps.
    static let smooth = PreviewTimingValues(
        bufferDelayMs: 1038,
        typingCps: 10.6,
        emitWordsMax: 5,
        interimSeconds: 8.0
    )
    static let snappy = PreviewTimingValues(
        bufferDelayMs: 350,
        typingCps: 28.0,
        emitWordsMax: 3,
        interimSeconds: 4.0
    )
    static let relaxed = PreviewTimingValues(
        bufferDelayMs: 1500,
        typingCps: 8.0,
        emitWordsMax: 8,
        interimSeconds: 8.0
    )
}

struct PreviewTimingConfiguration: Equatable {
    let overlayEnabled: Bool
    let values: PreviewTimingValues
}

func presetValues(_ preset: PreviewTimingPreset) -> PreviewTimingValues? {
    switch preset {
    case .smooth: return .smooth
    case .snappy: return .snappy
    case .relaxed: return .relaxed
    case .off, .custom: return nil
    }
}

func detectPreset(_ configuration: PreviewTimingConfiguration) -> PreviewTimingPreset {
    guard configuration.overlayEnabled else { return .off }
    for preset in [PreviewTimingPreset.smooth, .snappy, .relaxed] {
        guard let values = presetValues(preset) else { continue }
        let current = configuration.values
        let bufferClose = current.bufferDelayMs.absDiff(values.bufferDelayMs) <= 10
        let cpsClose = abs(current.typingCps - values.typingCps) <= 0.15
        let wordsMatch = current.emitWordsMax == values.emitWordsMax
        let interimClose = abs(current.interimSeconds - values.interimSeconds) <= 0.15
        if bufferClose, cpsClose, wordsMatch, interimClose {
            return preset
        }
    }
    return .custom
}

private extension UInt64 {
    func absDiff(_ other: UInt64) -> UInt64 {
        self >= other ? self - other : other - self
    }
}

/// Clears the app's preferences domain and relaunches a fresh instance. Used by
/// the destructive "Reset app data" flow so restored window frames / SwiftUI scene
/// state do not survive the wipe. The relaunch is deferred via a detached `open`
/// so this instance fully exits first — otherwise AppDelegate's duplicate-instance
/// guard would terminate the freshly-launched copy.
@MainActor
enum AppRelaunch {
    static func clearDefaultsAndRelaunch() {
        if let bundleId = Bundle.main.bundleIdentifier {
            UserDefaults.standard.removePersistentDomain(forName: bundleId)
            UserDefaults.standard.synchronize()
        }
        let bundlePath = Bundle.main.bundlePath
        let task = Process()
        task.launchPath = "/bin/sh"
        // `$0` carries the bundle path as a positional arg so it is safely quoted,
        // never interpolated into the script string.
        task.arguments = ["-c", "sleep 1; open \"$0\"", bundlePath]
        try? task.run()
        NSApp.terminate(nil)
    }
}

private struct BackgroundSettingsEngine: @unchecked Sendable {
    let engine: SettingsEngine
}

extension CsWhisperModelStatus {
    /// Placeholder for canvas / engine-less previews (no network, no disk probe).
    static let sampleUnavailable = CsWhisperModelStatus(
        available: false,
        embedded: false,
        path: nil,
        modelId: "whisper-large-v3-turbo",
        repo: "mlx-community/whisper-large-v3-turbo",
        sizeHint: "~1.6 GB"
    )
}

/// Bridges UniFFI download callbacks onto the main-actor SettingsViewModel.
final class WhisperDownloadProgressSink: CsWhisperDownloadListener, @unchecked Sendable {
    weak var model: SettingsViewModel?

    init(model: SettingsViewModel) {
        self.model = model
    }

    func onProgress(file: String, bytesDone: UInt64, bytesTotal: Int64) {
        let fraction: Double?
        if bytesTotal > 0 {
            fraction = min(1.0, Double(bytesDone) / Double(bytesTotal))
        } else {
            fraction = nil
        }
        let mbDone = Double(bytesDone) / 1_048_576.0
        let detail: String
        if bytesTotal > 0 {
            let mbTotal = Double(bytesTotal) / 1_048_576.0
            detail = String(format: "%@ · %.0f / %.0f MB", file, mbDone, mbTotal)
        } else {
            detail = String(format: "%@ · %.0f MB", file, mbDone)
        }
        DispatchQueue.main.async { [weak self] in
            self?.model?.applyWhisperDownloadProgress(detail: detail, fraction: fraction)
        }
    }

    func onComplete(path: String) {
        DispatchQueue.main.async { [weak self] in
            self?.model?.applyWhisperDownloadProgress(
                detail: "Saved · \(path)",
                fraction: 1.0
            )
        }
    }
}

/// Quick-start actions from the Creator panel's cards. Navigation cases route
/// the settings rail; `openOverlay` starts a real dictation session through an
/// injectable seam so the cards are never inert decorations again
/// (UI_DIVERGENCE_AUDIT pkt 4 — fake UX).
enum SettingsQuickStartAction: String, CaseIterable {
    case testMic
    case openOverlay
    case tuneShortcuts
}

@MainActor
final class SettingsViewModel: ObservableObject {
    @Published var section: SettingsSection = .creator
    /// Page within `section`, when that section is paginated. Always kept
    /// consistent with `section` by the `select` overloads — never written raw.
    @Published private(set) var page: SettingsPage?

    /// Dictation seam for the "Open overlay" quick-start card. Defaulted to the
    /// live tray toggle but only dereferenced on click, so unit tests can inject
    /// a spy without ever waking `AppModel.shared`.
    var onQuickStartDictation: () -> Void = { AppModel.shared.tray.toggleDictation() }

    func performQuickStart(_ action: SettingsQuickStartAction) {
        switch action {
        case .testMic: section = .audio
        case .tuneShortcuts: section = .shortcuts
        case .openOverlay: onQuickStartDictation()
        }
    }

    @Published private(set) var permissions: PermissionSnapshot
    @Published private(set) var settings: CsSettings
    @Published private(set) var keyStatus: CsKeyStatus
    @Published private(set) var providers: [CsProviderOption]
    @Published private var modelDiscoveries: [String: CsModelDiscovery] = [:]
    @Published private(set) var configDir: String
    @Published private(set) var needsOnboarding: Bool
    @Published private(set) var agentReadiness: CsAgenticReadiness
    @Published private(set) var mcpStatus: CsMcpStatusReport
    /// Native / enhanced / unavailable rows from `CodescribeAgentStatus.capabilityMatrix()`.
    @Published private(set) var capabilityMatrix: [CsCapabilityRow] = []
    @Published private(set) var mcpServers: [CsMcpServer] = []
    @Published private(set) var mcpTestResults: [String: CsMcpTestResult] = [:]
    @Published private(set) var mcpTestPending: Set<String> = []
    @Published private(set) var toolCapabilities: [CsToolCapability] = []
    @Published private(set) var permissionPolicy: CsPermissionPolicy = CsPermissionPolicy(
        defaultLevel: "ask",
        readOnlyDefault: "allow",
        sideEffectDefault: "ask",
        tools: [],
        servers: []
    )
    @Published private(set) var keyProbeResults: [String: CsApiKeyProbeResult] = [:]
    @Published private(set) var keyProbePending: Set<String> = []
    @Published private(set) var qualityRecords: [CsQualityRecord] = []
    @Published private(set) var customLexiconEntries: [CsLexiconEntry] = []
    @Published private(set) var voiceLabReadError: String?
    @Published private(set) var voiceLabEditPending: Set<String> = []
    @Published private(set) var voiceLabEditErrors: [String: String] = [:]
    /// Honest per-row save note: "Saved — N rules learned" or the zero/failed
    /// variants. The save itself succeeded whenever a note is present.
    @Published private(set) var voiceLabEditNotes: [String: String] = [:]
    @Published private(set) var voiceLabTeachPending: Bool = false
    @Published private(set) var voiceLabTeachMessage: String?
    @Published private(set) var audioInput: CsAudioInputSnapshot
    @Published private(set) var audioInputReadError: String?
    @Published private(set) var resetPreview: CsResetPreview
    @Published private(set) var licenseStatus: CsLicenseStatus
    /// Provider ids with a "Sign in with ChatGPT" flow in flight (browser open,
    /// local callback server listening). Guards double-clicks.
    @Published private(set) var accountLoginPending: Set<String> = []
    /// Last terminal outcome of an account login per provider ("timeout",
    /// "failed" …) — honest status for the row without raising a modal error.
    @Published private(set) var accountLoginNotices: [String: String] = [:]
    @Published var lastError: String?

    // MARK: - Local Whisper download (Settings → Dictation)

    /// Live availability of default Whisper weights (embedded or on disk).
    /// Named `localWhisperStatus` so it does not shadow UniFFI free functions
    /// `whisperModelStatus()` / `downloadWhisperModel(...)`.
    @Published private(set) var localWhisperStatus: CsWhisperModelStatus = .sampleUnavailable
    @Published private(set) var whisperDownloadInFlight = false
    /// Human status under the download control (file name + progress).
    @Published private(set) var whisperDownloadDetail: String?
    /// 0...1 when Content-Length is known; nil for indeterminate.
    @Published private(set) var whisperDownloadFraction: Double?
    private var whisperDownloadSink: WhisperDownloadProgressSink?

    // MARK: - Hotkeys (mode bindings)

    /// Persisted per-mode bindings as last read from disk.
    @Published private(set) var modeBindings: [CsModeBinding] = []
    /// The closed set of selectable gestures for the pickers.
    @Published private(set) var bindingOptions: [CsBindingOption] = []
    /// Editable copy the Shortcuts panel mutates before a save.
    @Published private(set) var draftBindings: [CsModeBinding] = []
    /// Conflicts for the CURRENT draft (recomputed on every edit).
    @Published private(set) var bindingConflicts: [CsHotkeyConflict] = []

    /// Build provenance comes from the running app bundle. The build pipeline
    /// writes all four fields in project.yml / scripts/build-app.sh.
    let buildInfo: AppBuildInfo
    var appVersion: String { buildInfo.version }

    private let engine: SettingsEngine?
    private let permissionProbe: PermissionProbing
    private let agentStatus: AgentStatusEngine?
    private let mcpAdmin: MCPAdminEngine?
    private let hotkeys: HotkeysEngine?
    private let licenseService: LicenseService
    private let laneTruthProvider: (CsLlmLane) -> CsLaneTruthSnapshot
    private var modelDiscoveryGenerations: [String: Int] = [:]
    private var assistiveModelEditGeneration = 0
    private var pendingAssistiveModelSelection: (
        providerId: String,
        modelEditGeneration: Int
    )?
    private var holdBadgeObserver: NSObjectProtocol?

    init(
        engine: SettingsEngine? = nil,
        permissionProbe: PermissionProbing = NativePermissionProbe(),
        agentStatus: AgentStatusEngine? = nil,
        mcpAdmin: MCPAdminEngine? = nil,
        hotkeys: HotkeysEngine? = nil,
        licenseService: LicenseService? = nil,
        buildInfo: AppBuildInfo = .current(),
        laneTruthProvider: @escaping (CsLlmLane) -> CsLaneTruthSnapshot = { lane in
            laneTruthSnapshot(lane: lane)
        },
        servingStatusProvider: @escaping () -> LastServingVerdict? = {
            guard let verdict = currentServingVerdict() else { return nil }
            return LastServingVerdict(
                engine: verdict.engine,
                routingMode: verdict.routingMode,
                disposition: verdict.disposition,
                fallbackUsed: verdict.fallbackUsed
            )
        }
    ) {
        self.engine = engine
        self.permissionProbe = permissionProbe
        self.agentStatus = agentStatus
        self.mcpAdmin = mcpAdmin
        self.hotkeys = hotkeys
        self.licenseService = licenseService ?? .preview
        self.buildInfo = buildInfo
        self.laneTruthProvider = laneTruthProvider
        self.servingStatusProvider = servingStatusProvider

        // Reading the settings snapshot is passive: it does not write config
        // or touch Keychain. Seed the picker from that same canonical snapshot
        // so reopening Shortcuts reflects the persisted chord before a user
        // changes anything.
        self.permissions = permissionProbe.snapshot()
        let initialSettings = engine?.loadSettings() ?? .sample
        self.settings = initialSettings
        self.deferredInsertShortcut = DeferredInsertShortcutOption(
            wireId: initialSettings.deferredInsertShortcut
        )
        self.keyStatus = .sampleAllSet
        self.providers = CsProviderOption.sampleProviders
        self.configDir = ""
        self.needsOnboarding = false
        self.agentReadiness = .sample
        self.mcpStatus = .sample
        self.capabilityMatrix = CsCapabilityRow.sampleMatrix
        self.voiceLabReadError = nil
        self.audioInput = .sample
        self.audioInputReadError = nil
        self.resetPreview = .sample
        self.licenseStatus = self.licenseService.status
        // K4: tray cycles arrive on the bus; reload Settings badge display.
        // Register after every stored property is initialized (Swift init order).
        holdBadgeObserver = NotificationCenter.default.addObserver(
            forName: ConfigChangeBus.holdBadgeDidChange,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.reloadHoldBadgeFromDisk()
            }
        }
    }

    deinit {
        if let holdBadgeObserver {
            NotificationCenter.default.removeObserver(holdBadgeObserver)
        }
    }

    /// Re-read live state (permissions can change while the window is open).
    func refresh() {
        permissions = permissionProbe.snapshot()
        refreshServingStatus()
        // A permission granted while Settings is open (e.g. via the checklist's
        // "Open System Settings") should bring hotkeys live without an app
        // restart. Idempotent bridge call — a no-op once the tap is already armed.
        hotkeys?.rearmAfterPermissionGrant()
        if let engine {
            applyLoadedSettings(engine.loadSettings())
            keyStatus = engine.keyStatus()
            providers = engine.availableProviders()
            configDir = engine.configDir()
            needsOnboarding = engine.shouldShowOnboarding()
            refreshModelDiscoveries(providerIds: [llmLane(.assistive).providerId, "openai-responses"])
            refreshVoiceLab()
            refreshAudioInput()
        }
        refreshWhisperModelStatus()
        refreshAgentStatus()
        reloadMcpServers()
        loadHotkeys()
        licenseService.refresh()
        licenseStatus = licenseService.status
    }

    var licenseError: String? { licenseService.lastError }

    @discardableResult
    func activateLicense(_ key: String) -> Bool {
        let activated = licenseService.activate(key)
        licenseStatus = licenseService.status
        return activated
    }

    func removeLicense() {
        licenseService.removeLicense()
        licenseStatus = licenseService.status
    }

    /// Re-probe Whisper install state (embedded / on-disk / missing).
    func refreshWhisperModelStatus() {
        // Live UniFFI path; sample mode (engine == nil in pure previews) keeps
        // the static placeholder so canvas previews stay offline-safe.
        guard engine != nil else { return }
        localWhisperStatus = whisperModelStatus()
    }

    /// Called from UniFFI download callbacks (main-queue hopped).
    fileprivate func applyWhisperDownloadProgress(detail: String, fraction: Double?) {
        whisperDownloadDetail = detail
        whisperDownloadFraction = fraction
    }

    /// Download default Whisper into `~/.codescribe/models/…` (idempotent).
    func startWhisperDownload() {
        guard engine != nil else { return }
        guard !whisperDownloadInFlight else { return }
        whisperDownloadInFlight = true
        whisperDownloadDetail = "Starting download…"
        whisperDownloadFraction = nil
        lastError = nil
        let sink = WhisperDownloadProgressSink(model: self)
        whisperDownloadSink = sink
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let status = try await downloadWhisperModel(listener: sink)
                self.localWhisperStatus = status
                self.whisperDownloadDetail = status.available
                    ? "Ready · \(status.path ?? status.modelId)"
                    : "Download finished but model still unavailable"
                self.whisperDownloadFraction = status.available ? 1.0 : nil
            } catch {
                self.lastError = String(describing: error)
                self.whisperDownloadDetail = "Download failed"
                self.whisperDownloadFraction = nil
            }
            self.whisperDownloadInFlight = false
            self.whisperDownloadSink = nil
        }
    }

    /// Re-probe just the agent substrate (readiness + MCP + capability matrix).
    /// Cheap on-disk reads; used by the Agent panel's "Refresh" action so
    /// re-checking MCP does not disturb the rest of the panel.
    func refreshAgentStatus() {
        guard let agentStatus else { return }
        agentReadiness = agentStatus.agenticReadiness()
        mcpStatus = agentStatus.mcpStatus()
        capabilityMatrix = agentStatus.capabilityMatrix()
    }

    // MARK: - Hotkeys (mode-binding editor)

    /// Re-read persisted bindings + the option catalog, then reset the editable
    /// draft to match disk and revalidate. A missing engine leaves the seeds.
    func loadHotkeys() {
        guard let hotkeys else { return }
        modeBindings = hotkeys.modeBindings()
        bindingOptions = hotkeys.availableBindings()
        draftBindings = modeBindings
        revalidateBindings()
    }

    /// Any blocking (reachability / system) conflict in the current draft.
    var hasBlockingBindingConflicts: Bool { bindingConflicts.contains { $0.blocking } }

    /// The draft differs from persisted state (something to save).
    var hasPendingBindingChanges: Bool {
        draftBindings.map(\.binding) != modeBindings.map(\.binding)
    }

    /// Save is allowed only for a changed, conflict-clean draft.
    var canSaveBindings: Bool { hasPendingBindingChanges && !hasBlockingBindingConflicts }

    /// Stage a binding change for one mode WITHOUT persisting, then re-validate so
    /// conflicts surface inline before the user commits.
    func editDraftBinding(mode: CsWorkMode, binding: CsShortcutBinding) {
        guard let index = draftBindings.firstIndex(where: { $0.mode == mode }) else { return }
        let label = bindingOptions.first { $0.binding == binding }?.label
            ?? draftBindings[index].bindingLabel
        draftBindings[index] = CsModeBinding(
            mode: mode,
            modeLabel: draftBindings[index].modeLabel,
            modeDescription: draftBindings[index].modeDescription,
            binding: binding,
            bindingLabel: label
        )
        revalidateBindings()
    }

    /// Recompute conflicts for the current draft via the revived shortcut registry.
    func revalidateBindings() {
        guard let hotkeys else {
            bindingConflicts = []
            return
        }
        bindingConflicts = hotkeys.validate(candidate: draftBindings)
    }

    /// Persist every changed mode through the core `set_mode_binding` contract
    /// (each write live-reloads the detector), then re-read disk truth. Guarded by
    /// `canSaveBindings`, so a conflicted or unchanged draft never writes.
    func saveBindings() {
        guard let hotkeys, canSaveBindings else { return }
        do {
            for draft in draftBindings {
                let current = modeBindings.first { $0.mode == draft.mode }
                if current?.binding != draft.binding {
                    try hotkeys.setModeBinding(mode: draft.mode, binding: draft.binding)
                }
            }
            loadHotkeys()
        } catch {
            lastError = String(describing: error)
        }
    }

    /// Reset all bindings to the built-in defaults and re-read.
    func resetBindingsToDefaults() {
        guard let hotkeys else { return }
        do {
            try hotkeys.resetToDefaults()
            loadHotkeys()
        } catch {
            lastError = String(describing: error)
        }
    }

    // MARK: - MCP server management (writes through the atomic config store)

    /// Re-read the configured MCP servers from `mcp.json`. A missing config is an
    /// empty list, not an error.
    func reloadMcpServers() {
        guard let mcpAdmin else { return }
        do {
            mcpServers = try mcpAdmin.listServers()
        } catch {
            lastError = String(describing: error)
            mcpServers = []
        }
    }

    // MARK: - Tool permissions (B2 gateway)

    enum PermissionDefaultKind {
        case global
        case readOnly
        case sideEffect
    }

    /// Reload durable policy + live capability list from the same registry the
    /// agent dispatcher builds.
    func reloadToolPermissions() {
        guard let mcpAdmin else { return }
        permissionPolicy = mcpAdmin.getPermissionPolicy()
        toolCapabilities = mcpAdmin.listToolCapabilities()
    }

    func setPermissionDefault(kind: PermissionDefaultKind, level: String) {
        guard let mcpAdmin else { return }
        var defaultLevel = permissionPolicy.defaultLevel
        var readOnly = permissionPolicy.readOnlyDefault
        var sideEffect = permissionPolicy.sideEffectDefault
        switch kind {
        case .global: defaultLevel = level
        case .readOnly: readOnly = level
        case .sideEffect: sideEffect = level
        }
        do {
            try mcpAdmin.setPermissionDefaults(
                defaultLevel: defaultLevel,
                readOnlyDefault: readOnly,
                sideEffectDefault: sideEffect
            )
            reloadToolPermissions()
        } catch {
            lastError = String(describing: error)
        }
    }

    func setToolPermission(identity: String, level: String) {
        guard let mcpAdmin else { return }
        do {
            try mcpAdmin.setToolPermission(identity: identity, level: level)
            reloadToolPermissions()
        } catch {
            lastError = String(describing: error)
        }
    }

    /// Add a server from the form. `args` is already split into tokens. On success
    /// the list + readiness re-probe so the panel reflects the new state.
    func addMcpServer(
        name: String, command: String, args: [String],
        endpoint: String = "", token: String = ""
    ) {
        guard let mcpAdmin else { return }
        do {
            try mcpAdmin.addServer(
                CsMcpServerInput(
                    name: name, command: command, args: args, enabled: true,
                    endpoint: endpoint, authRef: "", token: token
                )
            )
            reloadMcpServers()
            refreshAgentStatus()
        } catch {
            lastError = String(describing: error)
        }
    }

    /// Flip a server's `enabled` flag, preserving its command / args / env.
    func toggleMcpServer(_ server: CsMcpServer) {
        guard let mcpAdmin else { return }
        do {
            try mcpAdmin.updateServer(
                name: server.name,
                input: CsMcpServerInput(
                    name: server.name, command: server.command,
                    args: server.args, enabled: !server.enabled,
                    endpoint: server.endpoint, authRef: server.authRef, token: ""
                )
            )
            reloadMcpServers()
            refreshAgentStatus()
        } catch {
            lastError = String(describing: error)
        }
    }

    /// Remove a server and drop any cached test result for it.
    func removeMcpServer(_ name: String) {
        guard let mcpAdmin else { return }
        do {
            try mcpAdmin.removeServer(name: name)
            mcpTestResults[name] = nil
            reloadMcpServers()
            refreshAgentStatus()
        } catch {
            lastError = String(describing: error)
        }
    }

    /// Spawn + handshake the named server and record the result inline. Runs off
    /// the main actor (the engine detaches) so the up-to-10s test never freezes
    /// the window; `mcpTestPending` drives a spinner in the row.
    func testMcpServer(_ name: String) {
        guard let mcpAdmin else { return }
        guard !mcpTestPending.contains(name) else { return }
        mcpTestPending.insert(name)
        Task {
            let result = await mcpAdmin.testServer(name)
            mcpTestPending.remove(name)
            mcpTestResults[name] = result
        }
    }

    func select(_ target: SettingsSection) {
        guard target.availability == .available else { return }
        section = target
        // Landing on a section shows its first page; sections without pages keep
        // page == nil and render whole.
        page = SettingsPage.pages(in: target).first
        if target == .agent {
            refreshAssistiveModelDiscovery()
        }
        if target == .engine {
            refreshServingStatus()
        }
    }

    /// Select a specific page. Routes through `select` so the section's refresh
    /// side effects fire exactly once regardless of which row the user clicked.
    func select(_ target: SettingsPage) {
        select(target.section)
        page = target
    }

    func select(_ route: SettingsRoute) {
        switch route {
        case .section(let section): select(section)
        case .page(let page): select(page)
        }
    }

    /// The rail row that should read as selected for the current state.
    var route: SettingsRoute {
        if let page { return .page(page) }
        return .section(section)
    }

    // MARK: - Reset app data (recoverable destructive action)

    func refreshResetPreview() {
        guard let engine else { return }
        resetPreview = engine.resetPreview()
    }

    func resetImpactDescription(includeKeys: Bool, includePrompts: Bool) -> String {
        var message = "Moves \(resetImpactSummary(resetPreview)) to Trash."
        if includePrompts {
            message += " Your assistive.txt and three formatting prompt files will also move to Trash."
        } else {
            message += " Your assistive.txt and three formatting prompt files will be preserved."
        }
        if includeKeys {
            message += " API keys will also be removed from Keychain and are not recoverable from Trash."
        }
        return message + " Codescribe will relaunch as a fresh install."
    }

    /// Move all local app data to Trash through the Rust bridge, clear the app's
    /// UserDefaults domain, then relaunch so codescribe comes up fresh (first-run
    /// wizard from the top). `includeKeys` also removes the Keychain API keys.
    /// Pre-move failures stay in-process. A post-move failure carries a stable
    /// Rust marker and still forces relaunch, because continuing in a partially
    /// reset, permanently fenced process would be worse than the reported error.
    func resetAppData(includeKeys: Bool, includePrompts: Bool) {
        guard let engine else { return }
        do {
            try engine.resetAppData(includeKeys: includeKeys, includePrompts: includePrompts)
        } catch {
            let description = String(describing: error)
            lastError = description
            if resetFailureRequiresRelaunch(description) {
                AppRelaunch.clearDefaultsAndRelaunch()
            }
            return
        }
        AppRelaunch.clearDefaultsAndRelaunch()
    }

    func clearMcpConfiguration() {
        guard let engine else { return }
        do {
            try engine.clearMcpConfiguration()
            mcpTestResults = [:]
            reloadMcpServers()
            refreshAgentStatus()
        } catch {
            lastError = String(describing: error)
        }
    }

    // MARK: - Engine-panel derived values (runtime truth)

    /// Last serving verdict published by the runtime owner (not config).
    /// Tests inject directly; live path refreshes via `servingStatusProvider`
    /// (UniFFI `currentServingVerdict()`) in `refresh()` and on panel entry.
    var lastServingVerdict: LastServingVerdict?

    /// Runtime serving-truth source — defaults to the UniFFI bridge snapshot.
    let servingStatusProvider: () -> LastServingVerdict?

    /// Pull the latest stop-path serving verdict from the runtime owner.
    func refreshServingStatus() {
        lastServingVerdict = servingStatusProvider()
    }

    /// Active STT row — consumes runtime serving truth only.
    /// Configured engine/mode are preference controls, not this label.
    var activeSTT: String {
        formatActiveSTT(lastServing: lastServingVerdict)
    }

    /// STT is "healthy" (olive dot) when a local model is configured, or when a
    /// cloud endpoint is set. We can't probe live Whisper load state from the
    /// config engine alone — that lives on `CodescribeDictation` (tracked gap).
    var sttHealthy: Bool {
        settings.useLocalStt ? !settings.localModel.isEmpty
                             : (settings.sttEndpoint?.isEmpty == false)
    }

    var whisperLanguageCode: String { settings.whisperLanguage.shortCode }

    var sttModelDescription: String {
        settings.useLocalStt ? settings.localModel
                             : (settings.sttEndpoint ?? "cloud default")
    }

    private var assistiveKeyState: SettingsKeyState {
        guard let provider = llmLane(.assistive).provider else { return .unknown }
        let keyAvailable = provider.accountSignedIn
            || provider.apiKeySet
            || keyStatus.isSet(account: provider.apiKeyAccount)
        return keyAvailable ? .available : .missing
    }

    var settingsHealth: SettingsHealthState {
        healthState(
            stt: sttHealthy,
            keys: assistiveKeyState,
            agent: agentReadiness.ready
        )
    }

    /// Effective lane state after provider/shared fallbacks.
    func llmLane(_ lane: LLMLane) -> LLMLaneModel {
        let truth = laneTruthProvider(lane.bridgeLane)
        let providerId = truth.providerId
        let configuredModel = settings[keyPath: lane.modelPath]?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let discoveryProviderId = lane == .assistive ? providerId : "openai-responses"

        return LLMLaneModel(
            lane: lane,
            providerId: providerId,
            provider: providers.first { $0.id == providerId } ?? providers.first,
            resolvedEndpoint: truth.endpoint,
            configuredModel: configuredModel,
            resolvedModel: truth.model,
            discoveryEndpoint: lane == .assistive
                ? truth.endpoint
                : resolvedOpenAIEndpoint(for: .assistive),
            discovery: modelDiscoveries[discoveryProviderId]
                ?? CsModelDiscovery.sample(for: discoveryProviderId)
        )
    }

    private func refreshAssistiveModelDiscovery(includeOpenAI: Bool = false) {
        let providerId = llmLane(.assistive).providerId
        refreshModelDiscoveries(providerIds: includeOpenAI ? [providerId, "openai-responses"] : [providerId])
    }

    private func resolvedOpenAIEndpoint(for lane: LLMLane) -> String {
        // P2-05: lane/shared/default resolution stays here (UI settings surface);
        // suffix normalization is now delegated to core via FFI (single truth in
        // lane_truth::normalize_openai_responses_endpoint, exposed in bridge/config).
        let laneValue = settings[keyPath: lane.endpointPath]?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let sharedValue = settings[keyPath: LLMLane.main.endpointPath]?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let base = !laneValue.isEmpty
            ? laneValue
            : (!sharedValue.isEmpty
                ? sharedValue
                : "https://api.openai.com/v1/responses")

        if let engine {
            return engine.normalizeOpenaiResponsesEndpoint(base)
        }
        // Fallback for previews / no-engine (kept tiny; real path always has engine).
        // NOTE: suffix list duplication removed (L2 over-correct); core lane_truth::normalize
        // (via bridge) is the single source of truth for responses endpoint. Fallback does
        // minimal /v1 strip only to avoid duplicating known-suffixes array.
        var b = base.trimmingCharacters(in: .whitespacesAndNewlines.union(.init(charactersIn: "/")))
        if b.hasSuffix("/v1") { b.removeLast(3) }
        return b + "/v1/responses"
    }

    /// Persist an endpoint override for one LLM lane. Whitespace-only input is
    /// the reset signal: the core removes the optional JSON path so the next
    /// resolved fallback becomes effective immediately.
    func setLLMEndpoint(_ value: String, for lane: LLMLane) {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        settings[keyPath: lane.endpointPath] = trimmed.isEmpty ? nil : trimmed
        persist(lane.endpointKey, trimmed)
        refreshAgentStatus()
        if lane == .assistive {
            refreshAssistiveModelDiscovery(includeOpenAI: true)
        }
    }

    /// Persist a model override for one LLM lane. Empty clears the JSON override.
    func setLLMModel(_ value: String, for lane: LLMLane) {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        if lane == .assistive {
            assistiveModelEditGeneration += 1
            pendingAssistiveModelSelection = nil
        }
        settings[keyPath: lane.modelPath] = trimmed.isEmpty ? nil : trimmed
        persist(lane.modelKey, trimmed)
    }

    var formattingDescription: String {
        guard settings.aiFormattingEnabled else { return "disabled · compatibility gate" }
        return FormattingPolicyOption(storedValue: settings.formattingLevel)?.visibleName
            ?? "invalid policy"
    }

    /// Any LLM/STT provider key present (GitHub token is shown separately).
    var apiKeysStored: Bool {
        keyStatus.llmApiKeySet || keyStatus.llmAssistiveApiKeySet
            || keyStatus.llmAnthropicApiKeySet || keyStatus.llmFormattingApiKeySet || keyStatus.sttApiKeySet
    }

    var apiKeysDescription: String {
        apiKeysStored ? "Stored in Keychain" : "Not configured"
    }

    // MARK: - Creator mutations (write through the core router)

    func setLanguage(_ lang: CsLanguage) {
        settings.whisperLanguage = lang
        persist("WHISPER_LANGUAGE", lang.shortCode)
    }

    func setFormattingEnabled(_ on: Bool) {
        settings.aiFormattingEnabled = on
        persist("AI_FORMATTING_ENABLED", on ? "1" : "0")
    }

    func setFormattingLevel(_ level: String) {
        guard let policy = FormattingPolicyOption(storedValue: level) else {
            lastError = "Unknown formatting policy: \(level)"
            return
        }
        settings.formattingLevel = policy.rawValue
        persist("FORMATTING_LEVEL", policy.rawValue)
    }

    // MARK: - User panel (local-first product truth)

    var transcriptsPath: String {
        guard !configDir.isEmpty else { return "" }
        return URL(fileURLWithPath: configDir).appendingPathComponent("transcriptions").path
    }

    var transcriptTagPreview: String {
        transcriptTagTemplatePreview(settings.transcriptTagTemplate)
    }

    var transcriptTagTemplateWarning: String? {
        transcriptTagTemplateAppendWarning(settings.transcriptTagTemplate)
    }

    func setTranscriptTaggingEnabled(_ enabled: Bool) {
        settings.transcriptTaggingEnabled = enabled
        persist("TRANSCRIPT_TAGGING_ENABLED", enabled ? "1" : "0")
    }

    func setTranscriptTagTemplate(_ template: String) {
        settings.transcriptTagTemplate = template
        persist("TRANSCRIPT_TAG_TEMPLATE", template)
    }

    func restoreDefaultTranscriptTagTemplate() {
        setTranscriptTagTemplate(defaultTranscriptTagTemplate)
    }

    // MARK: - Audio (live hardware + existing settings contract)

    func refreshAudioInput() {
        guard let engine else { return }
        do {
            audioInput = try engine.loadAudioInputSnapshot()
            audioInputReadError = nil
        } catch {
            audioInput = CsAudioInputSnapshot(
                devices: [],
                configuredDevice: settings.audioInputDevice,
                runtimeDevice: nil,
                configuredDeviceAvailable: false,
                fallbackToDefault: false,
                runtimeConfigurationMatches: false
            )
            audioInputReadError = String(describing: error)
        }
    }

    func setAudioInputDevice(_ device: String) {
        settings.audioInputDevice = device
        persist("AUDIO_INPUT_DEVICE", device)
        refreshAudioInput()
    }

    func resetAudioInputDevice() {
        guard let engine else { return }
        do {
            try engine.resetAudioInputDevice()
            applyLoadedSettings(engine.loadSettings())
            refreshAudioInput()
        } catch {
            lastError = String(describing: error)
        }
    }

    func setToggleSilenceSeconds(_ seconds: Float) {
        settings.toggleSilenceSec = seconds
        persist("TOGGLE_SILENCE_SEC", String(format: "%.1f", seconds))
    }

    func setSoundFeedbackEnabled(_ enabled: Bool) {
        settings.beepOnStart = enabled
        persist("BEEP_ON_START", enabled ? "1" : "0")
    }

    func setSoundVolume(_ volume: Float) {
        settings.soundVolume = volume
        persist("SOUND_VOLUME", String(format: "%.2f", volume))
    }

    // MARK: - Voice Lab (live quality truth + preview timing)

    func refreshVoiceLab() {
        guard let engine else { return }
        do {
            qualityRecords = try engine.loadQualityRecentRecords(limit: 50)
            customLexiconEntries = try engine.loadLexiconCustomEntries()
            voiceLabReadError = nil
        } catch {
            qualityRecords = []
            customLexiconEntries = []
            voiceLabReadError = String(describing: error)
        }
    }

    /// Mine corrections.jsonl + proposed lexicon into the live custom dictionary.
    /// Teach replays the whole correction store and rewrites the lexicon — real
    /// disk I/O whose cost scales with the corpus. Running it inline froze
    /// Settings for the duration; it now runs off the main actor like the key
    /// probe above, with `voiceLabTeachPending` keeping the button honest until
    /// the result lands.
    func teachDictionaryFromStore() {
        guard let engine, !voiceLabTeachPending else { return }
        voiceLabTeachPending = true
        voiceLabTeachMessage = nil
        let backgroundEngine = BackgroundSettingsEngine(engine: engine)

        DispatchQueue.global(qos: .userInitiated).async { [backgroundEngine] in
            let outcome: Result<CsDictionaryTeachResult, Error>
            do {
                outcome = .success(try backgroundEngine.engine.teachDictionaryFromStore())
            } catch {
                outcome = .failure(error)
            }

            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.voiceLabTeachPending = false
                switch outcome {
                case .success(let result):
                    self.voiceLabTeachMessage =
                        "Taught +\(result.fromCorrections) from corrections, +\(result.fromProposed) from proposed → \(result.totalRules) live rules (\(result.rulesFromCorrectionSource) correction-sourced)."
                    self.refreshVoiceLab()
                case .failure(let error):
                    let message = String(describing: error)
                    self.voiceLabTeachMessage = "Teach failed: \(message)"
                    self.lastError = message
                }
            }
        }
    }

    @discardableResult
    func finalizeVoiceLabCorrection(id: String, canonical: String) -> Bool {
        guard let engine else { return false }
        let canonical = canonical.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !canonical.isEmpty, !voiceLabEditPending.contains(id) else { return false }

        voiceLabEditPending.insert(id)
        voiceLabEditErrors[id] = nil
        voiceLabEditNotes[id] = nil
        defer { voiceLabEditPending.remove(id) }
        do {
            let outcome = try engine.finalizeVoiceLabCorrection(id: id, canonical: canonical)
            voiceLabEditNotes[id] = Self.voiceLabSaveNote(outcome)
            refreshVoiceLab()
            return voiceLabReadError == nil
        } catch {
            let message = String(describing: error)
            voiceLabEditErrors[id] = message
            lastError = message
            return false
        }
    }

    /// The revision is persisted whenever the engine returns — the note only
    /// tells the truth about what the dictionary derived from the edit.
    static func voiceLabSaveNote(_ outcome: CsVoiceLabSaveResult) -> String {
        if let lexiconError = outcome.lexiconError {
            return "Saved — dictionary learning failed: \(lexiconError)"
        }
        switch outcome.pairsLearned {
        case 0: return "Saved; no dictionary rule derived"
        case 1: return "Saved — 1 rule learned"
        default: return "Saved — \(outcome.pairsLearned) rules learned"
        }
    }

    var previewTimingConfiguration: PreviewTimingConfiguration {
        PreviewTimingConfiguration(
            overlayEnabled: settings.transcriptionOverlayEnabled,
            values: PreviewTimingValues(
                bufferDelayMs: settings.bufferDelayMs ?? PreviewTimingValues.smooth.bufferDelayMs,
                typingCps: settings.typingCps ?? PreviewTimingValues.smooth.typingCps,
                emitWordsMax: settings.emitWordsMax ?? PreviewTimingValues.smooth.emitWordsMax,
                interimSeconds: settings.bufferedInterimSec ?? PreviewTimingValues.smooth.interimSeconds
            )
        )
    }

    var previewTimingPreset: PreviewTimingPreset {
        detectPreset(previewTimingConfiguration)
    }

    /// Preset writes go through the existing batch router: one settings.json
    /// transaction for overlay state plus all four coupled timing values.
    func applyPreviewTimingPreset(_ preset: PreviewTimingPreset) {
        switch preset {
        case .custom:
            return
        case .off:
            persistMany([
                CsConfigEntry(key: "TRANSCRIPTION_OVERLAY_ENABLED", value: "0"),
            ])
        case .smooth, .snappy, .relaxed:
            guard let values = presetValues(preset) else { return }
            persistMany([
                CsConfigEntry(key: "TRANSCRIPTION_OVERLAY_ENABLED", value: "1"),
                CsConfigEntry(
                    key: "CODESCRIBE_BUFFER_DELAY_MS",
                    value: String(values.bufferDelayMs)
                ),
                CsConfigEntry(
                    key: "CODESCRIBE_TYPING_CPS",
                    value: String(format: "%.1f", values.typingCps)
                ),
                CsConfigEntry(
                    key: "CODESCRIBE_EMIT_WORDS_MAX",
                    value: String(values.emitWordsMax)
                ),
                CsConfigEntry(
                    key: "CODESCRIBE_BUFFERED_INTERIM_SEC",
                    value: String(format: "%.1f", values.interimSeconds)
                ),
            ])
        }
    }

    func setPreviewBufferDelayMs(_ value: UInt64) {
        settings.bufferDelayMs = value
        persist("CODESCRIBE_BUFFER_DELAY_MS", String(value))
    }

    func setPreviewTypingCps(_ value: Float) {
        settings.typingCps = value
        persist("CODESCRIBE_TYPING_CPS", String(format: "%.1f", value))
    }

    func setPreviewEmitWordsMax(_ value: UInt64) {
        settings.emitWordsMax = value
        persist("CODESCRIBE_EMIT_WORDS_MAX", String(value))
    }

    func setPreviewInterimSeconds(_ value: Float) {
        settings.bufferedInterimSec = value
        persist("CODESCRIBE_BUFFERED_INTERIM_SEC", String(format: "%.1f", value))
    }

    // MARK: - STT engine / layered transcription (Engine panel controls)

    /// Selected STT engine id ("auto" | "apple" | "whisper"); empty → product default Apple.
    var sttEngineId: String {
        let raw = (settings.sttEngine ?? "apple").lowercased()
        switch raw {
        case "auto", "apple", "whisper", "candle", "onnx": return raw == "candle" ? "whisper" : raw
        default: return "apple"
        }
    }

    /// Display label for the current STT engine selection.
    var sttEngineLabel: String {
        switch sttEngineId {
        case "apple": return "Apple (live)"
        case "whisper", "candle": return "Whisper (Candle)"
        case "auto": return "Auto (Apple-first)"
        default: return "Apple (live)"
        }
    }

    /// Honest dual-brain note when preference and Active STT last run diverge.
    var sttEngineTruthNote: String? {
        let pref = sttEngineId
        let active = activeSTT.lowercased()
        if active.contains("not yet") || active.isEmpty { return nil }
        // Preference Apple but last run was Whisper recovery / file pass is OK
        // to mention once when the chip is clearly whisper while user picked apple.
        if pref == "apple", active.contains("whisper") {
            return "Preference: Apple live · last take used Whisper (final/recovery). Live partials stay Apple."
        }
        if pref == "whisper" || pref == "candle", active.contains("apple") {
            return "Preference: Whisper · last take was Apple live — check env override or restart."
        }
        return nil
    }

    func setSttEngine(_ id: String) {
        let normalized: String
        switch id.lowercased() {
        case "auto": normalized = "auto"
        case "whisper", "candle": normalized = "whisper"
        case "onnx": normalized = "onnx"
        default: normalized = "apple"
        }
        settings.sttEngine = normalized
        // Persist promotes to settings.json AND reconciles process env + .env
        // (single brain — no CODESCRIBE_STT_ENGINE lottery).
        persist("CODESCRIBE_STT_ENGINE", normalized)
    }

    /// Final-pass routing: always | smart | off (default smart).
    var finalPassModeId: String {
        let raw = (settings.finalPassMode ?? "smart").lowercased()
        switch raw {
        case "always", "off": return raw
        default: return "smart"
        }
    }

    var finalPassModeLabel: String {
        switch finalPassModeId {
        case "always": return "Always"
        case "off": return "Off"
        default: return "Smart"
        }
    }

    func setFinalPassMode(_ id: String) {
        let normalized: String
        switch id.lowercased() {
        case "always": normalized = "always"
        case "off": normalized = "off"
        default: normalized = "smart"
        }
        settings.finalPassMode = normalized
        persist("FINAL_PASS_MODE", normalized)
    }

    /// ON for any phase value ("phase1".."phase4" or bare "1".."4"); anything
    /// else (including "off"/absent) is OFF — mirrors the core `layered_phase`.
    var layeredTranscriptionEnabled: Bool {
        let value = settings.layeredTranscription ?? "off"
        return value.hasPrefix("phase") || Int(value) != nil
    }

    /// The GUI only exposes Phase 1 (Apple live layer + Whisper tail patch);
    /// phases 2-4 do not exist as features yet.
    func setLayeredTranscription(_ on: Bool) {
        let value = on ? "phase1" : "off"
        settings.layeredTranscription = value
        persist("CODESCRIBE_LAYERED_TRANSCRIPTION", value)
    }

    var holdBadgeOption: HoldBadgeOption {
        HoldBadgeOption(
            indicatorEnabled: settings.holdIndicator,
            size: settings.holdBadgeSize
        )
    }

    /// Off changes visibility only, preserving the stored size. A concrete size
    /// enables the indicator and writes both existing keys atomically.
    ///
    /// K3 (W10-E): persists immediately; takes effect at the *next* badge show
    /// (no live redraw of a visible caret badge).
    /// K4: posts `ConfigChangeBus.holdBadgeDidChange` so the tray reflects it.
    func setHoldBadgeOption(_ option: HoldBadgeOption) {
        guard let size = option.size else {
            settings.holdIndicator = false
            persist("HOLD_INDICATOR", "0")
            ConfigChangeBus.postHoldBadgeChanged()
            return
        }
        settings.holdIndicator = true
        settings.holdBadgeSize = size
        persistMany([
            CsConfigEntry(key: "HOLD_INDICATOR", value: "1"),
            CsConfigEntry(key: "HOLD_BADGE_SIZE", value: String(size)),
        ])
        ConfigChangeBus.postHoldBadgeChanged()
    }

    /// Reload badge fields from the engine after a peer surface (tray) wrote them.
    func reloadHoldBadgeFromDisk() {
        guard let engine else { return }
        applyLoadedSettings(engine.loadSettings())
        objectWillChange.send()
    }

    /// Assistive-arm modifier on the hold base: `"shift"` (default) or `"cmd"`.
    var holdArmModifier: String {
        let raw = settings.holdArmModifier.lowercased()
        return (raw == "cmd" || raw == "command") ? "cmd" : "shift"
    }

    func setHoldArmModifier(_ value: String) {
        let normalized = (value.lowercased() == "cmd" || value.lowercased() == "command")
            ? "cmd" : "shift"
        settings.holdArmModifier = normalized
        persist("HOLD_ARM_MODIFIER", normalized)
    }

    /// Deferred-insert chord from the canonical persisted settings snapshot.
    @Published private(set) var deferredInsertShortcut: DeferredInsertShortcutOption = .disabled

    /// Persists `CODESCRIBE_DEFERRED_INSERT_SHORTCUT` through the config
    /// router (auto-tiered → settings.json since the 2d3e2e27 promotion; the
    /// running detector live-reloads on write). Optimistic like every other
    /// setter here: the selection sticks and a failed write surfaces in
    /// `lastError`.
    func setDeferredInsertShortcut(_ option: DeferredInsertShortcutOption) {
        deferredInsertShortcut = option
        persist("CODESCRIBE_DEFERRED_INSERT_SHORTCUT", option.wireId)
    }

    // MARK: - Agent workspace roots (list_projects tool)

    /// Effective workspace roots the `list_projects` tool scans. Never empty —
    /// the bridge fills the built-in default (`~/.codescribe`) when unset.
    var agentWorkspaceRoots: [String] { settings.agentWorkspaceRoots }

    /// Persist the workspace roots as the colon-joined `AGENT_WORKSPACE_ROOTS`
    /// value. Blank/whitespace rows are dropped; an all-empty list clears the
    /// override so the core falls back to `~/.codescribe`.
    func setAgentWorkspaceRoots(_ roots: [String]) {
        let cleaned = roots
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        settings.agentWorkspaceRoots = cleaned.isEmpty ? ["~/.codescribe"] : cleaned
        persist("AGENT_WORKSPACE_ROOTS", cleaned.joined(separator: ":"))
    }

    private func persist(_ key: String, _ value: String) {
        guard let engine else { return }
        do {
            try engine.updateConfig(key: key, value: value)
            applyLoadedSettings(engine.loadSettings())
        } catch {
            lastError = String(describing: error)
        }
    }

    private func persistMany(_ entries: [CsConfigEntry]) {
        guard let engine else { return }
        do {
            try engine.updateConfigMany(entries: entries)
            applyLoadedSettings(engine.loadSettings())
        } catch {
            lastError = String(describing: error)
        }
    }

    /// Applies an already-read bridge snapshot without writing it back. Keep
    /// this as the only read path so passive reloads cannot reset the picker to
    /// a UI default while persisted truth says otherwise.
    private func applyLoadedSettings(_ loaded: CsSettings) {
        settings = loaded
        deferredInsertShortcut = DeferredInsertShortcutOption(
            wireId: loaded.deferredInsertShortcut
        )
    }

    // MARK: - Keys (Keychain-backed; secrets never read back)

    /// Friendly labels for the canonical Keychain accounts.
    static func keyLabel(for account: String) -> String {
        switch account {
        case "LLM_API_KEY": return "LLM API key"
        case "STT_API_KEY": return "Speech-to-text API key"
        case "LLM_FORMATTING_API_KEY": return "Formatting API key"
        case "LLM_ASSISTIVE_API_KEY": return "Assistive API key (OpenAI)"
        case "LLM_ANTHROPIC_API_KEY": return "Anthropic API key"
        case "LLM_XAI_API_KEY": return "xAI (Grok) API key"
        case "GITHUB_TOKEN": return "GitHub token"
        default: return account
        }
    }

    var keyAccounts: [String] { engine?.keyAccounts() ?? [] }

    // MARK: - Agent provider selection (assistive lane)

    func setAssistiveProvider(_ id: String) {
        settings.llmAssistiveProvider = id
        persist("LLM_ASSISTIVE_PROVIDER", id)
        // The stored model belonged to the previous provider; keeping it would make
        // the first send hit a model the new provider doesn't serve. Clear it so
        // the provider default applies immediately, then
        // allow only a fresh discovery to re-anchor it. Any manual model edit
        // cancels this pending auto-selection.
        setLLMModel("", for: .assistive)
        pendingAssistiveModelSelection = (
            providerId: id,
            modelEditGeneration: assistiveModelEditGeneration
        )
        refreshModelDiscoveries(providerIds: [id, "openai-responses"])
        refreshAgentStatus()
    }

    func saveKey(account: String, secret: String) {
        let trimmed = secret.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let engine else { return }
        do {
            try engine.setApiKey(account: account, secret: trimmed)
            keyProbeResults[account] = nil
            keyStatus = engine.keyStatus()
            providers = engine.availableProviders()
            if account == llmLane(.assistive).provider?.apiKeyAccount {
                refreshAssistiveModelDiscovery()
            }
            refreshAgentStatus()
        } catch {
            lastError = String(describing: error)
        }
    }

    func clearKey(account: String) {
        guard let engine else { return }
        do {
            try engine.clearApiKey(account: account)
            keyProbeResults[account] = nil
            keyStatus = engine.keyStatus()
            providers = engine.availableProviders()
            if account == llmLane(.assistive).provider?.apiKeyAccount {
                refreshAssistiveModelDiscovery()
            }
            refreshAgentStatus()
        } catch {
            lastError = String(describing: error)
        }
    }

    func testKey(account: String) {
        guard let engine else { return }
        guard !keyProbePending.contains(account) else { return }
        let backgroundEngine = BackgroundSettingsEngine(engine: engine)
        keyProbePending.insert(account)

        DispatchQueue.global(qos: .userInitiated).async { [backgroundEngine, account] in
            let result: Result<CsApiKeyProbeResult, Error>
            do {
                result = .success(try backgroundEngine.engine.testApiKey(account: account))
            } catch {
                result = .failure(error)
            }

            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.keyProbePending.remove(account)
                switch result {
                case .success(let probe):
                    self.keyProbeResults[account] = probe
                case .failure(let error):
                    self.keyProbeResults[account] = CsApiKeyProbeResult(
                        account: account,
                        status: .network,
                        message: String(describing: error),
                        probedEndpoint: nil
                    )
                    self.lastError = String(describing: error)
                }
            }
        }
    }

    /// Match any assistive provider row whose Keychain API-key account equals
    /// `account`. Used so OpenAI, Anthropic, and xAI each render their own
    /// account-login row under their key card (not OpenAI-only).
    func providerForKeyAccount(_ account: String) -> CsProviderOption? {
        providers.first { $0.apiKeyAccount == account }
    }

    /// Full "Sign in with ChatGPT" click-through: start the local callback
    /// server, open the authorize URL in the default browser, then await the
    /// roundtrip on a background queue. The await result (signed in / failed /
    /// timeout) refreshes the provider row — no restart, no zombie port.
    func startAccountLogin(providerId: String) {
        guard let engine else { return }
        guard !accountLoginPending.contains(providerId) else { return }

        let result: CsAccountLoginResult
        do {
            result = try engine.startAccountLogin(providerId: providerId)
        } catch {
            lastError = String(describing: error)
            return
        }
        guard let authUrl = result.authUrl, let url = URL(string: authUrl) else {
            accountLoginNotices[providerId] = result.message
            return
        }

        accountLoginPending.insert(providerId)
        accountLoginNotices[providerId] = nil
        NSWorkspace.shared.open(url)

        let backgroundEngine = BackgroundSettingsEngine(engine: engine)
        DispatchQueue.global(qos: .userInitiated).async { [backgroundEngine, providerId] in
            let outcome: Result<CsAccountLoginResult, Error>
            do {
                outcome = .success(
                    try backgroundEngine.engine.awaitAccountLogin(
                        providerId: providerId,
                        // P2-09: 300s chosen as pragmatic cap for OAuth browser roundtrip
                        // (user may need to 2FA, switch windows, consent). No new Settings
                        // knob (per charter). Cancel path: second start or sign-out flow
                        // or app close (server is torn down on timeout/failure).
                        // Discovery (P2-08) uses the same await; partial cancel support
                        // exists via pending set + supersede in core.
                        timeoutSeconds: 300
                    )
                )
            } catch {
                outcome = .failure(error)
            }

            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.accountLoginPending.remove(providerId)
                switch outcome {
                case .success(let login):
                    // "signed_in" needs no banner — the row status flips on the
                    // provider refresh below. Everything else is surfaced as-is.
                    self.accountLoginNotices[providerId] =
                        login.status == "signed_in" ? nil : login.message
                case .failure(let error):
                    self.accountLoginNotices[providerId] = String(describing: error)
                }
                if let engine = self.engine {
                    self.providers = engine.availableProviders()
                }
                self.refreshAgentStatus()
            }
        }
    }

    /// Sign out of the provider account (clears the stored tokens). API keys
    /// are untouched.
    func signOutAccount(providerId: String) {
        guard let engine else { return }
        do {
            try engine.signOutAccount(providerId: providerId)
            accountLoginNotices[providerId] = nil
            providers = engine.availableProviders()
            refreshAgentStatus()
        } catch {
            lastError = String(describing: error)
        }
    }

    /// Persist the OAuth client id (non-secret; settings.json) for the provider
    /// that owns it. Takes effect on the next click — the core re-reads settings
    /// per resolution. Advanced override only; shipped defaults cover OpenAI + xAI.
    func saveOauthClientId(providerId: String, value: String) {
        let settingKey: String
        switch providerId {
        case "openai-responses":
            settingKey = "LLM_OPENAI_OAUTH_CLIENT_ID"
        case "anthropic-messages":
            settingKey = "LLM_ANTHROPIC_OAUTH_CLIENT_ID"
        case "xai-responses":
            settingKey = "LLM_XAI_OAUTH_CLIENT_ID"
        default:
            lastError = "No OAuth client-id setting for provider \(providerId)"
            return
        }
        persist(settingKey, value.trimmingCharacters(in: .whitespacesAndNewlines))
        accountLoginNotices[providerId] = nil
        if let engine {
            providers = engine.availableProviders()
        }
        refreshAgentStatus()
    }

    /// The single discovery path for every lane/provider. Generation checks drop
    /// stale network results. Provider-switch auto-selection is held separately
    /// so a newer endpoint refresh inherits it while a manual model edit cancels it.
    private func refreshModelDiscoveries(providerIds: [String]) {
        let providerIds = Array(Set(providerIds))
        var generations: [String: Int] = [:]
        for providerId in providerIds {
            modelDiscoveryGenerations[providerId, default: 0] += 1
            generations[providerId] = modelDiscoveryGenerations[providerId]
        }
        guard let engine else {
            for providerId in providerIds {
                let discovery = CsModelDiscovery.sample(for: providerId)
                modelDiscoveries[providerId] = discovery
                applyPendingAssistiveModelSelection(
                    providerId: providerId,
                    discovery: discovery
                )
            }
            return
        }

        for providerId in providerIds {
            let loading = CsModelDiscovery(
                providerId: providerId,
                status: "loading",
                message: nil,
                models: []
            )
            modelDiscoveries[providerId] = loading
        }

        let backgroundEngine = BackgroundSettingsEngine(engine: engine)
        DispatchQueue.global(qos: .userInitiated).async { [backgroundEngine, providerIds, generations] in
            let discoveries = providerIds.map { providerId in
                (providerId, backgroundEngine.engine.discoverModels(providerId: providerId))
            }

            DispatchQueue.main.async { [weak self, discoveries, generations] in
                guard let self else { return }
                for (providerId, discovery) in discoveries {
                    guard self.modelDiscoveryGenerations[providerId] == generations[providerId] else {
                        continue
                    }
                    self.modelDiscoveries[providerId] = discovery
                    self.applyPendingAssistiveModelSelection(
                        providerId: providerId,
                        discovery: discovery
                    )
                }
            }
        }
    }

    private func applyPendingAssistiveModelSelection(
        providerId: String,
        discovery: CsModelDiscovery
    ) {
        guard let pending = pendingAssistiveModelSelection,
              pending.providerId == providerId
        else { return }

        let activeProviderId = settings.llmAssistiveProvider ?? "openai-responses"
        guard pending.modelEditGeneration == assistiveModelEditGeneration,
              activeProviderId == providerId
        else {
            pendingAssistiveModelSelection = nil
            return
        }

        guard discovery.status == "fresh",
              let firstModel = discovery.models.first?.id,
              !firstModel.isEmpty
        else { return }

        setLLMModel(firstModel, for: .assistive)
    }

    // MARK: - Prompts (editable BASE prompts)

    func formattingPrompt() -> String { formattingPromptSnapshot().content }
    func assistivePrompt() -> String { assistivePromptSnapshot().content }
    func formattingPromptSnapshot() -> CsPromptSnapshot {
        engine?.formattingPromptSnapshot() ?? .sampleFormatting
    }
    func formattingPromptSnapshot(level: FormattingPolicyOption) -> CsPromptSnapshot? {
        guard let engine else { return nil }
        do {
            return try engine.formattingPromptSnapshot(level: level.rawValue)
        } catch {
            lastError = String(describing: error)
            return nil
        }
    }
    func assistivePromptSnapshot() -> CsPromptSnapshot {
        engine?.assistivePromptSnapshot() ?? .sampleAssistive
    }
    func defaultFormattingPrompt() -> String {
        engine?.defaultFormattingPrompt() ?? CsSettings.samplePrompt
    }
    func defaultAssistivePrompt() -> String {
        engine?.defaultAssistivePrompt() ?? CsSettings.sampleAssistivePrompt
    }

    @discardableResult
    func saveFormattingPrompt(_ content: String) -> CsPromptSnapshot? {
        saveFormattingPrompt(.correction, content: content)
    }

    @discardableResult
    func saveFormattingPrompt(
        _ level: FormattingPolicyOption,
        content: String
    ) -> CsPromptSnapshot? {
        guard let engine else { return nil }
        do {
            try engine.setFormattingPrompt(level: level.rawValue, content: content)
            return try engine.formattingPromptSnapshot(level: level.rawValue)
        } catch {
            lastError = String(describing: error)
            return nil
        }
    }

    @discardableResult
    func saveAssistivePrompt(_ content: String) -> CsPromptSnapshot? {
        guard let engine else { return nil }
        do {
            try engine.setAssistivePrompt(content: content)
            return engine.assistivePromptSnapshot()
        } catch {
            lastError = String(describing: error)
            return nil
        }
    }

    @discardableResult
    func restoreFormattingPromptToDefault() -> CsPromptSnapshot? {
        restoreFormattingPromptToDefault(.correction)
    }

    @discardableResult
    func restoreFormattingPromptToDefault(
        _ level: FormattingPolicyOption
    ) -> CsPromptSnapshot? {
        guard let engine else { return nil }
        do {
            try engine.restoreFormattingPromptToDefault(level: level.rawValue)
            return try engine.formattingPromptSnapshot(level: level.rawValue)
        } catch {
            lastError = String(describing: error)
            return nil
        }
    }

    @discardableResult
    func restoreAssistivePromptToDefault() -> CsPromptSnapshot? {
        guard let engine else { return nil }
        do {
            try engine.restoreAssistivePromptToDefault()
            return engine.assistivePromptSnapshot()
        } catch {
            lastError = String(describing: error)
            return nil
        }
    }

    // MARK: - Preview seed

    static var preview: SettingsViewModel { preview(.creator) }

    static func preview(_ section: SettingsSection) -> SettingsViewModel {
        let model = SettingsViewModel(
            engine: MockSettingsEngine(),
            permissionProbe: MockPermissionProbe(.allGranted),
            agentStatus: MockAgentStatusEngine(),
            mcpAdmin: MockMCPAdminEngine(),
            hotkeys: MockHotkeysEngine(),
            laneTruthProvider: { lane in
                CsLaneTruthSnapshot(
                    lane: lane,
                    providerId: "openai-responses",
                    endpoint: "https://api.openai.com/v1/responses",
                    model: "gpt-5.2",
                    keyAccount: "LLM_ASSISTIVE_API_KEY",
                    keyPresent: true,
                    accountAuth: false,
                    available: true,
                    unavailableReason: nil
                )
            }
        )
        model.section = section
        model.reloadMcpServers()
        model.loadHotkeys()
        return model
    }
}
