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
      })
    )
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
      )
    )
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
      )
    )
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
    model = SettingsViewModel(engine: engine)
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
      )
    )
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
        )
      )
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
    let model = SettingsViewModel(engine: MockSettingsEngine())

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
  /// to the same storage level and names the same base file.
  func testPromptFilesKeepTheirStorageIdentity() {
    XCTAssertEqual(PromptFile.allCases.map(\.formattingLevel), [.correction, .smart, .max, nil])
    XCTAssertEqual(
      PromptFile.allCases.compactMap(\.formattingLevel),
      FormattingPolicyOption.editablePrompts
    )
    XCTAssertTrue(PromptFile.correction.editorSubtitle.hasSuffix("(formatting.txt)"))
    XCTAssertTrue(PromptFile.smart.editorSubtitle.hasSuffix("(formatting-smart.txt)"))
    XCTAssertTrue(PromptFile.max.editorSubtitle.hasSuffix("(formatting-max.txt)"))
    XCTAssertTrue(PromptFile.assistive.editorSubtitle.hasSuffix("(assistive.txt)"))
  }

  /// The Tools tab binds by key path now; each projection must read the
  /// registry snapshot and route its write to the same setter and kind the
  /// closure bindings used.
  func testToolPermissionPickersRouteThroughTheExistingSetters() async {
    let admin = RecordingPermissionAdmin(capabilities: [
      CsToolCapability(
        name: "search", identity: "loctree-mcp:search", origin: "mcp", server: "loctree-mcp",
        risk: "read_only", effective: "allow", requiresApprovalFlag: false)
    ])
    let model = SettingsViewModel(engine: MockSettingsEngine(), mcpAdmin: admin)
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
    let model = SettingsViewModel(agentStatus: MockAgentStatusEngine())
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
    let model = SettingsViewModel(engine: MockSettingsEngine())
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

  func testLegacyKeysAndAgentDeepLinksResolveToDedicatedPanels() {
    SettingsDeepLink.pendingSection = nil
    defer { SettingsDeepLink.pendingSection = nil }

    SettingsDeepLink.pendingSection = .keys
    XCTAssertEqual(SettingsDeepLink.consume()?.section.destination, .providers)
    XCTAssertNil(SettingsDeepLink.consume())

    XCTAssertEqual(SettingsDeepLink.agentConfigurationSection, .agent)
    SettingsDeepLink.pendingSection = SettingsDeepLink.agentConfigurationSection
    XCTAssertEqual(SettingsDeepLink.consume()?.section.destination, .agent)
    XCTAssertNil(SettingsDeepLink.consume())

    SettingsDeepLink.present(.audio, anchor: .audioReadiness)
    XCTAssertEqual(
      SettingsDeepLink.consume(),
      SettingsDeepLinkTarget(section: .audio, anchor: .audioReadiness)
    )
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
    let model = SettingsViewModel(engine: engine)
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
    let model = SettingsViewModel(engine: engine)
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
      }))

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
    let model = SettingsViewModel(engine: engine)

    model.applyPreviewTimingPreset(.smooth)

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
      ),
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
      healthState(stt: true, recording: true, keys: .available, agent: true),
      SettingsHealthState(level: .healthy, message: "systems ready", targetSection: nil)
    )
    XCTAssertEqual(
      healthState(stt: true, recording: true, keys: .missing, agent: false),
      SettingsHealthState(
        level: .degraded,
        message: "assistive lane: no key",
        targetSection: .keys
      )
    )
    XCTAssertEqual(
      healthState(stt: false, recording: true, keys: .available, agent: true),
      SettingsHealthState(
        level: .offline,
        message: "speech engine: unavailable",
        targetSection: .engine
      )
    )
    XCTAssertEqual(
      healthState(stt: true, recording: true, keys: .available, agent: false),
      SettingsHealthState(
        level: .offline,
        message: "assistive lane: not ready",
        targetSection: .engine
      )
    )
    XCTAssertEqual(
      healthState(stt: nil, recording: true, keys: .available, agent: true),
      SettingsHealthState(
        level: .unknown,
        message: "system health: unknown",
        targetSection: .engine
      )
    )
    XCTAssertEqual(
      healthState(stt: true, recording: false, keys: .available, agent: true),
      SettingsHealthState(
        level: .offline,
        message: "recording setup: action needed",
        targetSection: .audio
      )
    )
    XCTAssertEqual(
      healthState(stt: true, recording: nil, keys: .available, agent: true),
      SettingsHealthState(
        level: .unknown,
        message: "recording setup: checking",
        targetSection: .audio
      )
    )
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
    // The dictionary name derives from the SettingsSection title owner, so a
    // rail rename (e.g. Dictionary → Teacher) flows through automatically.
    XCTAssertEqual(
      LanguageIdentityPresentation.supportingCopy,
      "Programming vocabulary and your \(SettingsSection.voiceLab.title) entries enrich the selected language."
    )
    XCTAssertEqual(
      LanguageIdentityPresentation.supportingCopy,
      "Programming vocabulary and your Dictionary entries enrich the selected language."
    )
    XCTAssertFalse(LanguageIdentityPresentation.supportingCopy.contains("model weights"))
  }

  func testCreatorLanguageSelectionWritesStableRuntimeCodes() {
    var writes: [(key: String, value: String)] = []
    let engine = MockSettingsEngine(updateConfigObserver: { key, value in
      writes.append((key, value))
    })
    let model = SettingsViewModel(engine: engine)

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
      }))
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
      let model = SettingsViewModel(engine: MockSettingsEngine())
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
    let model = SettingsViewModel(engine: MockSettingsEngine())
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
    let model = SettingsViewModel(engine: engine)

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
    let model = SettingsViewModel(engine: engine)

    model.setTranscriptTagTemplate(
      "<codescribe warn=\"may contain misspelling\">{text}</codescribe>")

    XCTAssertEqual(writes.map(\.key), ["TRANSCRIPT_TAG_TEMPLATE"])
    XCTAssertEqual(
      writes.map(\.value),
      ["<codescribe warn=\"may contain misspelling\">{text}</codescribe>"]
    )
  }

  func testTranscriptTagTemplatePreviewWarnsAndAppendsWhenTextPlaceholderMissing() {
    let model = SettingsViewModel()

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
    let model = SettingsViewModel(engine: engine)

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
      engine: MockSettingsEngine(resetPreviewValue: preview)
    )

    model.refreshResetPreview()

    XCTAssertEqual(model.resetPreview.audioFiles, 5_000)
    XCTAssertEqual(
      model.resetImpactDescription(includeKeys: false, includePrompts: false),
      "Moves 5000 recordings from 42 days, 17 threads (512.0 MB) to Trash. "
        + "Your assistive.txt and three formatting prompt files will be preserved. "
        + "Codescribe will relaunch as a fresh install."
    )
    XCTAssertTrue(resetConfirmationMatches("RESET"))
    XCTAssertFalse(resetConfirmationMatches("reset"))
    XCTAssertFalse(resetConfirmationMatches(" RESET"))
  }

  func testPromptSourceLabelsExposeFileFallbackAndReadErrorTruth() {
    XCTAssertEqual(promptSourceLabel("custom_file"), "Custom file")
    XCTAssertEqual(promptSourceLabel("built_in_fallback"), "Built-in fallback")
    XCTAssertEqual(promptSourceLabel("read_error"), "Read error")
  }

  func testPromptRestoreTargetsOnlyTheConfirmedPrompt() {
    var restored: [String] = []
    let engine = MockSettingsEngine(
      promptRestoreObserver: { restored.append($0) }
    )
    let model = SettingsViewModel(engine: engine)

    XCTAssertNotNil(model.restoreFormattingPromptToDefault(.correction))
    XCTAssertNotNil(model.restoreFormattingPromptToDefault(.smart))
    XCTAssertNotNil(model.restoreFormattingPromptToDefault(.max))

    XCTAssertEqual(restored, ["correction", "smart", "max"])
  }

  func testFormattingPromptSnapshotsExposeDistinctPathsAndProvenance() throws {
    let model = SettingsViewModel(engine: MockSettingsEngine())
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
    let model = SettingsViewModel(engine: engine)

    XCTAssertNil(model.saveAssistivePrompt("replacement"))
    XCTAssertNotNil(model.lastError)
    XCTAssertEqual(model.assistivePromptSnapshot().content, CsSettings.sampleAssistivePrompt)
  }

  func testAppResetPreservesPromptsUnlessSeparateOptInIsEnabled() {
    var calls: [(keys: Bool, prompts: Bool)] = []
    let engine = MockSettingsEngine(
      resetAppDataObserver: { calls.append(($0, $1)) }
    )
    let model = SettingsViewModel(engine: engine)

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
    let model = SettingsViewModel(engine: engine)

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

  func testClearMcpConfigurationUsesDedicatedEngineContract() {
    var calls = 0
    let model = SettingsViewModel(
      engine: MockSettingsEngine(
        clearMcpConfigurationObserver: { calls += 1 }
      )
    )

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
        updateConfigObserver: { key, value in writes.append((key, value)) })
    )
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

    let model = SettingsViewModel(engine: engine)
    XCTAssertEqual(model.deferredInsertShortcut, .commandShiftV)
    XCTAssertTrue(writes.isEmpty, "construction must only read persisted truth")

    model.refresh()
    XCTAssertEqual(model.deferredInsertShortcut, .commandShiftV)
    XCTAssertTrue(writes.isEmpty, "passive refresh must not write the picker value back")
  }

  /// Active STT consumes last serving verdict; Apple→Whisper fallback must not
  /// display configured Apple preference.
  func testActiveSTTUsesServingVerdictNotConfiguredEngine() {
    let model = SettingsViewModel(engine: MockSettingsEngine())
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
    XCTAssertFalse(model.activeSTT.contains("Not yet served"))
  }

  func testActiveSTTRefreshesFromServingProviderOnDemand() {
    var snapshot: LastServingVerdict? = nil
    let model = SettingsViewModel(
      engine: MockSettingsEngine(),
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

  func testAsrModePickerPersistsPromotedKeysAndRequiresCloudConsent() {
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
    let model = SettingsViewModel(engine: engine)
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

    model.setSttLaneEndpoint("live", "wss://asr.example/v1/audio/transcribe")
    XCTAssertEqual(writes.last?.0, "STT_LIVE_ENDPOINT")
    XCTAssertEqual(model.sttLanes.last?.endpoint, "wss://asr.example/v1/audio/transcribe")
    model.setSttLaneEndpoint("file", "https://asr.example/v1/audio/transcriptions")
    XCTAssertEqual(writes.last?.0, "STT_FILE_ENDPOINT")
    XCTAssertEqual(model.sttLanes.first?.endpoint, "https://asr.example/v1/audio/transcriptions")
    model.setAsrGatewayUrl("https://gateway.example/session")
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
      )
    )
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
    XCTAssertTrue(pane[..<scroll.lowerBound].contains(".background(CSColor.windowWash)"))
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
  func listToolCapabilities() -> [CsToolCapability] { capabilities }
}
