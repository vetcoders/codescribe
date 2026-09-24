import SwiftUI

/// Segmented tab bar for a tabbed settings pane. Binds to the model's
/// `currentTab` by key path, so a click routes through `select(_:)` exactly
/// like a sidebar row, and search or deep links drive the same selection.
struct SettingsTabBar: View {
  @ObservedObject var model: SettingsViewModel
  let section: SettingsSection
  private let tabs: [SettingsTab]

  init(model: SettingsViewModel, section: SettingsSection) {
    self.model = model
    self.section = section
    self.tabs = SettingsTab.tabs(in: section)
  }

  /// Hugs its labels when they fit; otherwise compresses (segments truncate)
  /// rather than forcing the split view wider than the window.
  var body: some View {
    ViewThatFits(in: .horizontal) {
      bar.fixedSize()
      bar
    }
    .accessibilityIdentifier("settings-tabs-\(section.rawValue)")
  }

  private var bar: some View {
    Picker("\(section.title) tabs", selection: $model.currentTab) {
      ForEach(tabs) { tab in
        Text(tab.title).tag(Optional(tab))
      }
    }
    .pickerStyle(.segmented)
    .labelsHidden()
  }
}
