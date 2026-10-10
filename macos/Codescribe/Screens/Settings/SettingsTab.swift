import Foundation

/// A tab inside a settings pane. Long panes become one tab per subsystem under
/// a segmented tab bar (`SettingsTabBar`) instead of one endless scroll, a stack
/// of collapsibles, or a tree of child rows in the sidebar.
///
/// Tabs stay addressable without living in the sidebar: settings search matches
/// tab titles and keywords, a hit reveals the owning section, and clicking that
/// section lands on the matching tab (`searchLanding(in:query:)`). The selected
/// tab is navigation state on `SettingsViewModel`, never persisted.
enum SettingsTab: String, CaseIterable, Identifiable {
  // Agent
  case agentLanes
  case agentPrompts
  case agentWorkspace
  case agentStatus
  case agentTools
  case agentMcp
  // Dictation
  case dictationEngine
  case dictationWhisper
  case dictationPreview
  case dictationPrivacy
  case dictationPermissions

  var id: String { rawValue }

  var section: SettingsSection {
    switch self {
    case .agentLanes, .agentPrompts, .agentWorkspace, .agentStatus, .agentTools, .agentMcp:
      .agent
    case .dictationEngine, .dictationWhisper, .dictationPreview, .dictationPrivacy,
      .dictationPermissions:
      .engine
    }
  }

  /// Segment label. Short on purpose: six of them share one tab bar, and they
  /// should fit the pane at the 880pt minimum window. Each segment hugs its own
  /// label, so one long title no longer sets the width of all six; when the six
  /// still do not fit, `SettingsTabBar` scrolls horizontally instead of widening
  /// the pane, so a long translation costs a scroll, never a clipped pane.
  var title: String {
    switch self {
    case .agentLanes:
      String(localized: "AI models", comment: "Settings tab: provider and model per function")
    case .agentPrompts: String(localized: "Prompts", comment: "Settings tab: editable prompts")
    case .agentWorkspace:
      String(localized: "Workspace", comment: "Settings tab: agent workspace roots")
    case .agentStatus:
      String(localized: "Diagnostics", comment: "Settings tab: agent connection diagnostics")
    case .agentTools: String(localized: "Tools", comment: "Settings tab: tool permissions")
    case .agentMcp: "MCP"
    case .dictationEngine:
      String(localized: "Engine", comment: "Settings tab: speech-to-text engine")
    case .dictationWhisper: "Whisper"
    case .dictationPreview:
      String(localized: "Preview", comment: "Settings tab: live transcript preview timing")
    case .dictationPrivacy: String(localized: "Privacy", comment: "Settings tab: cloud and privacy")
    case .dictationPermissions:
      String(localized: "Permissions", comment: "Settings tab: macOS permission matrix")
    }
  }

  var headline: String {
    switch self {
    case .agentLanes: String(localized: "Model configuration.")
    case .agentPrompts: String(localized: "Prompts", comment: "Settings tab: editable prompts")
    case .agentWorkspace:
      String(localized: "Folders available to the Agent", comment: "Settings tab: Agent folders")
    case .agentStatus:
      String(localized: "Agent environment status", comment: "Settings tab: Diagnostics headline")
    case .agentTools: String(localized: "Tool permissions")
    case .agentMcp: String(localized: "MCP servers")
    case .dictationEngine:
      String(localized: "Speech recognition", comment: "Settings tab headline: Dictation › Engine")
    case .dictationWhisper:
      String(
        localized: "Local Whisper model", comment: "Settings tab headline: Dictation › Whisper")
    case .dictationPreview:
      String(
        localized: "Transcript display pace", comment: "Settings tab headline: Dictation › Preview")
    case .dictationPrivacy: CloudPrivacyCopy.title
    case .dictationPermissions: String(localized: "Permission matrix.")
    }
  }

  var blurb: String {
    switch self {
    case .agentLanes:
      String(
        localized:
          "Pick a provider and a model separately for the Agent and for transcript formatting. API keys and accounts are set up under Providers."
      )
    case .agentPrompts:
      String(
        localized:
          "Browse and edit the base prompts. Codescribe may add further instructions to them while it runs."
      )
    case .agentWorkspace:
      String(
        localized:
          "The Agent's built-in file and terminal tools read and write only inside these folders, and so do the paths Codescribe hands to MCP tools it can check. What an MCP server does on its own is outside this list."
      )
    case .agentStatus:
      String(localized: "Configuration state of the Agent, its available tools and integrations.")
    case .agentTools:
      String(
        localized:
          "Set when the Agent may use tools without asking, when it needs your approval, and when it must refuse."
      )
    case .agentMcp:
      String(localized: "Add MCP servers and manage the tools the Agent may use.")
    case .dictationEngine:
      String(
        localized: "Choose how speech becomes text. Changes apply from the next recording.",
        comment: "Settings tab blurb: Dictation › Engine")
    case .dictationWhisper:
      String(
        localized: "Pick a model, check its availability and manage the space it takes.",
        comment: "Settings tab blurb: Dictation › Whisper")
    case .dictationPreview:
      String(
        localized: "Adjust how quickly text appears in the preview window while recording."
      )
    case .dictationPrivacy:
      String(localized: "See when Codescribe uses online services.")
    case .dictationPermissions:
      String(localized: "Live macOS permission status. Click a missing permission to grant it.")
    }
  }

