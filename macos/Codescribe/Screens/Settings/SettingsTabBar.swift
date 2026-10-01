import SwiftUI

/// The native picker owns only its selection cursor. Changes become navigation
/// requests in onChange; search and deep links project the model back here.
struct SettingsTabBar: View {
  @ObservedObject var model: SettingsViewModel
  let section: SettingsSection
  private let tabs: [SettingsTab]
  @State private var selection: SettingsTab?

  init(model: SettingsViewModel, section: SettingsSection) {
    self.model = model
    self.section = section
    self.tabs = SettingsTab.tabs(in: section)
    _selection = State(initialValue: model.currentTab)
  }

  /// Hugs its labels when they fit; otherwise compresses (segments truncate)
  /// rather than forcing the split view wider than the window.
  var body: some View {
    ViewThatFits(in: .horizontal) {
      bar.fixedSize()
      bar
    }
    .accessibilityIdentifier("settings-tabs-\(section.rawValue)")
    .onChange(of: selection) { _, tab in
      guard let tab, tab.section == section, tab != model.currentTab else { return }
      model.select(tab)
    }
    .onChange(of: model.currentTab) { _, tab in
      selection = tab
    }
  }

  private var bar: some View {
    Picker("\(section.title) tabs", selection: $selection) {
      ForEach(tabs) { tab in
        Text(tab.title).tag(Optional(tab))
      }
    }
    .pickerStyle(.segmented)
    .controlSize(.regular)
    .labelsHidden()
  }
}
