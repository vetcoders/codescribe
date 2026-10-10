import AppKit
import XCTest

@testable import Codescribe

/// Source contract for the Settings appearance cut. The compiler is embargoed
/// for this worker, so these checks lock the structure a later build will compile:
/// semantic ink, one grouped inset, native navigation, and visible keyboard focus.
final class SettingsChromeContractTests: XCTestCase {
  func testSettingsDropsFixedDarkPaint() throws {
    let joined = try joinedSettingsSources()
    let banned = [
      "preferredColorScheme",
      "windowWash",
      ".csSettingsCard(",
      "CSColor.surfaceRaised",
      "CSColor.hairline",
      "CSColor.textHigh",
      "CSColor.textBody",
      "CSColor.textBodyAlt",
      "CSColor.textMuted",
      "CSColor.textMutedAlt",
      "CSColor.textFaint",
      "CSColor.textFaintAlt",
      "CSColor.terracottaLight",
      "CSColor.dangerLight",
      "cornerRadius: 10",
      "cornerRadius: 11",
      "focusEffectDisabled",
    ]
    for token in banned {
      XCTAssertFalse(joined.contains(token), "Settings still contains \(token)")
    }
  }

  func testSettingsKeepsNativeNavigationAndBrandSemantics() throws {
    let sources = try settingsSources()
    let joined = sources.values.joined(separator: "\n")
    let view = try XCTUnwrap(sources["SettingsView.swift"])
    XCTAssertTrue(view.contains("NavigationSplitView(columnVisibility:"))
    XCTAssertTrue(view.contains(".listStyle(.sidebar)"))
    XCTAssertTrue(view.contains(".searchable("))
    XCTAssertTrue(view.contains(".csFocusPolicy()"))
    XCTAssertTrue(view.contains(".controlSize(.regular)"))
    XCTAssertTrue(view.contains("func settingsGroupedInset("))
    XCTAssertTrue(view.contains("struct SettingsPageHeader"))
    XCTAssertFalse(view.contains("focusEffectDisabled"))

    let tabBar = try XCTUnwrap(sources["SettingsTabBar.swift"])
    XCTAssertTrue(tabBar.contains("NSSegmentedControl()"))
    XCTAssertTrue(tabBar.contains("control.segmentDistribution = .fit"))
    XCTAssertTrue(tabBar.contains("ScrollView(.horizontal, showsIndicators: false) { bar }"))
    XCTAssertTrue(tabBar.contains(".controlSize(.regular)"))
    // A bar pinned to its natural width widens the pane past the window.
    XCTAssertFalse(tabBar.contains(".fixedSize()"))
    XCTAssertFalse(tabBar.contains(".pickerStyle(.segmented)"))

    let pane = try XCTUnwrap(sources["SettingsTabbedPane.swift"])
    let scroll = try XCTUnwrap(pane.range(of: "ScrollView {"))
    let tabs = try XCTUnwrap(pane.range(of: "SettingsTabBar(model: model, section: section)"))
    XCTAssertLessThan(tabs.lowerBound, scroll.lowerBound)
    XCTAssertTrue(pane[scroll.lowerBound...].contains(".id(model.currentTab)"))
    // The per-tab identity swap must not crossfade: without an identity
    // transition the outgoing and incoming tabs paint over each other.
    let transition = try XCTUnwrap(pane.range(of: ".transition(.identity)"))
    let identity = try XCTUnwrap(pane.range(of: ".id(model.currentTab)"))
    XCTAssertLessThan(scroll.lowerBound, transition.lowerBound)
    XCTAssertLessThan(transition.lowerBound, identity.lowerBound)

    for token in [
      "CSColor.terracotta",
      "CSColor.oliveLight",
      "CSColor.danger",
      "CSColor.chromeAccent",
      "CSColor.amber",
    ] {
      XCTAssertTrue(containsToken(joined, token), "missing brand or status token \(token)")
    }
    XCTAssertTrue(containsToken(joined, "CSColor.olive"))
    XCTAssertGreaterThan(joined.components(separatedBy: "SettingsPageHeader(").count, 2)
    XCTAssertGreaterThan(joined.components(separatedBy: ".settingsGroupedInset(").count, 2)
  }

  /// The six tabs of a tabbed section fit the narrowest detail column, in
  /// English and in Polish. A language that breaks this still gets the
  /// scrolling bar, but the label is then too long and should be shortened
  /// before it ships.
  @MainActor
  func testTabBarsFitTheMinimumWindowInEnglishAndPolish() throws {
    let sources = try settingsSources()
    let view = try XCTUnwrap(sources["SettingsView.swift"])
    XCTAssertTrue(view.contains(".navigationSplitViewColumnWidth(min: 196, ideal: 216, max: 300)"))
    // An 880 pt window with the sidebar at its ideal width.
    XCTAssertEqual(SettingsView.detailMinWidth, 880 - 216)
    let pane = try XCTUnwrap(sources["SettingsTabbedPane.swift"])
    XCTAssertTrue(pane.contains(".padding(.horizontal, CSSpace.xl)"))
    // Detail column minus the bar's horizontal padding and the column divider.
    let column: CGFloat = SettingsView.detailMinWidth - 1 - 2 * CSSpace.xl

    let polish = try polishCatalog()
    let tabbed = SettingsSection.allCases.filter { !SettingsTab.tabs(in: $0).isEmpty }
    XCTAssertEqual(tabbed.count, 2)
    for section in tabbed {
      let english = SettingsTab.tabs(in: section).map(\.title)
      // Agent keeps six tabs; Dictation has four since the raw recognition
      // timings moved to Lab and the duplicated Permissions tab was cut
      // (round 14 — the Creator checklist is the one permission surface).
      XCTAssertEqual(english.count, section == .engine ? 4 : 6, "\(section.rawValue)")
      // Brand names ("MCP", "Whisper") are not catalog keys and read the same.
      let translated = english.map { polish[$0] ?? $0 }
      XCTAssertNotEqual(translated, english, "\(section.rawValue): no Polish labels resolved")
      for (language, titles) in [("en", english), ("pl", translated)] {
        let width = SettingsTabSegments.control(titles: titles).fittingSize.width
        XCTAssertGreaterThan(width, 0)
        XCTAssertLessThanOrEqual(
          width, column, "\(section.rawValue) tabs in \(language): \(titles)")
      }
    }
  }

  /// The window minimum sits on the detail column. A minimum width on the
  /// split view itself makes the opening sidebar stop halfway and jump
  /// (measured on macOS 27 below a 1096 pt window).
  func testWindowMinimumIsCarriedByTheDetailColumn() throws {
    let view = try XCTUnwrap(settingsSources()["SettingsView.swift"])
    let detail = try XCTUnwrap(view.range(of: "} detail: {"))
    let minimum = try XCTUnwrap(view.range(of: ".frame(minWidth: Self.detailMinWidth)"))
    let toolbar = try XCTUnwrap(view.range(of: ".navigationTitle("))
    XCTAssertLessThan(detail.lowerBound, minimum.lowerBound)
    XCTAssertLessThan(minimum.lowerBound, toolbar.lowerBound)
    XCTAssertTrue(
      view.contains(".frame(maxWidth: .infinity, minHeight: 620, maxHeight: .infinity)"))
    XCTAssertFalse(view.contains("minWidth: 880"))
  }

  /// A segmented control cannot shrink below its labels, and the Settings
  /// window follows the content minimum. `Allow · Ask · Deny` pinned to 180 pt
  /// spilled over its card in Polish (255 pt) and, three abreast, forced the
  /// window wider than the screen whenever Agent › Tools opened (Founder,
  /// 2026-10-07). Tool-permission pickers sit at their own width, one default
  /// per row; every picker that keeps a fixed frame is measured against its
  /// Polish labels here. A SwiftUI segmented picker gives every segment the
  /// width of its widest label, so the measurement distributes segments
  /// equally: proportional sizing passed `Off · Correction · Smart · Max` at
  /// 330 pt while the real control spilled `Max` past the card in both
  /// languages (Founder, 2026-10-08).
  @MainActor
  func testSegmentedPickersFitTheirFramesInEnglishAndPolish() throws {
    let sources = try settingsSources()
    let polish = try polishCatalog()
    func width(_ titles: [String]) -> CGFloat {
      let control = SettingsTabSegments.control(titles: titles)
      control.segmentDistribution = .fillEqually
      return control.fittingSize.width
    }

    let tools = try XCTUnwrap(sources["ToolPermissionsSection.swift"])
    XCTAssertEqual(tools.components(separatedBy: ".pickerStyle(.segmented)").count, 3)
    // Round 12: the defaults rows keep `.fixedSize()`; the card picker sits on
    // its own full-width row (`.frame(maxWidth: .infinity)` only widens), so
    // it never competes with the tool name for space.
    XCTAssertEqual(tools.components(separatedBy: ".fixedSize()").count, 2)
    XCTAssertTrue(tools.contains(".pickerStyle(.segmented)\n      .frame(maxWidth: .infinity)"))
    XCTAssertFalse(tools.contains(".frame(width: 180)"))
    XCTAssertFalse(tools.contains(".frame(maxWidth: 180)"))
    XCTAssertEqual(tools.components(separatedBy: "defaultRow(title: \"").count, 4)
    let levels = ["Allow", "Ask", "Deny"]
    let polishLevels = try levels.map { try XCTUnwrap(polish[$0]) }
    XCTAssertEqual(polishLevels, ["Zezwalaj", "Pytaj", "Blokuj"])
    // The card column at the minimum window: one column since round 12 (the
    // 190 pt source sidebar became a popup), minus the pane padding and the
    // card's own 14 pt sides. The picker owns its row, so it only has to fit.
    let browser = try XCTUnwrap(sources["ToolOverridesBrowser.swift"])
    XCTAssertFalse(browser.contains(".frame(width: 190)"), "the source sidebar is gone")
    let column = SettingsView.detailMinWidth - 2 * CSSpace.xl - 2 * 14
    for titles in [levels, polishLevels] {
      let picker = width(titles)
      XCTAssertGreaterThan(picker, 0)
      XCTAssertLessThanOrEqual(picker, column, "\(titles)")
    }

    // The formatting level picker sizes to its labels; no frame to outgrow.
    let creator = try XCTUnwrap(sources["CreatorPanel.swift"])
    let formatting = try XCTUnwrap(
      creator.range(of: "Picker(\"\", selection: formattingLevelBinding)"))
    let formattingTail = String(creator[formatting.upperBound...].prefix(400))
    XCTAssertTrue(formattingTail.contains(".fixedSize()"))
    XCTAssertNil(fixedFrameWidth(in: formattingTail))

    // Pickers that keep a fixed frame hold their Polish labels.
    let fixed: [(file: String, picker: String, titles: [String?])] = [
      ("CreatorPanel.swift", "Picker(\"\", selection: selection)", ["Polski", "English"]),
      ("ShortcutsPanel.swift", "Picker(\"Arm modifier\"", ["Shift", "Command"]),
      (
        "ShortcutsPanel.swift", "Picker(\"Pointer indicator\"",
        [polish["settings.holdBadge.size.off"], "4px", "8px", "12px"]
      ),
      ("ShortcutsPanel.swift", "Picker(\"Agent channel modifier\"", ["Ctrl", "Fn"]),
    ]
    for entry in fixed {
      let source = try XCTUnwrap(sources[entry.file])
      let start = try XCTUnwrap(source.range(of: entry.picker), entry.picker)
      let tail = String(source[start.upperBound...].prefix(600))
      let frame = try XCTUnwrap(fixedFrameWidth(in: tail), "\(entry.picker): no fixed frame")
      let titles = try entry.titles.map { try XCTUnwrap($0, entry.picker) }
      XCTAssertLessThanOrEqual(width(titles), frame, "\(entry.picker): \(titles)")
    }
  }

