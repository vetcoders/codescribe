import SwiftUI

/// The one grammar for every settings pane that outgrew a single scroll: the
/// section eyebrow, a segmented tab bar, then the selected tab's headline,
/// blurb and content — one tab at a time. It replaces both the stacked
/// collapsibles and the child rows the sidebar used to grow per pane.
struct SettingsTabbedPane<Content: View>: View {
  @ObservedObject var model: SettingsViewModel
  let section: SettingsSection
  @ViewBuilder let content: Content

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      EyebrowLabel(text: "Settings · \(section.title)")

      SettingsTabBar(model: model, section: section)
        .padding(.top, CSSpace.md)

      if let tab = model.currentTab {
        Text(tab.headline)
          .font(CSFont.ui(26, .bold))
          .tracking(-0.5)
          .foregroundStyle(CSColor.textHigh)
          .padding(.top, CSSpace.lg)

        Text(tab.blurb)
          .font(CSFont.ui(12.5))
          .lineSpacing(2)
          .foregroundStyle(CSColor.textMutedAlt)
          .padding(.top, CSSpace.sm)
      }

      content
        .padding(.top, CSSpace.lg)
    }
    .padding(.horizontal, CSSpace.xl)
    .padding(.vertical, CSSpace.section)
  }
}
