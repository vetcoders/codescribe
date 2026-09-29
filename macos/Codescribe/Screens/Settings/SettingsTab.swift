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
  case dictationHandsFree
  case dictationPrivacy
  case dictationPermissions

  var id: String { rawValue }

  var section: SettingsSection {
    switch self {
    case .agentLanes, .agentPrompts, .agentWorkspace, .agentStatus, .agentTools, .agentMcp:
      .agent
    case .dictationEngine, .dictationWhisper, .dictationPreview, .dictationHandsFree,
      .dictationPrivacy, .dictationPermissions:
      .engine
    }
  }

  /// Segment label. Short on purpose: macOS sizes every segment to the widest
  /// label, and six of them must fit the pane at the 880pt minimum window.
  var title: String {
    switch self {
    case .agentLanes: "LLM lanes"
    case .agentPrompts: "Prompts"
    case .agentWorkspace: "Workspace"
    case .agentStatus: "Capabilities"
    case .agentTools: "Tools"
    case .agentMcp: "MCP"
    case .dictationEngine: "Engine"
    case .dictationWhisper: "Whisper"
    case .dictationPreview: "Preview"
    case .dictationHandsFree: "Hands-free"
    case .dictationPrivacy: "Privacy"
    case .dictationPermissions: "Permissions"
    }
  }

  var headline: String {
    switch self {
    case .agentLanes: "Request lanes."
    case .agentPrompts: "Prompts."
    case .agentWorkspace: "Workspace roots."
    case .agentStatus: "Capabilities."
    case .agentTools: "Tool permissions."
    case .agentMcp: "MCP servers."
    case .dictationEngine: "What's actually running."
    case .dictationWhisper: "Local Whisper model."
    case .dictationPreview: "Preview timing."
    case .dictationHandsFree: "Hands-free silence."
    case .dictationPrivacy: "\(CloudPrivacyCopy.title)."
    case .dictationPermissions: "Permission matrix."
    }
  }

  var blurb: String {
    switch self {
    case .agentLanes:
      "Provider and model per request path. Endpoints and keys live on Providers; the resolved runtime truth is below the editors."
    case .agentPrompts:
      "Edits the BASE prompt file. The core still appends its tuning prompt at runtime."
    case .agentWorkspace:
      "Directories the agent may read and write. Everything outside them is out of reach."
    case .agentStatus:
      "What the local agent substrate can currently do, and why."
    case .agentTools:
      "Allow, ask, or deny — per tool. Deny wins over everything."
    case .agentMcp:
      "External MCP servers the agent can call, and their transports."
    case .dictationEngine:
      "Runtime rows reflect the live engine — changes apply on the next recording session."
    case .dictationWhisper:
      "The on-device model behind the direct Whisper engine and Local power refinement."
    case .dictationPreview:
      "How the overlay paces live text. Committed transcripts are unchanged."
    case .dictationHandsFree:
      "How long the Apple engine waits in silence before it rests."
    case .dictationPrivacy:
      "Where audio lives, and the one condition under which it leaves this Mac."
    case .dictationPermissions:
      "Live macOS permission status. Click a missing permission to grant it."
    }
  }

  var searchKeywords: [String] {
    switch self {
    case .agentLanes: ["provider", "model", "endpoint", "assistive", "formatting"]
    case .agentPrompts:
      [
        "prompt", "system prompt", "persona", "instructions", "assistive", "correction", "smart",
        "max", "formatting.txt", "assistive.txt",
      ]
    case .agentWorkspace: ["roots", "directory", "repo", "path"]
    case .agentStatus: ["capability", "native", "enhanced", "readiness"]
    case .agentTools: ["permission", "allow", "ask", "deny", "tool"]
    case .agentMcp: ["mcp", "server", "stdio", "transport"]
    case .dictationEngine: ["asr", "stt", "engine", "apple", "whisper", "runtime"]
    case .dictationWhisper: ["whisper", "model", "download", "fp16", "local"]
    case .dictationPreview: ["preview", "timing", "typing", "cadence", "overlay"]
    case .dictationHandsFree: ["hands-free", "silence", "toggle", "epoch"]
    case .dictationPrivacy: ["cloud", "privacy", "consent", "egress"]
    case .dictationPermissions: ["permission", "accessibility", "input monitoring", "tcc"]
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
