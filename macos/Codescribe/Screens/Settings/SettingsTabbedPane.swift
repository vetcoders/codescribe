import SwiftUI

/// The one grammar for every settings pane that outgrew a single scroll: a
/// pinned segmented tab bar, then the selected tab's headline, blurb and
/// content. The sidebar already names the section, so the pane does not
/// repeat it.
struct SettingsTabbedPane<Content: View>: View {
  @ObservedObject var model: SettingsViewModel
  let section: SettingsSection
  @ViewBuilder let content: Content

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsTabBar(model: model, section: section)
        .controlSize(.regular)
        .padding(.horizontal, CSSpace.xl)
        .padding(.vertical, CSSpace.md)
        .frame(maxWidth: .infinity, alignment: .leading)
      Divider()

      ScrollView {
        VStack(alignment: .leading, spacing: 0) {
          if let tab = model.currentTab {
            SettingsPageHeader(tab.headline, blurb: tab.blurb)
          }

          content
            .padding(.top, CSSpace.lg)
        }
        .padding(.horizontal, CSSpace.xl)
        .padding(.vertical, CSSpace.section)
        .frame(maxWidth: .infinity, alignment: .leading)
      }
      .scrollContentBackground(.hidden)
      .id(model.currentTab)
    }
  }
}
