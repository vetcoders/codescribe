import AppKit
import SwiftUI

/// Marks of the header's two content controls. The agent roster is one
/// Dingbats character, the ✦ of the roster's ❖ family, which every macOS font
/// cascade covers (Zapf Dingbats, Menlo). The transcription is the system
/// `note.text` symbol from the icon catalog (`CSIcon.notesMode`), one mark in
/// every form of the bar; the hand and the pen it replaces were one function.
enum OverlayHeaderGlyph {
  /// ✦ the agent roster.
  static let agents = "\u{2726}"
  /// The transcription's system symbol.
  static let transcriptionSymbol = CSIcon.notesMode.systemName

  /// Size inside the 22 pt header circle at which the ✦ carries the visual
  /// weight of the 9 pt semibold symbols beside it.
  static let pointSize: CGFloat = 11

  /// Menus take images, not text runs: the same mark as a template image, so
  /// the secondary-click menu shows what the header button shows.
  @MainActor
  static func menuImage(_ glyph: String) -> NSImage {
    let size = (glyph as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: 13)])
    let image = NSImage(
      size: NSSize(width: ceil(size.width), height: ceil(size.height)), flipped: false
    ) { _ in
      (glyph as NSString).draw(
        at: .zero, withAttributes: [.font: NSFont.systemFont(ofSize: 13)])
      return true
    }
    image.isTemplate = true
    return image
  }
}

/// The transcription control's mark. Chosen, it is a window state like the
/// pin: primary ink on the chrome's neutral wash (`OverlayHeaderAccent.state`),
/// never the listening terracotta; at rest the muted ink on its plate.
struct OverlayTranscriptionGlyph: View {
  let selected: Bool
  let palette: OverlayAppearancePalette

  var body: some View {
    OverlayMicrophoneGlyph(
      symbol: OverlayHeaderGlyph.transcriptionSymbol,
      tint: selected
        ? OverlayHeaderAccent.state.selectionTint(in: palette) : palette.mutedText.color,
      plateOpacity: selected ? 0 : 0.12)
  }
}

/// The header's secondary-click menu, shared by every header form (mini, midi,
/// expanded): the agent roster, the transcription, the six anchors with
/// free motion, the take-start preference, and how long an expansion stays
/// open. The visible header keeps only Agents, Transcription and the explicit Pin.
@MainActor
struct OverlayHeaderMenu: View {
  @Bindable var state: OverlayState

  var body: some View {
    if let error = state.expansionPreferenceError {
      Text(error)
      Divider()
    }
    Button {
      state.showAgentMonitor()
    } label: {
      Label {
        Text("Agents")
      } icon: {
        Image(nsImage: OverlayHeaderGlyph.menuImage(OverlayHeaderGlyph.agents))
      }
    }
    Button(action: state.showTranscription) {
      Label {
        Text("Transcription")
      } icon: {
        Image(systemName: OverlayHeaderGlyph.transcriptionSymbol)
      }
    }

    Divider()

    Section {
      ForEach(OverlayAnchor.allCases) { anchor in
        Button {
          state.selectPlacementAnchor(anchor)
        } label: {
          Label(anchor.label, systemImage: menuImage(for: anchor))
        }
      }
    } header: {
      Text(
        "Anchor",
        comment: "Overlay placement menu: header above the screen positions the overlay snaps to")
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
    // Native menu states draw the selected interval's checkmark. A Label's
    // decorative image is not the menu item's selected state.
    Menu {
      ForEach(Self.autoFoldIntervals, id: \.self) { seconds in
        Toggle(
          "\(Int(seconds)) s",
          isOn: Binding(
            get: { state.autoFoldSeconds == seconds },
            set: { if $0 { state.setAutoFoldSeconds(seconds) } })
        )
        .accessibilityIdentifier("overlay-auto-fold-\(seconds)")
      }
      Toggle(
        "Never",
        isOn: Binding(
          get: { state.autoFoldSeconds == 0 },
          set: { if $0 { state.setAutoFoldSeconds(0) } })
      )
      .accessibilityIdentifier("overlay-auto-fold-never")
    } label: {
      currentAutoFoldTitle
    }
  }

  private static let autoFoldIntervals: [UInt32] = [5, 10, 30, 60]

  private var currentAutoFoldTitle: Text {
    state.autoFoldSeconds == 0
      ? Text("Auto-fold: Never")
      : Text("Auto-fold: \(Int(state.autoFoldSeconds)) s of inactivity")
  }

  private func menuImage(for anchor: OverlayAnchor) -> String {
    guard !state.freeMotion, state.placementAnchor == anchor else {
      return anchor.systemImage
    }
    return "checkmark"
  }
}

/// The header's one window control: an explicit Pin / Unpin. Pinned is the
/// filled accent glyph on the chrome's pastel accent wash (Founder,
/// 2026-10-15: the three controls left of the microphone were faint circles).
/// Unpinning a resting panel lets it go after
/// `OverlayState.pinReleaseDelaySeconds` with a fade, never at the click.
@MainActor
struct OverlayPinButton: View {
  @Bindable var state: OverlayState
  let palette: OverlayAppearancePalette

  private var pinned: Bool { state.keepVisibleBetweenTakes }

  var body: some View {
    Button {
      state.setKeepVisibleBetweenTakes(!pinned)
    } label: {
      OverlayMicrophoneGlyph(
        symbol: pinned ? OverlayControlSymbols.pinned : OverlayControlSymbols.pin,
        // A window state wears no colour (Founder, 2026-11-05): the pinned
        // glyph is primary ink on the neutral wash of its chrome.
        tint: pinned ? palette.primaryText.color : palette.mutedText.color,
        plateOpacity: 0
      )
      .contentShape(Circle())
    }
    .buttonStyle(.plain)
    .csFocusOutline()
    .fixedSize()
    .modifier(
      OverlayHeaderControlChrome(title: help, accent: .state, palette: palette, selected: pinned)
    )
    .accessibilityLabel("Keep visible between takes")
    .accessibilityValue(
      pinned
        ? String(localized: "On", comment: "Toggle state")
        : String(localized: "Off", comment: "Toggle state")
    )
    .accessibilityAddTraits(.isToggle)
    .accessibilityAddTraits(pinned ? .isSelected : [])
    .accessibilityIdentifier("overlay-pin")
  }

  private var help: String {
    pinned
      ? String(
        localized: "Unpin overlay",
        comment: "Tooltip on the overlay pin while it keeps the overlay between takes")
      : String(
        localized: "Pin overlay",
        comment: "Tooltip on the overlay pin while the overlay leaves after each take")
  }
}
