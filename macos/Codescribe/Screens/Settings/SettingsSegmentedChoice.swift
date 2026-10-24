import AppKit
import SwiftUI

/// One native segmented control for a settings row with a handful of named
/// choices (interface language, transcription language, appearance). It is
/// the same `NSSegmentedControl` shape the tab bar draws, with `.fit`
/// distribution, so each segment hugs its own label: "Multilingual · Polish ·
/// English" no longer takes three times the widest word, and the three rows
/// in Settings › Creator read as one control family (Founder, 2026-10-21).
///
/// Projection only: the control shows `selection` and reports a pick through
/// the binding; it never mutates the model during a view update.
struct SettingsSegmentedChoice: NSViewRepresentable {
  let titles: [String]
  @Binding var selection: Int
  var isEnabled = true
  var accessibilityLabel: String
  var accessibilityIdentifier: String

  func makeCoordinator() -> Coordinator { Coordinator(parent: self) }

  func makeNSView(context: Context) -> NSSegmentedControl {
    let control = SettingsTabSegments.control(titles: titles)
    control.target = context.coordinator
    control.action = #selector(Coordinator.segmentPicked(_:))
    return control
  }

  func updateNSView(_ control: NSSegmentedControl, context: Context) {
    context.coordinator.parent = self
    SettingsTabSegments.apply(titles: titles, to: control)
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
    nsView.fittingSize
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
