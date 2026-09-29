import SwiftUI

/// Compact window-position affordance. It exposes the existing six anchors and
/// free-motion mode without reviving the old inline Picker/dropdown treatment.
@MainActor
struct OverlayPlacementMenu: View {
  @Bindable var state: OverlayState
  let palette: OverlayAppearancePalette

  var body: some View {
    Menu {
      if let error = state.expansionPreferenceError {
        Text(error)
        Divider()
      }
      Section("Anchor") {
        ForEach(OverlayAnchor.allCases) { anchor in
          Button {
            state.selectPlacementAnchor(anchor)
          } label: {
            Label(anchor.label, systemImage: menuImage(for: anchor))
          }
        }
      }

      Divider()

      Button {
        state.selectFreeMotion()
      } label: {
        Label(
          "Free motion",
          systemImage: state.freeMotion
            ? "checkmark"
            : "arrow.up.and.down.and.arrow.left.and.right"
        )
      }
      Divider()
      Toggle(
        "Show transcript by default",
        isOn: Binding(
          get: { state.expandedByDefault },
          set: { state.setExpandedByDefault($0) }
        )
      )
      .help("New recordings open with the transcript. Turn off to show only the recording bar.")
      .accessibilityIdentifier("overlay-expanded-by-default")
      Toggle(
        "Keep visible between takes",
        isOn: Binding(
          get: { state.keepVisibleBetweenTakes },
          set: { state.setKeepVisibleBetweenTakes($0) }
        )
      )
      .help("Keep the overlay open after a take until you close it")
      .accessibilityIdentifier("overlay-keep-visible-between-takes")
    } label: {
      ZStack(alignment: .bottomTrailing) {
        Image(systemName: OverlayControlSymbols.placement)
        if state.keepVisibleBetweenTakes {
          Image(systemName: "pin.fill")
            .font(.system(size: 7, weight: .bold))
            .offset(x: 5, y: 4)
            .accessibilityHidden(true)
        }
      }
      .font(.system(size: 11, weight: .semibold))
      .foregroundStyle(palette.mutedText.color)
      .frame(width: 24, height: 24)
      .contentShape(Rectangle())
    }
    .menuStyle(.button)
    .buttonStyle(.plain)
    .menuIndicator(.hidden)
    .fixedSize()
    .help("Position overlay")
    .accessibilityLabel("Position overlay")
    .accessibilityValue(
      (state.keepVisibleBetweenTakes ? "Pinned, " : "")
        + (state.freeMotion ? "Free motion" : state.placementAnchor.label)
    )
    .accessibilityHint("Choose a screen anchor or allow free dragging")
    .accessibilityIdentifier("overlay-placement-menu")
  }

  private func menuImage(for anchor: OverlayAnchor) -> String {
    guard !state.freeMotion, state.placementAnchor == anchor else {
      return anchor.systemImage
    }
    return "checkmark"
  }
}
