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
      // Agent keeps six tabs; Dictation has five since the raw recognition
      // timings moved to Lab.
      XCTAssertEqual(english.count, section == .engine ? 5 : 6, "\(section.rawValue)")
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
    XCTAssertEqual(tools.components(separatedBy: ".fixedSize()").count, 3)
    XCTAssertFalse(tools.contains(".frame(width: 180)"))
    XCTAssertFalse(tools.contains(".frame(maxWidth: 180)"))
    XCTAssertEqual(tools.components(separatedBy: "defaultRow(title: \"").count, 4)
    let levels = ["Allow", "Ask", "Deny"]
    let polishLevels = try levels.map { try XCTUnwrap(polish[$0]) }
    XCTAssertEqual(polishLevels, ["Zezwalaj", "Pytaj o zgodę", "Blokuj"])
    // The tools column at the minimum window: the detail column minus the pane
    // padding, the 190 pt server column, the gap between them and the row's
    // own padding. A row keeps at least 96 pt for the tool name.
    let browser = try XCTUnwrap(sources["ToolOverridesBrowser.swift"])
    XCTAssertTrue(browser.contains(".frame(width: 190)"))
    let column = SettingsView.detailMinWidth - 2 * CSSpace.xl - 190 - CSSpace.md - 2 * 14
    for titles in [levels, polishLevels] {
      let picker = width(titles)
      XCTAssertGreaterThan(picker, 0)
      XCTAssertLessThanOrEqual(picker + 8 + 96, column, "\(titles)")
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
    // The wire line is a developer-build fact, and only under Advanced.
    let accepts = try XCTUnwrap(panel.range(of: "Text(lane.accepts)"))
    let gate = try XCTUnwrap(panel.range(of: "if DeveloperSurface.isEnabled() {"))
    XCTAssertLessThan(gate.lowerBound, accepts.lowerBound)
    XCTAssertLessThan(
      accepts.lowerBound.utf16Offset(in: panel) - gate.lowerBound.utf16Offset(in: panel), 80)

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
      "Add a server that speaks OpenAI Responses or Anthropic Messages.",
      "Cloud transcription",
      "Your recordings leave your machine.",
      "Cloud mode is switched on under Dictation. The connection is set up here.",
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
    let autoSend = try XCTUnwrap(lanes.range(of: "Automatic send to the Agent"))
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
      "Pick a provider and a model separately for the Agent and for transcript formatting. API keys and accounts are set up under Providers.":
        "Wybierz dostawcę i model osobno dla Agenta oraz formatowania transkrypcji. Klucze API i konta skonfigurujesz w sekcji Dostawcy.",
      "Assistive": "Agent",
      "Formatting": "Formatowanie",
      "The model behind the Agent and the voice assistant":
        "Model obsługujący Agenta i asystenta głosowego",
      "Transcript cleanup and formatting": "Poprawianie i formatowanie transkrypcji",
      "Connected account": "Połączone konto",
      "Stored API key": "Zapisany klucz API",
      "Could not fetch %@ models. The API key was rejected. Check it under Providers.":
        "Nie udało się pobrać modeli %@. Klucz API został odrzucony. Sprawdź go w sekcji Dostawcy.",
      "Error details": "Szczegóły błędu",
      "Refresh": "Odśwież",
      "Reset model": "Przywróć model domyślny",
      "Active configuration details": "Szczegóły aktywnej konfiguracji",
      "%@ endpoint": "Adres API: %@",
      "Automatic send to the Agent": "Automatyczne wysyłanie do Agenta",
      "In Agent mode, send the untouched transcript after 5 seconds unless you start editing it.":
        "W trybie Agenta wyślij niezmienioną transkrypcję po 5 sekundach, jeśli nie rozpoczniesz jej edycji.",
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
  /// the project scan.
  func testWorkspaceTabNamesAgentAccessNotProjects() throws {
    let sources = try settingsSources()
    let tab = try XCTUnwrap(sources["SettingsTab.swift"])
    XCTAssertTrue(tab.contains("String(localized: \"Folders available to the Agent\""))
    XCTAssertFalse(tab.contains("\"Workspace roots.\""))
    XCTAssertTrue(
      tab.contains(
        "\"The Agent can read and write only inside these folders. It has no access outside them.\""
      ))

    let section = try XCTUnwrap(sources["WorkspaceRootsSection.swift"])
    XCTAssertTrue(section.contains("SettingsSectionLabel(String(localized: \"Allowed folders\"))"))
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
      "The Agent can read and write only inside these folders. It has no access outside them.":
        "Agent może odczytywać i zapisywać dane tylko w tych folderach. Poza nimi nie ma dostępu.",
      "Allowed folders": "Dozwolone foldery",
      "The Agent looks for projects and Git repositories in these folders. It also searches subfolders, but skips hidden folders and build directories.":
        "W tych folderach Agent szuka projektów i repozytoriów Git. Przeszukuje też podfoldery, ale pomija foldery ukryte i katalogi build.",
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
    XCTAssertTrue(panel.contains("Button(\"Restore default…\")"))
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
    let source = try XCTUnwrap(panel.range(of: "sourceLine\n"))
    let details = try XCTUnwrap(panel.range(of: "fileDetails\n"))
    XCTAssertLessThan(source.lowerBound, details.lowerBound, "the path sits under the source line")

    let polish = try polishCatalog()
    let expected: [String: String] = [
      "Prompts": "Prompty",
      "Browse and edit the base prompts. Codescribe may add further instructions to them while it runs.":
        "Przeglądaj i edytuj podstawowe prompty. Codescribe może dołączać do nich dodatkowe instrukcje podczas działania.",
      "Correction": "Korekta",
      "Correction prompt": "Prompt korekty",
      "Smart prompt": "Prompt Smart",
      "Max prompt": "Prompt Max",
      "Agent prompt": "Prompt Agenta",
      "Source: Built-in prompt": "Źródło: Wbudowany prompt",
      "Source: Custom prompt": "Źródło: Własny prompt",
      "File details": "Szczegóły pliku",
      "Restore default…": "Przywróć domyślny…",
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
    XCTAssertTrue(
      tab.contains(
        "\"Set when the Agent may use tools without asking, when it needs your approval, and when it must refuse.\""
      ))

    let section = try XCTUnwrap(sources["ToolPermissionsSection.swift"])
    XCTAssertFalse(
      section.contains("SettingsSectionLabel(String(localized: \"Tool permissions\"))"),
      "the tab headline already says it")
    XCTAssertFalse(section.contains("Defaults: read-only allow, side-effectful ask"))
    XCTAssertTrue(section.contains("defaultRow(title: \"Read data\""))
    XCTAssertTrue(section.contains("defaultRow(title: \"Changes, processes and network\""))
    XCTAssertTrue(section.contains("defaultRow(title: \"Unclassified tools\""))
    XCTAssertTrue(section.contains("\"Per-tool permissions · \\(model.toolCapabilities.count)\""))
    XCTAssertFalse(section.contains("Tool overrides"))
    XCTAssertTrue(section.contains("A rule set for one tool outranks its server's rule"))
    XCTAssertTrue(section.contains("Text(item.displayName)"))
    XCTAssertTrue(section.contains("Text(item.identity)"), "the raw identifier stays")
    XCTAssertTrue(section.contains("ToolPermissionLabels.risk(item.risk)"))
    XCTAssertTrue(section.contains("ToolPermissionLabels.ruleCaption(item.ruleSource)"))
    XCTAssertTrue(section.contains("if item.hasIndividualRule, let restoreInheritance {"))
    XCTAssertFalse(section.contains("Text(item.name)"), "the raw name is not the headline")

    let serverTab = try XCTUnwrap(sources["ToolServerTab.swift"])
    XCTAssertTrue(serverTab.contains("Text(ToolPermissionLabels.source(server))"))
    let search = try XCTUnwrap(sources["ToolSearchField.swift"])
    XCTAssertTrue(search.contains("Text(\"\\(serverCount) tool sources\""))
    XCTAssertFalse(search.contains("\\(serverCount) servers"))
    let browser = try XCTUnwrap(sources["ToolOverridesBrowser.swift"])
    XCTAssertTrue(browser.contains("model.clearToolPermission(identity: item.identity)"))

    let polish = try polishCatalog()
    let expected: [String: String] = [
      "Tool permissions": "Uprawnienia narzędzi",
      "Set when the Agent may use tools without asking, when it needs your approval, and when it must refuse.":
        "Ustaw, kiedy Agent może korzystać z narzędzi bez pytania, kiedy potrzebuje Twojej zgody, a kiedy ma odmówić wykonania działania.",
      "Deny": "Blokuj",
      "Read data": "Odczyt danych",
      "Changes, processes and network": "Zmiany, procesy i sieć",
      "Unclassified tools": "Niesklasyfikowane narzędzia",
      "Per-tool permissions · %lld": "Uprawnienia poszczególnych narzędzi · %lld",
      "Native": "Natywne",
      "Changes": "Zmiany",
      "Network": "Sieć",
      "Individual rule": "Własna reguła",
      "Inherited from the category default": "Dziedziczone z ustawienia kategorii",
      "Restore inheritance": "Przywróć dziedziczenie",
    ]
    for (key, value) in expected {
      XCTAssertEqual(polish[key], value, key)
    }
    for retired in [
      "Tool permissions.", "Allow, ask, or deny — per tool. Deny wins over everything.",
      "Tool overrides · %lld", "Read-only", "Side effects", "Global / unknown", "%lld servers",
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
    XCTAssertTrue(
      section.contains(
        "SettingsSectionLabel(String(localized: \"Detected installations and runtime\"))"))
    XCTAssertTrue(
      section.contains(
        "SettingsSectionLabel(String(localized: \"Available tools and integrations\"))"))
    XCTAssertTrue(section.contains("CapabilitySummary(rows: model.capabilityMatrix).line"))
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
      "Ready — %@ configured, access available, %@":
        "Gotowy — skonfigurowano %1$@, dostęp dostępny, %2$@",
      "Configured — agent not started yet": "Skonfigurowano — agent nie został jeszcze uruchomiony",
      "Not configured (optional)": "Nieskonfigurowane (opcjonalne)",
      "Detected installations and runtime": "Wykryte instalacje i runtime",
      "Technical details": "Szczegóły techniczne",
      "Available tools and integrations": "Dostępne narzędzia i integracje",
      "Built-in Codescribe tool": "Wbudowane narzędzie Codescribe",
      "tool: %@ · source: %@": "narzędzie: %1$@ · źródło: %2$@",
      "capability.tier.native": "Natywne",
      "capability.tier.enhanced": "Rozszerzone",
      "Configuration source:": "Źródło konfiguracji:",
      "Configured: %lld · Tested: %lld · Issues: %lld":
        "Skonfigurowane: %1$lld · Przetestowane: %2$lld · Problemy: %3$lld",
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
      "%lld configured", "not tested", "testing…", "fail: %@",
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
      section.contains("guard addError == nil else { return }"),
      "a failed add keeps the typed fields")

    let polish = try polishCatalog()
    let expected: [String: String] = [
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
    ] {
      XCTAssertNil(polish[retired], "retired key still in the catalog: \(retired)")
    }
  }

  /// The Audio pane reads as "microphone and recording": a headline without a
  /// trailing period, no storage-format or Core Audio vocabulary in the basic
  /// descriptions, a retention sentence that follows the selected choice, and
  /// the measured profile collapsed under a details disclosure.
  /// The Polish values for the new keys are imported through the sheet tool;
  /// this test pins the source structure, which does not depend on that import.
  func testAudioPaneReadsAsMicrophoneAndRecording() throws {
    let panel = try XCTUnwrap(try settingsSources()["AudioPanel.swift"])
    XCTAssertTrue(panel.contains("String(localized: \"Microphone and recording\")"))
    XCTAssertFalse(panel.contains("Microphone and recording."), "the headline takes no period")
    XCTAssertFalse(panel.contains("Hear the real input"))
    XCTAssertTrue(
      panel.contains(
        "\"Choose a microphone, check that recording is ready and adjust the sound settings.\""))
    XCTAssertTrue(panel.contains("Button(\"Refresh microphones\")"))
    XCTAssertTrue(panel.contains("Button(\"Use the system microphone\")"))
    XCTAssertFalse(panel.contains("Button(\"Use system default\")"))
    XCTAssertFalse(
      panel.contains("Restores the system default microphone."),
      "the button no longer repeats itself in a sentence")

    // Point 4: the basic descriptions name the microphone, not the storage
    // format or the macOS audio framework.
    XCTAssertFalse(panel.contains("settings.json"))
    XCTAssertFalse(panel.contains("Core Audio"))

    XCTAssertTrue(panel.contains("Text(audioRetentionDetail(model.audioRetention))"))
    XCTAssertFalse(
      panel.contains("Off discards the audio of new takes once processing finishes"),
      "the retention sentence is no longer unconditional")
    XCTAssertTrue(panel.contains("func audioRetentionDetail(_ choice: String) -> String"))

    XCTAssertTrue(panel.contains("String(localized: \"Recording start signal\")"))
    XCTAssertFalse(panel.contains("String(localized: \"Start sound\")"))
    XCTAssertTrue(panel.contains("Slider(value: soundVolumeBinding, in: 0...1, step: 0.05)"))
    XCTAssertTrue(panel.contains("Toggle(\"\", isOn: soundFeedbackBinding)"))

    // Point 6: one name for the committing row, the profile id behind a
    // disclosure, and the stored measurement read straight from the verdict.
    XCTAssertTrue(panel.contains("String(localized: \"Committing transcript fragments\")"))
    XCTAssertFalse(panel.contains("\"Seal lane armed\""))
    XCTAssertFalse(panel.contains("\"Seal lane must be enabled\""))
    XCTAssertFalse(panel.contains("\"Seal check waits for microphone access\""))
    XCTAssertTrue(panel.contains("@State private var showingCalibrationDetails = false"))
    XCTAssertTrue(panel.contains("DisclosureGroup(isExpanded: $showingCalibrationDetails)"))
    XCTAssertTrue(
      panel.contains("SettingsSectionLabel(String(localized: \"Calibration details\"))"))
    XCTAssertTrue(
      panel.contains(
        "func audioCalibrationDetails(_ readiness: CsAdmissionReadiness?) -> [AudioCalibrationDetail]"
      ))
    XCTAssertTrue(panel.contains("func recordingStartHint(_ dictationShortcut: String?) -> String"))
    XCTAssertFalse(panel.contains("Use \\(dictationShortcut) or choose Start recording."))
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
