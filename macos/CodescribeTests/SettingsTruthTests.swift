import AppKit
import SwiftUI
import XCTest
@testable import Codescribe

@MainActor
final class SettingsTruthTests: XCTestCase {
    /// The rail is a native `List(.sidebar)` now: selection fill, focus ring and
    /// keyboard navigation belong to AppKit, so the app owns only the CONTENT —
    /// which group a section sits in, its symbol, and what the search matches.
    func testEverySectionDeclaresItsGroupAndASymbolForTheNativeSidebar() {
        let visible = SettingsSection.allCases.filter { $0.availability != .hidden }
        XCTAssertEqual(visible.count, 10)

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

    /// Pagination must not lose a subsystem or strand a page: every page belongs
    /// to a real section, selecting a section lands on its first page, and a
    /// section without pages keeps rendering whole.
    func testPaginatedSectionsRouteToPagesWithoutLosingSubsystems() {
        let model = SettingsViewModel(engine: MockSettingsEngine())

        XCTAssertEqual(
            SettingsPage.pages(in: .agent),
            [.agentLanes, .agentWorkspace, .agentStatus, .agentTools, .agentMcp],
            "the Agent panel's five subsystems each need their own page"
        )
        for page in SettingsPage.allCases {
            XCTAssertFalse(page.title.isEmpty)
            XCTAssertFalse(page.symbol.isEmpty)
            XCTAssertTrue(SettingsPage.pages(in: page.section).contains(page))
        }

        // Landing on a paginated section opens its first page.
        model.select(SettingsSection.agent)
        XCTAssertEqual(model.page, .agentLanes)
        XCTAssertEqual(model.route, .page(.agentLanes))

        // Selecting a page keeps the parent section consistent.
        model.select(SettingsPage.agentMcp)
        XCTAssertEqual(model.section, .agent)
        XCTAssertEqual(model.route, .page(.agentMcp))

        // Leaving for an unpaginated section clears the page.
        model.select(SettingsSection.audio)
        XCTAssertNil(model.page)
        XCTAssertEqual(model.route, .section(.audio))

        // Route selection round-trips through the rail's binding type.
        model.select(SettingsRoute.page(.agentTools))
        XCTAssertEqual(model.route, .page(.agentTools))
        model.select(SettingsRoute.section(.license))
        XCTAssertEqual(model.route, .section(.license))
    }

    func testSettingsSearchReachesInsideLongSections() {
        // A page keyword surfaces its parent section…
        XCTAssertTrue(SettingsPage.matching(query: "mcp").contains(.agentMcp))
        XCTAssertEqual(SettingsPage.matching(query: "mcp").first?.section, .agent)
        // …even though the section's own title and keywords do not mention it.
        XCTAssertFalse(SettingsSection.agent.title.lowercased().contains("mcp"))

        XCTAssertTrue(SettingsPage.matching(query: "permission").contains(.agentTools))
        XCTAssertTrue(SettingsPage.matching(query: "roots").contains(.agentWorkspace))
        XCTAssertTrue(SettingsPage.matching(query: "zzzz").isEmpty)
        XCTAssertEqual(SettingsPage.matching(query: "  ").count, SettingsPage.allCases.count)
    }

    func testSectionAvailabilityKeepsPromisesHonest() {
        for section in [
            SettingsSection.creator, .shortcuts, .keys, .agent, .prompts, .engine, .audio, .voiceLab, .license, .user,
        ] {
            XCTAssertEqual(section.availability, .available)
            XCTAssertTrue(section.isInteractive)
        }
    }

    /// The full route map: stable id, one visible title owner, and the explicit
    /// panel destination SettingsView's detail switch consumes. All ten rail
    /// sections, including engine→Dictation, voiceLab→Dictionary,
    /// keys→Providers, and the dedicated Agent destination.
    func testSettingsSectionRoutesTitlesAndDestinationsOwnTheRail() {
        let expectations: [(SettingsSection, String, String, SettingsPanelDestination)] = [
            (.creator, "creator", "Creator", .creator),
            (.shortcuts, "shortcuts", "Hotkeys", .shortcuts),
            (.keys, "keys", "Providers", .providers),
            (.agent, "agent", "Agent", .agent),
            (.prompts, "prompts", "Prompts", .prompts),
            (.engine, "engine", "Dictation", .dictation),
            (.audio, "audio", "Audio", .audio),
            (.voiceLab, "voiceLab", "Dictionary", .dictionary),
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
        XCTAssertEqual(KeysPanel.ownedCapabilities, [.apiKeys])
        XCTAssertEqual(
            AgentPanel.ownedCapabilities,
            [.llmLanes, .workspaceRoots, .agentStatus, .mcpServers, .toolPermissions]
        )
        XCTAssertTrue(KeysPanel.ownedCapabilities.isDisjoint(with: AgentPanel.ownedCapabilities))
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
        XCTAssertEqual(SettingsDeepLink.consume()?.destination, .providers)
        XCTAssertNil(SettingsDeepLink.consume())

        XCTAssertEqual(SettingsDeepLink.agentConfigurationSection, .agent)
        SettingsDeepLink.pendingSection = SettingsDeepLink.agentConfigurationSection
        XCTAssertEqual(SettingsDeepLink.consume()?.destination, .agent)
        XCTAssertNil(SettingsDeepLink.consume())
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
        let keychainSnapshot = model.keyAccounts.map {
            "\($0):\(model.keyStatus.isSet(account: $0))"
        }

        model.select(.keys)
        _ = KeysPanel(model: model)
        model.select(.agent)
        _ = AgentPanel(model: model)

        XCTAssertTrue(configWrites.isEmpty, "the IA split must not write settings.json")
        XCTAssertEqual(
            model.keyAccounts.map { "\($0):\(model.keyStatus.isSet(account: $0))" },
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
        XCTAssertTrue(batchWrites.allSatisfy { $0.map(\.key) == ["HOLD_INDICATOR", "HOLD_BADGE_SIZE"] })
        XCTAssertEqual(batchWrites.compactMap { $0.last?.value }, ["4", "8", "12"])
    }

    /// Dictation owns every transcription-behavior write and each control keeps
    /// its exact promoted key/value contract after the IA move.
    func testDictationControlsWriteExactPromotedKeysAndValues() {
        var writes: [(key: String, value: String)] = []
        let model = SettingsViewModel(engine: MockSettingsEngine { key, value in
            writes.append((key, value))
        })

        model.setSttEngine("whisper")
        model.setLayeredTranscription(true)
        model.setLayeredTranscription(false)
        model.setToggleSilenceSeconds(3.5)
        model.setPreviewBufferDelayMs(1038)
        model.setPreviewTypingCps(10.6)
        model.setPreviewEmitWordsMax(5)
        model.setPreviewInterimSeconds(8.0)

        XCTAssertEqual(writes.map(\.key), [
            "CODESCRIBE_STT_ENGINE",
            "CODESCRIBE_LAYERED_TRANSCRIPTION",
            "CODESCRIBE_LAYERED_TRANSCRIPTION",
            "TOGGLE_SILENCE_SEC",
            "CODESCRIBE_BUFFER_DELAY_MS",
            "CODESCRIBE_TYPING_CPS",
            "CODESCRIBE_EMIT_WORDS_MAX",
            "CODESCRIBE_BUFFERED_INTERIM_SEC",
        ])
        XCTAssertEqual(writes.map(\.value), [
            "whisper", "phase1", "off", "3.5", "1038", "10.6", "5", "8.0",
        ])
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

    /// Agent owns the one lane-edit grammar. Every lane preserves its exact
    /// endpoint/model keys, and whitespace/empty input keeps the reset semantics
    /// (an empty write clears the JSON override).
    func testAgentLaneEditorsPreserveExactKeysAndEmptyResetSemantics() {
        var writes: [(key: String, value: String)] = []
        let model = SettingsViewModel(
            engine: MockSettingsEngine { key, value in
                writes.append((key, value))
            },
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

        let lanes: [(lane: LLMLane, endpointKey: String, modelKey: String)] = [
            (.assistive, "LLM_ASSISTIVE_ENDPOINT", "LLM_ASSISTIVE_MODEL"),
            (.formatting, "LLM_FORMATTING_ENDPOINT", "LLM_FORMATTING_MODEL"),
            (.main, "LLM_ENDPOINT", "LLM_MODEL"),
        ]

        for expectation in lanes {
            XCTAssertEqual(expectation.lane.endpointKey, expectation.endpointKey)
            XCTAssertEqual(expectation.lane.modelKey, expectation.modelKey)

            writes.removeAll()
            model.setLLMEndpoint(" https://example.test/v1 ", for: expectation.lane)
            model.setLLMModel("model-x", for: expectation.lane)
            model.setLLMEndpoint("   ", for: expectation.lane)
            model.setLLMModel("", for: expectation.lane)

            XCTAssertEqual(writes.map(\.key), [
                expectation.endpointKey,
                expectation.modelKey,
                expectation.endpointKey,
                expectation.modelKey,
            ])
            XCTAssertEqual(
                writes.map(\.value),
                ["https://example.test/v1", "model-x", "", ""]
            )
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
            XCTAssertEqual(model.section, section, "\(name) preview route drifted", file: file, line: line)
            XCTAssertNotEqual(
                ObjectIdentifier(Panel.self),
                ObjectIdentifier(EmptyView.self),
                "\(name) route resolved to an empty panel",
                file: file,
                line: line
            )
        }

        let dictation = SettingsViewModel.preview(.engine)
        assertConcretePanel(EnginePanel(model: dictation), model: dictation, section: .engine, name: "dictation")

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
        assertConcretePanel(KeysPanel(model: providers), model: providers, section: .keys, name: "providers")

        let agent = SettingsViewModel.preview(.agent)
        assertConcretePanel(AgentPanel(model: agent), model: agent, section: .agent, name: "agent")
        let previewLane = agent.llmLane(.assistive)
        XCTAssertEqual(previewLane.providerId, "openai-responses")
        XCTAssertEqual(previewLane.resolvedEndpoint, "https://api.openai.com/v1/responses")
    }

    func testHealthStateMatrix() {
        XCTAssertEqual(
            healthState(stt: true, keys: .available, agent: true),
            SettingsHealthState(level: .healthy, message: "systems ready", targetSection: nil)
        )
        XCTAssertEqual(
            healthState(stt: true, keys: .missing, agent: false),
            SettingsHealthState(
                level: .degraded,
                message: "assistive lane: no key",
                targetSection: .keys
            )
        )
        XCTAssertEqual(
            healthState(stt: false, keys: .available, agent: true),
            SettingsHealthState(
                level: .offline,
                message: "speech engine: unavailable",
                targetSection: .engine
            )
        )
        XCTAssertEqual(
            healthState(stt: true, keys: .available, agent: false),
            SettingsHealthState(
                level: .offline,
                message: "assistive lane: not ready",
                targetSection: .engine
            )
        )
        XCTAssertEqual(
            healthState(stt: nil, keys: .available, agent: true),
            SettingsHealthState(
                level: .unknown,
                message: "system health: unknown",
                targetSection: .engine
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
        let engine = MockSettingsEngine { key, value in
            writes.append((key, value))
        }
        let model = SettingsViewModel(engine: engine)

        model.setLanguage(.auto)
        model.setLanguage(.polish)
        model.setLanguage(.english)

        XCTAssertEqual(writes.map(\.key), [
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
        let model = SettingsViewModel(engine: MockSettingsEngine { key, value in
            writes.append((key, value))
        })
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
            let hostingView = NSHostingView(rootView: CreatorPanel(model: model).frame(
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
        let hostingView = NSHostingView(rootView: PromptPanel(model: model).frame(
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
        let engine = MockSettingsEngine { key, value in
            writes.append((key, value))
        }
        let model = SettingsViewModel(engine: engine)

        model.setTranscriptTaggingEnabled(true)
        model.setTranscriptTaggingEnabled(false)

        XCTAssertEqual(writes.map(\.key), [
            "TRANSCRIPT_TAGGING_ENABLED", "TRANSCRIPT_TAGGING_ENABLED",
        ])
        XCTAssertEqual(writes.map(\.value), ["1", "0"])
    }

    func testTranscriptTagTemplateWritesPromotedConfigKeyAndAllowsStaticAttributes() {
        var writes: [(key: String, value: String)] = []
        let engine = MockSettingsEngine { key, value in
            writes.append((key, value))
        }
        let model = SettingsViewModel(engine: engine)

        model.setTranscriptTagTemplate("<codescribe warn=\"may contain misspelling\">{text}</codescribe>")

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
        let engine = MockSettingsEngine { key, value in
            writes.append((key, value))
        }
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
            engine: MockSettingsEngine { key, value in writes.append((key, value)) }
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

    /// The picker's view-model state starts at the product default (Disabled —
    /// the chord is opt-in) and tracks every selection, so the segmented
    /// control reflects what was just persisted rather than snapping back.
    func testDeferredInsertPickerStateTracksSelection() {
        let model = SettingsViewModel(engine: MockSettingsEngine())

        XCTAssertEqual(model.deferredInsertShortcut, .disabled)
        model.setDeferredInsertShortcut(.commandShiftV)
        XCTAssertEqual(model.deferredInsertShortcut, .commandShiftV)
        model.setDeferredInsertShortcut(.disabled)
        XCTAssertEqual(model.deferredInsertShortcut, .disabled)
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
        XCTAssertEqual(
            formatActiveSTT(lastServing: fallback),
            "Whisper (fallback) · Smart final pass · changed"
        )

        model.lastServingVerdict = LastServingVerdict(
            engine: "local_apple",
            routingMode: "smart",
            disposition: "unchanged",
            fallbackUsed: false
        )
        XCTAssertEqual(
            model.activeSTT,
            "Apple on-device · Smart final pass · unchanged"
        )
    }
}