  /// The sidebar footer is one sentence-case line per state, and empty while
  /// the state is undetermined. The healthy line must stay on one line in the
  /// narrowest sidebar, in English and Polish.
  @MainActor
  func testHealthFooterReadsAsOneSentenceCaseLine() throws {
    let states = [
      healthState(stt: true, recording: true, keys: .available, agent: true, formatting: true),
      healthState(stt: false, recording: true, keys: .available, agent: true, formatting: true),
      healthState(stt: true, recording: false, keys: .available, agent: true, formatting: true),
      healthState(stt: true, recording: true, keys: .missing, agent: false, formatting: true),
      healthState(stt: true, recording: true, keys: .available, agent: false, formatting: true),
      healthState(stt: true, recording: true, keys: .available, agent: true, formatting: false),
      healthState(stt: true, recording: nil, keys: .available, agent: true, formatting: true),
    ]
    let messages = try states.map { try XCTUnwrap($0.message) }
    XCTAssertEqual(Set(messages).count, states.count)

    let undetermined = healthState(
      stt: nil, recording: true, keys: .available, agent: true, formatting: true)
    XCTAssertNil(undetermined.message)
    XCTAssertNil(undetermined.targetSection)

    let polish = try polishCatalog()
    for message in messages {
      let translated = try XCTUnwrap(polish[message], "no Polish row: \(message)")
      for line in [message, translated] {
        XCTAssertEqual(line.first?.isUppercase, true, line)
        XCTAssertFalse(line.contains("·"), line)
      }
    }

    let view = try XCTUnwrap(settingsSources()["SettingsView.swift"])
    XCTAssertTrue(view.contains("Circle().fill(health.level.color).frame(width: 6, height: 6)"))
    XCTAssertTrue(view.contains("if let message = health.message {"))
    XCTAssertTrue(view.contains("Text(message)\n        .font(CSFont.mono(10, .medium))"))
    // Narrowest sidebar minus the footer padding, the status dot and its gap.
    let slot: CGFloat = 196 - 2 * 16 - 6 - 8
    XCTAssertEqual(states.first?.level, .healthy)
    let healthy = try XCTUnwrap(messages.first)
    for line in [healthy, try XCTUnwrap(polish[healthy])] {
      let width = (line as NSString).size(withAttributes: [.font: CSFont.nsMono(10)]).width
      XCTAssertLessThanOrEqual(width, slot, line)
    }
  }

  /// Providers shows what a user acts on and nothing that explains the
  /// architecture: Keychain account names, factory endpoints, wire keys and
  /// the OAuth client-id override sit under one `Advanced` disclosure per
  /// card; a key row is one line with `Change` / `Add`; the account row has
  /// one action; URL rows carry no pre-emptive help text. Every first-level
  /// string has a Polish row.
  func testProvidersPanelKeepsTheFirstLevelPlain() throws {
    let sources = try settingsSources()
    let panel = try XCTUnwrap(sources["ProvidersPanel.swift"])
    let rows = try XCTUnwrap(sources["KeyRows.swift"])

    XCTAssertEqual(panel.components(separatedBy: "DisclosureGroup(\"Advanced\")").count, 3)
    XCTAssertTrue(panel.contains("String(localized: \"Providers\")"))
    XCTAssertFalse(panel.contains("\"Providers.\""))
    XCTAssertFalse(panel.contains("factory endpoint"))
    XCTAssertFalse(panel.contains("SettingsSectionLabel(String(localized: \"Vendors\"))"))
    XCTAssertFalse(panel.contains("help:"), "URL rows do not warn ahead of a rejected save")
    XCTAssertFalse(panel.contains("Text(lane.title)"), "lane titles are named by id")
    // Refresh feedback reserves its line beside the button; it is never
    // inserted above it, which reflowed the cards for one frame.
    XCTAssertFalse(
      panel.contains("if model.providerAccessPending || model.providerMutationPending {"),
      "status spinner must not be conditionally inserted")
    XCTAssertTrue(panel.contains("ProviderAccessStatusSlot("))
    // The wire line is a developer-build fact, and only under Advanced. The
    // bridge sends the transport arguments; the sentence is written here.
    let accepts = try XCTUnwrap(panel.range(of: "Text(Self.accepts(for: lane))"))
    let gate = try XCTUnwrap(panel.range(of: "if DeveloperSurface.isEnabled() {"))
    XCTAssertLessThan(gate.lowerBound, accepts.lowerBound)
    XCTAssertLessThan(
      accepts.lowerBound.utf16Offset(in: panel) - gate.lowerBound.utf16Offset(in: panel), 80)
    XCTAssertFalse(panel.contains("Text(lane.accepts)"), "bridge prose never reaches the UI")
    // The validator admits plain http/ws on loopback, so the frame says so.
    XCTAssertTrue(panel.contains("localized: \"Live WebSocket connection (ws(s); \\(lane.accepts))\""))
    XCTAssertTrue(panel.contains("localized: \"HTTP(S): \\(lane.accepts)\""))
    XCTAssertFalse(panel.contains("\"HTTPS: "))

    // The example in the name field is copy, not a sample value: `e.g.` has to
    // reach the catalog (PL-041).
    XCTAssertTrue(panel.contains("String(\n      localized: \"e.g. Libraxis\","))
    XCTAssertFalse(panel.contains("namePlaceholder = \"e.g. Libraxis\""))

    // Key row: label and state on one line, the editor behind the chip, no account name.
    let keyRow = try XCTUnwrap(rows.range(of: "struct KeyRow: View"))
    let keyRowEnd = try XCTUnwrap(rows.range(of: "extension KeyRow {"))
    let keyRowSource = rows[keyRow.lowerBound..<keyRowEnd.lowerBound]
    XCTAssertFalse(keyRowSource.contains("Text(account)"))
    XCTAssertTrue(keyRowSource.contains("isSet ? \"Change\" : \"Add\""))
    XCTAssertTrue(
      keyRowSource.contains("isSet ? \"Set\" : (optional ? \"Optional\" : \"Not set\")"))
    let chip = try XCTUnwrap(keyRowSource.range(of: "editing.toggle()"))
    let field = try XCTUnwrap(keyRowSource.range(of: "if editing {"))
    XCTAssertLessThan(chip.lowerBound, field.lowerBound)

    // Account row: Sign out XOR Sign in.
    let account = try XCTUnwrap(rows.range(of: "struct AccountLoginRow: View"))
    let accountSource = rows[account.lowerBound...]
    XCTAssertTrue(
      accountSource.contains("if signedIn {\n        SettingsChipButton(\n          \"Sign out\""))
    XCTAssertTrue(
      accountSource.contains(
        "} else {\n        SettingsChipButton(\n          enabled: provider.accountLoginEnabled"))
    XCTAssertFalse(accountSource.contains("Advanced · OAuth client id…"))

    let polish = try polishCatalog()
    for key in [
      "Providers",
      "Connect accounts or add API keys. Models are chosen under Agent › AI models.",
      "Refresh status",
      "Add provider",
      "Cloud transcription",
      "Connections for cloud transcription. The mode is chosen under Dictation.",
      "Recordings leave this computer only in Cloud mode, or when you start a cloud re-transcription yourself.",
      "File transcription",
      "Live transcription",
      "Endpoint",
      "API key",
      "Gateway session URL",
      "Optional. Used for live transcription.",
      "Keys are stored securely in the macOS Keychain.",
      "Advanced",
      "Keychain account",
      "Settings keys",
      "OAuth client id…",
      "Set", "Not set", "Optional", "Change", "Add",
      "%@ account", "Connected", "Connected as %@", "Not connected", "Sign out", "Sign in with %@",
      "This address needs http:// or https://.",
      "This address needs ws:// or wss://.",
    ] {
      let translated = try XCTUnwrap(polish[key], "no Polish row: \(key)")
      XCTAssertEqual(translated.first?.isUppercase, true, "\(key) → \(translated)")
    }
    XCTAssertEqual(polish["Providers"], "Dostawcy")
    XCTAssertEqual(polish["Cloud transcription"], "Transkrypcje w chmurze")
    XCTAssertEqual(polish["File transcription"], "Transkrypcja plików")
    XCTAssertEqual(polish["Live transcription"], "Transkrypcja na żywo")
    XCTAssertEqual(polish["Connected as %@"], "Połączono jako %@")
    // Lowercase on purpose: an abbreviation and a protocol name start these.
    XCTAssertEqual(polish["e.g. Libraxis"], "np. Libraxis")
    XCTAssertEqual(polish["HTTP(S): %@"], "HTTP(S): %@")
    XCTAssertEqual(
      polish["Live WebSocket connection (ws(s); %@)"],
      "Połączenie na żywo przez WebSocket (ws(s); %@)")
    XCTAssertNil(polish["HTTPS: %@"])
    XCTAssertNil(polish["Live WebSocket connection (wss; %@)"])
    for retired in [
      "Providers.", "Refresh provider access", "factory endpoint",
      "Speech-to-text Cloud Service", "Advanced · OAuth client id…",
      "secrets live only in the Keychain — presence shown, value hidden",
    ] {
      XCTAssertNil(polish[retired], "retired key still in the catalog: \(retired)")
    }
  }