  var searchKeywords: [String] {
    switch self {
    case .agentLanes:
      settingsSearchTerms(
        localized: String(
          localized: "settings.search.tab.agentLanes",
          defaultValue: "provider, model, endpoint, assistive, formatting",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    case .agentPrompts:
      settingsSearchTerms(
        fixed: ["formatting.txt", "assistive.txt"],
        localized: String(
          localized: "settings.search.tab.agentPrompts",
          defaultValue:
            "prompt, system prompt, persona, instructions, assistive, correction, smart, max",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    case .agentWorkspace:
      settingsSearchTerms(
        localized: String(
          localized: "settings.search.tab.agentWorkspace",
          defaultValue: "roots, directory, folders, access, repo, path",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    case .agentStatus:
      settingsSearchTerms(
        localized: String(
          localized: "settings.search.tab.agentStatus",
          defaultValue:
            "diagnostics, status, environment, connection, installation path, capability, readiness",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    case .agentTools:
      settingsSearchTerms(
        localized: String(
          localized: "settings.search.tab.agentTools",
          defaultValue: "permission, allow, ask, deny, tool",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    case .agentMcp:
      settingsSearchTerms(
        fixed: ["mcp", "stdio"],
        localized: String(
          localized: "settings.search.tab.agentMcp", defaultValue: "server, transport",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    case .dictationEngine:
      settingsSearchTerms(
        fixed: ["asr", "stt", "apple", "whisper"],
        localized: String(
          localized: "settings.search.tab.dictationEngine", defaultValue: "engine, runtime",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    case .dictationWhisper:
      settingsSearchTerms(
        fixed: ["whisper", "fp16"],
        localized: String(
          localized: "settings.search.tab.dictationWhisper", defaultValue: "model, download, local",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    case .dictationPreview:
      settingsSearchTerms(
        localized: String(
          localized: "settings.search.tab.dictationPreview",
          defaultValue: "preview, timing, pace, typing, cadence, overlay",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    case .dictationPrivacy:
      settingsSearchTerms(
        localized: String(
          localized: "settings.search.tab.dictationPrivacy",
          defaultValue: "cloud, privacy, consent, egress",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    case .dictationPermissions:
      settingsSearchTerms(
        fixed: ["tcc"],
        localized: String(
          localized: "settings.search.tab.dictationPermissions",
          defaultValue: "permission, accessibility, input monitoring",
          comment:
            "Search aliases, comma-separated, never shown. List the words people would type to find this; add synonyms freely"
        ))
    }
  }

  static func tabs(in section: SettingsSection) -> [SettingsTab] {
    allCases.filter { $0.section == section }
  }

  /// Tabs a query should surface, so search reaches inside a long section
  /// instead of stopping at its title.
  static func matching(query: String) -> [SettingsTab] {
    let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    guard !needle.isEmpty else { return allCases }
    return allCases.filter { tab in
      tab.title.localizedStandardContains(needle)
        || tab.searchKeywords.contains { $0.localizedStandardContains(needle) }
    }
  }

  /// The tab a section opened from an active search should land on: the first
  /// of its tabs the query matched, or nil (the section's own first tab) when
  /// the query names none of them. "mcp" opens Agent › MCP servers.
  static func searchLanding(in section: SettingsSection, query: String) -> SettingsTab? {
    guard !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
    let matched = Set(matching(query: query))
    return tabs(in: section).first { matched.contains($0) }
  }
}

extension SettingsSection {
  /// Sidebar rows a query reveals: a title/keyword hit on the section itself,
  /// or on any of its tabs (so "mcp" surfaces Agent).
  static func revealed(by query: String) -> [SettingsSection] {
    let direct = Set(matching(query: query))
    let hits =
      query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
      ? direct
      : direct.union(SettingsTab.matching(query: query).map(\.section))
    return allCases.filter { hits.contains($0) && $0.availability != .hidden }
  }
}

/// Search aliases for a settings surface, in two parts. `fixed` names what
/// nobody translates — products, protocols, file names — and matches in every
/// interface language. `localized` is one catalog row: a comma-separated list
/// the translator owns and may extend with the words their users type.
func settingsSearchTerms(fixed: [String] = [], localized: String = "") -> [String] {
  let separators: Set<Character> = [",", "，", "、"]
  let words = localized.split(whereSeparator: separators.contains)
    .map { $0.trimmingCharacters(in: .whitespaces) }
    .filter { !$0.isEmpty }
  return fixed + words
}
