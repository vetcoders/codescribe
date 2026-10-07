import AppKit
import Combine
import SwiftUI

let defaultTranscriptTagTemplate =
  "<codescribe mode=\"{mode}\" lang=\"{lang}\">\n{text}\n</codescribe>"
let transcriptTagTemplatePlaceholders = ["{mode}", "{lang}", "{text}", "{conf}", "{flags}"]

func transcriptTagTemplatePreview(
  _ template: String,
  mode: String = "dictation",
  lang: String = "pl",
  text: String = "…",
  conf: String = "medium",
  flags: String = "possible_hallucination_logprob"
) -> String {
  var rendered =
    template
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
    : String(
      localized: "Missing {text}; delivered transcript will be appended after the template.",
      comment: "{text} is a template token the user types — keep it verbatim"
    )
}

/// Runtime serving truth for the Active STT row (not configured preference).
struct LastServingVerdict: Equatable {
  let engine: String
  let routingMode: String
  let disposition: String?
  let fallbackUsed: Bool
}

/// Format Active STT from the last serving verdict. Config projection is forbidden.
/// Live Apple is `local_apple` → `Apple`. Do not append the dead Smart-final-pass token.
/// `Apple` and `Whisper` are proper names and stay verbatim; every other word is copy.
func formatActiveSTT(lastServing: LastServingVerdict?) -> String {
  guard let verdict = lastServing else {
    return String(
      localized: "Not yet served",
      comment: "Active STT row before the first transcription of this launch"
    )
  }
  switch verdict.engine {
  case "local_apple":
    return "Apple"
  case "local_whisper":
    return verdict.fallbackUsed
      ? String(
        localized: "Whisper (fallback)",
        comment: "Whisper is a product name; the parenthesis says it was not the chosen engine"
      )
      : "Whisper"
  case "streaming_whisper":
    return String(
      localized: "Streaming Whisper",
      comment: "Active STT row. Whisper is a product name; Streaming says it transcribed live"
    )
  case "cloud_stt":
    return String(
      localized: "Cloud",
      comment: "Active STT row: the cloud engine served the last transcription"
    )
  default:
    return verdict.engine.isEmpty
      ? String(localized: "Unknown", comment: "Active STT row: the engine id was empty")
      : verdict.engine
  }
}

/// Readiness of the mode-selected local refinement lane and its diagnostic env override.
enum LocalWhisperRuntimeState: Equatable {
  case notSelected
  case livePatchingConfigured
  case livePatchingNotReady
  case degradedEnvOverride
}

func resolveLocalWhisperRuntimeState(
  asrModeId: String,
  layeredValue: String?,
  modelAvailable: Bool
) -> LocalWhisperRuntimeState {
  guard asrModeId == "local_power" else { return .notSelected }
  let value = layeredValue?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
  guard value == nil || value == "" || value == "phase1" || value == "1" else {
    return .degradedEnvOverride
  }
  return modelAvailable ? .livePatchingConfigured : .livePatchingNotReady
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

  /// Display name of the formatting level. `rawValue` stays the identity that
  /// `update_config` persists, so the label never derives from it.
  var visibleName: String {
    switch self {
    case .off:
      return String(
        localized: "settings.formatting.level.off", defaultValue: "Off",
        comment: "Formatting level: no AI formatting")
    case .correction:
      return String(localized: "Correction", comment: "Formatting level: fix-ups only")
    case .smart:
      return String(localized: "Smart", comment: "Formatting level: balanced editing")
    case .max: return String(localized: "Max", comment: "Formatting level: maximum prose polish")
    }
  }

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

  /// Identity is the pixel size, never the shown label.
  var id: String {
    guard let size else { return "off" }
    return "px-\(size)"
  }
  var visibleName: String {
    guard let size else {
      return String(
        localized: "settings.holdBadge.size.off", defaultValue: "Off",
        comment: "Hold badge size: the badge is disabled")
    }
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
    case .disabled:
      return String(localized: "Disabled", comment: "Deferred-insert chord: no chord is bound")
    case .commandOptionV: return "⌘⌥V"
    case .commandShiftV: return "⌘⇧V"
    case .commandControlV: return "⌘⌃V"
    }
  }
}

/// Automatic paste policy (`PASTE_MODE`, core `PasteMode`). One stored
/// choice shared by Settings and the tray Quick settings row.
extension CsPasteMode {
  /// Settings order; the tray row cycles through it.
  static let allModes: [CsPasteMode] = [.safe, .comfort, .off]

  /// Canonical wire identifier for `update_config` (`PasteMode::as_str`).
  var wireId: String {
    switch self {
    case .safe: return "safe"
    case .comfort: return "comfort"
    case .off: return "off"
    }
  }

  var visibleName: String {
    switch self {
    case .safe: return String(localized: "Safe", comment: "Paste policy: cautious destinations")
    case .comfort:
      return String(localized: "Comfort", comment: "Paste policy: paste wherever the caret is")
    case .off:
      return String(
        localized: "settings.paste.policy.off", defaultValue: "Off",
        comment: "Paste policy: never paste automatically")
    }
  }

  /// One sentence per mode for the Settings segmented control.
  var blurb: String {
    switch self {
    case .safe:
      return String(
        localized:
          "Pastes only into a text field; terminals only when it doesn't look like a command."
      )
    case .comfort:
      return String(
        localized:
          "Pastes wherever the caret is, terminals too; commands and password fields are held."
      )
    case .off:
      return String(localized: "Never pastes automatically; the transcript stays on the overlay.")
    }
  }

  /// Safe → Comfort → Off → Safe.
  var next: CsPasteMode {
    let index = Self.allModes.firstIndex(of: self) ?? 0
    return Self.allModes[(index + 1) % Self.allModes.count]
  }
}

/// Panel a rail section routes to. `SettingsView`'s detail switch consumes this
/// map exhaustively, so routing stays testable without rendering.
enum SettingsPanelDestination: Equatable {
  case creator
  case shortcuts
  case providers
  case agent
  case dictation
  case audio
  case dictionary
  case lab
  case license
  case user
}

/// Testable ownership contract for the two settings surfaces that used to be
/// mixed together. This is UI metadata only; it never participates in storage.
enum SettingsPanelCapability: Hashable {
  case providers
  case llmLanes
  case workspaceRoots
  case agentStatus
  case mcpServers
  case toolPermissions
  case prompts
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
  case engine
  case audio
  case voiceLab
  case lab
  case license
  case user

  var id: String { rawValue }

  var title: String {
    switch self {
    case .creator:
      return String(localized: "Creator", comment: "Settings section: first-run setup and basics")
    case .shortcuts: return String(localized: "Hotkeys", comment: "Settings section")
    case .keys:
      return String(localized: "Providers", comment: "Settings section: AI provider accounts")
    case .agent: return String(localized: "Agent", comment: "Settings section")
    case .engine: return String(localized: "Dictation", comment: "Settings section")
    case .audio: return String(localized: "Audio", comment: "Settings section")
    case .voiceLab:
      return String(localized: "Dictionary", comment: "Settings section: custom vocabulary")
    case .lab:
      return String(localized: "Lab", comment: "Settings section: developer experiments")
    case .license: return String(localized: "License", comment: "Settings section")
    case .user: return String(localized: "User", comment: "Settings section: account profile")
    }
  }

  var destination: SettingsPanelDestination {
    switch self {
    case .creator: return .creator
    case .shortcuts: return .shortcuts
    case .keys: return .providers
    case .agent: return .agent
    case .engine: return .dictation
    case .audio: return .audio
    case .voiceLab: return .dictionary
    case .lab: return .lab
    case .license: return .license
    case .user: return .user
    }
  }

  var availability: SettingsSectionAvailability {
    switch self {
    case .lab:
      return DeveloperSurface.isEnabled() ? .available : .hidden
    case .creator, .shortcuts, .keys, .agent, .engine, .audio, .voiceLab, .license, .user:
      return .available
    }
  }

  var isInteractive: Bool { availability == .available }

  /// Sidebar grouping. A flat nine-item list forces the user to read every row;
  /// native sidebars carry `Section` headers for free, so the rail states what
  /// each area is FOR instead of relying on the reader's memory.
  var group: SettingsSectionGroup {
    switch self {
    case .creator, .shortcuts, .audio: return .setup
    case .keys, .agent, .engine, .voiceLab, .lab: return .intelligence
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
    case .engine: return "waveform"
    case .audio: return "mic"
    case .voiceLab: return "character.book.closed"
    case .lab: return "waveform.path.ecg"
    case .license: return "checkmark.seal"
    case .user: return "person.crop.circle"
    }
  }