  /// Agent › AI models (round 1 of the Polish pass): plain cards, identifiers
  /// and endpoints folded under a collapsed details group, the discovery
  /// failure in one sentence with the provider's words on request, and the
  /// Polish copy exactly as the Founder specified it.
  func testAgentModelsTabKeepsIdentifiersOutOfTheCards() throws {
    let sources = try settingsSources()
    let tab = try XCTUnwrap(sources["SettingsTab.swift"])
    XCTAssertTrue(tab.contains("String(localized: \"AI models\""))
    XCTAssertTrue(tab.contains("case .agentLanes: String(localized: \"Model configuration.\")"))
    XCTAssertFalse(tab.contains("\"LLM lanes\""))

    let panel = try XCTUnwrap(sources["AgentPanel.swift"])
    XCTAssertFalse(
      panel.contains("subtitle: lane.providerKey"), "settings keys left the Provider card")
    XCTAssertFalse(panel.contains("subtitle: lane.modelKey"), "settings keys left the Model card")
    XCTAssertTrue(panel.contains("TextField(laneModel.resolvedModel, text: $modelDraft)"))
    XCTAssertTrue(panel.contains(".onAppear { modelDraft = laneModel.configuredModel }"))
    XCTAssertTrue(panel.contains("Button(String(localized: \"Reset model\""))
    XCTAssertTrue(panel.contains(".disabled(!hasOverride)"), "Reset is live only with an override")
    XCTAssertTrue(panel.contains("model.setLLMModel(\"\", for: lane)"))
    XCTAssertTrue(panel.contains("DisclosureGroup(\"Error details\")"))
    XCTAssertTrue(panel.contains("if failureShownOnAgent {"))
    XCTAssertFalse(panel.contains("Pick a provider and a model per request path"))

    let lanes = try XCTUnwrap(sources["AgentLanesTab.swift"])
    XCTAssertTrue(lanes.contains("@State private var detailsExpanded = false"))
    XCTAssertTrue(lanes.contains("DisclosureGroup(isExpanded: $detailsExpanded)"))
    XCTAssertTrue(lanes.contains("value: \"\\(lane.providerKey) · \\(lane.modelKey)\""))
    let autoSend = try XCTUnwrap(lanes.range(of: "Auto-send to the Agent"))
    let details = try XCTUnwrap(lanes.range(of: "Active configuration details"))
    XCTAssertLessThan(autoSend.lowerBound, details.lowerBound)
    XCTAssertFalse(lanes.contains("Resolved runtime truth"))

    let model = try XCTUnwrap(sources["SettingsViewModel.swift"])
    XCTAssertTrue(model.contains("case \"key_rejected\":"))
    XCTAssertFalse(
      model.contains("Model discovery failed: \\(message)"), "raw body left the main line")

    let polish = try polishCatalog()
    let expected: [String: String] = [
      "AI models": "Modele AI",
      "Model configuration.": "Konfiguracja modeli",
      "Choose the Agent and formatting models. Accounts and keys live under Providers.":
        "Wybierz modele Agenta i formatowania. Konta i klucze ustawisz w Dostawcach.",
      "Assistive": "Agent",
      "Formatting": "Formatowanie",
      "The Agent and voice assistant model": "Model Agenta i asystenta głosowego",
      "The transcript cleanup model": "Model poprawiania transkrypcji",
      "Connected account": "Połączone konto",
      "Stored API key": "Zapisany klucz API",
      "Could not fetch %@ models. The API key was rejected. Check it under Providers.":
        "Nie udało się pobrać modeli %@. Klucz API został odrzucony. Sprawdź go w sekcji Dostawcy.",
      "Error details": "Szczegóły błędu",
      "Refresh": "Odśwież",
      "Reset model": "Przywróć model domyślny",
      "Active configuration details": "Szczegóły aktywnej konfiguracji",
      "%@ endpoint": "Adres API: %@",
      "Auto-send to the Agent": "Wysyłaj automatycznie do Agenta",
      "Sends the transcript after 5 seconds unless you start editing it.":
        "Wyślij transkrypcję po 5 sekundach, jeśli nie rozpoczniesz edycji.",
      "Model list unavailable: the stored API key was rejected. The connected account still covers requests. Check the key under Providers.":
        "Lista modeli niedostępna: zapisany klucz API został odrzucony. Połączone konto nadal obsługuje żądania. Sprawdź klucz w sekcji Dostawcy.",
    ]
    for (key, value) in expected {
      XCTAssertEqual(polish[key], value, key)
    }
    for retired in [
      "LLM lanes", "Request lanes.", "Transcript delivery", "Resolved runtime truth", "Reset",
      "account", "no key required",
      "Model discovery failed. Check Settings › Providers, then refresh models in Settings › Agent › LLM lanes.",
      "Model discovery failed: %@. Check Settings › Providers, then refresh models in Settings › Agent › LLM lanes.",
      "Connect accounts or add API keys. Models are chosen under Agent › LLM lanes.",
    ] {
      XCTAssertNil(polish[retired], "retired key still in the catalog: \(retired)")
    }
  }

  /// Round 2 of the Polish pass: the Prompts tab keeps file names and raw
  /// source ids out of the first level, never renders an unsaved draft as the
  /// saved prompt, and names the prompt a restore will replace.
  /// Round 3: the Workspace tab names folder access, not "projects", and the
  /// one list stays neutral because one setting feeds both the path policy and
  /// the project scan. The boundary it promises is the one the code enforces:
  /// `app/agent/tools/path_policy.rs` gates our own tools, and an MCP server is
  /// a separate process that never passes through it (PL-028).
  func testWorkspaceTabNamesAgentAccessNotProjects() throws {
    let sources = try settingsSources()
    let tab = try XCTUnwrap(sources["SettingsTab.swift"])
    XCTAssertTrue(tab.contains("String(localized: \"Folders available to the Agent\""))
    XCTAssertFalse(tab.contains("\"Workspace roots.\""))
    XCTAssertTrue(
      tab.contains(
        "\"The Agent's built-in tools, and the paths it hands to MCP tools it can check, are bounded to these folders. MCP servers otherwise operate independently.\""
      ),
      "the 'it can check' qualifier is load-bearing: only validated MCP paths are bounded")
    XCTAssertFalse(
      tab.contains("It has no access outside them."),
      "the description must not claim a boundary around every MCP process")

    let section = try XCTUnwrap(sources["WorkspaceRootsSection.swift"])
    XCTAssertTrue(section.contains("WorkspaceSectionHeader(String(localized: \"Allowed folders\"))"))
    XCTAssertFalse(section.contains("(list_projects)"), "tool names stay out of the UI copy")
    XCTAssertTrue(section.contains("Label(\"Add folder…\", systemImage: \"plus\")"))
    XCTAssertTrue(section.contains("Button(action: pickFolder)"), "the ellipsis opens a picker")
    XCTAssertTrue(section.contains("panel.canChooseDirectories = true"))
    XCTAssertTrue(section.contains("Text(\"Save changes\")"))
    XCTAssertTrue(section.contains(".help(\"Remove folder\")"))
    XCTAssertTrue(section.contains(".accessibilityLabel(\"Remove folder\")"))
    XCTAssertTrue(
      section.contains("Label(\"Undo remove\", systemImage: \"arrow.uturn.backward\")"),
      "an accidental remove is undoable before Save")
    XCTAssertTrue(section.contains("rows.insert(last.path, at: min(last.index, rows.count))"))
    XCTAssertTrue(section.contains("Text(\"Discard changes\")"))

    let polish = try polishCatalog()
    let expected: [String: String] = [
      "Folders available to the Agent": "Foldery dostępne dla Agenta",
      "The Agent's built-in tools, and the paths it hands to MCP tools it can check, are bounded to these folders. MCP servers otherwise operate independently.":
        "Wbudowane narzędzia Agenta i ścieżki, które przekazuje sprawdzanym narzędziom MCP, są ograniczone do tych folderów. Same serwery MCP działają niezależnie.",
      "Allowed folders": "Dozwolone foldery",
      "The Agent looks for projects and Git repositories here, skipping hidden folders and build directories.":
        "Agent szuka tu projektów i repozytoriów Git, pomijając ukryte foldery i katalogi build.",
      "Add folder…": "Dodaj folder…",
      "Save changes": "Zapisz zmiany",
      "Remove folder": "Usuń folder",
      "Undo remove": "Cofnij usunięcie",
      "Discard changes": "Odrzuć zmiany",
      "Choose a folder the Agent may read and write":
        "Wybierz folder, w którym Agent może odczytywać i zapisywać dane",
    ]
    for (key, value) in expected {
      XCTAssertEqual(polish[key], value, key)
    }
    for retired in [
      "Workspace roots.", "Agent workspace roots", "Add root", "Save roots",
      "The Agent can read and write only inside these folders. It has no access outside them.",
      "The Agent's built-in file tools read and write only inside these folders. MCP servers have separate access rules.",
      "Directories the Agent may read and write. Everything outside them is out of reach.",
      "Directories the Agent scans for git checkouts to resolve a project name to a path (list_projects). Recursive, a few levels deep; build and hidden folders are skipped.",
    ] {
      XCTAssertNil(polish[retired], "retired key still in the catalog: \(retired)")
    }
  }

