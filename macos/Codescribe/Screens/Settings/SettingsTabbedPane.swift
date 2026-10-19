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
        .padding(.horizontal, SettingsScale.pageGutter)
        .padding(.vertical, SettingsScale.tabBarVertical)
        .frame(maxWidth: .infinity, alignment: .leading)
      Divider()

      ScrollView {
        VStack(alignment: .leading, spacing: 0) {
          if let tab = model.currentTab {
            SettingsPageHeader(tab.headline, blurb: tab.blurb)
          }

          content
            .padding(.top, SettingsScale.sectionGap)
        }
        .settingsPage()
        .frame(maxWidth: .infinity, alignment: .leading)
        // `.scrollIndicators(.hidden)` stood on the scroll view and could not
        // keep its promise with a mouse attached (Founder, 2026-10-18).
        .csAutohidingScrollers()
      }
      .scrollContentBackground(.hidden)
      // A new identity per tab resets the scroll offset, but it also makes the
      // switch an insert-and-remove, which SwiftUI crossfades by default and
      // paints both tabs over each other for a frame or two. Swap outright.
      .transition(.identity)
      .id(model.currentTab)
    }
  }
}
