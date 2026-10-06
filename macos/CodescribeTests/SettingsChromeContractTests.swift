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
      XCTAssertEqual(english.count, 6, "\(section.rawValue)")
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
      "Connect accounts or add API keys. Models are chosen under Agent › LLM lanes.",
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