  func testPromptsTabKeepsFileNamesOutOfTheFirstLevel() throws {
    let sources = try settingsSources()
    let tab = try XCTUnwrap(sources["SettingsTab.swift"])
    XCTAssertTrue(
      tab.contains(
        "case .agentPrompts: String(localized: \"Prompts\", comment: \"Settings tab: editable prompts\")"
      ), "the Prompts headline lost its trailing period")
    XCTAssertFalse(tab.contains("Edits the BASE prompt file"))

    let files = try XCTUnwrap(sources["PromptFile.swift"])
    XCTAssertFalse(files.contains(".txt)"), "file names left the prompt descriptions")

    let panel = try XCTUnwrap(sources["PromptPanel.swift"])
    XCTAssertTrue(panel.contains("@State private var editingFiles: Set<PromptFile> = []"))
    XCTAssertTrue(
      panel.contains("private var savedText: String { snapshot?.content ?? \"\" }"),
      "VIEW renders the saved snapshot, never the draft")
    XCTAssertTrue(panel.contains("raw: savedText.isEmpty"))
    XCTAssertTrue(panel.contains("TextEditor(text: $draft)"))
    XCTAssertTrue(panel.contains("Button(\"Cancel\", action: onDiscard)"))
    XCTAssertTrue(panel.contains("Button(\"Restore default\")"))
    XCTAssertFalse(panel.contains("Button(\"Restore…\")"))
    XCTAssertTrue(
      panel.contains("\"Only \\(title) will change:"), "the confirmation names the prompt")
    XCTAssertTrue(
      panel.contains("its custom file is removed and the built-in prompt takes over"),
      "the confirmation says what restoring does")
    XCTAssertTrue(panel.contains("failure: failures[file],"), "failures are shown per file")
    XCTAssertTrue(panel.contains(".accessibilityIdentifier(\"settings-prompt-failure\")"))
    XCTAssertTrue(
      panel.contains("failures[file] = PromptOperationFailure("),
      "a nil snapshot records a failure instead of a refreshed snapshot")
    XCTAssertTrue(panel.contains("DisclosureGroup(isExpanded: $detailsExpanded)"))
    XCTAssertTrue(panel.contains("Text(\"File details\")"))
    XCTAssertFalse(panel.contains("ScrollView {\n      MarkdownText"), "no nested scrolling")
    // Round 9 order: quiet source tag in the header, the prompt content, and
    // File details as the panel's last line.
    let header = try XCTUnwrap(panel.range(of: "private var header: some View"))
    let tagDecl = try XCTUnwrap(panel.range(of: "private var sourceTag: some View"))
    let tagUse = try XCTUnwrap(panel[header.upperBound...].range(of: "sourceTag"))
    XCTAssertLessThan(tagUse.lowerBound, tagDecl.lowerBound, "the tag sits in the header")
    let body = try XCTUnwrap(panel.range(of: "promptBody\n"))
    let details = try XCTUnwrap(panel.range(of: "fileDetails\n"))
    XCTAssertLessThan(body.lowerBound, details.lowerBound, "File details close the panel")

    let polish = try polishCatalog()
    let expected: [String: String] = [
      "Prompts": "Prompty",
      "Browse and edit the base prompts.": "Przeglądaj i edytuj prompty bazowe.",
      "Correction": "Korekta",
      "Correction prompt": "Prompt korekty",
      "Smart prompt": "Prompt Smart",
      "Max prompt": "Prompt Max",
      "Agent prompt": "Prompt Agenta",
      "Source: Built-in prompt": "Źródło: Wbudowany prompt",
      "Source: Custom prompt": "Źródło: Własny prompt",
      "File details": "Szczegóły pliku",
      "Restore default": "Przywróć domyślny",
      "settings.prompt.source.builtIn": "Wbudowany",
      "settings.prompt.source.custom": "Własny",
      "The custom file is created on save.": "Własny plik powstanie przy zapisie.",
      "Preview of the original prompt text.": "Podgląd oryginalnej treści promptu.",
      "The built-in prompt is already in use.": "W użyciu jest już wbudowany prompt.",
      "Unsaved changes": "Niezapisane zmiany",
      "Edit": "Edytuj",
      "Save": "Zapisz",
      "Only %@ will change: its custom file is removed and the built-in prompt takes over. The previous version remains recoverable in the prompt backups folder.":
        "Zmieni się tylko %@: własny plik zostanie usunięty, a w użyciu będzie wbudowany prompt. Poprzednią wersję można odzyskać z folderu kopii zapasowych promptów.",
      "Could not complete restoring %@. Check the current source shown above.":
        "Nie udało się dokończyć przywracania: %@. Sprawdź aktualne źródło pokazane powyżej.",
      "Could not complete saving %@. Check the current source shown above.":
        "Nie udało się dokończyć zapisu: %@. Sprawdź aktualne źródło pokazane powyżej.",
    ]
    for (key, value) in expected {
      XCTAssertEqual(polish[key], value, key)
    }
    for retired in [
      "Prompts.", "Assistive prompt", "Custom file", "Built-in fallback", "Restore…",
      "Restore default…",
      "No custom prompt file yet. Saving creates one at this path.",
      "Browse and edit the base prompts. Codescribe may add further instructions to them while it runs.",
      "Only %@ will change. The previous version remains recoverable in the prompt backups folder.",
      "Correction only AI formatting (formatting.txt)",
      "Base system prompt for the Agent (assistive.txt)",
      "Edits the BASE prompt file. The core still appends its tuning prompt at runtime.",
    ] {
      XCTAssertNil(polish[retired], "retired key still in the catalog: \(retired)")
    }
  }

