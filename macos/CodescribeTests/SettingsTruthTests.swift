import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class SettingsTruthTests: XCTestCase {
  func testDebugReceiptSeparatesConfigurationFromServingAndOmitsEndpoints() {
    var settings = CsSettings.sample
    settings.asrMode = "local_power"
    settings.formattingLevel = "max"
    settings.llmFormattingModel = "format-model"
    settings.llmAssistiveModel = "agent-model"
    settings.sttFileEndpoint = "https://secret.invalid/file?token=do-not-copy"
    settings.sttLiveEndpoint = "wss://secret.invalid/live?token=do-not-copy"
    settings.transcriptTagTemplate = "private template content"
    let text = codescribeDebugInfo(
      build: AppBuildInfo(
        version: "1.2.3", build: "456", commit: "abc123", builtAt: "fixture-time"),
      osVersion: "fixture-os", recording: true, settings: settings,
      lastServing: CsLastServingVerdict(
        engine: "local_whisper", routingMode: "off", disposition: "changed", fallbackUsed: true),
      settingsFile: "/fixture/settings/settings.json", dataDirectory: "/fixture/data",
      notesDirectory: "/fixture/notes"
    )
    XCTAssertTrue(text.contains("source commit: abc123"))
    XCTAssertTrue(text.contains("built at: fixture-time"))
    XCTAssertTrue(text.contains("configured ASR mode: local_power"))
    XCTAssertFalse(text.contains("configured STT engine:"))
    XCTAssertTrue(text.contains("last completed serving engine: local_whisper"))
    XCTAssertTrue(text.contains("not proof of the active capture snapshot"))
    XCTAssertTrue(text.contains("configured formatting model: format-model"))
    XCTAssertTrue(text.contains("configured agent model: agent-model"))
    XCTAssertTrue(text.contains("settings file: /fixture/settings/settings.json"))
    XCTAssertTrue(text.contains("app data dir: /fixture/data"))
    XCTAssertFalse(text.contains("secret.invalid"))
    XCTAssertFalse(text.contains("do-not-copy"))
    XCTAssertFalse(text.contains("private template content"))
  }

  func testDebugReceiptDoesNotInferServingFromConfiguredMode() {
    let text = codescribeDebugInfo(
      build: AppBuildInfo(version: "1", build: "1", commit: "unknown", builtAt: "unknown"),
      osVersion: "fixture-os", recording: false, settings: .sample, lastServing: nil,
      settingsFile: "/fixture/settings.json", dataDirectory: "/fixture/data",
      notesDirectory: "/fixture/notes"
    )
    XCTAssertTrue(text.contains("last completed serving engine: not yet observed in this process"))
    XCTAssertFalse(text.contains("last serving disposition:"))
    XCTAssertTrue(text.contains("review before sharing"))
  }

  func testDebugReceiptPreservesBuildAndServingWhenConfigurationIsUnavailable() {
    let text = codescribeDebugInfo(
      build: AppBuildInfo(version: "1", build: "2", commit: "source-sha", builtAt: "fixture-time"),
      osVersion: "fixture-os", recording: false, settings: nil,
      lastServing: CsLastServingVerdict(
        engine: "apple", routingMode: "off", disposition: "unchanged", fallbackUsed: false),
      settingsFile: "/fixture/settings.json", dataDirectory: "/fixture/data",
      notesDirectory: "/fixture/notes"
    )
    XCTAssertTrue(text.contains("configuration: unavailable"))
    XCTAssertTrue(text.contains("source commit: source-sha"))
    XCTAssertTrue(text.contains("last completed serving engine: apple"))
    XCTAssertTrue(text.contains("settings file: /fixture/settings.json"))
    XCTAssertFalse(text.contains("configured STT engine:"))
    XCTAssertFalse(text.contains("system default"))
    XCTAssertFalse(text.contains("configuration: resolved now"))
  }

  func testMaxApprovalInvalidationDuringReadIsNotLost() async {
    let request = PendingToolApproval(
      callID: "call", sessionID: "session", threadID: "consultation",
      tool: "write_file", server: "native", risk: "mutating", summary: "write",
      command: nil, cwd: nil, paths: []
    )
    var reads = 0
    var model: SettingsViewModel!
    model = SettingsViewModel(
      engine: MockSettingsEngine(pendingMaxApprovalsObserver: {
        reads += 1
        if reads == 1 {
          await model.refreshMaxToolApprovals()
          return []
        }
        return [request]
      }), permissionProbe: MockPermissionProbe())
    await model.refreshMaxToolApprovals()
    XCTAssertEqual(reads, 2)
    XCTAssertEqual(model.maxToolApprovals, [request])
    XCTAssertFalse(model.maxApprovalBusy)
  }

  func testMaxApprovalForwardsExactIdentityAndRefreshesAfterVerdict() async {
    let request = PendingToolApproval(
      callID: "call", sessionID: "session", threadID: "consultation",
      tool: "write_file", server: "native", risk: "mutating", summary: "write",
      command: nil, cwd: nil, paths: ["/workspace/a"]
    )
    var pending = [request]
    var resolutions = 0
    let model = SettingsViewModel(
      engine: MockSettingsEngine(
        pendingMaxApprovalsObserver: { pending },
        resolveMaxApprovalObserver: { received, approved, remember in
          XCTAssertEqual(received, request)
          XCTAssertTrue(approved)
          XCTAssertFalse(remember)
          resolutions += 1
          pending = []
          return true
        }
      ), permissionProbe: MockPermissionProbe())
    await model.refreshMaxToolApprovals()
    XCTAssertEqual(model.maxToolApprovals, [request])
    await model.resolveMaxToolApproval(request, approved: true)
    XCTAssertEqual(resolutions, 1)
    XCTAssertTrue(model.maxToolApprovals.isEmpty)
    XCTAssertNil(model.maxApprovalError)
    XCTAssertFalse(model.maxApprovalBusy)
    await model.resolveMaxToolApproval(request, approved: true)
    XCTAssertEqual(resolutions, 1, "a removed card must not be submitted again")
  }

  func testMaxApprovalReadFailureDoesNotAuthorizeAStaleCard() async {
    let request = PendingToolApproval(
      callID: "call", sessionID: "session", threadID: "consultation",
      tool: "write_file", server: "native", risk: "mutating", summary: "write",
      command: nil, cwd: nil, paths: []
    )
    var failRead = false
    var resolutions = 0
    let model = SettingsViewModel(
      engine: MockSettingsEngine(
        pendingMaxApprovalsObserver: {
          if failRead {
            throw NSError(domain: "ApprovalTest", code: 1)
          }
          return [request]
        },
        resolveMaxApprovalObserver: { _, _, _ in
          resolutions += 1
          return false
        }
      ), permissionProbe: MockPermissionProbe())
    await model.refreshMaxToolApprovals()
    failRead = true
    await model.refreshMaxToolApprovals()
    XCTAssertNotNil(model.maxApprovalError)
    await model.resolveMaxToolApproval(request, approved: true)
    XCTAssertEqual(resolutions, 0)
    failRead = false
    await model.refreshMaxToolApprovals()
    XCTAssertNil(model.maxApprovalError)
    await model.resolveMaxToolApproval(request, approved: false)
    XCTAssertEqual(resolutions, 1)
    XCTAssertEqual(model.maxApprovalError, "This permission request is no longer active.")
  }

  func testNewMaxConsultationWaitsForBackendAndRefusesDuplicateRequests() async {
    var settings = CsSettings.sample
    settings.aiFormattingEnabled = true
    settings.formattingLevel = "max"
    var calls = 0
    var model: SettingsViewModel!
    let engine = MockSettingsEngine(
      settings: settings,
      beginNewMaxConsultationObserver: {
        calls += 1
        XCTAssertTrue(model.newMaxConsultationPending)
        XCTAssertNil(model.maxConsultationNotice)
        await model.beginNewMaxConsultation()
        XCTAssertEqual(calls, 1)
        return "new-consultation-id"
      }
    )
    model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())
    model.refresh()

    await model.beginNewMaxConsultation()

    XCTAssertEqual(calls, 1)
    XCTAssertFalse(model.newMaxConsultationPending)
    XCTAssertEqual(
      model.maxConsultationNotice,
      "New consultation started. Previous history is preserved."
    )
    XCTAssertEqual(model.settings.formattingLevel, "max")
  }

  func testNewMaxConsultationReportsBackendRefusalWithoutSuccess() async {
    var settings = CsSettings.sample
    settings.aiFormattingEnabled = true
    settings.formattingLevel = "max"
    let model = SettingsViewModel(
      engine: MockSettingsEngine(
        settings: settings,
        beginNewMaxConsultationObserver: {
          throw NSError(
            domain: "ConsultationTest", code: 1,
            userInfo: [NSLocalizedDescriptionKey: "A turn is still active."]
          )
        }
      ), permissionProbe: MockPermissionProbe())
    model.refresh()
    await model.beginNewMaxConsultation()

    XCTAssertFalse(model.newMaxConsultationPending)
    XCTAssertEqual(
      model.maxConsultationNotice,
      "Could not start a new consultation: A turn is still active."
    )
  }

  func testNewMaxConsultationIsUnavailableOutsideEnabledMax() async {
    for policy in ["off", "correction", "smart", "max"] {
      var settings = CsSettings.sample
      settings.formattingLevel = policy
      settings.aiFormattingEnabled = policy != "max"
      var calls = 0
      let model = SettingsViewModel(
        engine: MockSettingsEngine(
          settings: settings,
          beginNewMaxConsultationObserver: {
            calls += 1
            return "unexpected"
          }
        ), permissionProbe: MockPermissionProbe())
      model.refresh()
      XCTAssertFalse(model.maxConsultationEnabled)
      await model.beginNewMaxConsultation()
      XCTAssertEqual(calls, 0)
      XCTAssertFalse(model.newMaxConsultationPending)
      XCTAssertNil(model.maxConsultationNotice)
    }
  }

  /// The rail is a native `List(.sidebar)` now: selection fill, focus ring and
  /// keyboard navigation belong to AppKit, so the app owns only the CONTENT —
  /// which group a section sits in, its symbol, and what the search matches.
  func testEverySectionDeclaresItsGroupAndASymbolForTheNativeSidebar() {
    let visible = SettingsSection.allCases.filter { $0.availability != .hidden }
    XCTAssertEqual(visible.count, 9)

    for section in visible {
      XCTAssertFalse(section.symbol.isEmpty, "\(section.rawValue) needs an SF Symbol")
      XCTAssertFalse(section.searchKeywords.isEmpty, "\(section.rawValue) needs search aliases")
    }

    // Every group carries rows, and every section lands in exactly one.
    for group in SettingsSectionGroup.allCases {
      XCTAssertFalse(
        visible.filter { $0.group == group }.isEmpty,
        "group \(group.rawValue) would render as an empty header"
      )
    }
    XCTAssertEqual(
      visible.map(\.group).count,
      visible.count,
      "group is total — no section can be absent from the rail"
    )
  }

  func testSettingsSearchMatchesTitlesAndWhatThePanelActuallyDoes() {
    // Empty query keeps the whole rail.
    XCTAssertEqual(
      SettingsSection.matching(query: "   ").count,
      SettingsSection.allCases.filter { $0.availability != .hidden }.count
    )

    // Title match, case-insensitive.
    XCTAssertEqual(SettingsSection.matching(query: "hotkeys"), [.shortcuts])
    XCTAssertEqual(SettingsSection.matching(query: "LICENSE"), [.license])

    // Keyword match: the user searches for the job, not the tab name.
    XCTAssertTrue(SettingsSection.matching(query: "api key").contains(.keys))
    XCTAssertTrue(SettingsSection.matching(query: "mikrofon").contains(.audio))
    XCTAssertTrue(SettingsSection.matching(query: "whisper").contains(.engine))
    XCTAssertTrue(SettingsSection.matching(query: "słownik").contains(.voiceLab))

    // A miss returns nothing rather than the whole list.
    XCTAssertTrue(SettingsSection.matching(query: "zzzz").isEmpty)
  }

  /// Tabs must not lose a subsystem or strand a tab: every tab belongs to a
  /// real section, selecting a section lands on its first tab, and a section
  /// without tabs keeps rendering whole.
  func testTabbedSectionsRouteToTabsWithoutLosingSubsystems() {
    let model = SettingsViewModel(
      engine: MockSettingsEngine(), permissionProbe: MockPermissionProbe())

    XCTAssertEqual(
      SettingsTab.tabs(in: .agent),
      [.agentLanes, .agentPrompts, .agentWorkspace, .agentStatus, .agentTools, .agentMcp],
      "the Agent panel's six subsystems each need their own tab"
    )
    XCTAssertEqual(
      SettingsTab.tabs(in: .engine),
      [
        .dictationEngine, .dictationWhisper, .dictationPreview, .dictationHandsFree,
        .dictationPrivacy, .dictationPermissions,
      ],
      "every former Dictation collapsible is a tab"
    )
    for tab in SettingsTab.allCases {
      XCTAssertFalse(tab.title.isEmpty)
      XCTAssertFalse(tab.headline.isEmpty)
      XCTAssertFalse(tab.blurb.isEmpty)
      XCTAssertFalse(tab.searchKeywords.isEmpty)
      XCTAssertTrue(SettingsTab.tabs(in: tab.section).contains(tab))
    }

    // Landing on a tabbed section opens its first tab.
    model.select(SettingsSection.agent)
    XCTAssertEqual(model.tab, .agentLanes)
    XCTAssertEqual(model.currentTab, .agentLanes)

    // Selecting a tab keeps the parent section consistent.
    model.select(SettingsTab.agentMcp)
    XCTAssertEqual(model.section, .agent)
    XCTAssertEqual(model.currentTab, .agentMcp)

    // The tab bar writes through the same path; a nil write is dropped.
    model.currentTab = .agentTools
    XCTAssertEqual(model.currentTab, .agentTools)
    model.currentTab = nil
    XCTAssertEqual(model.currentTab, .agentTools)

    // Leaving for a section without tabs clears the tab.
    model.select(SettingsSection.audio)
    XCTAssertNil(model.tab)
    XCTAssertNil(model.currentTab)

    // A raw section write (quick start, previews) shows that section's first
    // tab — never a stale tab from the previous section.
    model.select(SettingsTab.agentMcp)
    model.section = .engine
    XCTAssertEqual(model.currentTab, .dictationEngine)

    // Sidebar selection round-trips through the rail's binding; nil is dropped.
    model.sidebarSelection = .license
    XCTAssertEqual(model.section, .license)
    model.sidebarSelection = nil
    XCTAssertEqual(model.section, .license)
  }

  func testSettingsSearchReachesInsideTabs() {
    // A tab keyword reveals its parent section…
    XCTAssertTrue(SettingsTab.matching(query: "mcp").contains(.agentMcp))
    XCTAssertEqual(SettingsTab.matching(query: "mcp").first?.section, .agent)
    XCTAssertTrue(SettingsSection.revealed(by: "mcp").contains(.agent))
    XCTAssertFalse(SettingsSection.agent.title.lowercased().contains("mcp"))

    // …and opening that section from the search lands on the tab it named.
    XCTAssertEqual(SettingsTab.searchLanding(in: .agent, query: "mcp"), .agentMcp)
    XCTAssertEqual(SettingsTab.searchLanding(in: .agent, query: "permission"), .agentTools)
    XCTAssertEqual(
      SettingsTab.searchLanding(in: .engine, query: "permission"), .dictationPermissions)
    XCTAssertNil(SettingsTab.searchLanding(in: .agent, query: "  "))
    XCTAssertNil(
      SettingsTab.searchLanding(in: .agent, query: "agent"),
      "a section-name query opens the section on its first tab"
    )

    // Prompts have one home: every prompt term reveals Agent and lands on Prompts.
    for query in ["system prompt", "persona", "formatting.txt", "assistive.txt"] {
      XCTAssertEqual(SettingsSection.revealed(by: query), [.agent], query)
      XCTAssertEqual(SettingsTab.searchLanding(in: .agent, query: query), .agentPrompts, query)
    }

    XCTAssertTrue(SettingsTab.matching(query: "roots").contains(.agentWorkspace))
    XCTAssertTrue(SettingsTab.matching(query: "zzzz").isEmpty)
    XCTAssertTrue(SettingsSection.revealed(by: "zzzz").isEmpty)
    XCTAssertEqual(SettingsTab.matching(query: "  ").count, SettingsTab.allCases.count)
    XCTAssertEqual(
      SettingsSection.revealed(by: "  "),
      SettingsSection.matching(query: ""),
      "an empty query keeps the whole rail"
    )
  }

  /// The prompt picker moved; prompt identity did not. Each segment still maps
  /// to the same storage level; the file name lives under File details only.
  func testPromptFilesKeepTheirStorageIdentity() {
    XCTAssertEqual(PromptFile.allCases.map(\.formattingLevel), [.correction, .smart, .max, nil])
    XCTAssertEqual(
      PromptFile.allCases.compactMap(\.formattingLevel),
      FormattingPolicyOption.editablePrompts
    )
    XCTAssertEqual(
      PromptFile.allCases.map(\.editorTitle),
      ["Correction prompt", "Smart prompt", "Max prompt", "Agent prompt"]
    )
    for file in PromptFile.allCases {
      XCTAssertFalse(
        file.editorSubtitle.contains(".txt"), "file names belong under File details: \(file)")
    }
    XCTAssertTrue(
      PromptFile.assistive.editorSubtitle.contains("Voice chat uses its own instructions"),
      "assistive.txt feeds only the act-on-request lane (compose_agent_system_prompt)")
  }

  /// The Tools tab binds by key path now; each projection must read the
  /// registry snapshot and route its write to the same setter and kind the
  /// closure bindings used.
  func testToolPermissionPickersRouteThroughTheExistingSetters() async {
    let admin = RecordingPermissionAdmin(capabilities: [
      CsToolCapability(
        name: "search", identity: "loctree-mcp:search", origin: "mcp", server: "loctree-mcp",
        risk: "read_only", effective: "allow", ruleSource: "tool", requiresApprovalFlag: false)
    ])
    let model = SettingsViewModel(
      engine: MockSettingsEngine(), permissionProbe: MockPermissionProbe(), mcpAdmin: admin)
    model.reloadToolPermissions()
    for _ in 0..<100 where model.toolCapabilities.isEmpty { await Task.yield() }

    XCTAssertEqual(model[toolLevel: "loctree-mcp:search"], "allow")
    XCTAssertEqual(model[toolLevel: "ghost:tool"], "", "an unknown identity selects nothing")
    XCTAssertEqual(model.readOnlyDefaultPicker, "allow")
    XCTAssertEqual(model.sideEffectDefaultPicker, "ask")
    XCTAssertEqual(model.globalDefaultPicker, "ask")

    model[toolLevel: "loctree-mcp:search"] = "deny"
    XCTAssertEqual(admin.toolWrites.map(\.identity), ["loctree-mcp:search"])
    XCTAssertEqual(admin.toolWrites.map(\.level), ["deny"])
    model.clearToolPermission(identity: "loctree-mcp:search")
    XCTAssertEqual(admin.toolClears, ["loctree-mcp:search"])

    model.readOnlyDefaultPicker = "deny"
    XCTAssertEqual(admin.defaultWrites.last?.readOnlyDefault, "deny")
    model.sideEffectDefaultPicker = "deny"
    XCTAssertEqual(admin.defaultWrites.last?.sideEffectDefault, "deny")
    model.globalDefaultPicker = "deny"
    XCTAssertEqual(admin.defaultWrites.last?.defaultLevel, "deny")
    XCTAssertEqual(admin.defaultWrites.count, 3)
  }

  func testSectionAvailabilityKeepsPromisesHonest() {
    for section in [
      SettingsSection.creator, .shortcuts, .keys, .agent, .engine, .audio, .voiceLab,
      .license, .user,
    ] {
      XCTAssertEqual(section.availability, .available)
      XCTAssertTrue(section.isInteractive)
    }
  }

  /// The full route map: stable id, one visible title owner, and the explicit
  /// panel destination SettingsView's detail switch consumes. Every rail
  /// section, including engine→Dictation, voiceLab→Dictionary,
  /// keys→Providers, and the dedicated Agent destination (which also owns the
  /// prompts — there is no separate Prompts row).
  func testSettingsSectionRoutesTitlesAndDestinationsOwnTheRail() {
    let expectations: [(SettingsSection, String, String, SettingsPanelDestination)] = [
      (.creator, "creator", "Creator", .creator),
      (.shortcuts, "shortcuts", "Hotkeys", .shortcuts),
      (.keys, "keys", "Providers", .providers),
      (.agent, "agent", "Agent", .agent),
      (.engine, "engine", "Dictation", .dictation),
      (.audio, "audio", "Audio", .audio),
      (.voiceLab, "voiceLab", "Dictionary", .dictionary),
      (.lab, "lab", "Lab", .lab),
      (.license, "license", "License", .license),
      (.user, "user", "User", .user),
    ]

    XCTAssertEqual(SettingsSection.allCases.count, expectations.count)
    for (section, id, title, destination) in expectations {
      XCTAssertEqual(section.rawValue, id)
      XCTAssertEqual(section.id, id)
      XCTAssertEqual(section.title, title)
      XCTAssertEqual(section.destination, destination)
    }
    // No two sections may share a destination or a visible title.
    XCTAssertEqual(
      Set(SettingsSection.allCases.map(\.destination)).count,
      SettingsSection.allCases.count
    )
    XCTAssertEqual(
      Set(SettingsSection.allCases.map(\.title)).count,
      SettingsSection.allCases.count
    )
  }

  func testProvidersAndAgentOwnDisjointSettingsCapabilities() {
    XCTAssertEqual(ProvidersPanel.ownedCapabilities, [.providers])
    XCTAssertEqual(
      AgentPanel.ownedCapabilities,
      [.llmLanes, .prompts, .workspaceRoots, .agentStatus, .mcpServers, .toolPermissions]
    )
    XCTAssertTrue(ProvidersPanel.ownedCapabilities.isDisjoint(with: AgentPanel.ownedCapabilities))
  }

  /// Capability matrix sample seed used by Settings previews and offline VM
  /// construction must expose the three tiers the bridge reports.
  func testCapabilityMatrixSampleExposesNativeEnhancedUnavailableTiers() {
    let tiers = Set(CsCapabilityRow.sampleMatrix.map(\.tier))
    XCTAssertTrue(tiers.contains("native"))
    XCTAssertTrue(tiers.contains("enhanced"))
    XCTAssertTrue(tiers.contains("unavailable"))
    for row in CsCapabilityRow.sampleMatrix {
      XCTAssertFalse(row.op.isEmpty)
      XCTAssertFalse(row.reason.isEmpty)
    }
    let model = SettingsViewModel(
      permissionProbe: MockPermissionProbe(), agentStatus: MockAgentStatusEngine())
    model.refreshAgentStatus()
    XCTAssertEqual(model.capabilityMatrix.count, CsCapabilityRow.sampleMatrix.count)
    XCTAssertEqual(model.capabilityMatrix.first?.op, "fs.list")
  }

  /// Capability rows come from the live registry surface — the panel must not
  /// invent tools the dispatcher does not know.
  func testToolPermissionsPanelUsesCapabilityIdentityContract() {
    // Identity key format is the single contract shared with the Rust gate.
    let samples = [
      ("desktop-commander:write_file", "allow"),
      ("native:read_file", "ask"),
    ]
    for (identity, level) in samples {
      XCTAssertFalse(identity.isEmpty)
      XCTAssertTrue(["allow", "ask", "deny"].contains(level))
      // Server portion of MCP identity is lowercased; tool name is preserved.
      if identity.hasPrefix("native:") {
        XCTAssertTrue(identity.split(separator: ":").count >= 2)
      } else {
        let parts = identity.split(separator: ":", maxSplits: 1).map(String.init)
        XCTAssertEqual(parts.count, 2)
        XCTAssertEqual(parts[0], parts[0].lowercased())
      }
    }
    XCTAssertTrue(AgentPanel.ownedCapabilities.contains(.toolPermissions))
  }

  /// The Tools tab shows a readable name above the raw identifier, localizes
  /// the risk class and tells an individual rule from an inherited one; the
  /// identity string itself is never touched.
  func testToolPermissionLabelsHumanizeNamesAndKeepIdentifiers() {
    XCTAssertEqual(ToolPermissionLabels.displayName(for: "apply_patch"), "Apply patch")
    XCTAssertEqual(ToolPermissionLabels.displayName(for: "brave-web-search"), "Brave web search")
    XCTAssertEqual(ToolPermissionLabels.displayName(for: "mcp__dc__write_file"), "Write file")
    XCTAssertEqual(ToolPermissionLabels.displayName(for: "ls"), "Ls")
    XCTAssertEqual(ToolPermissionLabels.displayName(for: ""), "")

    XCTAssertEqual(ToolPermissionLabels.source("native"), "Native")
    XCTAssertEqual(ToolPermissionLabels.source("Desktop-Commander"), "Desktop-Commander")
    XCTAssertEqual(ToolPermissionLabels.origin("mcp:brave-search"), "MCP")
    XCTAssertEqual(ToolPermissionLabels.risk("read_only"), "Read data")
    XCTAssertEqual(ToolPermissionLabels.risk("process_control"), "Processes")
    XCTAssertEqual(ToolPermissionLabels.risk("unknown"), "Unclassified")
    XCTAssertEqual(ToolPermissionLabels.risk("exotic"), "exotic", "unknown classes stay raw")

    let inherited = ToolPermissionItem(
      capability: CsToolCapability(
        name: "apply_patch", identity: "native:apply_patch", origin: "native", server: "",
        risk: "mutating", effective: "ask", ruleSource: "default", requiresApprovalFlag: false))
    XCTAssertEqual(inherited.displayName, "Apply patch")
    XCTAssertEqual(inherited.identity, "native:apply_patch")
    XCTAssertFalse(inherited.hasIndividualRule)
    XCTAssertEqual(
      ToolPermissionLabels.ruleCaption(inherited.ruleSource), "Inherited from the category default")
    let individual = ToolPermissionItem(
      capability: CsToolCapability(
        name: "search", identity: "loctree-mcp:search", origin: "mcp:loctree-mcp",
        server: "loctree-mcp", risk: "read_only", effective: "deny", ruleSource: "tool",
        requiresApprovalFlag: false))
    XCTAssertTrue(individual.hasIndividualRule)
    XCTAssertEqual(ToolPermissionLabels.ruleCaption("tool"), "Individual rule")
    XCTAssertEqual(ToolPermissionLabels.ruleCaption("server"), "Inherited from the server rule")
  }

  /// P0-9 residual: permissions hierarchy groups server→tool, filters by query,
  /// and keeps `native` first without inventing tools.
  func testToolPermissionGroupingHierarchyAndSearch() {
    let items = [
      ToolPermissionItem(
        name: "write_file",
        identity: "desktop-commander:write_file",
        server: "desktop-commander",
        origin: "mcp",
        risk: "side_effect",
        effective: "ask"
      ),
      ToolPermissionItem(
        name: "read_file",
        identity: "native:read_file",
        server: "native",
        origin: "builtin",
        risk: "read_only",
        effective: "allow"
      ),
      ToolPermissionItem(
        name: "search",
        identity: "brave-search:search",
        server: "brave-search",
        origin: "mcp",
        risk: "read_only",
        effective: "allow"
      ),
      ToolPermissionItem(
        name: "list_dir",
        identity: "desktop-commander:list_dir",
        server: "desktop-commander",
        origin: "mcp",
        risk: "read_only",
        effective: "allow"
      ),
    ]

    XCTAssertEqual(
      ToolPermissionGrouping.groupKey(server: "", identity: "native:read_file"),
      "native"
    )
    XCTAssertEqual(
      ToolPermissionGrouping.groupKey(server: "desktop-commander", identity: "x:y"),
      "desktop-commander"
    )

    let all = ToolPermissionGrouping.groups(from: items, query: "")
    XCTAssertEqual(all.map(\.server), ["native", "brave-search", "desktop-commander"])
    XCTAssertEqual(all[0].items.map(\.name), ["read_file"])
    XCTAssertEqual(all[2].items.map(\.name), ["list_dir", "write_file"])

    let filtered = ToolPermissionGrouping.groups(from: items, query: "write")
    XCTAssertEqual(filtered.count, 1)
    XCTAssertEqual(filtered[0].server, "desktop-commander")
    XCTAssertEqual(filtered[0].items.map(\.identity), ["desktop-commander:write_file"])

    XCTAssertTrue(ToolPermissionGrouping.matches(items[2], query: "brave"))
    XCTAssertFalse(ToolPermissionGrouping.matches(items[1], query: "zzz"))
  }

  func testCreatorQuickStartCardsRouteOrStartDictation() {
    let model = SettingsViewModel(
      engine: MockSettingsEngine(), permissionProbe: MockPermissionProbe())
    var dictationStarts = 0
    model.onQuickStartDictation = { dictationStarts += 1 }

    model.performQuickStart(.testMic)
    XCTAssertEqual(model.section, .audio)
    model.performQuickStart(.tuneShortcuts)
    XCTAssertEqual(model.section, .shortcuts)
    model.performQuickStart(.openOverlay)
    XCTAssertEqual(dictationStarts, 1)
    XCTAssertEqual(model.section, .shortcuts, "openOverlay must not touch rail routing")
  }

  func testDeepLinkNotificationsReachOnlyTheirOwner() {
    let links = SettingsDeepLink()
    let other = SettingsDeepLink()
    let delivered = expectation(
      forNotification: SettingsDeepLink.pendingSectionDidChange, object: links)
    delivered.assertForOverFulfill = true
    other.present(.audio, anchor: .audioReadiness)
    links.present(.agent)
    wait(for: [delivered], timeout: 0.2)
    XCTAssertEqual(links.consume()?.section, .agent)
    XCTAssertEqual(other.consume()?.anchor, .audioReadiness)
  }

  func testSectionAndAgentDeepLinksResolveToDedicatedPanels() throws {
    let links = SettingsDeepLink()
    let model = SettingsViewModel(
      engine: MockSettingsEngine(), permissionProbe: MockPermissionProbe())

    links.pendingSection = .keys
    XCTAssertEqual(links.consume()?.section.destination, .providers)
    XCTAssertNil(links.consume())

    XCTAssertEqual(SettingsDeepLink.agentConfigurationSection, .agent)
    links.pendingSection = SettingsDeepLink.agentConfigurationSection
    let sectionTarget = try XCTUnwrap(links.consume())
    XCTAssertEqual(sectionTarget.section.destination, .agent)
    XCTAssertNil(sectionTarget.tab)
    XCTAssertNil(links.consume())
    model.select(sectionTarget)
    XCTAssertEqual(model.currentTab, .agentLanes)

    links.present(tab: .agentMcp)
    let mcpTarget = try XCTUnwrap(links.consume())
    XCTAssertEqual(mcpTarget, SettingsDeepLinkTarget(tab: .agentMcp))
    model.select(mcpTarget)
    XCTAssertEqual(model.section, .agent)
    XCTAssertEqual(model.currentTab, .agentMcp)

    links.present(.audio, anchor: .audioReadiness)
    XCTAssertEqual(
      links.consume(),
      SettingsDeepLinkTarget(section: .audio, anchor: .audioReadiness)
    )
    model.select(SettingsDeepLinkTarget(section: .audio, anchor: .audioReadiness))
    XCTAssertEqual(model.section, .audio)
    XCTAssertNil(model.currentTab)
  }

  /// The Rust core decides "am I a test?" partly from this process's environment
  /// (`core/config/keychain.rs::in_xctest_host`). It has to: this suite hosts its tests
  /// inside the app, so the core sees an app binary, no libtest, and no
  /// `target/**/deps/` path — every Rust harness signal reads production.
  ///
  /// Getting that wrong is not cosmetic. Before the detector existed, the core made real
  /// Keychain calls from an ad-hoc-signed host whose signature changes on every rebuild,
  /// and this file's `testHoldBadgeControlRoundTrips…` swung between 0.001 s and 43.059 s
  /// across otherwise identical runs. The detector keys on env markers Xcode exports and
  /// has moved between versions, so it fails OPEN — no markers means "production", i.e.
  /// the slow, wrong classification comes back silently.
  ///
  /// This assertion is the only thing standing between that and a future Xcode bump.
  func testXCTestEnvMarkersPinTheSignalTheCoreKeysOn() {
    let env = ProcessInfo.processInfo.environment
    let markers = ["XCTestConfigurationFilePath", "XCTestSessionIdentifier", "XCTestBundlePath"]
    let present = markers.filter { env[$0] != nil }
    XCTAssertFalse(
      present.isEmpty,
      """
      No XCTest environment marker found in the test host, so \
      core/config/keychain.rs::in_xctest_host() now returns false for this very run and \
      the core is treating the Swift suite as a production launch again. \
      Looked for: \(markers.joined(separator: ", ")). \
      Fix the marker list on BOTH sides before trusting this suite's timings.
      """
    )
  }

  func testSettingsSplitConstructionDoesNotWriteConfigOrKeychain() {
    var configWrites: [(String, String)] = []
    let engine = MockSettingsEngine(
      updateConfigObserver: { configWrites.append(($0, $1)) }
    )
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())
    // Provider accounts carry their own presence; service keys read CsKeyStatus.
    let presence: (SettingsViewModel) -> [String] = { model in
      model.providers.map { "\($0.apiKeyAccount):\($0.apiKeySet)" }
        + model.serviceKeyAccounts.map { "\($0):\(model.keyStatus.isSet(account: $0))" }
    }
    let keychainSnapshot = presence(model)

    model.select(.keys)
    _ = ProvidersPanel(model: model)
    model.select(.agent)
    _ = AgentPanel(model: model)

    XCTAssertTrue(configWrites.isEmpty, "the IA split must not write settings.json")
    XCTAssertEqual(
      presence(model),
      keychainSnapshot,
      "the IA split must preserve the complete Keychain presence snapshot"
    )
  }

  func testHoldBadgeControlRoundTripsAllPositionsAndOffPreservesSize() {
    var persisted = CsSettings.sample
    persisted.holdIndicator = true
    persisted.holdBadgeSize = 8
    var singleWrites: [(String, String)] = []
    var batchWrites: [[CsConfigEntry]] = []
    let engine = MockSettingsEngine(
      settingsLoader: { persisted },
      updateConfigManyObserver: { entries in
        batchWrites.append(entries)
        for entry in entries {
          if entry.key == "HOLD_INDICATOR" {
            persisted.holdIndicator = entry.value == "1"
          } else if entry.key == "HOLD_BADGE_SIZE", let size = UInt32(entry.value) {
            persisted.holdBadgeSize = size
          }
        }
      },
      updateConfigObserver: { key, value in
        singleWrites.append((key, value))
        if key == "HOLD_INDICATOR" { persisted.holdIndicator = value == "1" }
      }
    )
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())
    model.refresh()

    model.setHoldBadgeOption(.off)
    XCTAssertEqual(model.holdBadgeOption, .off)
    XCTAssertEqual(model.settings.holdBadgeSize, 8, "Off must preserve the stored size")
    XCTAssertEqual(singleWrites.map(\.0), ["HOLD_INDICATOR"])

    for option in [HoldBadgeOption.four, .eight, .twelve] {
      model.setHoldBadgeOption(option)
      XCTAssertEqual(model.holdBadgeOption, option)
    }
    XCTAssertEqual(batchWrites.count, 3)
    XCTAssertTrue(
      batchWrites.allSatisfy { $0.map(\.key) == ["HOLD_INDICATOR", "HOLD_BADGE_SIZE"] })
    XCTAssertEqual(batchWrites.compactMap { $0.last?.value }, ["4", "8", "12"])
  }

  /// Dictation owns every transcription-behavior write and each control keeps
  /// its exact promoted key/value contract after the IA move.
  func testDictationControlsWriteExactPromotedKeysAndValues() {
    var writes: [(key: String, value: String)] = []
    let model = SettingsViewModel(
      engine: MockSettingsEngine(updateConfigObserver: { key, value in
        writes.append((key, value))
      }), permissionProbe: MockPermissionProbe())

    model.setToggleSilenceSeconds(3.5)
    model.setWhisperContextWindowSeconds(4.5)
    model.setLightPlusSentencePauseSeconds(0.9)
    model.setPreviewBufferDelayMs(1038)
    model.setPreviewTypingCps(10.6)
    model.setPreviewEmitWordsMax(5)
    model.setPreviewInterimSeconds(8.0)

    XCTAssertEqual(
      writes.map(\.key),
      [
        "TOGGLE_SILENCE_SEC",
        "WHISPER_CONTEXT_WINDOW_SEC",
        "LIGHT_PLUS_SENTENCE_PAUSE_SEC",
        "CODESCRIBE_BUFFER_DELAY_MS",
        "CODESCRIBE_TYPING_CPS",
        "CODESCRIBE_EMIT_WORDS_MAX",
        "CODESCRIBE_BUFFERED_INTERIM_SEC",
      ])
    XCTAssertEqual(
      writes.map(\.value),
      [
        "3.5", "4.5", "0.9", "1038", "10.6", "5", "8.0",
      ])
  }

  /// The Lab pickers write their exact promoted keys on a developer build and
  /// nothing at all on a production bundle.
  func testLabPickersWriteExactPromotedKeysOnlyOnTheDeveloperSurface() {
    var writes: [(key: String, value: String)] = []
    let model = SettingsViewModel(
      engine: MockSettingsEngine(updateConfigObserver: { key, value in
        writes.append((key, value))
      }), permissionProbe: MockPermissionProbe())

    model.setWhisperAdaptiveBuffer(true)
    model.setFormatOnDevice(true)
    model.setFormatOnDevice(false)

    guard DeveloperSurface.isEnabled() else {
      XCTAssertTrue(writes.isEmpty, "production bundle must not persist Lab knobs")
      return
    }
    XCTAssertEqual(
      writes.map(\.key),
      [
        "WHISPER_ADAPTIVE_BUFFER",
        "CODESCRIBE_FORMAT_ON_DEVICE",
        "CODESCRIBE_FORMAT_ON_DEVICE",
      ])
    XCTAssertEqual(writes.map(\.value), ["1", "1", "0"])
  }

  func testLocalWhisperDiagnosticEnvTokensMatchRuntimePolicy() {
    for value in ["", "  ", "phase1", " PHASE1 ", "1"] {
      XCTAssertEqual(
        resolveLocalWhisperRuntimeState(
          asrModeId: "local_power", layeredValue: value, modelAvailable: true
        ),
        .livePatchingConfigured
      )
    }
    for value in ["off", "0", "false", "no", " OFF ", "phase2"] {
      XCTAssertEqual(
        resolveLocalWhisperRuntimeState(
          asrModeId: "local_power", layeredValue: value, modelAvailable: true
        ),
        .degradedEnvOverride
      )
    }
  }

  func testLocalWhisperRuntimeTruthRequiresModelAndExactReadback() {
    XCTAssertEqual(
      resolveLocalWhisperRuntimeState(
        asrModeId: "local_power",
        layeredValue: nil,
        modelAvailable: true
      ),
      .livePatchingConfigured,
      "unset is the runtime's armed product default"
    )
    XCTAssertEqual(
      resolveLocalWhisperRuntimeState(
        asrModeId: "local_power",
        layeredValue: "off",
        modelAvailable: false
      ),
      .degradedEnvOverride,
      "explicit off is disarmed even when the model is also unavailable"
    )
    XCTAssertEqual(
      resolveLocalWhisperRuntimeState(
        asrModeId: "local_power",
        layeredValue: "phase1",
        modelAvailable: true
      ),
      .livePatchingConfigured
    )
    XCTAssertEqual(
      resolveLocalWhisperRuntimeState(
        asrModeId: "local_power",
        layeredValue: "phase2",
        modelAvailable: true
      ),
      .degradedEnvOverride,
      "unknown env values cannot masquerade as the armed phase1 contract"
    )
    XCTAssertEqual(
      resolveLocalWhisperRuntimeState(
        asrModeId: "local_power",
        layeredValue: nil,
        modelAvailable: false
      ),
      .livePatchingNotReady,
      "armed-by-default still needs a validated FP16 bundle"
    )
    XCTAssertEqual(
      resolveLocalWhisperRuntimeState(
        asrModeId: "cloud",
        layeredValue: "phase1",
        modelAvailable: true
      ),
      .notSelected,
      "Cloud must not present local Whisper as its provider"
    )
  }

  func testSmoothPresetValuesMatchOperatorDefaultExactly() throws {
    let smooth = try XCTUnwrap(presetValues(.smooth))

    XCTAssertEqual(smooth.bufferDelayMs, 1038)
    XCTAssertEqual(smooth.typingCps, 10.6, accuracy: 0.0001)
    XCTAssertEqual(smooth.emitWordsMax, 5)
    XCTAssertEqual(smooth.interimSeconds, 8.0, accuracy: 0.0001)
  }

  func testDetectPresetRecognizesAllFiveStatesWithTolerance() throws {
    for preset in [PreviewTimingPreset.smooth, .snappy, .relaxed] {
      let values = try XCTUnwrap(presetValues(preset))
      XCTAssertEqual(
        detectPreset(PreviewTimingConfiguration(overlayEnabled: true, values: values)),
        preset
      )
    }

    XCTAssertEqual(
      detectPreset(
        PreviewTimingConfiguration(overlayEnabled: false, values: PreviewTimingValues.smooth)
      ),
      .off
    )

    let withinTolerance = PreviewTimingValues(
      bufferDelayMs: 1048,
      typingCps: 10.74,
      emitWordsMax: 5,
      interimSeconds: 8.14
    )
    XCTAssertEqual(
      detectPreset(
        PreviewTimingConfiguration(overlayEnabled: true, values: withinTolerance)
      ),
      .smooth
    )

    let custom = PreviewTimingValues(
      bufferDelayMs: 1100,
      typingCps: 10.6,
      emitWordsMax: 5,
      interimSeconds: 8.0
    )
    XCTAssertEqual(
      detectPreset(PreviewTimingConfiguration(overlayEnabled: true, values: custom)),
      .custom
    )
  }

  func testSmoothPresetUsesOneAtomicSettingsBatch() {
    var batches: [[CsConfigEntry]] = []
    let engine = MockSettingsEngine(updateConfigManyObserver: { entries in
      batches.append(entries)
    })
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())
    var overlayPreferenceNotices = 0
    model.onOverlayPreferenceChanged = { overlayPreferenceNotices += 1 }

    model.applyPreviewTimingPreset(.custom)
    XCTAssertEqual(batches.count, 0)
    XCTAssertEqual(overlayPreferenceNotices, 0, "Custom writes nothing and announces nothing")

    model.applyPreviewTimingPreset(.smooth)

    XCTAssertEqual(overlayPreferenceNotices, 1)
    XCTAssertEqual(batches.count, 1)
    let values = Dictionary(uniqueKeysWithValues: batches[0].map { ($0.key, $0.value) })
    XCTAssertEqual(values["TRANSCRIPTION_OVERLAY_ENABLED"], "1")
    XCTAssertEqual(values["CODESCRIBE_BUFFER_DELAY_MS"], "1038")
    XCTAssertEqual(values["CODESCRIBE_TYPING_CPS"], "10.6")
    XCTAssertEqual(values["CODESCRIBE_EMIT_WORDS_MAX"], "5")
    XCTAssertEqual(values["CODESCRIBE_BUFFERED_INTERIM_SEC"], "8.0")

    model.applyPreviewTimingPreset(.off)
    XCTAssertEqual(batches.count, 2)
    XCTAssertEqual(batches[1].map(\.key), ["TRANSCRIPTION_OVERLAY_ENABLED"])
    XCTAssertEqual(batches[1].map(\.value), ["0"])
    XCTAssertEqual(
      overlayPreferenceNotices, 2,
      "Off tells the overlay's owner, so a panel already on screen can close")
  }

  /// Agent owns the one lane-edit grammar: a lane binds a provider (through
  /// the bridge registry, `LLM_<LANE>_PROVIDER`) and a model (promoted key).
  /// There is no endpoint key to write. A provider switch clears the model;
  /// whitespace is trimmed; an empty write clears the JSON override.
  func testAgentLaneEditorsPreserveExactKeysAndEmptyResetSemantics() {
    var writes: [(key: String, value: String)] = []
    let store = MockProviderStore()
    let model = SettingsViewModel(
      engine: MockSettingsEngine(
        providerStore: store,
        updateConfigObserver: { key, value in writes.append((key, value)) }
      ), permissionProbe: MockPermissionProbe(),
      runtimeLlmLaneProvider: { store.runtimeLane($0) }
    )

    XCTAssertEqual(LLMLane.allCases, [.assistive, .formatting], "the Main lane is gone (D4)")
    let lanes: [(lane: LLMLane, providerKey: String, modelKey: String)] = [
      (.assistive, "LLM_ASSISTIVE_PROVIDER", "LLM_ASSISTIVE_MODEL"),
      (.formatting, "LLM_FORMATTING_PROVIDER", "LLM_FORMATTING_MODEL"),
    ]

    for expectation in lanes {
      XCTAssertEqual(expectation.lane.providerKey, expectation.providerKey)
      XCTAssertEqual(expectation.lane.modelKey, expectation.modelKey)

      writes.removeAll()
      model.setLaneProvider("xai-responses", for: expectation.lane)
      model.setLLMModel(" grok-4.5 ", for: expectation.lane)
      model.setLLMModel("", for: expectation.lane)

      XCTAssertEqual(store.laneProviders[expectation.lane.bridgeLane], "xai-responses")
      XCTAssertEqual(model.llmLane(expectation.lane).providerId, "xai-responses")
      XCTAssertEqual(
        writes.map(\.key),
        [expectation.modelKey, expectation.modelKey, expectation.modelKey]
      )
      XCTAssertEqual(writes.map(\.value), ["", "grok-4.5", ""])
    }
  }

  /// Every consolidated owner can be constructed from the hermetic preview
  /// injection path. Pixel rendering belongs in a UI/visual test: AppKit-backed
  /// controls (`Slider`, `Picker`, `Toggle`) can recurse in off-window
  /// `NSHostingView` / `ImageRenderer` layout on macOS 26.
  func testFiveOwnerPanelsConstructFromHermeticPreviews() {
    func assertConcretePanel<Panel: View>(
      _ panel: Panel,
      model: SettingsViewModel,
      section: SettingsSection,
      name: String,
      file: StaticString = #filePath,
      line: UInt = #line
    ) {
      _ = panel
      XCTAssertEqual(
        model.section, section, "\(name) preview route drifted", file: file, line: line)
      XCTAssertNotEqual(
        ObjectIdentifier(Panel.self),
        ObjectIdentifier(EmptyView.self),
        "\(name) route resolved to an empty panel",
        file: file,
        line: line
      )
    }

    let dictation = SettingsViewModel.preview(.engine)
    assertConcretePanel(
      EnginePanel(model: dictation), model: dictation, section: .engine, name: "dictation")

    let audio = SettingsViewModel.preview(.audio)
    assertConcretePanel(AudioPanel(model: audio), model: audio, section: .audio, name: "audio")

    let dictionary = SettingsViewModel.preview(.voiceLab)
    assertConcretePanel(
      VoiceLabPanel(model: dictionary),
      model: dictionary,
      section: .voiceLab,
      name: "dictionary"
    )

    let providers = SettingsViewModel.preview(.keys)
    assertConcretePanel(
      ProvidersPanel(model: providers), model: providers, section: .keys, name: "providers")

    let agent = SettingsViewModel.preview(.agent)
    assertConcretePanel(AgentPanel(model: agent), model: agent, section: .agent, name: "agent")
    let previewLane = agent.llmLane(.assistive)
    XCTAssertEqual(previewLane.providerId, "openai-responses")
    XCTAssertEqual(previewLane.providerDisplayName, "OpenAI")
    XCTAssertEqual(previewLane.resolvedEndpoint, "https://api.openai.com/v1/responses")
  }

  func testHealthStateMatrix() {
    XCTAssertEqual(
      healthState(stt: true, recording: true, keys: .available, agent: true, formatting: true),
      SettingsHealthState(
        level: .healthy, message: "Ready to work", targetSection: nil
      )
    )
    XCTAssertEqual(
      healthState(stt: true, recording: true, keys: .missing, agent: false, formatting: true),
      SettingsHealthState(
        level: .degraded,
        message: "Agent needs setup",
        targetSection: .keys
      )
    )
    XCTAssertEqual(
      healthState(stt: false, recording: true, keys: .available, agent: true, formatting: true),
      SettingsHealthState(
        level: .offline,
        message: "Transcription unavailable",
        targetSection: .engine
      )
    )
    XCTAssertEqual(
      healthState(stt: true, recording: true, keys: .available, agent: false, formatting: true),
      SettingsHealthState(
        level: .offline,
        message: "Agent unavailable",
        targetSection: .agent
      )
    )
    XCTAssertEqual(
      healthState(stt: nil, recording: true, keys: .available, agent: true, formatting: true),
      SettingsHealthState(
        level: .unknown,
        message: nil,
        targetSection: nil
      )
    )
    XCTAssertEqual(
      healthState(stt: true, recording: false, keys: .available, agent: true, formatting: true),
      SettingsHealthState(
        level: .offline,
        message: "Recording needs setup",
        targetSection: .audio
      )
    )
    XCTAssertEqual(
      healthState(stt: true, recording: nil, keys: .available, agent: true, formatting: true),
      SettingsHealthState(
        level: .unknown,
        message: "Checking recording…",
        targetSection: .audio
      )
    )
  }

  func testReadinessRequiresEnabledFormattingButDoesNotRequireDisabledLane() {
    XCTAssertEqual(
      healthState(stt: true, recording: true, keys: .available, agent: true, formatting: false)
        .level,
      .degraded)
    XCTAssertEqual(
      healthState(stt: true, recording: true, keys: .available, agent: true, formatting: nil).level,
      .unknown)
    let disabled = healthState(
      stt: true, recording: true, keys: .available, agent: true,
      formatting: false, formattingRequired: false)
    XCTAssertEqual(disabled.level, .healthy)
    XCTAssertEqual(disabled.message, "Ready to work")
    XCTAssertEqual(
      healthState(stt: false, recording: true, keys: .available, agent: true, formatting: false)
        .level,
      .offline, "the known speech failure must remain visible")
  }

  func testCreatorLanguagePresentationKeepsTruthfulIdentityAndAccessibility() {
    let choices = LanguageIdentityPresentation.choices

    XCTAssertEqual(choices.map(\.title), ["Multilingual", "Polish", "English"])
    XCTAssertEqual(choices.map(\.isFineTuned), [false, true, true])
    XCTAssertEqual(
      choices.map(\.accessibilityLabel),
      ["Multilingual", "Polish, Fine-tuned", "English, Fine-tuned"]
    )
    XCTAssertEqual(choices[1].accessibilityValue(isSelected: true), "Selected")
    XCTAssertEqual(choices[2].accessibilityValue(isSelected: false), "Not selected")
    // The footnote names the rail section literally: Polish needs the
    // locative, so the title cannot be interpolated. A rail rename must fail
    // here until the sentence is reworded with it.
    XCTAssertEqual(
      LanguageIdentityPresentation.supportingCopy,
      "Domain vocabulary and Dictionary entries improve speech recognition."
    )
    XCTAssertTrue(
      LanguageIdentityPresentation.supportingCopy.contains(SettingsSection.voiceLab.title))
    XCTAssertFalse(LanguageIdentityPresentation.supportingCopy.contains("model weights"))
  }

  func testCreatorLanguageSelectionWritesStableRuntimeCodes() {
    var writes: [(key: String, value: String)] = []
    let engine = MockSettingsEngine(updateConfigObserver: { key, value in
      writes.append((key, value))
    })
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())

    model.setLanguage(.auto)
    model.setLanguage(.polish)
    model.setLanguage(.english)

    XCTAssertEqual(
      writes.map(\.key),
      [
        "WHISPER_LANGUAGE", "WHISPER_LANGUAGE", "WHISPER_LANGUAGE",
      ])
    XCTAssertEqual(writes.map(\.value), ["auto", "pl", "en"])
  }

  func testFormattingPolicyNamesAliasesAndWritesAreNormalized() {
    XCTAssertEqual(
      FormattingPolicyOption.allCases.map(\.visibleName),
      ["Off", "Correction", "Smart", "Max"]
    )
    XCTAssertEqual(FormattingPolicyOption(storedValue: "raw"), .off)
    XCTAssertEqual(FormattingPolicyOption(storedValue: "medium"), .correction)
    XCTAssertEqual(FormattingPolicyOption(storedValue: "creative"), .max)
    XCTAssertNil(FormattingPolicyOption(storedValue: "aggressive"))

    var writes: [(String, String)] = []
    let model = SettingsViewModel(
      engine: MockSettingsEngine(updateConfigObserver: { key, value in
        writes.append((key, value))
      }), permissionProbe: MockPermissionProbe())
    for value in ["raw", "medium", "smart", "creative"] {
      model.setFormattingLevel(value)
    }
    model.setFormattingLevel("aggressive")

    XCTAssertEqual(writes.map(\.0), Array(repeating: "FORMATTING_LEVEL", count: 4))
    XCTAssertEqual(writes.map(\.1), ["off", "correction", "smart", "max"])
    XCTAssertNotNil(model.lastError)
  }

  func testCreatorPanelRendersAtCompactAndLargeWidths() throws {
    for (name, width) in [("compact", 620.0), ("large", 900.0)] {
      let size = CGSize(width: width, height: 900)
      let model = SettingsViewModel(
        engine: MockSettingsEngine(), permissionProbe: MockPermissionProbe())
      let hostingView = NSHostingView(
        rootView: CreatorPanel(model: model).frame(
          width: size.width,
          height: size.height,
          alignment: .topLeading
        ))
      hostingView.frame = CGRect(origin: .zero, size: size)
      hostingView.layoutSubtreeIfNeeded()

      guard let bitmap = hostingView.bitmapImageRepForCachingDisplay(in: hostingView.bounds) else {
        return XCTFail("Could not allocate \(name) CreatorPanel bitmap")
      }
      hostingView.cacheDisplay(in: hostingView.bounds, to: bitmap)
      guard let png = bitmap.representation(using: .png, properties: [:]) else {
        return XCTFail("Could not encode \(name) CreatorPanel PNG")
      }
      XCTAssertGreaterThan(png.count, 20_000)

      let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent("codescribe-settings-captures", isDirectory: true)
      try FileManager.default.createDirectory(
        at: directory,
        withIntermediateDirectories: true
      )
      try png.write(to: directory.appendingPathComponent("creator-language-\(name).png"))
    }
  }

  func testPromptPanelRendersAllFormattingOwners() throws {
    let size = CGSize(width: 900, height: 1_900)
    let model = SettingsViewModel(
      engine: MockSettingsEngine(), permissionProbe: MockPermissionProbe())
    let hostingView = NSHostingView(
      rootView: PromptPanel(model: model).frame(
        width: size.width,
        height: size.height,
        alignment: .topLeading
      ))
    hostingView.frame = CGRect(origin: .zero, size: size)
    hostingView.layoutSubtreeIfNeeded()

    guard let bitmap = hostingView.bitmapImageRepForCachingDisplay(in: hostingView.bounds) else {
      return XCTFail("Could not allocate PromptPanel bitmap")
    }
    hostingView.cacheDisplay(in: hostingView.bounds, to: bitmap)
    guard let png = bitmap.representation(using: .png, properties: [:]) else {
      return XCTFail("Could not encode PromptPanel PNG")
    }
    XCTAssertGreaterThan(png.count, 40_000)

    let directory = FileManager.default.temporaryDirectory
      .appendingPathComponent("codescribe-settings-captures", isDirectory: true)
    try FileManager.default.createDirectory(
      at: directory,
      withIntermediateDirectories: true
    )
    try png.write(to: directory.appendingPathComponent("prompt-owners.png"))
  }

  func testTaggingToggleWritesPromotedConfigKey() {
    var writes: [(key: String, value: String)] = []
    let engine = MockSettingsEngine(updateConfigObserver: { key, value in
      writes.append((key, value))
    })
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())

    model.setTranscriptTaggingEnabled(true)
    model.setTranscriptTaggingEnabled(false)

    XCTAssertEqual(
      writes.map(\.key),
      [
        "TRANSCRIPT_TAGGING_ENABLED", "TRANSCRIPT_TAGGING_ENABLED",
      ])
    XCTAssertEqual(writes.map(\.value), ["1", "0"])
  }

  func testTranscriptTagTemplateWritesPromotedConfigKeyAndAllowsStaticAttributes() {
    var writes: [(key: String, value: String)] = []
    let engine = MockSettingsEngine(updateConfigObserver: { key, value in
      writes.append((key, value))
    })
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())

    model.setTranscriptTagTemplate(
      "<codescribe warn=\"may contain misspelling\">{text}</codescribe>")

    XCTAssertEqual(writes.map(\.key), ["TRANSCRIPT_TAG_TEMPLATE"])
    XCTAssertEqual(
      writes.map(\.value),
      ["<codescribe warn=\"may contain misspelling\">{text}</codescribe>"]
    )
  }

  func testTranscriptTagTemplatePreviewWarnsAndAppendsWhenTextPlaceholderMissing() {
    let model = SettingsViewModel(permissionProbe: MockPermissionProbe())

    model.setTranscriptTagTemplate("<codescribe conf=\"{conf}\" flags=\"{flags}\">")

    XCTAssertEqual(
      model.transcriptTagPreview,
      "<codescribe conf=\"medium\" flags=\"possible_hallucination_logprob\">\n…"
    )
    XCTAssertEqual(
      model.transcriptTagTemplateWarning,
      "Missing {text}; delivered transcript will be appended after the template."
    )
  }

  func testRestoreTranscriptTagTemplateWritesDefault() {
    var writes: [(key: String, value: String)] = []
    let engine = MockSettingsEngine(updateConfigObserver: { key, value in
      writes.append((key, value))
    })
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())

    model.restoreDefaultTranscriptTagTemplate()

    XCTAssertEqual(writes.map(\.key), ["TRANSCRIPT_TAG_TEMPLATE"])
    XCTAssertEqual(writes.map(\.value), [defaultTranscriptTagTemplate])
  }

  func testResetPreviewMapsLiveCountsIntoConcreteConfirmationCopy() {
    let preview = CsResetPreview(
      audioFiles: 5_000,
      transcriptDays: 42,
      threads: 17,
      totalBytes: 536_870_912
    )
    let model = SettingsViewModel(
      engine: MockSettingsEngine(resetPreviewValue: preview), permissionProbe: MockPermissionProbe()
    )

    model.refreshResetPreview()

    XCTAssertEqual(model.resetPreview.audioFiles, 5_000)
    XCTAssertEqual(
      model.resetImpactDescription(includeKeys: false, includePrompts: false),
      "Moves 5,000 recordings from 42 days, 17 threads (512.0 MB) to Trash. "
        + "Your assistive.txt and three formatting prompt files will be preserved. "
        + "Codescribe will relaunch as a fresh install."
    )
    XCTAssertTrue(resetConfirmationMatches("RESET"))
    XCTAssertFalse(resetConfirmationMatches("reset"))
    XCTAssertFalse(resetConfirmationMatches(" RESET"))
  }

  func testPromptSourceLabelsExposeFileFallbackAndReadErrorTruth() {
    XCTAssertEqual(promptSourceLabel("custom_file"), "Source: Custom prompt")
    XCTAssertEqual(promptSourceLabel("built_in_fallback"), "Source: Built-in prompt")
    XCTAssertEqual(promptSourceLabel("read_error"), "Source: Built-in prompt (file unreadable)")
    XCTAssertEqual(promptSourceLabel(nil), "Source unavailable")
  }

  /// File details tell an existing custom file apart from the path a custom
  /// prompt would be created at; an empty file is named as empty, not missing.
  func testPromptFileStatusSeparatesExistingFromCreatable() {
    XCTAssertEqual(
      promptFileStatus(source: "custom_file", fileExists: true), "Custom prompt file in use.")
    XCTAssertEqual(
      promptFileStatus(source: "built_in_fallback", fileExists: false),
      "No custom prompt file yet. Saving creates one at this path.")
    XCTAssertEqual(
      promptFileStatus(source: "built_in_fallback", fileExists: true),
      "The file exists but is empty, so the built-in prompt is in use.")
    XCTAssertEqual(
      promptFileStatus(source: "read_error", fileExists: true),
      "The file exists but could not be read.")
  }

  func testPromptRestoreTargetsOnlyTheConfirmedPrompt() {
    var restored: [String] = []
    let engine = MockSettingsEngine(
      promptRestoreObserver: { restored.append($0) }
    )
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())

    XCTAssertNotNil(model.restoreFormattingPromptToDefault(.correction))
    XCTAssertNotNil(model.restoreFormattingPromptToDefault(.smart))
    XCTAssertNotNil(model.restoreFormattingPromptToDefault(.max))

    XCTAssertEqual(restored, ["correction", "smart", "max"])
  }

  /// Diagnostics renders rows from the core's facet + state, never by parsing
  /// the English value: the structured parts land in the sentence.
  func testDiagnosticsRowsRenderFromFacetAndStateNotTheEnglishValue() {
    let ready = CsMcpStatusRow(
      label: "Agentic readiness:", value: "ready — raw english", tone: .good,
      facet: .readiness, state: .ready, count: 26, subject: "xAI (Grok)", detail: "")
    XCTAssertEqual(ready.localizedLabel, "Overall status")
    XCTAssertEqual(
      ready.localizedValue, "Ready — xAI (Grok) configured, access available, 26 native tools")

    let provider = CsMcpStatusRow(
      label: "Provider:", value: "", tone: .bad,
      facet: .provider, state: .accessUnavailable, count: nil, subject: "OpenAI",
      detail: "OPENAI_API_KEY")
    XCTAssertEqual(provider.localizedLabel, "Model provider")
    XCTAssertEqual(provider.localizedValue, "OpenAI — no access (sign in or set OPENAI_API_KEY)")

    let roots = CsMcpStatusRow(
      label: "Workspace roots:", value: "", tone: .good,
      facet: .workspaceRoots, state: .synchronized, count: 1, subject: "", detail: "")
    XCTAssertEqual(roots.localizedLabel, "Folders available to the Agent")
    XCTAssertEqual(roots.localizedValue, "1 folder — native tools synchronized")

    let prview = CsMcpStatusRow(
      label: "PRView integration:", value: "", tone: .warn,
      facet: .prviewIntegration, state: .configured, count: nil, subject: "prview-mcp", detail: "")
    XCTAssertEqual(prview.localizedLabel, "PRView integration")
    XCTAssertEqual(prview.localizedValue, "Configured — agent not started yet (server prview-mcp)")

    let server = CsMcpStatusRow(
      label: "curl:", value: "", tone: .bad,
      facet: .mcpServer, state: .failed, count: nil, subject: "curl", detail: "command not found")
    XCTAssertEqual(server.localizedLabel, "curl")
    XCTAssertEqual(server.localizedValue, "Failed: command not found")

    XCTAssertEqual(CsMcpRowTone.good.label, "Good")
    XCTAssertEqual(CsMcpRowTone.warn.label, "Warning")
    XCTAssertEqual(CsMcpRowTone.bad.label, "Error")
    XCTAssertEqual(CsMcpRowTone.neutral.label, "Not checked")
  }

  /// The capability summary counts tiers; the row headline comes from tier +
  /// provider so the English reason stays a tooltip.
  func testCapabilitySummaryCountsTiersAndHeadlinesDropTheRawReason() {
    let summary = CapabilitySummary(rows: CsCapabilityRow.sampleMatrix)
    XCTAssertEqual(summary.native, 1)
    XCTAssertEqual(summary.enhanced, 1)
    XCTAssertEqual(summary.unavailable, 1)
    XCTAssertEqual(summary.line, "Native: 1 · Enhanced: 1 · Unavailable: 1")

    let rows = CsCapabilityRow.sampleMatrix
    XCTAssertEqual(rows[0].localizedTier, "Native")
    XCTAssertEqual(rows[0].localizedHeadline, "Built-in Codescribe tool")
    XCTAssertEqual(rows[0].localizedDetail, "tool: list_directory · source: native")
    XCTAssertEqual(
      rows[1].localizedHeadline, "Built-in tool, enriched by Loctree while it is healthy")
    XCTAssertEqual(rows[2].localizedTier, "Unavailable")
    XCTAssertEqual(
      rows[2].localizedHeadline, "Unavailable — no built-in tool and no healthy MCP server")
    XCTAssertNil(
      CsCapabilityRow(op: "x", tier: "unavailable", provider: "", nativeTool: "", reason: "")
        .localizedDetail)
  }

  /// One MCP table line per configured server: the probe row joins by name
  /// and the cached test result becomes its own column.
  func testMcpServerLinesMergeProbeRowsWithTestResults() {
    let servers = [
      CsMcpServer(
        name: "loctree-mcp", command: "loct", args: [], envKeys: [], enabled: true,
        transport: "stdio",
        endpoint: "", authRef: ""),
      CsMcpServer(
        name: "aicx-mcp", command: "aicx", args: [], envKeys: [], enabled: true, transport: "stdio",
        endpoint: "", authRef: ""),
      CsMcpServer(
        name: "orphan", command: "x", args: [], envKeys: [], enabled: true, transport: "stdio",
        endpoint: "", authRef: ""),
    ]
    let results = [
      "loctree-mcp": CsMcpTestResult(
        ok: true, toolCount: 9, serverName: "loctree-mcp", serverVersion: "1.2",
        protocolVersion: "", error: ""),
      "aicx-mcp": CsMcpTestResult(
        ok: false, toolCount: 0, serverName: "aicx-mcp", serverVersion: "", protocolVersion: "",
        error: "timeout"),
    ]
    let lines = McpServerLine.merge(
      servers: servers, statusRows: CsMcpStatusReport.sample.rows, results: results,
      pending: ["orphan"])
    XCTAssertEqual(lines.map(\.name), ["loctree-mcp", "aicx-mcp", "orphan"])
    XCTAssertEqual(lines[0].status?.localizedValue, "Live — 9 tools")
    XCTAssertEqual(lines[0].testText, "OK — 9 tools · v1.2")
    XCTAssertEqual(lines[0].testTone, .good)
    XCTAssertEqual(lines[1].status?.state, .configured)
    XCTAssertEqual(lines[1].testText, "Failed: timeout")
    XCTAssertEqual(lines[1].testTone, .bad)
    XCTAssertNil(lines[2].status, "a server without a probe row keeps its test column only")
    XCTAssertEqual(lines[2].testText, "Testing…")
    XCTAssertEqual(lines[2].testTone, .warn)
  }

  /// After a restore the refreshed snapshot reads "Built-in prompt": the
  /// custom file is gone, so the source flips and the path stays.
  func testPromptRestoreReturnsTheBuiltInSnapshot() throws {
    let model = SettingsViewModel(
      engine: MockSettingsEngine(), permissionProbe: MockPermissionProbe())
    XCTAssertEqual(model.formattingPromptSnapshot(level: .correction)?.source, "custom_file")
    XCTAssertEqual(model.assistivePromptSnapshot().source, "custom_file")

    let formatting = try XCTUnwrap(model.restoreFormattingPromptToDefault(.correction))
    XCTAssertEqual(formatting.source, "built_in_fallback")
    XCTAssertEqual(formatting.path, CsPromptSnapshot.sampleFormatting.path)
    XCTAssertEqual(model.formattingPromptSnapshot(level: .correction)?.source, "built_in_fallback")
    XCTAssertEqual(
      model.formattingPromptSnapshot(level: .smart)?.source, "built_in_fallback",
      "untouched prompts keep their own source")

    let assistive = try XCTUnwrap(model.restoreAssistivePromptToDefault())
    XCTAssertEqual(assistive.source, "built_in_fallback")
    XCTAssertEqual(assistive.content, CsSettings.sampleAssistivePrompt)
    XCTAssertNil(model.lastError)
  }

  /// A restore the engine refuses returns no snapshot and keeps the error, so
  /// the panel shows a failure and the custom prompt stays in use.
  func testPromptRestoreFailureReturnsNoSnapshotAndKeepsTheError() {
    let engine = MockSettingsEngine(
      promptRestoreObserver: { _ in
        throw NSError(
          domain: "Prompt", code: 7, userInfo: [NSLocalizedDescriptionKey: "removal refused"])
      }
    )
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())

    XCTAssertNil(model.restoreFormattingPromptToDefault(.correction))
    XCTAssertEqual(model.lastError?.contains("removal refused"), true)
    XCTAssertEqual(
      model.formattingPromptSnapshot(level: .correction)?.source, "custom_file",
      "a failed restore leaves the custom prompt in use")
    XCTAssertNil(model.restoreAssistivePromptToDefault())
    XCTAssertEqual(model.assistivePromptSnapshot().source, "custom_file")
  }

  func testPromptFailureLabelsNameTheOperationAndWhatDidNotChange() {
    XCTAssertEqual(
      promptFailureLabel(.restore, title: "Smart prompt"),
      "Could not restore Smart prompt. The custom prompt is still in use.")
    XCTAssertEqual(
      promptFailureLabel(.save, title: "Agent prompt"),
      "Could not save Agent prompt. The file on disk is unchanged.")
    XCTAssertEqual(
      PromptOperationFailure(operation: .restore, detail: "x"),
      PromptOperationFailure(operation: .restore, detail: "x"))
  }

  func testFormattingPromptSnapshotsExposeDistinctPathsAndProvenance() throws {
    let model = SettingsViewModel(
      engine: MockSettingsEngine(), permissionProbe: MockPermissionProbe())
    let snapshots = try FormattingPolicyOption.editablePrompts.map { level in
      try XCTUnwrap(model.formattingPromptSnapshot(level: level))
    }

    XCTAssertEqual(
      snapshots.map { URL(fileURLWithPath: $0.path).lastPathComponent },
      ["formatting.txt", "formatting-smart.txt", "formatting-max.txt"]
    )
    XCTAssertEqual(
      snapshots.map(\.source),
      ["custom_file", "built_in_fallback", "built_in_fallback"]
    )
  }

  func testFailedPromptSaveDoesNotClaimARefreshedSnapshot() {
    let engine = MockSettingsEngine(
      promptSaveObserver: { _, _ in
        throw NSError(domain: "PromptWrite", code: 1)
      }
    )
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())

    XCTAssertNil(model.saveAssistivePrompt("replacement"))
    XCTAssertNotNil(model.lastError)
    XCTAssertEqual(model.assistivePromptSnapshot().content, CsSettings.sampleAssistivePrompt)
  }

  func testAppResetPreservesPromptsUnlessSeparateOptInIsEnabled() {
    var calls: [(keys: Bool, prompts: Bool)] = []
    let engine = MockSettingsEngine(
      resetAppDataObserver: { calls.append(($0, $1)) }
    )
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())

    // Exercise the bridge contract directly: SettingsViewModel relaunches
    // after success, which is intentionally not invoked in XCTest.
    try? engine.resetAppData(includeKeys: false, includePrompts: false)
    try? engine.resetAppData(includeKeys: true, includePrompts: true)

    XCTAssertEqual(calls.map(\.keys), [false, true])
    XCTAssertEqual(calls.map(\.prompts), [false, true])
    XCTAssertTrue(
      model.resetImpactDescription(includeKeys: false, includePrompts: true)
        .contains("assistive.txt and three formatting prompt files will also move to Trash")
    )
  }

  func testAgentResetIsSeparatelyConfirmedAndNamesPreservedSurfaces() throws {
    var calls = 0
    let preview = CsAgentResetPreview(
      threads: 2,
      files: 5,
      totalBytes: 2_048,
      secretsPresent: true
    )
    let engine = MockSettingsEngine(
      agentResetPreviewValue: preview,
      resetAgentDataObserver: { calls += 1 }
    )
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())

    model.refreshAgentResetPreview()
    XCTAssertEqual(model.agentResetPreview.threads, 2)
    XCTAssertTrue(resetAgentConfirmationMatches("RESET AGENT"))
    XCTAssertFalse(resetAgentConfirmationMatches("RESET"))
    XCTAssertFalse(resetAgentConfirmationMatches("reset agent"))
    XCTAssertTrue(model.resetAgentImpactDescription().contains("Recordings, transcriptions"))
    XCTAssertTrue(model.resetAgentImpactDescription().contains("license"))

    try engine.resetAgentData()
    XCTAssertEqual(calls, 1)
  }

  func testAgentDefaultsResetLeavesHotkeysAndOtherPreferencesUntouched() {
    let suite = "SettingsTruthTests.agent-reset-\(UUID().uuidString)"
    guard let defaults = UserDefaults(suiteName: suite) else {
      XCTFail("create isolated defaults suite")
      return
    }
    defaults.set("queued Agent turn", forKey: AgentChatStore.acceptedTurnsDefaultsKey)
    defaults.set("attachment metadata", forKey: AgentChatStore.attachmentMetadataDefaultsKey)
    defaults.set("ctrl-space", forKey: "codescribe.hotkey")
    defaults.set("dictation preference", forKey: "codescribe.dictation")

    AppRelaunch.clearAgentDefaults(defaults: defaults)

    XCTAssertNil(defaults.object(forKey: AgentChatStore.acceptedTurnsDefaultsKey))
    XCTAssertNil(defaults.object(forKey: AgentChatStore.attachmentMetadataDefaultsKey))
    XCTAssertEqual(defaults.string(forKey: "codescribe.hotkey"), "ctrl-space")
    XCTAssertEqual(defaults.string(forKey: "codescribe.dictation"), "dictation preference")
    defaults.removePersistentDomain(forName: suite)
  }

  func testOnlyPostMutationAgentResetErrorsRequireRelaunch() {
    XCTAssertFalse(agentResetFailureRequiresRelaunch("failed to prepare Agent Trash destination"))
    XCTAssertTrue(
      agentResetFailureRequiresRelaunch(
        "CODESCRIBE_AGENT_RESET_RELAUNCH_REQUIRED: failed to remove Agent secret"
      )
    )
  }

  func testOnlyPostDestructiveResetErrorsRequireRelaunch() {
    XCTAssertFalse(resetFailureRequiresRelaunch("failed to prepare Trash destination"))
    XCTAssertTrue(
      resetFailureRequiresRelaunch(
        "CODESCRIBE_RESET_RELAUNCH_REQUIRED: app data moved but prompt restore failed"
      )
    )
  }

  /// A flipped `enabled` flag invalidates the cached handshake: the card must
  /// not keep saying "passed" about a configuration that was just edited.
  func testToggleMcpServerDropsTheStaleTestResult() async {
    let admin = ScriptedMcpAdmin(servers: [
      CsMcpServer(
        name: "loctree-mcp", command: "loct", args: ["mcp"], envKeys: [], enabled: true,
        transport: "stdio", endpoint: "", authRef: "")
    ])
    let model = SettingsViewModel(
      engine: MockSettingsEngine(), permissionProbe: MockPermissionProbe(), mcpAdmin: admin)
    model.reloadMcpServers()
    model.testMcpServer("loctree-mcp")
    for _ in 0..<100 where model.mcpTestPending.contains("loctree-mcp") { await Task.yield() }
    XCTAssertEqual(model.mcpTestResults["loctree-mcp"]?.ok, true)

    model.toggleMcpServer(model.mcpServers[0])

    XCTAssertNil(model.mcpTestResults["loctree-mcp"])
    XCTAssertEqual(model.mcpServers.first?.enabled, false)
    XCTAssertEqual(admin.updates, ["loctree-mcp"])
  }

  /// A rejected add hands the store's message back to the form, which keeps
  /// the typed fields; a successful add returns nil.
  func testAddMcpServerReportsTheStoreFailure() {
    let admin = ScriptedMcpAdmin(servers: [], addFailure: "server name already exists")
    let model = SettingsViewModel(
      engine: MockSettingsEngine(), permissionProbe: MockPermissionProbe(), mcpAdmin: admin)

    XCTAssertEqual(
      model.addMcpServer(name: "prview", command: "prview", args: ["mcp"]),
      "server name already exists")
    XCTAssertEqual(model.lastError, "server name already exists")
    XCTAssertTrue(model.mcpServers.isEmpty)

    admin.addFailure = nil
    XCTAssertNil(model.addMcpServer(name: "prview", command: "prview", args: ["mcp"]))
    XCTAssertEqual(model.mcpServers.map(\.name), ["prview"])
  }

  /// The card reads the server rule from the live policy instead of a literal.
  func testMcpServerPermissionLevelReadsTheLivePolicy() async {
    let model = SettingsViewModel(
      engine: MockSettingsEngine(), permissionProbe: MockPermissionProbe(),
      mcpAdmin: ScriptedMcpAdmin(servers: [], rules: ["prview=ask", "dc=deny"]))
    model.reloadToolPermissions()
    for _ in 0..<100 where model.permissionPolicy.servers.isEmpty { await Task.yield() }
    XCTAssertEqual(model.mcpServerPermissionLevel("prview"), "ask")
    XCTAssertEqual(model.mcpServerPermissionLevel("dc"), "deny")
    XCTAssertNil(model.mcpServerPermissionLevel("loctree-mcp"))
  }

  func testClearMcpConfigurationUsesDedicatedEngineContract() {
    var calls = 0
    let model = SettingsViewModel(
      engine: MockSettingsEngine(
        clearMcpConfigurationObserver: { calls += 1 }
      ), permissionProbe: MockPermissionProbe())

    model.clearMcpConfiguration()

    XCTAssertEqual(calls, 1)
  }

  /// RED contract for the missing promoted deferred-insert picker. The test
  /// names the intended Settings view-model seam directly; until the picker
  /// exists the Swift target must fail at that missing API, not pass through
  /// a test-only surrogate.
  func testDeferredInsertPickerRoundTrips() {
    var writes: [(String, String)] = []
    let model = SettingsViewModel(
      engine: MockSettingsEngine(
        updateConfigObserver: { key, value in writes.append((key, value)) }),
      permissionProbe: MockPermissionProbe())
    _ = ShortcutsPanel(model: model)

    let cases: [(DeferredInsertShortcutOption, String)] = [
      (.disabled, "disabled"),
      (.commandOptionV, "command_option_v"),
      (.commandShiftV, "command_shift_v"),
      (.commandControlV, "command_control_v"),
    ]
    for (option, wireValue) in cases {
      model.setDeferredInsertShortcut(option)
      XCTAssertEqual(writes.last?.0, "CODESCRIBE_DEFERRED_INSERT_SHORTCUT")
      XCTAssertEqual(writes.last?.1, wireValue)
    }
  }

  /// A passive Settings load must restore the canonical persisted chord and
  /// never route it back through `update_config`. That prevents reopening
  /// Shortcuts from overwriting a non-default choice with the UI default.
  func testDeferredInsertPickerRestoresPersistedSelectionWithoutWriteBack() {
    var persisted = CsSettings.sample
    persisted.deferredInsertShortcut = "command_shift_v"
    var writes: [(String, String)] = []
    let engine = MockSettingsEngine(
      settingsLoader: { persisted },
      updateConfigObserver: { writes.append(($0, $1)) }
    )

    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())
    XCTAssertEqual(model.deferredInsertShortcut, .commandShiftV)
    XCTAssertTrue(writes.isEmpty, "construction must only read persisted truth")

    model.refresh()
    XCTAssertEqual(model.deferredInsertShortcut, .commandShiftV)
    XCTAssertTrue(writes.isEmpty, "passive refresh must not write the picker value back")
  }

  /// Safe / Comfort / Off write the one promoted `PASTE_MODE` key with the
  /// core wire spelling, and a passive load restores the persisted mode
  /// without writing it back.
  func testPasteModePickerRoundTripsAndRestoresWithoutWriteBack() {
    var persisted = CsSettings.sample
    persisted.pasteMode = .comfort
    var writes: [(String, String)] = []
    let model = SettingsViewModel(
      engine: MockSettingsEngine(
        settingsLoader: { persisted },
        updateConfigObserver: { writes.append(($0, $1)) }), permissionProbe: MockPermissionProbe()
    )
    _ = ShortcutsPanel(model: model)
    XCTAssertEqual(model.pasteMode, .comfort)
    model.refresh()
    XCTAssertTrue(writes.isEmpty, "a passive load must not write the paste mode back")

    let cases: [(CsPasteMode, String)] = [(.safe, "safe"), (.comfort, "comfort"), (.off, "off")]
    for (mode, wireValue) in cases {
      persisted.pasteMode = mode
      model.setPasteMode(mode)
      XCTAssertEqual(writes.last?.0, "PASTE_MODE")
      XCTAssertEqual(writes.last?.1, wireValue)
      XCTAssertEqual(model.pasteMode, mode)
    }
    XCTAssertEqual(CsPasteMode.allModes.count, 3)
    XCTAssertEqual(Set(CsPasteMode.allModes.map(\.blurb)).count, 3, "one sentence per mode")
  }

  /// Active STT consumes last serving verdict; Apple→Whisper fallback must not
  /// display configured Apple preference.
  func testActiveSTTUsesServingVerdictNotConfiguredEngine() {
    let model = SettingsViewModel(
      engine: MockSettingsEngine(), permissionProbe: MockPermissionProbe())
    // No runtime verdict yet — never project configured engine as Active STT.
    model.lastServingVerdict = nil
    XCTAssertEqual(model.activeSTT, "Not yet served")
    XCTAssertEqual(formatActiveSTT(lastServing: nil), "Not yet served")

    // Deterministic Apple→Whisper fallback status.
    let fallback = LastServingVerdict(
      engine: "local_whisper",
      routingMode: "smart",
      disposition: "changed",
      fallbackUsed: true
    )
    model.lastServingVerdict = fallback
    let label = model.activeSTT
    XCTAssertTrue(label.contains("Whisper"), "got \(label)")
    XCTAssertTrue(label.contains("fallback"), "got \(label)")
    XCTAssertFalse(label.contains("Apple"), "fallback must not show Apple: \(label)")
    XCTAssertEqual(formatActiveSTT(lastServing: fallback), "Whisper (fallback)")

    model.lastServingVerdict = LastServingVerdict(
      engine: "local_apple",
      routingMode: "smart",
      disposition: "unchanged",
      fallbackUsed: false
    )
    XCTAssertEqual(model.activeSTT, "Apple")
    XCTAssertFalse(model.activeSTT.contains("Smart final pass"))

    let labels = ["streaming_whisper": "Streaming Whisper", "cloud_stt": "Cloud"]
    for (engine, label) in labels {
      let verdict = LastServingVerdict(
        engine: engine, routingMode: "smart", disposition: nil, fallbackUsed: false)
      XCTAssertEqual(formatActiveSTT(lastServing: verdict), label)
    }
    XCTAssertFalse(model.activeSTT.contains("Not yet served"))
  }

  func testActiveSTTRefreshesFromServingProviderOnDemand() {
    var snapshot: LastServingVerdict? = nil
    let model = SettingsViewModel(
      engine: MockSettingsEngine(), permissionProbe: MockPermissionProbe(),
      servingStatusProvider: { snapshot }
    )
    XCTAssertEqual(model.activeSTT, "Not yet served")

    snapshot = LastServingVerdict(
      engine: "local_apple",
      routingMode: "off",
      disposition: nil,
      fallbackUsed: false
    )
    model.refreshServingStatus()
    XCTAssertEqual(model.activeSTT, "Apple")
  }

  func testAsrModePickerPersistsPromotedKeysAndRequiresCloudConsent() throws {
    var writes: [(String, String)] = []
    var persisted = CsSettings.sample
    persisted.asrMode = "cloud"
    persisted.cloudConsent = nil
    let applyWrite: (String, String) -> Void = { key, value in
      writes.append((key, value))
      switch key {
      case "CODESCRIBE_ASR_MODE": persisted.asrMode = value
      case "CODESCRIBE_CLOUD_CONSENT": persisted.cloudConsent = value
      case "CODESCRIBE_LAYERED_TRANSCRIPTION": persisted.layeredTranscription = value
      case "CODESCRIBE_ASR_GATEWAY_URL": persisted.asrGatewayUrl = value
      case "STT_FILE_ENDPOINT": persisted.sttFileEndpoint = value
      case "STT_LIVE_ENDPOINT": persisted.sttLiveEndpoint = value
      default: break
      }
    }
    let engine = MockSettingsEngine(
      settingsLoader: { persisted },
      updateConfigManyObserver: { entries in
        for entry in entries { applyWrite(entry.key, entry.value) }
      },
      updateConfigObserver: applyWrite
    )
    let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())
    _ = EnginePanel(model: model)

    XCTAssertEqual(model.asrModeId, "apple_only")
    XCTAssertEqual(model.asrModeLabel, "Apple only")
    XCTAssertFalse(model.cloudConsentGranted)

    model.setAsrMode("local_power")
    XCTAssertEqual(writes.map(\.0), ["CODESCRIBE_ASR_MODE"])
    XCTAssertEqual(writes.map(\.1), ["local_power"])
    XCTAssertEqual(model.asrModeId, "local_power")

    writes.removeAll()
    model.setAsrMode("cloud")
    XCTAssertEqual(writes.map(\.0), ["CODESCRIBE_CLOUD_CONSENT", "CODESCRIBE_ASR_MODE"])
    XCTAssertEqual(writes.map(\.1), ["granted", "cloud"])
    XCTAssertEqual(model.asrModeId, "cloud")
    XCTAssertTrue(model.cloudConsentGranted)

    writes.removeAll()
    model.setAsrMode("apple_only")
    XCTAssertEqual(writes.map(\.0), ["CODESCRIBE_ASR_MODE"])
    XCTAssertEqual(writes.map(\.1), ["apple_only"])
    XCTAssertEqual(model.asrModeId, "apple_only")

    try model.setSttLaneEndpoint("live", "wss://asr.example/v1/audio/transcribe")
    XCTAssertEqual(writes.last?.0, "STT_LIVE_ENDPOINT")
    XCTAssertEqual(model.sttLanes.last?.endpoint, "wss://asr.example/v1/audio/transcribe")
    try model.setSttLaneEndpoint("file", "https://asr.example/v1/audio/transcriptions")
    XCTAssertEqual(writes.last?.0, "STT_FILE_ENDPOINT")
    XCTAssertEqual(model.sttLanes.first?.endpoint, "https://asr.example/v1/audio/transcriptions")
    try model.setAsrGatewayUrl("https://gateway.example/session")
    XCTAssertEqual(writes.last?.0, "CODESCRIBE_ASR_GATEWAY_URL")
  }

  func testModeSelectionPreservesDiagnosticEnvOverrideWithoutWritingIt() {
    var persisted = CsSettings.sample
    persisted.asrMode = "local_power"
    persisted.layeredTranscription = "off"
    var writes: [String] = []
    let model = SettingsViewModel(
      engine: MockSettingsEngine(
        settingsLoader: { persisted },
        updateConfigObserver: { key, value in
          writes.append(key)
          if key == "CODESCRIBE_ASR_MODE" { persisted.asrMode = value }
        }
      ), permissionProbe: MockPermissionProbe())
    model.refresh()
    model.setAsrMode("local_power")

    XCTAssertEqual(writes, ["CODESCRIBE_ASR_MODE"])
    XCTAssertEqual(model.settings.layeredTranscription, "off")
    XCTAssertEqual(model.localWhisperRuntimeState, .degradedEnvOverride)
  }

  func testTabbedHeaderIsOutsideItsOnlyScrollView() throws {
    let pane = try settingsLayoutSource("SettingsTabbedPane.swift")
    let scroll = try XCTUnwrap(pane.range(of: "      ScrollView {"))
    let header = try XCTUnwrap(pane.range(of: "SettingsTabBar(model: model, section: section)"))
    XCTAssertLessThan(header.lowerBound, scroll.lowerBound)
    XCTAssertEqual(
      pane.components(separatedBy: "SettingsTabBar(model: model, section: section)").count, 2)
    XCTAssertFalse(pane.contains("windowWash"))
    XCTAssertFalse(pane.contains("preferredColorScheme"))
    XCTAssertTrue(pane[..<scroll.lowerBound].contains("Divider()"))
    XCTAssertTrue(
      pane[scroll.lowerBound...].contains("SettingsPageHeader(tab.headline, blurb: tab.blurb)"))
    XCTAssertTrue(pane[scroll.lowerBound...].contains(".id(model.currentTab)"))

    let detail = try settingsLayoutSource("SettingsView.swift")
    XCTAssertTrue(detail.contains("case .dictation:\n          EnginePanel(model: model)"))
    XCTAssertTrue(detail.contains("case .agent:\n          AgentPanel(model: model)"))
  }

  func testDeepLinkAnchorScrollsWithinItsPaneBelowThePinnedHeader() throws {
    let detail = try settingsLayoutSource("SettingsView.swift")
    let reader = try XCTUnwrap(detail.range(of: "ScrollViewReader { proxy in"))
    let scrollTo = try XCTUnwrap(detail.range(of: "proxy.scrollTo(anchor, anchor: .top)"))
    XCTAssertLessThan(reader.lowerBound, scrollTo.lowerBound)
    XCTAssertTrue(detail.contains("pendingScrollAnchor = target.anchor"))

    let pane = try settingsLayoutSource("SettingsTabbedPane.swift")
    let header = try XCTUnwrap(pane.range(of: "SettingsTabBar(model: model, section: section)"))
    let scroll = try XCTUnwrap(pane.range(of: "      ScrollView {"))
    let content = try XCTUnwrap(pane.range(of: "          content\n"))
    XCTAssertLessThan(header.lowerBound, scroll.lowerBound)
    XCTAssertLessThan(scroll.lowerBound, content.lowerBound)
  }

  func testShortcutsKeepsThePlainDetailScrollWithoutATabHeader() throws {
    let detail = try settingsLayoutSource("SettingsView.swift")
    let plainScroll = try XCTUnwrap(detail.range(of: "default:\n          ScrollView {"))
    let shortcuts = try XCTUnwrap(
      detail.range(of: "case .shortcuts:\n      ShortcutsPanel(model: model)"))
    XCTAssertTrue(detail[plainScroll.lowerBound...].contains("untabbedDetail"))
    XCTAssertLessThan(plainScroll.lowerBound, shortcuts.lowerBound)
    XCTAssertFalse(detail.contains("SettingsTabBar("))
  }

  private func settingsLayoutSource(_ name: String) throws -> String {
    let source = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .appendingPathComponent("Codescribe/Screens/Settings")
      .appendingPathComponent(name)
    return try String(contentsOf: source, encoding: .utf8)
  }
}

