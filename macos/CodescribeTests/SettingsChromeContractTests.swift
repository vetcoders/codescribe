import XCTest

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
    XCTAssertTrue(tabBar.contains(".pickerStyle(.segmented)"))
    XCTAssertTrue(tabBar.contains(".controlSize(.regular)"))

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

  func testAvailabilityTintsUseSolidTerracotta() throws {
    let model = try XCTUnwrap(settingsSources()["SettingsViewModel.swift"])
    XCTAssertEqual(
      model.components(separatedBy: "? CSColor.oliveLight : CSColor.terracotta").count, 3)
    XCTAssertTrue(model.contains("static func availabilityTint(for provider: CsProviderOption)"))
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