  /// The Tools tab is a permissions screen: Polish headline without a repeated
  /// section label, category defaults named by what they cover, a whole-catalog
  /// count, native as a UI label only, readable names above raw identifiers and
  /// an inheritance caption with a way back from an individual rule.
  func testToolsTabReadsAsAPermissionScreen() throws {
    let sources = try settingsSources()
    let tab = try XCTUnwrap(sources["SettingsTab.swift"])
    XCTAssertTrue(tab.contains("case .agentTools: String(localized: \"Tool permissions\")"))
    XCTAssertFalse(tab.contains("\"Tool permissions.\""))
    XCTAssertFalse(tab.contains("Deny wins over everything"))
    XCTAssertTrue(tab.contains("\"Choose when the Agent may use tools.\""))

    let section = try XCTUnwrap(sources["ToolPermissionsSection.swift"])
    XCTAssertFalse(
      section.contains("SettingsSectionLabel(String(localized: \"Tool permissions\"))"),
      "the tab headline already says it")
    XCTAssertFalse(section.contains("Defaults: read-only allow, side-effectful ask"))
    XCTAssertTrue(section.contains("defaultRow(title: \"Read data\""))
    XCTAssertTrue(section.contains("defaultRow(title: \"Changes, processes and network\""))
    XCTAssertTrue(section.contains("defaultRow(title: \"Unclassified\""))
    XCTAssertTrue(section.contains("ToolsSectionHeader(String(localized: \"Individual tools\"))"))
    XCTAssertFalse(section.contains("Tool overrides"))
    XCTAssertTrue(section.contains("A rule for one tool outranks its server's rule"))
    XCTAssertTrue(section.contains("Text(item.displayName)"))
    XCTAssertTrue(section.contains("Text(item.identity)"), "the raw identifier stays")
    XCTAssertTrue(
      section.contains("let category = ToolPermissionLabels.risk(risk)"),
      "the category rides the card's one facts line (sourceSummary)")
    XCTAssertTrue(section.contains("Text(verbatim: item.sourceSummary)"))
    XCTAssertTrue(section.contains("ToolPermissionLabels.ruleCaption(item.ruleSource)"))
    XCTAssertTrue(section.contains("if item.hasIndividualRule, let restoreInheritance {"))
    XCTAssertTrue(
      section.contains("if model.toolCatalogLoading {"),
      "MCP discovery takes seconds: the tab says so instead of showing an empty catalog")
    XCTAssertFalse(
      section.contains("HStack(spacing: 8) {\n          Text(ToolPermissionLabels.ruleCaption"),
      "the rule caption and the restore link stack vertically so the narrow column never splits a word"
    )
    XCTAssertFalse(section.contains("Text(item.name)"), "the raw name is not the headline")
    // Our own tools are named in the interface language; an MCP server's tools
    // keep the vendor's spelling, so only `native:` rows reach the catalog.
    XCTAssertTrue(
      section.contains("ToolPermissionLabels.displayName(for: name, identity: identity)"))
    XCTAssertTrue(section.contains("if identity.hasPrefix(\"native:\"), let own"))
    XCTAssertTrue(
      section.contains("localized: \"tools.native.read_file\", defaultValue: \"Read a file\""))
    // Both lines truncate on purpose, so the pair stays readable in a tooltip.
    XCTAssertTrue(section.contains("private var fullIdentification: String"))
    XCTAssertTrue(section.contains(".help(fullIdentification)"))

    // Round 12: the source sidebar became a popup in the browser; the old
    // ToolServerTab file is gone with it.
    XCTAssertNil(sources["ToolServerTab.swift"], "the sidebar source button is cut")
    let search = try XCTUnwrap(sources["ToolSearchField.swift"])
    XCTAssertTrue(search.contains("TextField(\"Search tools…\""))
    let browser = try XCTUnwrap(sources["ToolOverridesBrowser.swift"])
    XCTAssertTrue(browser.contains("model.clearToolPermission(identity: item.identity)"))
    XCTAssertTrue(browser.contains("Self.sourceLabel(server: group.server, count: group.items.count)"))

    let polish = try polishCatalog()
    let expected: [String: String] = [
      "Tool permissions": "Uprawnienia narzędzi",
      "Choose when the Agent may use tools.": "Wybierz, kiedy Agent może korzystać z narzędzi.",
      "Individual tools": "Poszczególne narzędzia",
      "Source": "Źródło",
      "Search tools…": "Szukaj narzędzia…",
      "Deny": "Blokuj",
      "Read data": "Odczyt danych",
      "Changes, processes and network": "Zmiany, procesy i sieć",

      "Native": "Natywne",
      "Changes": "Zmiany",
      "Network": "Sieć",
      "Individual rule": "Własna reguła",
      "Category default": "Ustawienie kategorii",
      "Server rule": "Reguła serwera",
      "Remove rule": "Usuń regułę",
      "Discovering tools from the MCP servers…": "Wykrywanie narzędzi z serwerów MCP…",
      "tools.native.read_file": "Odczytaj plik",
      "tools.native.write_file": "Zapisz plik",
      "tools.native.run_process": "Uruchom proces",
      "tools.native.take_screenshot": "Zrób zrzut ekranu",
    ]
    for (key, value) in expected {
      XCTAssertEqual(polish[key], value, key)
    }
    for retired in [
      "Tool permissions.", "Allow, ask, or deny — per tool. Deny wins over everything.",
      "Per-tool permissions · %lld", "%lld tool sources", "Unclassified tools",
      "Search server or tool",
      "Set when the Agent may use tools without asking, when it needs your approval, and when it must refuse.",
      "Tool overrides · %lld", "Read-only", "Side effects", "Global / unknown", "%lld servers",
      "Inherited from the category default", "Inherited from the server rule", "Restore inheritance",
    ] {
      XCTAssertNil(polish[retired], "retired key still in the catalog: \(retired)")
    }
  }

  /// Diagnostics is a status screen: Polish headline and labels, one summary
  /// line per inventory with the full list collapsed, and status words next
  /// to every dot.
  func testDiagnosticsTabReadsAsAStatusScreen() throws {
    let sources = try settingsSources()
    let tab = try XCTUnwrap(sources["SettingsTab.swift"])
    XCTAssertTrue(tab.contains("String(localized: \"Agent environment status\""))
    XCTAssertFalse(tab.contains("\"Connection details.\""))
    XCTAssertTrue(
      tab.contains(
        "\"Configuration state of the Agent, its available tools and integrations.\""))

    let section = try XCTUnwrap(sources["AgentStatusSection.swift"])
    // Round 11: summary card first, then Integrations, Tools and servers,
    // Detected installations, and one collapsed Technical details holding the
    // full readiness table.
    XCTAssertTrue(section.contains("AgentStatusSummary("))
    XCTAssertTrue(section.contains("AgentIntegrationLine.lines(rows: model.agentReadiness.rows)"))
    XCTAssertTrue(section.contains("CapabilitySummary(rows: model.capabilityMatrix)"))
    XCTAssertTrue(section.contains("McpServerSummary(lines: serverLines)"))
    XCTAssertTrue(
      section.contains("@State private var showingTechnicalDetails = false"),
      "technical details are collapsed by default")
    XCTAssertTrue(section.contains("DisclosureGroup(isExpanded: $showingTechnicalDetails)"))
    XCTAssertTrue(
      section.contains("DisclosureGroup(isExpanded: $showingCapabilityRows)"),
      "the capability matrix is collapsed by default")
    XCTAssertTrue(
      section.contains("DisclosureGroup(isExpanded: $showingServerRows)"),
      "the merged server table is collapsed by default")
    XCTAssertTrue(section.contains("@State private var showingCapabilityRows = false"))
    XCTAssertTrue(section.contains("@State private var showingServerRows = false"))
    XCTAssertFalse(section.contains("Per-server probe"), "the duplicate probe list is gone")
    XCTAssertTrue(section.contains("Text(\"Configuration source:\")"))
    XCTAssertTrue(section.contains("McpServerLine.merge("))
    XCTAssertTrue(section.contains("StatusToneMark(tone: row.tone)"))
    XCTAssertTrue(section.contains(".help(tone.label)"))
    XCTAssertTrue(section.contains(".accessibilityLabel(tone.label)"))
    XCTAssertTrue(section.contains("Text(row.localizedLabel)"))
    XCTAssertTrue(section.contains("Text(row.localizedValue)"))
    XCTAssertFalse(section.contains("Text(row.label)"), "English core labels never reach the UI")
    XCTAssertFalse(section.contains("Text(row.value)"), "English core values never reach the UI")
    XCTAssertFalse(section.contains("\"tool: \\(row.nativeTool)"), "mono detail is localized")

    // A stored key is not a successful request: the verdict says what the app
    // actually knows, so "access" never stands in for credentials on file.
    let presentation = try XCTUnwrap(sources["AgentStatusPresentation.swift"])
    XCTAssertTrue(
      presentation.contains(
        "\"Ready — \\(subject) configured, can send requests, \\(nativeToolCount)\""))
    XCTAssertTrue(presentation.contains("\"\\(subject) — can send requests\""))
    XCTAssertFalse(
      presentation.contains("access available"), "readiness is never reported as a made request")
    XCTAssertFalse(
      presentation.contains("credentials available"),
      "a key-optional provider is request-ready without stored credentials")

    let polish = try polishCatalog()
    let expected: [String: String] = [
      "Agent environment status": "Stan środowiska Agenta",
      "Configuration state of the Agent, its available tools and integrations.":
        "Stan konfiguracji Agenta, dostępnych narzędzi i integracji.",
      "Overall status": "Stan ogólny",
      "Model provider": "Dostawca modelu",
      "Native tools": "Narzędzia natywne",
      "VibeCrafted runtime": "Runtime VibeCrafted",
      "PRView integration": "Integracja PRView",
      "Ready — %@ configured, can send requests, %@":
        "Gotowy — skonfigurowano %1$@, może wysyłać żądania, %2$@",
      "%@ — can send requests": "%@ — może wysyłać żądania",
      "Configured — agent not started yet": "Skonfigurowano — agent nie został jeszcze uruchomiony",
      "Not configured (optional)": "Nieskonfigurowane (opcjonalne)",
      "Technical details": "Szczegóły techniczne",
      "Agent status": "Stan Agenta",
      "Integrations": "Integracje",
      "Tools and servers": "Narzędzia i serwery",
      "Agent tools": "Narzędzia Agenta",
      "Detected installations": "Wykryte instalacje",
      "Configuration ready": "Konfiguracja gotowa",
      "Optional · not configured": "Opcjonalne · nieskonfigurowane",
      "Awaits first run": "Oczekuje na uruchomienie",
      "Detected": "Wykryto",
      "Needs repair": "Wymaga naprawy",
      "Built-in Codescribe tool": "Wbudowane narzędzie Codescribe",
      "tool: %@ · source: %@": "narzędzie: %1$@ · źródło: %2$@",
      "capability.tier.native": "Natywne",
      "capability.tier.enhanced": "Rozszerzone",
      "Configuration source:": "Źródło konfiguracji:",

      "status.tone.good": "Gotowe",
      "status.tone.warn": "Ostrzeżenie",
      "status.tone.neutral": "Nie sprawdzono",
      "Not tested": "Nie sprawdzono",
    ]
    for (key, value) in expected {
      XCTAssertEqual(polish[key], value, key)
    }
    for retired in [
      "Connection details", "Connection details.", "Capability matrix", "Per-server probe",
      "Detected installations and runtime", "Available tools and integrations",
      "Show servers",
      "The Agent has not run yet — server status is checked on its first turn.",
      "Native: %lld · Enhanced: %lld · Unavailable: %lld",
      "Configured: %lld · Tested: %lld · Issues: %lld",
      "%lld configured", "not tested", "testing…", "fail: %@",
      "Ready — %@ configured, access available, %@", "%@ — access available",
      "Ready — %@ configured, credentials available, %@", "%@ — credentials available",
    ] {
      XCTAssertNil(polish[retired], "retired key still in the catalog: \(retired)")
    }
  }