/// Serves one permission snapshot and records the writes the Tools tab makes.
@MainActor
private final class RecordingPermissionAdmin: MCPAdminEngine {
  private(set) var toolWrites: [(identity: String, level: String)] = []
  private(set) var toolClears: [String] = []
  private(set) var defaultWrites: [CsPermissionPolicy] = []
  private var policy = CsPermissionPolicy(
    defaultLevel: "ask", readOnlyDefault: "allow", sideEffectDefault: "ask", tools: [], servers: [])
  private let capabilities: [CsToolCapability]

  init(capabilities: [CsToolCapability]) { self.capabilities = capabilities }

  func listServers() throws -> [CsMcpServer] { [] }
  func addServer(_ input: CsMcpServerInput) throws {}
  func updateServer(name: String, input: CsMcpServerInput) throws {}
  func removeServer(name: String) throws {}
  func testServer(_ name: String) async -> CsMcpTestResult {
    CsMcpTestResult(
      ok: false, toolCount: 0, serverName: name, serverVersion: "", protocolVersion: "",
      error: "unused")
  }
  func getPermissionPolicy() -> CsPermissionPolicy { policy }
  func setPermissionDefaults(
    defaultLevel: String, readOnlyDefault: String, sideEffectDefault: String
  ) throws {
    policy = CsPermissionPolicy(
      defaultLevel: defaultLevel, readOnlyDefault: readOnlyDefault,
      sideEffectDefault: sideEffectDefault, tools: policy.tools, servers: policy.servers)
    defaultWrites.append(policy)
  }
  func setToolPermission(identity: String, level: String) throws {
    toolWrites.append((identity, level))
  }
  func clearToolPermission(identity: String) throws { toolClears.append(identity) }
  func listToolCapabilities() -> [CsToolCapability] { capabilities }
}

