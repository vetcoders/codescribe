import AppKit
import SwiftUI

/// The native segmented control owns only its selection cursor. Clicks become
/// navigation requests; search and deep links project the model back here.
///
/// The bar may never widen the pane. `SettingsTabbedPane` stacks it above the
/// page scroll view, so a bar that asks for more width than the detail column
/// has pushes the whole pane — tab bar and page alike — past the window and
/// clips it on both sides. Hence the two variants: the control at its own hug
/// width while the column can afford it, the same control inside a horizontal
/// scroll view once it cannot. The scrolling variant is width-flexible, so it
/// always fits and the pane keeps the column width in every language.
struct SettingsTabBar: View {
  @ObservedObject var model: SettingsViewModel
  let section: SettingsSection

  var body: some View {
    ViewThatFits(in: .horizontal) {
      bar
      ScrollView(.horizontal, showsIndicators: false) { bar }
    }
    .controlSize(.regular)
    .accessibilityIdentifier("settings-tabs-\(section.rawValue)")
  }

  private var bar: some View {
    SettingsTabSegments(model: model, section: section)
  }
}

/// One native `NSSegmentedControl` per tabbed pane, sized by its own labels.
/// `.fit` distribution lets every segment hug its title instead of all six
/// taking the widest title's width: in Polish "Tryb bezdotykowy" alone would
/// otherwise set a width no 880 pt window can show. The control is always
/// reported at its fitting size — fit or scroll is decided in SwiftUI, never
/// by squeezing the control.
private struct SettingsTabSegments: NSViewRepresentable {
  @ObservedObject var model: SettingsViewModel
  let section: SettingsSection

  private var tabs: [SettingsTab] { SettingsTab.tabs(in: section) }

  private var accessibilityName: String {
    String(localized: "\(section.title) tabs")
  }

  func makeCoordinator() -> Coordinator { Coordinator(parent: self) }

  func makeNSView(context: Context) -> NSSegmentedControl {
    let control = NSSegmentedControl()
    control.segmentStyle = .automatic
    control.trackingMode = .selectOne
    control.controlSize = .regular
    control.segmentDistribution = .fit
    // SwiftUI sizes the control from sizeThatFits; these priorities only keep
    // AppKit from stretching or squeezing it on its own.
    control.setContentHuggingPriority(.defaultHigh, for: .horizontal)
    control.setContentCompressionResistancePriority(.required, for: .horizontal)
    control.setAccessibilityLabel(accessibilityName)
    control.target = context.coordinator
    control.action = #selector(Coordinator.segmentPicked(_:))
    apply(titles: tabs.map(\.title), to: control)
    return control
  }

  func updateNSView(_ control: NSSegmentedControl, context: Context) {
    context.coordinator.parent = self
    // Titles and the label change when the interface language does; the
    // control itself survives that without being rebuilt.
    apply(titles: tabs.map(\.title), to: control)
    control.setAccessibilityLabel(accessibilityName)
    // Projection only — the model is never mutated during a view update. A
    // current tab from another section leaves this bar with no selection.
    let selected = model.currentTab.flatMap { tabs.firstIndex(of: $0) } ?? -1
    guard control.selectedSegment != selected else { return }
    if selected >= 0 {
      control.setSelected(true, forSegment: selected)
    } else {
      control.selectedSegment = -1
    }
  }

  func sizeThatFits(
    _ proposal: ProposedViewSize, nsView: NSSegmentedControl, context: Context
  ) -> CGSize? {
    nsView.fittingSize
  }

  /// Writes labels only where they differ, so a redraw does not reset segments
  /// that already read correctly.
  private func apply(titles: [String], to control: NSSegmentedControl) {
    if control.segmentCount != titles.count {
      control.segmentCount = titles.count
    }
    for (index, title) in titles.enumerated() where control.label(forSegment: index) != title {
      control.setLabel(title, forSegment: index)
    }
  }

  @MainActor
  final class Coordinator: NSObject {
    var parent: SettingsTabSegments

    init(parent: SettingsTabSegments) {
      self.parent = parent
    }

    @objc func segmentPicked(_ sender: NSSegmentedControl) {
      let tabs = parent.tabs
      guard tabs.indices.contains(sender.selectedSegment) else { return }
      let tab = tabs[sender.selectedSegment]
      guard tab.section == parent.section, tab != parent.model.currentTab else { return }
      parent.model.select(tab)
    }
  }
}
