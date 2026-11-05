import AppKit
import SwiftUI

/// One native segmented control for a settings row with a handful of named
/// choices (interface language, transcription language, appearance). It is
/// the same `NSSegmentedControl` shape the tab bar draws. Ordinary rows use
/// `.fit` distribution, so each segment hugs its label. Rows that name a
/// matching peer share the wider native fitting width and fill it in
/// proportion to their labels, without a fixed pixel width or clipped text.
///
/// Projection only: the control shows `selection` and reports a pick through
/// the binding; it never mutates the model during a view update.
struct SettingsSegmentedChoice: NSViewRepresentable {
  let titles: [String]
  @Binding var selection: Int
  /// Localized labels of a peer control whose overall width this row matches.
  /// Both peers pass each other's titles, so neither can clip the wider set.
  var matchingTitles: [String]? = nil
  var isEnabled = true
  var accessibilityLabel: String
  var accessibilityIdentifier: String

  func makeCoordinator() -> Coordinator { Coordinator(parent: self) }

  func makeNSView(context: Context) -> NSSegmentedControl {
    let control = SettingsTabSegments.control(titles: titles)
    control.segmentDistribution = matchingTitles == nil ? .fit : .fillProportionally
    control.target = context.coordinator
    control.action = #selector(Coordinator.segmentPicked(_:))
    return control
  }

  func updateNSView(_ control: NSSegmentedControl, context: Context) {
    context.coordinator.parent = self
    SettingsTabSegments.apply(titles: titles, to: control)
    let distribution: NSSegmentedControl.Distribution =
      matchingTitles == nil ? .fit : .fillProportionally
    if control.segmentDistribution != distribution { control.segmentDistribution = distribution }
    control.isEnabled = isEnabled
    control.setAccessibilityLabel(accessibilityLabel)
    control.setAccessibilityIdentifier(accessibilityIdentifier)
    if titles.indices.contains(selection) {
      if control.selectedSegment != selection {
        control.setSelected(true, forSegment: selection)
      }
    } else if control.selectedSegment != -1 {
      control.selectedSegment = -1
    }
  }

  func sizeThatFits(
    _ proposal: ProposedViewSize, nsView: NSSegmentedControl, context: Context
  ) -> CGSize? {
    var size = nsView.fittingSize
    if let matchingTitles {
      let peer = SettingsTabSegments.control(titles: matchingTitles)
      size.width = max(size.width, peer.fittingSize.width)
    }
    return size
  }

  @MainActor
  final class Coordinator: NSObject {
    var parent: SettingsSegmentedChoice

    init(parent: SettingsSegmentedChoice) {
      self.parent = parent
    }

    @objc func segmentPicked(_ sender: NSSegmentedControl) {
      let picked = sender.selectedSegment
      guard parent.titles.indices.contains(picked), picked != parent.selection else { return }
      parent.selection = picked
    }
  }
}