/// MCP admin double with a scripted add failure and a recorded update log.
@MainActor
private final class ScriptedMcpAdmin: MCPAdminEngine {
  struct StoreFailure: Error, CustomStringConvertible {
    let description: String
  }

  private var servers: [CsMcpServer]
  private let serverRules: [String]
  var addFailure: String?
  private(set) var updates: [String] = []

  init(servers: [CsMcpServer], addFailure: String? = nil, rules serverRules: [String] = []) {
    self.servers = servers
    self.addFailure = addFailure
    self.serverRules = serverRules
  }

  func listServers() throws -> [CsMcpServer] { servers }

  func addServer(_ input: CsMcpServerInput) throws {
    if let addFailure { throw StoreFailure(description: addFailure) }
    servers.append(
      CsMcpServer(
        name: input.name, command: input.command, args: input.args, envKeys: [],
        enabled: input.enabled, transport: input.endpoint.isEmpty ? "stdio" : "remote",
        endpoint: input.endpoint, authRef: input.authRef))
  }

  func updateServer(name: String, input: CsMcpServerInput) throws {
    updates.append(name)
    guard let index = servers.firstIndex(where: { $0.name == name }) else { return }
    servers[index] = CsMcpServer(
      name: input.name, command: input.command, args: input.args,
      envKeys: servers[index].envKeys, enabled: input.enabled,
      transport: input.endpoint.isEmpty ? "stdio" : "remote",
      endpoint: input.endpoint, authRef: input.authRef)
  }

  func removeServer(name: String) throws { servers.removeAll { $0.name == name } }

  func testServer(_ name: String) async -> CsMcpTestResult {
    CsMcpTestResult(
      ok: true, toolCount: 3, serverName: name, serverVersion: "1.0", protocolVersion: "",
      error: "")
  }

  func getPermissionPolicy() -> CsPermissionPolicy {
    CsPermissionPolicy(
      defaultLevel: "ask", readOnlyDefault: "allow", sideEffectDefault: "ask", tools: [],
      servers: serverRules)
  }
}