  /// The MCP tab reads as a list of servers: Polish headline, the configured
  /// state as a flag, the last handshake as a test result, technical detail
  /// collapsed, labelled form fields that survive a failed add.
  func testMcpTabReadsAsAServerScreen() throws {
    let sources = try settingsSources()
    let tab = try XCTUnwrap(sources["SettingsTab.swift"])
    XCTAssertTrue(tab.contains("case .agentMcp: String(localized: \"MCP servers\")"))
    XCTAssertFalse(tab.contains("\"MCP servers.\""))
    XCTAssertTrue(
      tab.contains("\"Add MCP servers and manage the tools the Agent may use.\""))

    let view = try XCTUnwrap(sources["SettingsView.swift"])
    XCTAssertTrue(view.contains(".navigationTitle(Text(\"Settings\"))"))
    XCTAssertFalse(view.contains(".navigationTitle(Text(verbatim: \"\"))"))
    XCTAssertTrue(view.contains("window?.titleVisibility = .hidden"))
    XCTAssertTrue(view.contains("private func adoptHostWindow(_ window: NSWindow?)"))

    let section = try XCTUnwrap(sources["MCPServersSection.swift"])
    XCTAssertFalse(section.contains("Manage MCP servers"), "the tab headline already says it")
    XCTAssertTrue(section.contains("String(localized: \"Connection not tested\")"))
    XCTAssertTrue(section.contains("String(localized: \"Checking the connection…\")"))
    XCTAssertTrue(section.contains("String(localized: \"Last test: failed\")"))
    XCTAssertTrue(section.contains("Last test: passed · \\(Int(result.toolCount)) tools"))
    XCTAssertFalse(section.contains("policy: ask"), "no frozen policy literal on the card")
    XCTAssertTrue(section.contains("ToolPermissionLabels.level(permissionLevel)"))
    XCTAssertTrue(section.contains("@State private var showingDetails = false"))
    XCTAssertTrue(section.contains("DisclosureGroup(isExpanded: $showingDetails)"))
    XCTAssertTrue(section.contains("@State private var showingTechnicalDetails = false"))
    XCTAssertTrue(section.contains("DisclosureGroup(isExpanded: $showingTechnicalDetails)"))
    XCTAssertTrue(section.contains("Text(\"Move MCP configuration to Trash…\")"))
    XCTAssertFalse(section.contains("Clear MCP configuration"))
    XCTAssertTrue(
      section.contains("String(localized: \"mcp.server.enabled\", defaultValue: \"Enabled\")"))
    XCTAssertTrue(view.contains("HostingWindowReader(onWindow: adoptHostWindow)"))
    for label in ["Server name", "Launch command", "Command arguments", "Server URL"] {
      XCTAssertTrue(section.contains("\"\(label)\", placeholder: \""), label)
    }
    XCTAssertEqual(section.components(separatedBy: "labeledField(").count, 6)
    XCTAssertTrue(section.contains("fieldLabel(\"Access token (optional)\")"))
    XCTAssertTrue(
      section.contains("guard addError == nil else {"),
      "a failed add keeps the typed fields")

    // P2-006: the caption is the field's accessibility name, not the
    // placeholder or the typed text; the token field has a name at all.
    XCTAssertTrue(section.contains("TextField(title, text: text, prompt: Text(placeholder))"))
    XCTAssertFalse(section.contains("TextField(placeholder, text: text)"))
    XCTAssertTrue(
      section.contains("SecureField(text: $token, prompt: nil) { Text(\"Access token (optional)\") }"))
    XCTAssertFalse(section.contains("SecureField(text: $token, prompt: nil) { EmptyView() }"))

    // P2-005: the form renders the translated refusal under its field and
    // never the store's `Config(msg: …)` text.
    XCTAssertTrue(section.contains(") -> MCPAddFailure?"))
    XCTAssertTrue(section.contains("@State private var addError: MCPAddFailure?"))
    XCTAssertTrue(section.contains("if let addError, Self.field(for: addError.field) == focus {"))
    XCTAssertTrue(section.contains("Text(verbatim: failure.message)"))
    XCTAssertTrue(section.contains("if let field = Self.field(for: addError?.field) { focusedField = field }"))
    // The store is the one validator: the form no longer pre-filters the
    // cases it now knows how to show, and the name reaches the store raw.
    XCTAssertTrue(section.contains("!name.isEmpty || !(remote ? endpoint : command).isEmpty"))
    XCTAssertFalse(section.contains("hasPrefix(\"http\")"))
    XCTAssertTrue(section.contains("addError = onAdd(\n      name,\n"))
    XCTAssertFalse(section.contains("name.trimmingCharacters(in: .whitespaces)"))

    // P1-002: Remove asks; the alert names the server and the consequence.
    XCTAssertTrue(section.contains("onRemove: { model.requestMcpServerRemoval(server.name) }"))
    XCTAssertFalse(section.contains("onRemove: { model.removeMcpServer("))
    XCTAssertTrue(section.contains("presenting: model.mcpRemovalCandidate"))
    XCTAssertTrue(section.contains("Text(\"Remove \\(model.mcpRemovalCandidate ?? \"\") from MCP servers?\")"))
    XCTAssertTrue(section.contains("Button(\"Cancel\", role: .cancel) { model.cancelMcpServerRemoval() }"))
    XCTAssertTrue(
      section.contains("Button(\"Remove server\", role: .destructive) { model.confirmMcpServerRemoval(name) }"))
    XCTAssertTrue(section.contains(".help(\"Remove this server from mcp.json…\")"))

    let polish = try polishCatalog()
    let expected: [String: String] = [
      "Remove %@ from MCP servers?": "Usunąć %@ z serwerów MCP?",
      "Remove server": "Usuń serwer",
      "Removes %@ from mcp.json and deletes its Keychain token. The Agent loses this server's tools until you add it again.":
        "Usuwa %@ z pliku mcp.json i kasuje jego token z pęku kluczy. Agent traci narzędzia tego serwera, dopóki nie dodasz go ponownie.",
      "Remove this server from mcp.json…": "Usuń ten serwer z pliku mcp.json…",
      "The server URL is invalid. Enter a full HTTP or HTTPS URL with a hostname.":
        "Adres serwera jest nieprawidłowy. Wpisz pełny adres HTTP lub HTTPS z nazwą hosta.",
      "A server with this name already exists. Choose another name.":
        "Serwer o tej nazwie już istnieje. Wybierz inną nazwę.",
      "Enter the command that starts the server.": "Wpisz polecenie, które uruchamia serwer.",
      "MCP servers": "Serwery MCP",
      "Add MCP servers and manage the tools the Agent may use.":
        "Dodawaj serwery MCP i zarządzaj narzędziami, z których może korzystać Agent.",
      "Connection not tested": "Nie sprawdzono połączenia",
      "Checking the connection…": "Sprawdzanie połączenia…",
      "Last test: failed": "Ostatni test: nieudany",
      "mcp.server.enabled": "Włączony",
      "mcp.server.disabled": "Wyłączony",
      "Local process": "Proces lokalny",
      "HTTP connection": "Połączenie HTTP",
      "Server name": "Nazwa serwera",
      "Launch command": "Polecenie uruchomieniowe",
      "Command arguments": "Argumenty polecenia",
      "Server URL": "Adres URL serwera",
      "Access token (optional)": "Token dostępu (opcjonalnie)",
      "The token is stored in the macOS Keychain, never in mcp.json.":
        "Token jest zapisywany w pęku kluczy macOS, nie w pliku mcp.json.",
      "Move MCP configuration to Trash…": "Przenieś konfigurację MCP do Kosza…",
      "Move MCP configuration to Trash?": "Przenieść konfigurację MCP do Kosza?",
      "Test": "Sprawdź",
      "Settings": "Ustawienia",
    ]
    for (key, value) in expected {
      XCTAssertEqual(polish[key], value, key)
    }
    for retired in [
      "MCP servers.", "Manage MCP servers", "Remote HTTP", "disconnected — not tested",
      "disconnected — disabled", "connecting…", "degraded — %@",
      "remote · no authentication · policy: ask", "remote · token in Keychain · policy: ask",
      "Clear MCP configuration…", "name (e.g. prview)", "endpoint (https://…/mcp)",
      "Remove this server from mcp.json",
    ] {
      XCTAssertNil(polish[retired], "retired key still in the catalog: \(retired)")
    }
  }