  /// Extra terms the settings search matches beyond the visible title, so the
  /// user can look for what a panel DOES ("api key", "mikrofon") instead of
  /// having to guess the tab name. Shape: `settingsSearchTerms(fixed:localized:)`.
  var searchKeywords: [String] {
    switch self {
    case .creator:
      settingsSearchTerms(
        localized: String(
          localized: "settings.search.section.creator",
          defaultValue: "setup, onboarding, permissions, quick start, language",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    case .shortcuts:
      settingsSearchTerms(
        localized: String(
          localized: "settings.search.section.shortcuts",
          defaultValue: "hotkey, keyboard, shortcut, trigger, hold, toggle",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    case .keys:
      settingsSearchTerms(
        fixed: ["openai", "anthropic"],
        localized: String(
          localized: "settings.search.section.keys",
          defaultValue: "api key, provider, endpoint, model, token",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    case .agent:
      settingsSearchTerms(
        fixed: ["mcp"],
        localized: String(
          localized: "settings.search.section.agent",
          defaultValue: "tools, workspace, permissions, server, auto-send",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    case .engine:
      settingsSearchTerms(
        fixed: ["stt", "whisper", "apple", "asr"],
        localized: String(
          localized: "settings.search.section.engine",
          defaultValue: "speech, transcription, cloud, consent",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    case .audio:
      settingsSearchTerms(
        localized: String(
          localized: "settings.search.section.audio",
          defaultValue: "microphone, mikrofon, input, device, levels",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    case .voiceLab:
      settingsSearchTerms(
        localized: String(
          localized: "settings.search.section.voiceLab",
          defaultValue: "lexicon, dictionary, vocabulary, corrections, słownik",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    case .lab:
      // Developer surface: its names are not interface copy.
      settingsSearchTerms(fixed: ["lab", "voice lab", "three-judge", "seismograph"])
    case .license:
      settingsSearchTerms(
        localized: String(
          localized: "settings.search.section.license",
          defaultValue: "subscription, activation, trial, billing",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    case .user:
      settingsSearchTerms(
        localized: String(
          localized: "settings.search.section.user",
          defaultValue: "account, profile, sign in, identity",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
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
      section.title.localizedStandardContains(needle)
        || section.searchKeywords.contains { $0.localizedStandardContains(needle) }
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
    case .setup: return String(localized: "Setup", comment: "Sidebar group of settings sections")
    case .intelligence:
      return String(localized: "Intelligence", comment: "Sidebar group of settings sections")
    case .account:
      return String(localized: "Account", comment: "Sidebar group of settings sections")
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
  /// `nil` when nothing operational can be said yet: the footer stays empty.
  let message: String?
  let targetSection: SettingsSection?
}

/// Pure aggregate used by the rail footer and its XCTest matrix. Known failures
/// beat unknown inputs so the footer never hides a concrete problem behind a
/// muted "unknown" state. Every message is one sentence-case line that names
/// the area, never the cause: the owning panel explains. An undetermined state
/// has no message, except the recording check, which says it is running.
func healthState(
  stt: Bool?,
  recording: Bool?,
  keys: SettingsKeyState,
  agent: Bool?,
  formatting: Bool? = nil,
  formattingRequired: Bool = true
) -> SettingsHealthState {
  if stt == false {
    return SettingsHealthState(
      level: .offline,
      message: String(
        localized: "Transcription unavailable",
        comment: "Settings health footer, sentence case"
      ),
      targetSection: .engine
    )
  }
  if recording == false {
    return SettingsHealthState(
      level: .offline,
      message: String(
        localized: "Recording needs setup",
        comment: "Settings health footer, sentence case"
      ),
      targetSection: .audio
    )
  }
  if keys == .missing {
    return SettingsHealthState(
      level: .degraded,
      // Shares the onboarding row: no supported account or API key.
      message: String(localized: "Agent needs setup"),
      targetSection: .keys
    )
  }
  if agent == false {
    return SettingsHealthState(
      level: .offline,
      message: String(
        localized: "Agent unavailable",
        comment: "Settings health footer, sentence case"
      ),
      targetSection: .agent
    )
  }
  if formattingRequired && formatting == false {
    return SettingsHealthState(
      level: .degraded,
      message: String(
        localized: "Formatting unavailable",
        comment: "Settings health footer, sentence case"
      ),
      targetSection: .agent
    )
  }
  if stt == nil || recording == nil || keys == .unknown || agent == nil
    || (formattingRequired && formatting == nil)
  {
    return SettingsHealthState(
      level: .unknown,
      message: recording == nil
        ? String(
          localized: "Checking recording…",
          comment: "Settings health footer, sentence case"
        )
        : nil,
      targetSection: recording == nil ? .audio : nil
    )
  }
  return SettingsHealthState(
    level: .healthy,
    message: String(
      localized: "Ready to work",
      comment: "Settings health footer, sentence case: nothing needs attention"
    ),
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

/// The words a person types to confirm a reset. They are compared, not read as
/// copy, so they stay the same in every interface language; sentences that
/// name them take them as an argument.
let resetConfirmationWord = "RESET"
let resetAgentConfirmationWord = "RESET AGENT"

func resetConfirmationMatches(_ text: String) -> Bool {
  text == resetConfirmationWord
}

func resetAgentConfirmationMatches(_ text: String) -> Bool {
  text == resetAgentConfirmationWord
}

/// Rust marks failures that occurred after the first irreversible data move.
/// Those are errors for audit/recovery, but staying in the current process is
/// no longer safe because its app-data plane is deliberately latched.
func resetFailureRequiresRelaunch(_ description: String) -> Bool {
  description.contains("CODESCRIBE_RESET_RELAUNCH_REQUIRED")
}

func agentResetFailureRequiresRelaunch(_ description: String) -> Bool {
  description.contains("CODESCRIBE_AGENT_RESET_RELAUNCH_REQUIRED")
}

/// Three independent counts: the catalog carries one plural variation per
/// argument. The megabyte figure is pre-formatted, so it stays a plain string.
func resetImpactSummary(_ preview: CsResetPreview) -> String {
  let recordings = Int(preview.audioFiles)
  let days = Int(preview.transcriptDays)
  let threads = Int(preview.threads)
  let megabytes = String(format: "%.1f", Double(preview.totalBytes) / 1_048_576.0)
  return String(
    localized: "\(recordings) recordings from \(days) days, \(threads) threads (\(megabytes) MB)",
    comment: "Reset impact; each count needs its own plural variation"
  )
}

enum SettingsAnchor: String, Hashable {
  case audioInput
  case audioReadiness
}

struct SettingsDeepLinkTarget: Equatable {
  let section: SettingsSection
  let anchor: SettingsAnchor?
  let tab: SettingsTab?

  init(section: SettingsSection, anchor: SettingsAnchor? = nil) {
    self.section = section
    self.anchor = anchor
    tab = nil
  }

  init(tab: SettingsTab, anchor: SettingsAnchor? = nil) {
    section = tab.section
    self.anchor = anchor
    self.tab = tab
  }
}

/// One-shot deep-link target for the Settings window. A surface outside
/// Settings can name both the owning section and an exact repair surface
/// inside it. The app routes through one shared instance; tests exercise
/// their own instance, so the live Settings window (which legitimately
/// consumes the shared one the moment a target is posted) cannot race
/// their assertions.
@MainActor
final class SettingsDeepLink {
  static let shared = SettingsDeepLink()
  static let pendingSectionDidChange = Notification.Name(
    "codescribe.settingsDeepLink.pendingSectionDidChange")
  static let agentConfigurationSection: SettingsSection = .agent

  private var pendingTarget: SettingsDeepLinkTarget? {
    didSet {
      guard pendingTarget != nil else { return }
      NotificationCenter.default.post(name: Self.pendingSectionDidChange, object: self)
    }
  }

  /// Section-only requests clear an older anchor before navigation.
  var pendingSection: SettingsSection? {
    get { pendingTarget?.section }
    set {
      pendingTarget = newValue.map { SettingsDeepLinkTarget(section: $0, anchor: nil) }
    }
  }

  func present(_ section: SettingsSection, anchor: SettingsAnchor? = nil) {
    pendingTarget = SettingsDeepLinkTarget(section: section, anchor: anchor)
  }

  func present(tab: SettingsTab, anchor: SettingsAnchor? = nil) {
    pendingTarget = SettingsDeepLinkTarget(tab: tab, anchor: anchor)
  }

  /// Take the pending target (if any), clearing it so a later open is unaffected.
  func consume() -> SettingsDeepLinkTarget? {
    guard let target = pendingTarget else { return nil }
    pendingTarget = nil
    return target
  }
}

/// The two request lanes (D4: no Main fallback lane). A lane binds a provider
/// reference (`vendor id` | `custom:<id>`) and a model; the endpoint is the
/// provider's, never the lane's.
enum LLMLane: String, CaseIterable, Identifiable, Hashable {
  case assistive
  case formatting

  var id: String { rawValue }

  var bridgeLane: CsLlmLane { self == .assistive ? .assistive : .formatting }

  var title: String {
    self == .assistive
      ? String(localized: "Assistive", comment: "Request lane: agent and voice assistant")
      : String(localized: "Formatting", comment: "Request lane: transcript cleanup")
  }

  var subtitle: String {
    self == .assistive
      ? String(localized: "Agent and voice-assistant requests")
      : String(localized: "Transcript cleanup and formatting")
  }

  var providerKey: String {
    self == .assistive ? "LLM_ASSISTIVE_PROVIDER" : "LLM_FORMATTING_PROVIDER"
  }

  var modelKey: String { self == .assistive ? "LLM_ASSISTIVE_MODEL" : "LLM_FORMATTING_MODEL" }

  // Spelled out as `if` on purpose: the ternary over two key-path literals
  // crashes the type checker ("failed to produce diagnostic", Xcode 27).
  var modelPath: WritableKeyPath<CsSettings, String?> {
    if self == .assistive { return \CsSettings.llmAssistiveModel }
    return \CsSettings.llmFormattingModel
  }
}

/// One read model for a request lane: the loader-sealed runtime truth plus the
/// registry row and the latest discovery for that provider. Both Settings
/// panels consume this snapshot, so resolution cannot drift between them.
struct LLMLaneModel {
  let lane: LLMLane
  let runtime: CsRuntimeLlmLane
  let provider: CsProviderOption?
  let configuredModel: String
  let discovery: CsModelDiscovery
  var credentialAccessResolved = true
  var credentialAccessError: String?

  var providerId: String { runtime.providerId }
  var providerDisplayName: String { provider?.displayName ?? runtime.providerDisplayName }
  var resolvedEndpoint: String { runtime.endpoint }
  var resolvedModel: String { runtime.model }
  var modelOptions: [CsModelOption] { discovery.models }

  /// Discovery drives the Menu when it is fresh or cached and non-empty; the Model ID
  /// field stays beside it either way (custom hosts may publish no list).
  var usesDiscoveredPicker: Bool {
    !modelOptions.isEmpty && ["fresh", "cached"].contains(discovery.status)
  }

  /// `unavailableReason` arrives from the core and cannot be localized here.
  var availabilityDescription: String {
    guard credentialAccessResolved else { return String(localized: "Checking provider access…") }
    if credentialAccessError != nil { return String(localized: "Provider access unavailable") }
    if !runtime.available {
      return runtime.unavailableReason
        ?? String(localized: "unavailable", comment: "Lane availability, lower case")
    }
    if runtime.accountAuth {
      return String(localized: "account", comment: "Lane auth: signed-in account, lower case")
    }
    if runtime.keyPresent {
      return String(localized: "API key", comment: "Lane auth: an API key is stored")
    }
    return String(localized: "no key required", comment: "Lane auth, lower case")
  }

  var availabilityTint: Color {
    guard credentialAccessResolved, credentialAccessError == nil else { return Color.secondary }
    return runtime.available ? CSColor.oliveLight : CSColor.terracotta
  }

  var discoveryDescription: String {
    guard credentialAccessResolved else { return String(localized: "Checking provider access…") }
    if credentialAccessError != nil { return String(localized: "Provider access unavailable") }
    switch discovery.status {
    case "fresh":
      let count = modelOptions.count
      return count == 0
        ? String(localized: "No models returned. Enter a Model ID in Settings › Agent › LLM lanes.")
        : String(
          localized: "\(count) models discovered from provider",
          comment: "Model discovery status; needs a plural variation"
        )
    case "cached":
      if let message = discovery.message, !message.isEmpty {
        return String(
          localized: "using cached models — \(message)",
          comment: "The placeholder is a status message from the core"
        )
      }
      return String(localized: "using cached models", comment: "Model discovery status")
    case "no_key":
      if runtime.accountAuth {
        return String(
          localized:
            "Account sign-in supports Assistive requests, but model discovery requires a provider API key. Keep the current model or enter a Model ID in Settings › Agent › LLM lanes."
        )
      }
      if lane == .formatting, provider?.accountSignedIn == true {
        return String(
          localized:
            "Formatting requires this provider's API key; an Assistive account does not authorize it. Add the key in Settings › Providers."
        )
      }
      return String(
        localized:
          "Add this provider's API key in Settings › Providers to discover models, or enter a Model ID in Settings › Agent › LLM lanes."
      )
    case "loading": return String(localized: "discovering models…", comment: "In-progress status")
    default:
      if let message = discovery.message, !message.isEmpty {
        return String(
          localized:
            "Model discovery failed: \(message). Check Settings › Providers, then refresh models in Settings › Agent › LLM lanes.",
          comment: "The placeholder is a status message from the core"
        )
      }
      return String(
        localized:
          "Model discovery failed. Check Settings › Providers, then refresh models in Settings › Agent › LLM lanes."
      )
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

extension UInt64 {
  fileprivate func absDiff(_ other: UInt64) -> UInt64 {
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
  /// Erases only durable Agent queue/attachment state. Agent window layout and
  /// all non-Agent preferences (including hotkeys and dictation) deliberately
  /// survive this narrow reset.
  static func clearAgentDefaults(defaults: UserDefaults = .standard) {
    defaults.removeObject(forKey: AgentChatStore.acceptedTurnsDefaultsKey)
    defaults.removeObject(forKey: AgentChatStore.attachmentMetadataDefaultsKey)
    defaults.synchronize()
  }

  static func clearAgentDefaultsAndRelaunch() {
    clearAgentDefaults()
    relaunch()
  }

  static func clearDefaultsAndRelaunch() {
    if let bundleId = Bundle.main.bundleIdentifier {
      UserDefaults.standard.removePersistentDomain(forName: bundleId)
      UserDefaults.standard.synchronize()
    }
    relaunch()
  }

  private static func relaunch() {
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

private enum WhisperDownloadEvent: Sendable {
  case progress(detail: String, fraction: Double?)
  case complete(path: String)
}

/// Value-only UniFFI callback adapter; the view model owns the one ordered
/// MainActor consumer.
final class WhisperDownloadProgressSink: CsWhisperDownloadListener, Sendable {
  private let continuation: AsyncStream<WhisperDownloadEvent>.Continuation

  fileprivate init(continuation: AsyncStream<WhisperDownloadEvent>.Continuation) {
    self.continuation = continuation
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
    continuation.yield(.progress(detail: detail, fraction: fraction))
  }

  func onComplete(path: String) {
    continuation.yield(.complete(path: path))
  }

  func finish() {
    continuation.finish()
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
  static let agentBridgeLaunchSynchronizationDidFinish = Notification.Name(
    "com.vetcoders.codescribe.agent-bridge-launch-synchronization"
  )
  // A launch-result projection for Settings instances opened after the task
  // finishes. Installation ownership remains in the on-disk managed receipt.
  private static var agentBridgeLaunchNotice: String?

  static func recordAgentBridgeLaunchSynchronization(_ detail: String?) {
    agentBridgeLaunchNotice = detail
    NotificationCenter.default.post(name: agentBridgeLaunchSynchronizationDidFinish, object: nil)
  }

  @Published var section: SettingsSection = .creator
  /// Selected tab within `section`, when that section has tabs. Navigation
  /// state only — never persisted. Written exclusively by the `select`
  /// overloads; `currentTab` clamps it against raw `section` writes.
  @Published private(set) var tab: SettingsTab?

  /// The tab `section` shows: the selected tab when it belongs to `section`,
  /// otherwise the section's first tab; nil for sections without tabs. Settable
  /// so the tab bar binds by key path; writes route through `select(_:)` and a
  /// nil write is dropped.
  var currentTab: SettingsTab? {
    get {
      if let tab, tab.section == section { return tab }
      return SettingsTab.tabs(in: section).first
    }
    set {
      if let newValue { select(newValue) }
    }
  }

  /// Optional navigation request; a nil write must not blank the detail pane.
  /// Native controls keep their reconciliation state in the view and submit
  /// requests from onChange, rather than binding directly to this setter.
  var sidebarSelection: SettingsSection? {
    get { section }
    set {
      if let newValue { select(newValue) }
    }
  }

  /// Dictation seam for the "Open overlay" quick-start card. Defaulted to the
  /// live tray toggle but only dereferenced on click, so unit tests can inject
  /// a spy without ever waking `AppModel.shared`.
  var onQuickStartDictation: () -> Void = { AppModel.shared.tray.toggleDictation() }

  /// Overlay seam for the preview preset. A preset writes the "Transcription
  /// Overlay" preference; the overlay's owner closes a panel already on screen
  /// when it turns off. Dereferenced only after a preset write, so unit tests
  /// can inject a spy without ever waking `AppModel.shared`.
  var onOverlayPreferenceChanged: () -> Void = {
    AppModel.shared.overlay.overlayPreferenceChanged()
  }

  func performQuickStart(_ action: SettingsQuickStartAction) {
    switch action {
    case .testMic: section = .audio
    case .tuneShortcuts: section = .shortcuts
    case .openOverlay: onQuickStartDictation()
    }
  }

  @Published private(set) var permissions: PermissionSnapshot
  @Published private(set) var creatorAgentBridgeStatus = AgentBridgeInstallationStatus.unavailable
  @Published private(set) var creatorAgentBridgeError: String?
  @Published private(set) var creatorAgentBridgeNotice: String?
  /// Launch-synchronization diagnostics, not a user-facing notice: `App.swift`
  /// writes this detail to the app log, and Agent Diagnostics → Connection
  /// details shows it under the installed paths.
  @Published private(set) var creatorAgentBridgeLaunchDetail: String?
  @Published private(set) var settings: CsSettings
  @Published private(set) var newMaxConsultationPending = false
  @Published private(set) var maxConsultationNotice: String?
  @Published private(set) var maxToolApprovals: [PendingToolApproval] = []
  @Published private(set) var maxApprovalBusy = false
  @Published private(set) var maxApprovalError: String?
  private var maxApprovalRefreshRequested = false
  @Published private(set) var keyStatus: CsKeyStatus
  @Published private(set) var providers: [CsProviderOption]
  /// Speech-to-text lanes (File, Live): atomic endpoint + key rows on Providers.
  @Published private(set) var sttLanes: [CsSttLane]
  @Published private var modelDiscoveries: [String: CsModelDiscovery] = [:]
  /// Set when removing a custom provider bounced one or more lanes back to the
  /// default vendor (bridge `lanesReset`). Cleared on the next lane edit.
  @Published private(set) var laneResetNotice: String?
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
  @Published private(set) var unchangedQualityTakes: UInt64 = 0
  @Published private(set) var customLexiconEntries: [CsLexiconEntry] = []
  @Published private(set) var ruleCandidates: [CsRuleCandidate] = []
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
  /// The controller's own admission verdict (nil until the first refresh).
  @Published private(set) var admission: CsAdmissionReadiness?
  @Published private(set) var admissionReadError: String?
  /// A guided calibration capture is in flight (the mic is open ~10 s).
  @Published private(set) var calibrationPending: Bool = false
  @Published private(set) var calibrationStartedAt: Date?
  /// Last calibration outcome, success or refusal, for the Audio row.
  @Published private(set) var calibrationNotice: String?
  @Published private(set) var resetPreview: CsResetPreview
  @Published private(set) var agentResetPreview: CsAgentResetPreview
  var licenseStatus: CsLicenseStatus { licenseService.status }
  var licenseReadState: LicenseService.ReadState { licenseService.readState }
  var licenseBusy: Bool { licenseService.isBusy }
  var licenseAllowsAgentMode: Bool { licenseService.canUseAgentic }
  private var licenseChangeSink: AnyCancellable?
  private var agentBridgeSynchronizationSink: AnyCancellable?
  /// Provider ids with a "Sign in with ChatGPT" flow in flight (browser open,
  /// local callback server listening). Guards double-clicks.
  @Published private(set) var accountLoginPending: Set<String> = []
  /// Last terminal outcome of an account login per provider ("timeout",
  /// "failed" …) — honest status for the row without raising a modal error.
  @Published private(set) var accountLoginNotices: [String: String] = [:]
  @Published var lastError: String?

  @Published private(set) var providerAccessPending = false
  @Published private(set) var providerMutationPending = false
  @Published private(set) var providerAccessResolved = false
  /// When the last provider snapshot landed; the receipt the Providers panel
  /// shows next to `Refresh status` once the spinner is gone.
  @Published private(set) var providerAccessCheckedAt: Date?
  @Published private(set) var providerAccessError: String?
  @Published private(set) var providerAccountErrors: [String: String] = [:]
  private var providerAccessGeneration: UInt64 = 0
  private var providerRefreshRequested = false

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
  private var whisperDownloadEventTask: Task<Void, Never>?

  // MARK: - Local Whisper model picker (Settings → Dictation)

  /// Canonical option catalog + saved preference + resident-engine truth.
  /// Nil until the first live refresh; previews stay on the engine's sample.
  @Published private(set) var whisperModelCatalog: CsWhisperModelCatalog?
  /// Honest outcome of the last selection ("applies from the next take" …).
  @Published private(set) var whisperModelNotice: String?
  /// Real validation/load error of the last failed selection; the picker never
  /// announces a new active model while this is set.
  @Published private(set) var whisperModelError: String?
  @Published private(set) var whisperModelSwitchPending = false

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
  private let creatorAgentBridge: AgentBridgeInstalling
  private let permissionProbe: PermissionProbing
  private let agentStatus: AgentStatusEngine?
  private let mcpAdmin: MCPAdminEngine?
  private let hotkeys: HotkeysEngine?
  private let licenseService: LicenseService
  private let runtimeLlmLaneProvider: (CsLlmLane) -> CsRuntimeLlmLane
  private let audioRecordingControlProvider: () -> (state: OverlayState, tray: TrayViewModel)?
  /// Sealed runtime lane projections for this refresh. `llmLane` is read from
  /// SwiftUI `body` (once per menu item); the FFI load is not.
  private var runtimeLaneCache: [LLMLane: CsRuntimeLlmLane] = [:]
  private var modelDiscoveryGenerations: [String: Int] = [:]

  init(
    engine: SettingsEngine? = nil,
    creatorAgentBridge: AgentBridgeInstalling = RealAgentBridgeInstaller(),
    permissionProbe: PermissionProbing = NativePermissionProbe(),
    agentStatus: AgentStatusEngine? = nil,
    mcpAdmin: MCPAdminEngine? = nil,
    hotkeys: HotkeysEngine? = nil,
    licenseService: LicenseService? = nil,
    buildInfo: AppBuildInfo = .current(),
    runtimeLlmLaneProvider: @escaping (CsLlmLane) -> CsRuntimeLlmLane = { lane in
      runtimeLlmLane(lane: lane)
    },
    audioRecordingControlProvider: (() -> (state: OverlayState, tray: TrayViewModel)?)? = nil,
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
    self.creatorAgentBridge = creatorAgentBridge
    self.permissionProbe = permissionProbe
    self.agentStatus = agentStatus
    self.mcpAdmin = mcpAdmin
    self.hotkeys = hotkeys
    self.licenseService = licenseService ?? .preview
    self.buildInfo = buildInfo
    self.runtimeLlmLaneProvider = runtimeLlmLaneProvider
    self.audioRecordingControlProvider =
      audioRecordingControlProvider ?? Self.liveAudioRecordingControlProvider(for: engine)
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
    self.keyStatus = engine?.keyStatus() ?? .sampleAllSet
    self.providers = engine == nil ? CsProviderOption.sampleProviders : []
    self.providerAccessResolved = engine == nil
    self.sttLanes = engine?.sttLanes() ?? [.sampleFile, .sampleLive]
    self.configDir = ""
    self.needsOnboarding = false
    self.agentReadiness = engine == nil ? .sample : CsAgenticReadiness(configPathDisplay: "", ready: false, rows: [])
    self.mcpStatus = .sample
    self.capabilityMatrix = CsCapabilityRow.sampleMatrix
    self.voiceLabReadError = nil
    self.audioInput = .sample
    self.audioInputReadError = nil
    self.admission = nil
    self.admissionReadError = nil
    self.resetPreview = .sample
    self.agentResetPreview = .sample
    licenseChangeSink = self.licenseService.objectWillChange.sink { [weak self] _ in
      self?.objectWillChange.send()
    }
    // Settings owns this projection even when a deep link skips Creator.
    agentBridgeSynchronizationSink = NotificationCenter.default.publisher(
      for: Self.agentBridgeLaunchSynchronizationDidFinish
    ).sink { [weak self] _ in
      self?.refreshCreatorAgentBridge()
    }
    lastServingVerdict = servingStatusProvider()
  }

  /// Passive inspection of the bundled installer; never attaches an agent.
  func refreshCreatorAgentBridge() {
    creatorAgentBridgeStatus = creatorAgentBridge.status()
    creatorAgentBridgeLaunchDetail = Self.agentBridgeLaunchNotice
  }

  /// Add/update one client while preserving other managed clients. Creator
  /// has no implicit deselection or deletion action.
  func installCreatorAgentBridge(for client: AgentBridgeClient) {
    let current = creatorAgentBridge.status()
    creatorAgentBridgeStatus = current
    creatorAgentBridgeError = nil
    creatorAgentBridgeNotice = nil
    guard current.payloadAvailable else {
      creatorAgentBridgeError = current.detail
      return
    }
    do {
      let clients = Set(current.installedClients).union([client])
      creatorAgentBridgeStatus = try creatorAgentBridge.install(selectedClients: clients)
      creatorAgentBridgeNotice = String(
        localized:
          "Skill installed from this app. Reload skills in your agent client, then invoke /codescribe. Installation does not attach a listener or verify voice delivery.",
        comment: "/codescribe is a command the user types — keep it verbatim"
      )
    } catch {
      creatorAgentBridgeError = error.userFacingMessage
      creatorAgentBridgeStatus = creatorAgentBridge.status()
    }
  }

  /// Called only after explicit confirmation to preserve and replace a manual copy.
  func adoptCreatorManualSkill(for client: AgentBridgeClient) {
    creatorAgentBridgeError = nil
    creatorAgentBridgeNotice = nil
    do {
      let result = try creatorAgentBridge.adoptManualSkill(client: client)
      creatorAgentBridgeStatus = result.status
      let preserved = result.backupPaths.joined(separator: "\n")
      creatorAgentBridgeNotice = String(
        localized:
          "Installed from this app. Original folder preserved at:\n\(preserved)\nReload your agent client's skills, then invoke /codescribe. Voice delivery is not yet verified.",
        comment:
          "The placeholder is a newline-separated path list; /codescribe is a command — keep it verbatim"
      )
    } catch {
      creatorAgentBridgeError = error.userFacingMessage
      creatorAgentBridgeStatus = creatorAgentBridge.status()
    }
  }

  /// Re-read live state (permissions can change while the window is open).
  func refresh() {
    refreshPermissions()
    refreshServingStatus()
    if let engine {
      applyLoadedSettings(engine.loadSettings())
      refreshProviderAccess()
      configDir = engine.configDir()
      needsOnboarding = engine.shouldShowOnboarding()
      refreshVoiceLab()
      refreshAudioInput()
    }
    refreshWhisperModelStatus()
    refreshWhisperModelCatalog()
    refreshAgentStatus()
    refreshCreatorAgentBridge()
    reloadMcpServers()
    loadHotkeys()
    licenseService.refresh()
  }

  func refreshPermissions() {
    permissions = permissionProbe.snapshot()
    // A permission granted while Settings is open (e.g. via the checklist's
    // "Open System Settings") should bring hotkeys live without an app
    // restart. Idempotent bridge call — a no-op once the tap is already armed.
    hotkeys?.rearmAfterPermissionGrant()
  }

  func refreshLicense() { licenseService.refresh() }

  var licenseError: String? { licenseService.lastError }
  var licenseErrorDetails: String? { licenseService.lastErrorDetails }

  @discardableResult
  func activateLicense(_ key: String) async -> Bool {
    await licenseService.activate(key)
  }

  func removeLicense() async {
    await licenseService.removeLicense()
  }

  /// Re-probe Whisper install state (embedded / on-disk / missing).
  func refreshWhisperModelStatus() {
    // Live UniFFI path; sample mode (engine == nil in pure previews) keeps
    // the static placeholder so canvas previews stay offline-safe.
    guard engine != nil else { return }
    localWhisperStatus = whisperModelStatus()
  }

  /// Re-read the canonical local-Whisper catalog (options + saved preference +
  /// resident-engine truth). Refreshed together with the install status so the
  /// picker, the footprint rows and the install row never disagree.
  func refreshWhisperModelCatalog() {
    guard let engine else { return }
    whisperModelCatalog = engine.loadWhisperModelCatalog()
  }

  /// The saved selection, for the picker's binding. Falls back to the settings
  /// snapshot before the first live catalog refresh.
  var whisperModelSelection: String {
    whisperModelCatalog?.configured ?? settings.localModel
  }

  /// Persist a picker selection through the bridge: validate → settings.json →
  /// switch the running process (or defer to the recording-idle boundary).
  /// On failure the real error is shown and the picker snaps back to the
  /// persisted truth on the next catalog refresh.
  func selectWhisperModel(_ reference: String) {
    guard let engine else { return }
    guard !whisperModelSwitchPending else { return }
    whisperModelSwitchPending = true
    whisperModelNotice = nil
    whisperModelError = nil
    Task { @MainActor [weak self] in
      guard let self else { return }
      defer { self.whisperModelSwitchPending = false }
      do {
        let outcome = try await engine.selectLocalWhisperModel(reference: reference)
        self.whisperModelNotice =
          outcome.applied
          ? String(
            localized: "Saved · applies from the next take",
            comment: "Whisper model picker: selection persisted and the engine switches at the next take"
          )
          : outcome.pending
            ? String(
              localized: "Saved · applies when the current recording finishes",
              comment: "Whisper model picker: selection deferred because a take owns the engine"
            )
            : String(
              localized: "Saved · an override decides the active model (see note below)",
              comment: "Whisper model picker: selection persisted but an env override shadows it"
            )
      } catch {
        self.whisperModelError = String(describing: error)
      }
      self.refreshWhisperModelCatalog()
      self.refreshWhisperModelStatus()
    }
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
    whisperDownloadDetail = String(localized: "Starting download…")
    whisperDownloadFraction = nil
    lastError = nil
    let channel = AsyncStream<WhisperDownloadEvent>.makeStream()
    let sink = WhisperDownloadProgressSink(continuation: channel.continuation)
    whisperDownloadSink = sink
    whisperDownloadEventTask = Task { @MainActor [weak self] in
      for await event in channel.stream {
        guard let self else { return }
        switch event {
        case .progress(let detail, let fraction):
          self.applyWhisperDownloadProgress(detail: detail, fraction: fraction)
        case .complete(let path):
          self.applyWhisperDownloadProgress(
            detail: String(
              localized: "Saved · \(path)",
              comment: "Download finished; the placeholder is a file path"
            ),
            fraction: 1.0
          )
        }
      }
    }
    Task { @MainActor [weak self] in
      guard let self else { return }
      do {
        let status = try await downloadWhisperModel(listener: sink)
        self.localWhisperStatus = status
        let location = status.path ?? status.modelId
        self.whisperDownloadDetail =
          status.available
          ? String(
            localized: "Ready · \(location)",
            comment: "The placeholder is a model path or model id"
          )
          : String(localized: "Download finished but model still unavailable")
        self.whisperDownloadFraction = status.available ? 1.0 : nil
      } catch {
        self.lastError = String(describing: error)
        self.whisperDownloadDetail = String(localized: "Download failed")
        self.whisperDownloadFraction = nil
      }
      sink.finish()
      if let eventTask = self.whisperDownloadEventTask {
        await eventTask.value
      }
      self.whisperDownloadInFlight = false
      self.whisperDownloadSink = nil
      self.whisperDownloadEventTask = nil
    }
  }

  /// Re-probe just the agent substrate (readiness + MCP + capability matrix).
  /// Cheap on-disk reads; used by the Agent panel's "Refresh" action so
  /// re-checking MCP does not disturb the rest of the panel.
  func refreshAgentStatus() {
    runtimeLaneCache.removeAll()
    guard let agentStatus else { return }
    guard providerAccessResolved else { return }
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
    let label =
      bindingOptions.first { $0.binding == binding }?.label
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
  ///
  /// NEVER on the main thread: building the capability list spawns and
  /// handshakes every configured MCP server (`discover_mcp_tools_blocking`),
  /// which costs seconds with a dozen servers configured. The 2026-08-13
  /// beachball sample held the whole app in `pthread_join` inside this call —
  /// on panel open AND on every permission click. Snapshot off-main, publish
  /// back on the main actor; a stale list for a moment beats a frozen app.
  func reloadToolPermissions() {
    guard let mcpAdmin else { return }
    Task { @MainActor [weak self] in
      let (policy, capabilities) = await mcpAdmin.loadPermissionSurface()
      guard let self else { return }
      self.permissionPolicy = policy
      self.toolCapabilities = capabilities
    }
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
    // Landing on a section shows its first tab; sections without tabs keep
    // tab == nil and render whole.
    select(target, tab: SettingsTab.tabs(in: target).first)
  }

  /// Select a specific tab without publishing a temporary first-tab landing.
  func select(_ target: SettingsTab) {
    select(target.section, tab: target)
  }

  private func select(_ target: SettingsSection, tab targetTab: SettingsTab?) {
    guard target.availability == .available else { return }
    if targetTab == .agentStatus { refreshCreatorAgentBridge() }
    guard section != target || tab != targetTab else { return }
    if section != target { section = target }
    if tab != targetTab { tab = targetTab }
    if target == .agent {
      refreshModelDiscoveries(providerIds: LLMLane.allCases.map { llmLane($0).providerId })
    }
    if target == .engine {
      refreshServingStatus()
    }
  }

  func select(_ target: SettingsDeepLinkTarget) {
    if let tab = target.tab {
      select(tab)
    } else {
      select(target.section)
    }
  }

  // MARK: - Reset app data (recoverable destructive action)

  func refreshResetPreview() {
    guard let engine else { return }
    resetPreview = engine.resetPreview()
  }

  /// Composed sentence by sentence: the impact summary is its own key, and each
  /// following sentence is localized whole so a translation can be reordered.
  func resetImpactDescription(includeKeys: Bool, includePrompts: Bool) -> String {
    let impact = resetImpactSummary(resetPreview)
    var message = String(
      localized: "Moves \(impact) to Trash.",
      comment: "The placeholder is the counted reset impact"
    )
    if includePrompts {
      message += " "
      message += String(
        localized: "Your assistive.txt and three formatting prompt files will also move to Trash.",
        comment: "assistive.txt is a file name — keep it verbatim"
      )
    } else {
      message += " "
      message += String(
        localized: "Your assistive.txt and three formatting prompt files will be preserved.",
        comment: "assistive.txt is a file name — keep it verbatim"
      )
    }
    if includeKeys {
      message += " "
      message += String(
        localized: "API keys will also be removed from Keychain and are not recoverable from Trash."
      )
    }
    return message + " " + String(localized: "Codescribe will relaunch as a fresh install.")
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

  // MARK: - Reset Agent (narrow destructive action)

  func refreshAgentResetPreview() {
    guard let engine else { return }
    agentResetPreview = engine.resetAgentPreview()
  }

  /// Two independent counts in the first sentence: the catalog carries one
  /// plural variation per argument. The following sentences are whole keys.
  func resetAgentImpactDescription() -> String {
    let preview = agentResetPreview
    let threads = Int(preview.threads)
    let files = Int(preview.files)
    let secretState =
      preview.secretsPresent
      ? String(
        localized:
          "Agent provider and MCP connector secrets are present and will be deleted permanently."
      )
      : String(localized: "No Agent provider or MCP connector secrets are currently stored.")
    let moved = String(
      localized: "Moves \(threads) Agent threads and \(files) Agent files to Trash.",
      comment: "Agent reset impact; each count needs its own plural variation"
    )
    let unchanged = String(
      localized:
        "Recordings, transcriptions, dictionary and lexicon data, quality corpus and reports, prompts, audio, hotkeys, dictation settings, license, and macOS permissions stay unchanged."
    )
    return moved + " " + secretState + " " + unchanged
  }

  func resetAgentData() {
    guard let engine else { return }
    do {
      try engine.resetAgentData()
      mcpTestResults = [:]
      reloadMcpServers()
      refreshAgentStatus()
      refreshAgentResetPreview()
      AppRelaunch.clearAgentDefaultsAndRelaunch()
    } catch {
      let description = String(describing: error)
      lastError = description
      if agentResetFailureRequiresRelaunch(description) {
        AppRelaunch.clearAgentDefaultsAndRelaunch()
      }
    }
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
  /// (UniFFI `currentServingVerdict()`) in `refresh()`, on panel appear, and
  /// after stop. Published so a take while Settings is open does not keep a
  /// stale nil snapshot.
  @Published var lastServingVerdict: LastServingVerdict?

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

  /// STT is "healthy" (olive dot) when a local model is configured, or when
  /// either cloud lane has an endpoint. Runtime serving truth comes from the
  /// shared controller snapshot, not this configuration-only health estimate.
  var sttHealthy: Bool {
    if settings.useLocalStt { return !settings.localModel.isEmpty }
    return settings.sttFileEndpoint?.isEmpty == false
      || settings.sttLiveEndpoint?.isEmpty == false
  }

  var whisperLanguageCode: String { settings.whisperLanguage.shortCode }

  var sttModelDescription: String {
    // `localModel` (LOCAL_MODEL) is the local execution path; `whisperModel`
    // (WHISPER_MODEL) is the cloud HTTP STT model id. Show the value the
    // active engine actually resolves so the label and the runtime path agree.
    let localEngineInPlay = settings.useLocalStt || asrModeId == "local_power"
    let preference = localEngineInPlay
      ? settings.localModel
      : (settings.whisperModel ?? settings.localModel)
    return preference.isEmpty
      ? String(localized: "unset", comment: "Model row: no model preference stored, lower case")
      : preference
  }

  private var assistiveKeyState: SettingsKeyState {
    guard providerAccessResolved, providerAccessError == nil else { return .unknown }
    guard let provider = llmLane(.assistive).provider else { return .unknown }
    let runtime = llmLane(.assistive).runtime
    if providerAccountErrors[runtime.providerId] != nil, !runtime.keyPresent, provider.keyRequired {
      return .unknown
    }
    let keyAvailable = runtime.accountAuth || runtime.keyPresent || !provider.keyRequired
    return keyAvailable ? .available : .missing
  }

  var settingsHealth: SettingsHealthState {
    healthState(
      stt: sttHealthy,
      recording: admissionReadError == nil ? admission?.ready : nil,
      keys: assistiveKeyState,
      agent: providerAccessResolved && providerAccessError == nil && assistiveKeyState != .unknown
        ? agentReadiness.ready && llmLane(.assistive).runtime.available : nil,
      formatting: providerAccessResolved && providerAccessError == nil
        ? llmLane(.formatting).runtime.available : nil,
      formattingRequired: cloudFormattingRequired
    )
  }

  /// Enabled Formatting still exposes cloud requests even with on-device execution selected.
  var cloudFormattingRequired: Bool {
    settings.aiFormattingEnabled
      && FormattingPolicyOption(storedValue: settings.formattingLevel) != .off
  }

  func laneUsageDescription(_ lane: LLMLane) -> String {
    if lane == .assistive { return llmLane(lane).availabilityDescription }
    if !settings.aiFormattingEnabled
      || FormattingPolicyOption(storedValue: settings.formattingLevel) == .off
    {
      return String(localized: "Formatting is disabled. This lane is not required for readiness.")
    }
    if settings.formatOnDevice {
      return String(
        localized:
          "Apple on-device formatting is selected. Cloud requests still require this lane's credentials. \(llmLane(lane).availabilityDescription)",
        comment: "The placeholder is the resolved cloud lane availability")
    }
    return llmLane(lane).availabilityDescription
  }

  /// Effective lane state: loader-sealed runtime truth + registry row +
  /// discovery for THAT lane's provider (vendor or custom alike, D2).
  func llmLane(_ lane: LLMLane) -> LLMLaneModel {
    let runtime: CsRuntimeLlmLane
    if let cached = runtimeLaneCache[lane] {
      runtime = cached
    } else {
      let loaded = runtimeLlmLaneProvider(lane.bridgeLane)
      runtimeLaneCache[lane] = loaded
      runtime = loaded
    }
    let configuredModel =
      settings[keyPath: lane.modelPath]?
      .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    return LLMLaneModel(
      lane: lane,
      runtime: runtime,
      provider: providers.first { $0.id == runtime.providerId },
      configuredModel: configuredModel,
      discovery: modelDiscoveries[runtime.providerId]
        ?? CsModelDiscovery.sample(for: runtime.providerId),
      credentialAccessResolved: providerAccessResolved,
      credentialAccessError: providerAccessError
        ?? (lane == .assistive && !runtime.keyPresent ? providerAccountErrors[runtime.providerId] : nil)
    )
  }

  /// Bind a lane to a provider (vendor id or `custom:<id>`); the bridge validates
  /// and persists `LLM_<LANE>_PROVIDER`. The stored model belonged to the previous
  /// provider, so it is cleared (integrator decision, W1-T3R).
  func setLaneProvider(_ providerId: String, for lane: LLMLane) {
    guard let engine else { return }
    do {
      try engine.setLaneProvider(lane: lane.bridgeLane, providerId: providerId)
    } catch {
      lastError = String(describing: error)
      return
    }
    laneResetNotice = nil
    setLLMModel("", for: lane)
    refreshModelDiscovery(providerId: providerId)
    refreshAgentStatus()
  }

  /// Persist a model for one LLM lane. Empty clears the JSON override.
  func setLLMModel(_ value: String, for lane: LLMLane) {
    let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
    settings[keyPath: lane.modelPath] = trimmed.isEmpty ? nil : trimmed
    persist(lane.modelKey, trimmed)
  }

  var formattingDescription: String {
    guard settings.aiFormattingEnabled else {
      return String(
        localized: "disabled · AI formatting off",
        comment: "Formatting status: the AI formatting master switch is off")
    }
    return FormattingPolicyOption(storedValue: settings.formattingLevel)?.visibleName
      ?? String(localized: "invalid policy", comment: "Formatting status: stored level is unknown")
  }

  // MARK: - Creator mutations (write through the core router)

  var maxConsultationEnabled: Bool {
    settings.aiFormattingEnabled
      && FormattingPolicyOption(storedValue: settings.formattingLevel) == .max
  }

  func refreshMaxToolApprovals() async {
    guard let engine else { return }
    maxApprovalRefreshRequested = true
    guard !maxApprovalBusy else { return }
    maxApprovalBusy = true
    defer { maxApprovalBusy = false }
    repeat {
      maxApprovalRefreshRequested = false
      do {
        maxToolApprovals = try await engine.pendingMaxToolApprovals()
        maxApprovalError = nil
      } catch {
        maxApprovalError = error.userFacingMessage
      }
    } while maxApprovalRefreshRequested
  }

  func resolveMaxToolApproval(
    _ request: PendingToolApproval, approved: Bool, remember: Bool = false
  ) async {
    guard let engine, !maxApprovalBusy, maxApprovalError == nil,
      maxToolApprovals.contains(request)
    else { return }
    maxApprovalBusy = true
    do {
      let resolved = try await engine.resolveMaxToolApproval(
        request, approved: approved, remember: remember
      )
      maxApprovalError =
        resolved ? nil : String(localized: "This permission request is no longer active.")
      maxToolApprovals = try await engine.pendingMaxToolApprovals()
    } catch {
      maxApprovalError = error.userFacingMessage
    }
    maxApprovalBusy = false
    if maxApprovalRefreshRequested {
      await refreshMaxToolApprovals()
    }
  }

  func beginNewMaxConsultation() async {
    guard maxConsultationEnabled, !newMaxConsultationPending else { return }
    guard let engine else {
      maxConsultationNotice = String(localized: "Consultation reset is unavailable.")
      return
    }
    newMaxConsultationPending = true
    maxConsultationNotice = nil
    defer { newMaxConsultationPending = false }
    do {
      _ = try await engine.beginNewMaxConsultation()
      maxConsultationNotice = String(
        localized: "New consultation started. Previous history is preserved."
      )
    } catch {
      maxConsultationNotice = String(
        localized: "Could not start a new consultation: \(error.userFacingMessage)",
        comment: "The placeholder is a system error message"
      )
    }
  }

  func setLanguage(_ lang: CsLanguage) {
    settings.whisperLanguage = lang
    persist("WHISPER_LANGUAGE", lang.shortCode)
  }

  func setFormattingEnabled(_ on: Bool) {
    settings.aiFormattingEnabled = on
    persist("AI_FORMATTING_ENABLED", on ? "1" : "0")
  }

  func setAgentAutoSend(_ on: Bool) {
    persist("AGENT_AUTO_SEND", on ? "1" : "0")
  }

  func setFormattingLevel(_ level: String) {
    guard let policy = FormattingPolicyOption(storedValue: level) else {
      lastError = String(
        localized: "Unknown formatting policy: \(level)",
        comment: "The placeholder is a stored policy identifier"
      )
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

  /// Borrow both existing owners together only when Audio appears. Settings
  /// never attaches a listener or opens a separate recorder.
  func audioRecordingControls() -> (state: OverlayState, tray: TrayViewModel)? {
    audioRecordingControlProvider()
  }

  /// Fake engines stay detached unless their matching owners are injected.
  /// Resolving live owners is deferred until Audio appears, never model init.
  private static func liveAudioRecordingControlProvider(
    for engine: SettingsEngine?
  ) -> () -> (state: OverlayState, tray: TrayViewModel)? {
    guard engine is RealSettingsEngine else { return { nil } }
    return {
      let app = AppModel.shared
      return (state: app.overlay.state, tray: app.tray)
    }
  }

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

  var audioRetention: String { settings.audioRetention }

  func setAudioRetention(_ value: String) {
    // The picker reflects the reloaded effective snapshot, including save failures.
    persist("AUDIO_RETENTION", value)
  }

  func setAudioInputDevice(_ device: String) {
    settings.audioInputDevice = device
    persist("AUDIO_INPUT_DEVICE", device)
    refreshAudioInput()
  }

  // MARK: - Acoustic admission (controller truth, never a second decision)

  /// Seconds of normal speech the guided calibration captures.
  static let calibrationCaptureSeconds: UInt32 = 10

  static func calibrationProgress(elapsedSeconds: TimeInterval) -> Double {
    min(max(elapsedSeconds / Double(calibrationCaptureSeconds), 0), 1)
  }

  static func calibrationRemainingSeconds(elapsedSeconds: TimeInterval) -> Int {
    max(Int(ceil(Double(calibrationCaptureSeconds) - elapsedSeconds)), 0)
  }

  func refreshAdmission() async {
    guard let engine else { return }
    do {
      admission = try await engine.loadAdmissionReadiness()
      admissionReadError = nil
    } catch {
      admissionReadError = String(describing: error)
    }
  }

  /// Persist the product's mandatory-lane policy through the existing config
  /// router. The bridge resolves any power-user env override separately; this
  /// action never creates, edits, or clears that override.
  func setSealLaneArmed(_ armed: Bool) {
    persist("CODESCRIBE_SILERO_FUSION", armed ? "1" : "0")
  }

  /// Run the guided calibration through the real recorder path, then re-read
  /// admission so the row reflects the controller's verdict, not a guess.
  func runCalibration() async {
    guard let engine, !calibrationPending else { return }
    calibrationPending = true
    calibrationStartedAt = Date()
    calibrationNotice = nil
    defer {
      calibrationPending = false
      calibrationStartedAt = nil
    }
    do {
      let report = try await engine.calibrateEnergy(
        seconds: Self.calibrationCaptureSeconds)
      calibrationNotice = Self.calibrationSummary(report)
    } catch {
      let failure = String(describing: error)
      calibrationNotice = String(
        localized: "Calibration refused: \(failure)",
        comment: "The placeholder is a technical failure description"
      )
    }
    await refreshAdmission()
  }

  /// One-line, number-honest summary of a stored calibration profile.
  static func calibrationSummary(_ report: CsEnergyCalibrationReport) -> String {
    let speech = String(format: "%.1f", report.activeSpeechMedianDbfs)
    let peak = String(format: "%.1f", report.peakDbfs)
    let floor = String(format: "%.1f", report.existenceThresholdDbfs)
    let seconds = String(format: "%.1f", report.measuredSeconds)
    let device = report.deviceName
    return String(
      localized:
        "Calibrated \(device): speech \(speech) dBFS, peak \(peak) dBFS over \(seconds) s → existence floor \(floor) dBFS",
      comment:
        "First placeholder is a device name; the rest are pre-formatted numbers. dBFS is a unit"
    )
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

  func setWhisperContextWindowSeconds(_ seconds: Float) {
    settings.whisperContextWindowSec = seconds
    persist("WHISPER_CONTEXT_WINDOW_SEC", String(format: "%.1f", seconds))
  }

  func setLightPlusSentencePauseSeconds(_ seconds: Float) {
    let bounded = min(2.0, max(0.3, seconds))
    settings.lightPlusSentencePauseSec = bounded
    persist("LIGHT_PLUS_SENTENCE_PAUSE_SEC", String(format: "%.1f", bounded))
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
      let listing = try engine.loadQualityRecentListing(limit: 50)
      qualityRecords = listing.records
      unchangedQualityTakes = listing.unchangedTakes
      customLexiconEntries = try engine.loadLexiconCustomEntries()
      ruleCandidates = try engine.loadRuleCandidates(minOccurrences: 2)
      voiceLabReadError = nil
    } catch {
      qualityRecords = []
      unchangedQualityTakes = 0
      customLexiconEntries = []
      ruleCandidates = []
      voiceLabReadError = String(describing: error)
    }
  }

  /// Teach one rule candidate (variant → canonical) into the live custom
  /// dictionary. Updates the candidate list on success.
  func teachRuleCandidate(target: String, variant: String) {
    guard let engine, !voiceLabTeachPending else { return }
    voiceLabTeachPending = true
    voiceLabTeachMessage = nil
    Task { @MainActor [weak self] in
      guard let self else { return }
      do {
        let result = try engine.teachSpan(variant: variant, canonical: target, kind: "vocabulary")
        self.voiceLabTeachPending = false
        self.voiceLabTeachMessage = result.acknowledgement
        self.refreshVoiceLab()
      } catch {
        self.voiceLabTeachPending = false
        let message = String(describing: error)
        self.voiceLabTeachMessage = String(
          localized: "Teach failed: \(message)",
          comment: "The placeholder is a technical failure description"
        )
        self.lastError = message
      }
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
    Task { @MainActor [weak self] in
      guard let self else { return }
      do {
        let result = try await engine.teachDictionaryFromStoreAsync()
        self.voiceLabTeachPending = false
        let fromCorrections = Int(result.fromCorrections)
        let fromProposed = Int(result.fromProposed)
        let totalRules = Int(result.totalRules)
        let correctionSourced = Int(result.rulesFromCorrectionSource)
        self.voiceLabTeachMessage = String(
          localized:
            "Taught +\(fromCorrections) from corrections, +\(fromProposed) from proposed → \(totalRules) live rules (\(correctionSourced) correction-sourced).",
          comment: "Counted teach summary; the live-rules count needs a plural variation"
        )
        self.refreshVoiceLab()
      } catch {
        self.voiceLabTeachPending = false
        let message = String(describing: error)
        self.voiceLabTeachMessage = String(
          localized: "Teach failed: \(message)",
          comment: "The placeholder is a technical failure description"
        )
        self.lastError = message
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
      return String(
        localized: "Saved — dictionary learning failed: \(lexiconError)",
        comment: "The placeholder is a technical failure description"
      )
    }
    if outcome.pairsLearned == 0 {
      return String(localized: "Saved; no dictionary rule derived")
    }
    let rules = Int(outcome.pairsLearned)
    return String(
      localized: "Saved — \(rules) rules learned",
      comment: "Needs a plural variation: one rule learned / N rules learned"
    )
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
        CsConfigEntry(key: "TRANSCRIPTION_OVERLAY_ENABLED", value: "0")
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
    onOverlayPreferenceChanged()
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

  // MARK: - ASR mode

  /// Product ASR lane shown in Dictation. Cloud never displays without granted consent.
  var asrModeId: String {
    let raw = (settings.asrMode ?? "apple_only").lowercased()
    switch raw {
    case "local_power":
      return "local_power"
    case "cloud":
      return cloudConsentGranted ? "cloud" : "apple_only"
    default:
      return "apple_only"
    }
  }

  var asrModeLabel: String {
    switch asrModeId {
    case "local_power":
      return String(localized: "Local power", comment: "Dictation lane: local model weights")
    case "cloud": return String(localized: "Cloud", comment: "Dictation lane: cloud refinement")
    default:
      return String(localized: "Apple only", comment: "Dictation lane: on-device Apple Speech")
    }
  }

  var cloudConsentGranted: Bool {
    settings.cloudConsent?.lowercased() == "granted"
  }

  /// Persist the product lane. Selecting Cloud is the explicit grant.
  func setAsrMode(_ id: String) {
    switch id.lowercased() {
    case "local_power":
      settings.asrMode = "local_power"
      persist("CODESCRIBE_ASR_MODE", "local_power")
      refreshWhisperModelStatus()
    case "cloud":
      settings.asrMode = "cloud"
      settings.cloudConsent = "granted"
      persistMany([
        CsConfigEntry(key: "CODESCRIBE_CLOUD_CONSENT", value: "granted"),
        CsConfigEntry(key: "CODESCRIBE_ASR_MODE", value: "cloud"),
      ])
    default:
      settings.asrMode = "apple_only"
      persist("CODESCRIBE_ASR_MODE", "apple_only")
    }
  }

  var asrGatewayUrl: String { settings.asrGatewayUrl ?? "" }

  /// Throws the bridge's rejection so the row can show it under the field.
  func setAsrGatewayUrl(_ value: String) throws {
    try persistOrThrow("CODESCRIBE_ASR_GATEWAY_URL", value.trimmingCharacters(in: .whitespaces))
  }

  var localWhisperRuntimeState: LocalWhisperRuntimeState {
    resolveLocalWhisperRuntimeState(
      asrModeId: asrModeId,
      layeredValue: settings.layeredTranscription,
      modelAvailable: localWhisperStatus.available
    )
  }

  /// Re-read the diagnostic env override and full model-bundle validation.
  /// This recheck never paints an optimistic ON state.
  func recheckLocalWhisperRuntime() {
    guard let engine else { return }
    applyLoadedSettings(engine.loadSettings())
    refreshWhisperModelStatus()
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
  /// Peer surfaces re-read the same persisted snapshot when they appear.
  func setHoldBadgeOption(_ option: HoldBadgeOption) {
    guard let size = option.size else {
      settings.holdIndicator = false
      persist("HOLD_INDICATOR", "0")
      return
    }
    settings.holdIndicator = true
    settings.holdBadgeSize = size
    persistMany([
      CsConfigEntry(key: "HOLD_INDICATOR", value: "1"),
      CsConfigEntry(key: "HOLD_BADGE_SIZE", value: String(size)),
    ])
  }

  /// Assistive-arm modifier on the hold base: `"shift"` (default) or `"cmd"`.
  var holdArmModifier: String {
    let raw = settings.holdArmModifier.lowercased()
    return (raw == "cmd" || raw == "command") ? "cmd" : "shift"
  }

  func setHoldArmModifier(_ value: String) {
    let normalized =
      (value.lowercased() == "cmd" || value.lowercased() == "command")
      ? "cmd" : "shift"
    settings.holdArmModifier = normalized
    persist("HOLD_ARM_MODIFIER", normalized)
  }

  /// Agent-channel modifier: `"ctrl"` (default) or `"fn"`. Command is not offered.
  var channelModifier: String {
    settings.channelModifier.lowercased() == "fn" ? "fn" : "ctrl"
  }

  func setChannelModifier(_ value: String) {
    let normalized = value.lowercased() == "fn" ? "fn" : "ctrl"
    settings.channelModifier = normalized
    writeHotkeySurface("AGENT_CHANNEL_MODIFIER", normalized) {
      try hotkeys?.setChannelModifier(normalized)
    }
  }

  /// Quick Fn press toggles dictation. Off until the Founder turns it on.
  var fnTapTogglesDictation: Bool { settings.fnTapTogglesDictation }

  func setFnTapTogglesDictation(_ enabled: Bool) {
    settings.fnTapTogglesDictation = enabled
    writeHotkeySurface("FN_TAP_TOGGLES_DICTATION", enabled ? "1" : "0") {
      try hotkeys?.setFnTapTogglesDictation(enabled)
    }
  }

  /// Middle mouse button follows Fn. Off until chosen. The click is not swallowed.
  var middleMouseActsAsFn: Bool { settings.middleMouseActsAsFn }

  func setMiddleMouseActsAsFn(_ enabled: Bool) {
    settings.middleMouseActsAsFn = enabled
    writeHotkeySurface("MIDDLE_MOUSE_ACTS_AS_FN", enabled ? "1" : "0") {
      try hotkeys?.setMiddleMouseActsAsFn(enabled)
    }
  }

  /// One settings.json write. The hotkeys seam persists when it is injected;
  /// otherwise the shared config router does. Either path reloads the detector.
  private func writeHotkeySurface(
    _ key: String, _ value: String, viaHotkeys: () throws -> Void
  ) {
    if hotkeys != nil {
      do {
        try viaHotkeys()
        if let engine { applyLoadedSettings(engine.loadSettings()) }
      } catch {
        lastError = String(describing: error)
      }
      return
    }
    persist(key, value)
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

  /// Automatic paste mode from the canonical settings snapshot.
  var pasteMode: CsPasteMode { settings.pasteMode }

  /// Persists `PASTE_MODE` through the config router (settings.json; the
  /// running controller reloads on write). Optimistic like the other setters:
  /// the selection sticks and a failed write surfaces in `lastError`.
  func setPasteMode(_ mode: CsPasteMode) {
    settings.pasteMode = mode
    persist("PASTE_MODE", mode.wireId)
  }

  // MARK: - Agent workspace roots (list_projects tool)

  /// Effective workspace roots the `list_projects` tool scans. Never empty —
  /// the bridge fills the built-in default (`~/.codescribe`) when unset.
  var agentWorkspaceRoots: [String] { settings.agentWorkspaceRoots }

  /// Persist the workspace roots as the colon-joined `AGENT_WORKSPACE_ROOTS`
  /// value. Blank/whitespace rows are dropped; an all-empty list clears the
  /// override so the core falls back to `~/.codescribe`.
  func setAgentWorkspaceRoots(_ roots: [String]) {
    let cleaned =
      roots
      .map { $0.trimmingCharacters(in: .whitespaces) }
      .filter { !$0.isEmpty }
    settings.agentWorkspaceRoots = cleaned.isEmpty ? ["~/.codescribe"] : cleaned
    persist("AGENT_WORKSPACE_ROOTS", cleaned.joined(separator: ":"))
  }

  /// Persist one lane's endpoint (`STT_FILE_ENDPOINT` / `STT_LIVE_ENDPOINT`). Blank
  /// clears; the bridge validates the scheme per lane and a rejection lands in `lastError`.
  /// Throws the bridge's rejection (wrong scheme for the lane, plaintext off
  /// loopback, credentials in the URL) so the row can show it under the field.
  func setSttLaneEndpoint(_ id: String, _ value: String) throws {
    guard let lane = sttLanes.first(where: { $0.id == id }) else { return }
    providerAccessGeneration &+= 1
    defer { if let engine { sttLanes = engine.sttLanes() } }
    try persistOrThrow(lane.endpointWireKey, value.trimmingCharacters(in: .whitespaces))
  }

  func setWhisperAdaptiveBuffer(_ enabled: Bool) {
    guard DeveloperSurface.isEnabled() else { return }
    persist("WHISPER_ADAPTIVE_BUFFER", enabled ? "1" : "0")
  }

  func setFormatOnDevice(_ enabled: Bool) {
    guard DeveloperSurface.isEnabled() else { return }
    persist("CODESCRIBE_FORMAT_ON_DEVICE", enabled ? "1" : "0")
  }

  private func persist(_ key: String, _ value: String) {
    do {
      try persistOrThrow(key, value)
    } catch {
      lastError = String(describing: error)
    }
  }

  /// `persist` for rows that present the rejection inline instead of as `lastError`.
  private func persistOrThrow(_ key: String, _ value: String) throws {
    guard let engine else { return }
    try engine.updateConfig(key: key, value: value)
    applyLoadedSettings(engine.loadSettings())
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
    runtimeLaneCache.removeAll()
  }

  // MARK: - Keys (Keychain-backed; secrets never read back)

  /// Labels for the static Keychain accounts; custom rows render "API key" on their card.
  static func keyLabel(for account: String) -> String {
    switch account {
    case "LLM_LIBRAXIS_API_KEY":
      return String(localized: "Libraxis API key", comment: "Libraxis is a company name")
    case "LLM_OPENAI_API_KEY":
      return String(localized: "OpenAI API key", comment: "OpenAI is a company name")
    case "LLM_XAI_API_KEY":
      return String(localized: "xAI (Grok) API key", comment: "xAI and Grok are product names")
    case "LLM_ANTHROPIC_API_KEY":
      return String(localized: "Anthropic API key", comment: "Anthropic is a company name")
    case "STT_FILE_API_KEY":
      return String(localized: "File transcription key", comment: "Stored secret label")
    case "STT_LIVE_API_KEY":
      return String(localized: "Live transcript key", comment: "Stored secret label")
    case "GITHUB_TOKEN":
      return String(localized: "GitHub token", comment: "GitHub is a product name")
    default: return account
    }
  }

  // MARK: - Provider registry (Settings › Providers)

  /// Factory-pinned vendors in registry order.
  var vendorProviders: [CsProviderOption] { providers.filter { $0.kind == "vendor" } }

  /// Operator-defined rows (`custom:<id>`), unbounded.
  var customProviders: [CsProviderOption] { providers.filter { $0.kind == "custom" } }

  /// Non-provider Keychain accounts (GitHub). STT keys ride on `sttLanes`.
  var serviceKeyAccounts: [String] { engine?.serviceKeyAccounts() ?? [] }

  /// Lane-picker dot: credential present or key-optional host → green; else red.
  static func availabilityTint(for provider: CsProviderOption, lane: LLMLane = .assistive) -> Color
  {
    provider.apiKeySet
      || (lane == .assistive && provider.wire == "responses" && provider.accountSignedIn)
      || !provider.keyRequired
      ? CSColor.oliveLight : CSColor.terracotta
  }

  /// Bridge rows take the bare slug; the picker id carries the `custom:` prefix (§D 17:55Z).
  private static func customRowId(_ providerId: String) -> String {
    providerId.hasPrefix("custom:") ? String(providerId.dropFirst("custom:".count)) : providerId
  }

  /// One UI read in flight; repeated focus/appear requests join it. A mutation
  /// invalidates its generation and requests one follow-up after completion.
  func refreshProviderAccess() {
    guard let engine else { return }
    if providerMutationPending { providerRefreshRequested = true; return }
    guard !providerAccessPending else { return }
    // This read consumes the queued request; later mutations may queue another.
    providerRefreshRequested = false
    providerAccessPending = true
    let generation = providerAccessGeneration
    Task { @MainActor [self] in
      defer {
        providerAccessPending = false
        if providerRefreshRequested {
          providerRefreshRequested = false
          refreshProviderAccess()
        }
      }
      do {
        let snapshot = try await engine.providerAccessSnapshot()
        guard generation == providerAccessGeneration,
          snapshot.revision == engine.providerAccessRevision()
        else { providerRefreshRequested = true; return }
        applyLoadedSettings(engine.loadSettings())
        providers = snapshot.providers
        providerAccountErrors = snapshot.accountErrors
        keyStatus = snapshot.keyStatus
        sttLanes = snapshot.sttLanes
        providerAccessResolved = true
        providerAccessCheckedAt = Date()
        providerAccessError = nil
        refreshAgentStatus()
        refreshModelDiscoveries(providerIds: LLMLane.allCases.map { llmLane($0).providerId })
      } catch {
        guard generation == providerAccessGeneration else {
          providerRefreshRequested = true
          return
        }
        providerAccessError = error.userFacingMessage
      }
    }
  }

  private func beginProviderMutation() throws {
    guard !providerMutationPending else { throw ProviderMutationError.busy }
    providerMutationPending = true
    providerAccessGeneration &+= 1
  }

  private func finishProviderMutation(_ engine: SettingsEngine) {
    providerMutationPending = false
    reloadProviders(engine)
  }

  private enum ProviderMutationError: LocalizedError {
    case busy
    var errorDescription: String? { String(localized: "Provider access is still being updated.") }
  }

  /// Validation is the bridge's; the thrown error is the form's message, not a modal.
  func addCustomProvider(_ draft: CsCustomProviderDraft) async throws {
    guard let engine else { return }
    try beginProviderMutation()
    defer { finishProviderMutation(engine) }
    _ = try await engine.addCustomProviderAsync(draft: draft)
  }

  func updateCustomProvider(id: String, _ draft: CsCustomProviderDraft) async throws {
    guard let engine else { return }
    try beginProviderMutation()
    defer { finishProviderMutation(engine) }
    _ = try await engine.updateCustomProviderAsync(id: Self.customRowId(id), draft: draft)
  }

  /// Removes the row and its key; lanes that pointed at it come back as `lanesReset`.
  func removeCustomProvider(id: String) {
    guard let engine, !providerMutationPending else { return }
    Task { @MainActor [self] in
      do {
        try beginProviderMutation()
        defer { finishProviderMutation(engine) }
        let removal = try await engine.removeCustomProviderAsync(id: Self.customRowId(id))
        let lanes = removal.lanesReset.map { lane in
          LLMLane.allCases.first { $0.bridgeLane == lane }?.title ?? "\(lane)"
        }
        if lanes.isEmpty {
          laneResetNotice = nil
        } else {
          let joined = lanes.formatted(.list(type: .and))
          laneResetNotice =
            lanes.count == 1
            ? String(
              localized:
                "\(joined) lane reset to the default vendor — the custom provider was removed",
              comment: "The placeholder is one lane name"
            )
            : String(
              localized:
                "\(joined) lanes reset to the default vendor — the custom provider was removed",
              comment: "The placeholder is a localized list of lane names"
            )
        }
      } catch {
        lastError = error.userFacingMessage
      }
    }
  }

  /// Settings + presence + registry after any provider/key mutation.
  private func reloadProviders(_ engine: SettingsEngine) {
    applyLoadedSettings(engine.loadSettings())
    if providerAccessPending { providerRefreshRequested = true }
    else { refreshProviderAccess() }
  }

  // MARK: - Keys (Keychain-backed; secrets never read back)

  func saveKey(account: String, secret: String) async throws {
    let trimmed = secret.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty, let engine else { return }
    try beginProviderMutation()
    defer { finishProviderMutation(engine) }
    do {
      try await engine.setApiKeyAsync(account: account, secret: trimmed)
      keyProbeResults[account] = nil
      lastError = nil
    } catch {
      lastError = error.userFacingMessage
      throw error
    }
  }

  func clearKey(account: String) async throws {
    guard let engine else { return }
    try beginProviderMutation()
    defer { finishProviderMutation(engine) }
    do {
      try await engine.clearApiKeyAsync(account: account)
      keyProbeResults[account] = nil
      lastError = nil
    } catch {
      lastError = error.userFacingMessage
      throw error
    }
  }

  func testKey(account: String) {
    guard let engine else { return }
    guard !keyProbePending.contains(account) else { return }
    keyProbePending.insert(account)
    Task { @MainActor [weak self] in
      guard let self else { return }
      do {
        let probe = try await engine.testApiKeyAsync(account: account)
        self.keyProbePending.remove(account)
        self.keyProbeResults[account] = probe
      } catch {
        self.keyProbePending.remove(account)
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

    Task { @MainActor [weak self] in
      guard let self else { return }
      do {
        let login = try await engine.awaitAccountLoginAsync(
          providerId: providerId,
          // P2-09: 300s is the cap for OAuth browser roundtrip and 2FA.
          timeoutSeconds: 300
        )
        self.accountLoginPending.remove(providerId)
        self.accountLoginNotices[providerId] =
          login.status == "signed_in" ? nil : login.message
      } catch {
        self.accountLoginPending.remove(providerId)
        self.accountLoginNotices[providerId] = String(describing: error)
      }
      self.providerAccessGeneration &+= 1
      if let currentEngine = self.engine { self.reloadProviders(currentEngine) }
    }
  }

  /// Sign out of the provider account (clears the stored tokens). API keys
  /// are untouched.
  func signOutAccount(providerId: String) {
    guard let engine, !providerMutationPending else { return }
    Task { @MainActor [self] in
      do {
        try beginProviderMutation()
        defer { finishProviderMutation(engine) }
        try await engine.signOutAccountAsync(providerId: providerId)
        accountLoginNotices[providerId] = nil
      } catch { lastError = error.userFacingMessage }
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
      lastError = String(
        localized: "No OAuth client-id setting for provider \(providerId)",
        comment: "The placeholder is a provider identifier"
      )
      return
    }
    persist(settingKey, value.trimmingCharacters(in: .whitespacesAndNewlines))
    accountLoginNotices[providerId] = nil
    providerAccessGeneration &+= 1
    if let engine { reloadProviders(engine) }
  }

  /// Re-run discovery for one provider (vendor or custom).
  func refreshModelDiscovery(providerId: String) {
    refreshModelDiscoveries(providerIds: [providerId])
  }

  /// The single discovery path for every lane/provider. Generation checks drop
  /// stale network results.
  private func refreshModelDiscoveries(providerIds: [String]) {
    let providerIds = Array(Set(providerIds))
    var generations: [String: Int] = [:]
    for providerId in providerIds {
      modelDiscoveryGenerations[providerId, default: 0] += 1
      generations[providerId] = modelDiscoveryGenerations[providerId]
    }
    guard let engine else {
      for providerId in providerIds {
        modelDiscoveries[providerId] = CsModelDiscovery.sample(for: providerId)
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

    Task { @MainActor [weak self] in
      var discoveries: [(String, CsModelDiscovery)] = []
      for providerId in providerIds {
        discoveries.append(
          (providerId, await engine.discoverModelsAsync(providerId: providerId))
        )
      }
      guard let self else { return }
      for (providerId, discovery) in discoveries {
        guard self.modelDiscoveryGenerations[providerId] == generations[providerId] else {
          continue
        }
        self.modelDiscoveries[providerId] = discovery
      }
    }
  }

  // MARK: - Prompts (editable BASE prompts)

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
      runtimeLlmLaneProvider: { lane in
        CsRuntimeLlmLane(
          lane: lane,
          providerId: "openai-responses",
          providerDisplayName: "OpenAI",
          wire: "responses",
          endpoint: "https://api.openai.com/v1/responses",
          model: "gpt-5.2",
          keyAccount: "LLM_OPENAI_API_KEY",
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