  /// The Audio pane reads as "microphone and recording": a headline without a
  /// trailing period, no storage-format or Core Audio vocabulary in the basic
  /// descriptions, one status line instead of four numbered steps, a retention
  /// sentence only where a choice expires something, and the measured profile
  /// collapsed under a details disclosure.
  /// The Polish values for the new keys are imported through the sheet tool;
  /// this test pins the source structure, which does not depend on that import.
  func testAudioPaneReadsAsMicrophoneAndRecording() throws {
    let panel = try XCTUnwrap(try settingsSources()["AudioPanel.swift"])
    XCTAssertTrue(panel.contains("String(localized: \"Microphone and recording\")"))
    XCTAssertFalse(panel.contains("Microphone and recording."), "the headline takes no period")
    XCTAssertFalse(panel.contains("Hear the real input"))
    XCTAssertTrue(
      panel.contains("String(localized: \"Choose a microphone and adjust recording.\")"))
    XCTAssertFalse(
      panel.contains("check that recording is ready and adjust the sound settings"),
      "the blurb no longer lists what the sections below already say")

    // The microphone card: one picker and the live input; the one refresh
    // action is the shared chip on the section header (round 8). The system
    // microphone is the first option of the picker, not a second button that
    // says the same thing.
    XCTAssertTrue(panel.contains("SettingsRefreshButton("))
    XCTAssertTrue(panel.contains("axLabel: \"Refresh audio input devices\""))
    XCTAssertFalse(panel.contains("Button(\"Refresh microphones\")"))
    XCTAssertFalse(panel.contains("Button(\"Use the system microphone\")"))
    XCTAssertTrue(panel.contains("defaultValue: \"System default\""))
    XCTAssertTrue(
      panel.contains(
        "func audioInputCardState(_ snapshot: CsAudioInputSnapshot) -> AudioInputCardState"))

    // Point 4: the basic descriptions name the microphone, not the storage
    // format or the macOS audio framework.
    XCTAssertFalse(panel.contains("settings.json"))
    XCTAssertFalse(panel.contains("Core Audio"))

    XCTAssertTrue(panel.contains("if let detail = audioRetentionDetail(model.audioRetention)"))
    XCTAssertFalse(
      panel.contains("Off discards the audio of new takes once processing finishes"),
      "the retention sentence is no longer unconditional")
    XCTAssertTrue(panel.contains("func audioRetentionDetail(_ choice: String) -> String?"))

    XCTAssertTrue(panel.contains("String(localized: \"Recording start sound\")"))
    XCTAssertFalse(panel.contains("String(localized: \"Sound feedback\")"))
    XCTAssertFalse(panel.contains("String(localized: \"Start sound\")"))
    XCTAssertFalse(
      panel.contains("Play the recorder's live start confirmation"),
      "the subtitle only repeated the toggle label")
    XCTAssertTrue(panel.contains("Slider(value: soundVolumeBinding, in: 0...1, step: 0.05)"))
    XCTAssertTrue(panel.contains("Toggle(\"\", isOn: soundFeedbackBinding)"))

    // One status line plus two value rows. The blocker, its explanation and
    // its remedy are projected, never improvised by the view.
    XCTAssertTrue(
      panel.contains(
        "func audioReadinessSummary("))
    XCTAssertTrue(panel.contains("func audioSealLaneFact(_ admission: CsAdmissionReadiness?)"))
    XCTAssertFalse(
      panel.contains("Text(verbatim: \"\\(step.id.rawValue + 1)\")"),
      "the readiness rows are no longer numbered steps")
    XCTAssertFalse(
      panel.contains("Toggle(\"Committing transcript fragments\""),
      "the committing switch lives on the Lab desk")

    // Point 6: one name for the committing row, the profile id behind a
    // disclosure, and the stored measurement read straight from the verdict.
    XCTAssertTrue(panel.contains("String(localized: \"Committing transcript fragments\")"))
    XCTAssertFalse(panel.contains("\"Seal lane armed\""))
    XCTAssertFalse(panel.contains("\"Seal lane must be enabled\""))
    XCTAssertFalse(panel.contains("\"Seal check waits for microphone access\""))
    XCTAssertTrue(panel.contains("@State private var showingCalibrationDetails = false"))
    XCTAssertTrue(panel.contains("DisclosureGroup(isExpanded: $showingCalibrationDetails)"))
    XCTAssertTrue(panel.contains("GridRow {"), "calibration details are label/value pairs")
    XCTAssertTrue(
      panel.contains("SettingsSectionLabel(String(localized: \"Calibration details\"))"))
    XCTAssertTrue(
      panel.contains(
        "func audioCalibrationDetails(_ readiness: CsAdmissionReadiness?) -> [AudioCalibrationDetail]"
      ))
    XCTAssertTrue(panel.contains("func recordingStartHint(_ dictationShortcut: String?) -> String"))
    XCTAssertFalse(panel.contains("Use \\(dictationShortcut) or choose Start recording."))

    // The one switch that can disarm the recorder's own precondition is a
    // power-user control, and the Lab desk is where it now lives.
    let lab = try XCTUnwrap(try settingsSources()["LabPanel.swift"])
    XCTAssertTrue(lab.contains("Toggle(\"Committing transcript fragments\", isOn: sealLaneBinding)"))
    XCTAssertTrue(lab.contains("Text(\"Required before a recording can start.\")"))
  }

  /// Settings → About: the app, its data and the resets, in English and in
  /// Polish. Everyday facts stay visible; the build details, the configuration
  /// notice details, the template editor and both resets open on demand. The
  /// resets keep their safeguards; only the chrome around them got quieter.
  func testAboutPaneReadsAsTheAppAndItsData() throws {
    let panel = try XCTUnwrap(try settingsSources()["UserPanel.swift"])
    XCTAssertTrue(panel.contains("String(localized: \"About the app and your data\""))
    XCTAssertTrue(panel.contains("\"Codescribe version, local data and privacy.\""))
    // One version line; commit and build date only under Version details.
    XCTAssertTrue(panel.contains("Text(verbatim: \"Codescribe \\(model.buildInfo.version)\")"))
    XCTAssertTrue(panel.contains("\"Build \\(model.buildInfo.build)\""))
    XCTAssertTrue(panel.contains("@State private var showingVersionDetails = false"))
    XCTAssertTrue(panel.contains("if showingVersionDetails {"))
    XCTAssertTrue(panel.contains("detailRow(String(localized: \"Commit\"), model.buildInfo.commit, mono: true)"))
    XCTAssertTrue(panel.contains("detailRow(String(localized: \"Built\"), readableBuildDate, mono: false)"))
    XCTAssertFalse(panel.contains("SettingsSectionLabel(String(localized: \"Running build\"))"))
    XCTAssertFalse(panel.contains("Build timestamp:"), "the raw timestamp is not a second date")
    // The configuration notice stays visible and opens to key, effect and action.
    XCTAssertTrue(panel.contains("configRepairSummary().map(ConfigRepairNotice.init(raw:))"))
    XCTAssertTrue(panel.contains("configNoticeCard(repairNotice)"))
    XCTAssertTrue(panel.contains("notice.reviewItems(envFile: envFileDisplay)"))
    XCTAssertTrue(panel.contains("Text(verbatim: notice.raw)"))
    XCTAssertTrue(panel.contains("String(localized: \"App data\""))
    XCTAssertTrue(panel.contains("pathRow(String(localized: \"Transcripts\"), model.transcriptsPath)"))
    XCTAssertTrue(panel.contains("(path as NSString).abbreviatingWithTildeInPath"))
    // No switch that cannot do anything: the opt-in shows only with a live service.
    XCTAssertTrue(panel.contains("if ActivationPingConfiguration.production.isEnabled {\n        activationPingSection"))
    XCTAssertTrue(panel.contains("@AppStorage(ActivationPing.optInDefaultsKey)"))
    XCTAssertFalse(panel.contains(".disabled(!availability.serviceEnabled)"))
    // Markers: one switch, the editor closed by default, every tool kept.
    XCTAssertTrue(panel.contains("String(localized: \"Transcript markers\""))
    XCTAssertTrue(panel.contains("String(localized: \"Add markers to text\""))
    XCTAssertTrue(panel.contains("\"Mark text delivered to other apps\""))
    XCTAssertTrue(panel.contains("@State private var showingTemplate = false"))
    XCTAssertTrue(panel.contains("DisclosureGroup(isExpanded: $showingTemplate)"))
    XCTAssertTrue(panel.contains("\"Edit template and preview\""))
    XCTAssertTrue(panel.contains("model.insertTranscriptTagPlaceholder(placeholder)"))
    XCTAssertTrue(panel.contains("Button(String(localized: \"Restore default template\""))
    XCTAssertTrue(panel.contains("model.transcriptTagTemplateWarning"))
    XCTAssertTrue(panel.contains("Text(model.transcriptTagPreview)"))
    XCTAssertTrue(panel.contains("String(localized: \"Information and documentation\""))
    XCTAssertTrue(panel.contains("String(localized: \"Terms of Use\""))
    XCTAssertTrue(panel.contains("String(localized: \"Documentation\""))
    // Resets: one closed section, two separate actions, red only on the buttons.
    XCTAssertTrue(panel.contains("@State private var showingResets = false"))
    XCTAssertTrue(panel.contains("if showingResets {"))
    XCTAssertTrue(
      panel.contains("String(localized: \"Reset data\""),
      "the Reset header is a ProvidersSectionHeader since round 16")
    XCTAssertTrue(panel.contains("Text(\"Reset Agent data\""))
    XCTAssertTrue(panel.contains("Text(\"Reset app data\""))
    XCTAssertFalse(panel.contains("String(localized: \"Danger zone\")"))
    XCTAssertFalse(panel.contains(".strokeBorder(CSColor.danger.opacity(0.55)"), "no red card borders")
    XCTAssertTrue(
      panel.contains(
        "\"Also reset my base prompts (assistive.txt, formatting.txt, formatting-smart.txt and formatting-max.txt)\""
      ))
    XCTAssertTrue(
      panel.contains("\"Also remove API keys from Keychain (not recoverable from Trash)\""))
    // Safeguards stay: typed words, both checkboxes, the alerts.
    XCTAssertTrue(panel.contains("Type \\(resetConfirmationWord) to continue"))
    XCTAssertTrue(panel.contains("Type \\(resetAgentConfirmationWord) to continue"))
    XCTAssertTrue(panel.contains("model.resetImpactDescription"))
    XCTAssertTrue(panel.contains("model.resetAgentImpactDescription"))
    XCTAssertTrue(panel.contains(".disabled(!resetConfirmationMatches(confirmationText))"))
    XCTAssertTrue(panel.contains(".disabled(!resetAgentConfirmationMatches(confirmationText))"))

    let polish = try polishCatalog()
    let expected: [String: String] = [
      "About": "O aplikacji",
      "About the app and your data": "O aplikacji i danych",
      "Codescribe version, local data and privacy.":
        "Wersja Codescribe, lokalne dane i prywatność.",
      "Build %@": "Build %@",
      "Version details": "Szczegóły wersji",
      "The configuration needs a review": "Konfiguracja wymaga sprawdzenia",
      "See which setting is out of date": "Zobacz, które ustawienie jest nieaktualne",
      "Local data": "Dane lokalne",
      "App data": "Dane aplikacji",
      "Transcripts": "Transkrypcje",
      "Transcript markers": "Znaczniki transkrypcji",
      "Add markers to text": "Dodawaj znaczniki do tekstu",
      "Mark text delivered to other apps": "Oznacz tekst przekazywany do innych aplikacji",
      "Edit template and preview": "Edytuj szablon i podgląd",
      "Template preview": "Podgląd szablonu",
      "Restore default template": "Przywróć domyślny szablon",
      "Information and documentation": "Informacje i dokumentacja",
      "Privacy Policy": "Polityka prywatności",
      "Terms of Use": "Warunki korzystania",
      "Documentation": "Dokumentacja",
      "Reset data": "Resetowanie danych",
      "Expand": "Rozwiń",
      "Reset Agent data": "Resetuj dane Agenta",
      "Reset app data": "Resetuj dane aplikacji",
    ]
    for (key, value) in expected {
      XCTAssertEqual(polish[key], value, key)
    }
  }

  /// Settings → Dictionary: a fixed header, three honest counters, versions
  /// compared stage by stage, and a Learn action that states its scope first.
  func testDictionaryPaneReadsAsCorrectionsAndRules() throws {
    let panel = try XCTUnwrap(try settingsSources()["VoiceLabPanel.swift"])
    XCTAssertTrue(panel.contains("String(localized: \"Dictionary and corrections\")"))
    XCTAssertTrue(panel.contains("\"Browse corrections and dictionary rules.\""))
    XCTAssertFalse(panel.contains("dictionaryHeadline("), "no dynamic multi-line headline")
    XCTAssertFalse(panel.contains("dictionarySubtitle("), "no repeated provenance subtitle")
    XCTAssertTrue(panel.contains("dictionaryCounters("))
    XCTAssertTrue(panel.contains("String(localized: \"Learn from corrections\""))
    XCTAssertTrue(
      panel.contains(
        "Text(learnScopeMessage(corrections: Int(clamping: model.totalQualityCorrections)))"))
    XCTAssertFalse(panel.contains("Button(\"Teach\") {\n            model.teachDictionaryFromStore()"))
    XCTAssertTrue(panel.contains("DisclosureGroup(isExpanded: $showingDiagnostics)"))
    XCTAssertTrue(panel.contains("\"Changes in the transcript\""))
    XCTAssertFalse(panel.contains("Text(\"Changed\""))
    XCTAssertTrue(panel.contains("stageDiffBlock(stage, index: stageIndex, showTitle: stages.count > 1)"))
    XCTAssertTrue(panel.contains("diffEyebrow(diffSpanKind(span).label)"))
    XCTAssertTrue(panel.contains("fullComparisonLabel("))
    XCTAssertTrue(panel.contains("\"Corrected text\""))
    XCTAssertFalse(panel.contains("\"Corrected original\""))
    XCTAssertTrue(panel.contains("correctionFooter("))
    XCTAssertFalse(panel.contains("Text(\"revision \\(row.revision)\")"))
    XCTAssertTrue(panel.contains("ProvidersSectionHeader(String(localized: \"Dictionary rules\"))"))
    XCTAssertTrue(panel.contains("lexiconProvenanceLine("))
    XCTAssertTrue(panel.contains("model.customLexiconEntries.count <= dictionaryRuleListLimit"))
    XCTAssertTrue(panel.contains("if corrections.count > 1 {"))
    XCTAssertTrue(panel.contains("if model.ruleCandidates.count > 1 {"))
    // The archive walk never runs inside the view body or on the main actor
    // (review, 2026-10-09): the card pairs its audio in a task and caches it.
    XCTAssertFalse(panel.contains("let audioLookup = archivedAudioLookup("))
    XCTAssertTrue(panel.contains("let audioLookup = audioLookups[row.id]"))
    XCTAssertTrue(panel.contains(".task(id: [row.id, String(audioLookupGeneration)]) {"))
    XCTAssertTrue(panel.contains(".disabled(retranscribeReason != nil || audioLookup == nil)"))
    XCTAssertTrue(
      panel.contains("await pairedArchivedAudio(\n          configDir: lease.rootDirectory(), rawText: row.rawText)"))
    XCTAssertFalse(panel.contains("archivedAudioURL(configDir: lease.rootDirectory()"))
    // Learn and the counters quote the corpus, not the capped page.
    XCTAssertTrue(
      panel.contains("learnScopeMessage(corrections: Int(clamping: model.totalQualityCorrections))"))
    XCTAssertTrue(panel.contains("corrections: Int(clamping: model.totalQualityCorrections),"))
    XCTAssertFalse(panel.contains("learnScopeMessage(corrections: corrections.count)"))

    let polish = try polishCatalog()
    let expected: [String: String] = [
      "Dictionary and corrections": "Słownik i poprawki",
      "Browse corrections and dictionary rules.": "Przeglądaj poprawki i reguły słownika.",
      "Changes in the transcript": "Zmiany w transkrypcji",
      "Corrected text": "Poprawiony tekst",
      "Learn from corrections": "Ucz z poprawek",
      "Recent corrections": "Ostatnie poprawki",
      "Dictionary rules": "Reguły słownika",
      "Before": "Przed",
      "After": "Po",
      "Recording unavailable": "Nagranie niedostępne",
      "Diagnostic details": "Szczegóły diagnostyczne",
      "from a correction": "Na podstawie poprawki",
      "added by hand": "Dodano ręcznie",
    ]
    for (key, value) in expected {
      XCTAssertEqual(polish[key], value, key)
    }
    XCTAssertNil(polish["My rules · %lld"], "retired with the Dictionary rules header")
    XCTAssertEqual(polish["Version %llu"], "Wersja %llu")
    XCTAssertEqual(
      polish["Full comparison · %lld → %lld characters"], "Pełne porównanie · %1$lld → %2$lld znaków")
  }

  func testAvailabilityTintsUseSolidTerracotta() throws {
    let model = try XCTUnwrap(settingsSources()["SettingsViewModel.swift"])
    XCTAssertEqual(
      model.components(separatedBy: "? CSColor.oliveLight : CSColor.terracotta").count, 3)
    XCTAssertTrue(
      model.contains(
        "static func availabilityTint(for provider: CsProviderOption, lane: LLMLane = .assistive)"))
    XCTAssertFalse(model.contains("terracottaLight"))
  }

  /// Dictionary rule origins and our own tool names are copy, so the catalog
  /// carries Polish for every one of them; an MCP server's tools are not ours
  /// to translate and stay out of the catalog.
  func testOwnNamesAndRuleOriginsAreTranslated() throws {
    let polish = try polishCatalog()
    let expected: [String: String] = [
      "dictionary.rule.origin.correction": "Na podstawie poprawki",
      "dictionary.rule.origin.manual": "Dodana ręcznie",
      "dictionary.rule.origin.import": "Z importu",
      "dictionary.rule.origin.unknown": "Źródło nieznane",
    ]
    for (key, value) in expected {
      XCTAssertEqual(polish[key], value, key)
    }
    let nativeNames = polish.keys.filter { $0.hasPrefix("tools.native.") }
    XCTAssertEqual(nativeNames.count, 26, "every native tool is named in Polish")
    for key in nativeNames {
      let value = try XCTUnwrap(polish[key])
      XCTAssertFalse(value.isEmpty, key)
      XCTAssertFalse(value.contains("_"), "\(key) still reads as an identifier")
    }
  }

  private func settingsSources() throws -> [String: String] {
    let root = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .appendingPathComponent("Codescribe/Screens/Settings")
    let files =
      try FileManager.default.contentsOfDirectory(at: root, includingPropertiesForKeys: nil)
      .filter { $0.pathExtension == "swift" }
      .sorted { $0.lastPathComponent < $1.lastPathComponent }
    XCTAssertGreaterThan(files.count, 30)
    var sources: [String: String] = [:]
    for file in files {
      sources[file.lastPathComponent] = try String(contentsOf: file, encoding: .utf8)
    }
    return sources
  }

  /// English key to Polish value, straight from the source catalog.
  private func polishCatalog() throws -> [String: String] {
    let url = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .appendingPathComponent("Codescribe/Resources/Localization/Localizable.xcstrings")
    let catalog = try XCTUnwrap(
      JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
    let strings = try XCTUnwrap(catalog["strings"] as? [String: Any])
    var polish: [String: String] = [:]
    for (key, entry) in strings {
      guard let localizations = (entry as? [String: Any])?["localizations"] as? [String: Any],
        let unit = (localizations["pl"] as? [String: Any])?["stringUnit"] as? [String: Any],
        let value = unit["value"] as? String
      else { continue }
      polish[key] = value
    }
    return polish
  }

  /// The first fixed width (`frame(width:)` or `frame(maxWidth:)`) in `source`.
  private func fixedFrameWidth(in source: String) -> CGFloat? {
    let hits = [".frame(width: ", ".frame(maxWidth: "].compactMap {
      key -> (String.Index, CGFloat)? in
      guard let range = source.range(of: key) else { return nil }
      let digits = String(source[range.upperBound...].prefix { $0.isNumber || $0 == "." })
      guard let value = Double(digits) else { return nil }
      return (range.lowerBound, CGFloat(value))
    }
    return hits.min { $0.0 < $1.0 }?.1
  }

  private func joinedSettingsSources() throws -> String {
    try settingsSources().values.joined(separator: "\n")
  }

  /// True when `token` occurs and the next character is not a letter, so
  /// `CSColor.olive` does not pass merely because `CSColor.oliveLight` exists.
  private func containsToken(_ source: String, _ token: String) -> Bool {
    var rest = Substring(source)
    while let range = rest.range(of: token) {
      let after = rest[range.upperBound...].first
      if after == nil || !(after?.isLetter ?? false) {
        return true
      }
      rest = rest[range.upperBound...]
    }
    return false
  }
}
