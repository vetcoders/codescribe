import SwiftUI

/// The one scale of the Settings central pane.
///
/// `CSSpace`, `CSRadius` and `CSFont` are shared with the overlay, the tray and
/// the Agent window, so a desktop-sized Settings pane cannot be reached by
/// editing their values — it would shrink those surfaces too. This enum is the
/// Settings pane's own rung of that scale: it names each role once and resolves
/// it to a shared token wherever a shared token already carries the value, so
/// there is no second rhythm to keep in sync. The sidebar is not part of it;
/// `CSSidebarColumn` owns the navigation column.
///
/// Two rules hold the pane together and every value below follows from them:
///
/// 1. **Headers sit above the cards, setting names sit inside them.** A section
///    title is never smaller or lighter than the settings underneath it:
///    `pageTitle` > `sectionTitle` ≥ `settingName` > `helper`, all at
///    primary strength except the helper rung.
/// 2. **One control family.** Every flat Settings action is a
///    `SettingsChipButton`: one typeface rung, one pair of insets, one shape
///    that its focus ring can follow.
enum SettingsScale {

  // MARK: - Page frame

  /// Left and right gutter of the central pane, tabbed or not.
  static let pageGutter: CGFloat = CSSpace.section
  /// Top and bottom inset of a page inside its scroll view.
  static let pageVertical: CGFloat = CSSpace.lg
  /// Above and below the pinned tab strip of a tabbed pane.
  static let tabBarVertical: CGFloat = CSSpace.sm

  // MARK: - Section rhythm

  /// From a section's last card to the next section's title.
  static let sectionGap: CGFloat = CSSpace.lg
  /// From a section title to the card it introduces — deliberately a third of
  /// `sectionGap`, so a title belongs to what follows it and not to the card
  /// above it. The title grew a rung, this gap gave the rung back.
  static let headerGap: CGFloat = CSSpace.xs
  /// Between a page title and its blurb.
  static let titleGap: CGFloat = CSSpace.xs

  // MARK: - Card

  /// Inset between a card's edge and its content.
  static let cardPadding: CGFloat = CSSpace.md
  /// A card's corner. Shared with the overlay's cards on purpose: the radius
  /// is the product's shape language, not a density knob.
  static let cardRadius: CGFloat = CSRadius.card
  /// Between two stacked settings inside one card.
  static let rowGap: CGFloat = CSSpace.sm
  /// Horizontal inset of a full-bleed row inside a card (`RuntimeRow`).
  static let rowInsetH: CGFloat = CSSpace.md
  /// Vertical inset of a full-bleed row inside a card.
  static let rowInsetV: CGFloat = CSSpace.control
  /// Key column of a key/value row, sized for the `metaKey` rung.
  static let rowKeyWidth: CGFloat = 144

  // MARK: - Controls

  /// A chip's corner. `CSRadius.chip` is also the default corner of
  /// `csFocusRing()`, so the keyboard outline follows the chip's own shape.
  static let controlRadius: CGFloat = CSRadius.chip
  static let controlInsetH: CGFloat = 10
  static let controlInsetV: CGFloat = 5
  /// The longest a slider track grows. A track that spans the whole pane
  /// turns every pixel of width into precision nobody asked for (Founder,
  /// 2026-10-20); the value sits at the track's end instead of the far edge.
  static let sliderWidth: CGFloat = 360

  // MARK: - Type

  /// The pane's one headline, above everything a page contains.
  static let pageTitle = CSFont.ui(16, .semibold)
  /// The sentence under the page title.
  static let pageBlurb = CSFont.ui(12)
  /// A peer section's title, above its cards.
  static let sectionTitle = CSFont.ui(13.5, .semibold)
  /// The name of one setting, inside a card.
  static let settingName = CSFont.ui(12.5, .semibold)
  /// The current value of one setting, inside a card.
  static let settingValue = CSFont.ui(12.5, .semibold)
  /// A value that is a path, an identifier, a version or a shortcut.
  static let monoValue = CSFont.mono(12, .semibold)
  /// Secondary text: what a setting does, under its name.
  static let helper = CSFont.ui(11.5)
  /// The key column of a runtime-truth row.
  static let metaKey = CSFont.mono(11, .medium)
  /// The label of a chip or any other flat Settings action.
  static let controlLabel = CSFont.ui(11.5, .semibold)
  /// A one-word state beside a row.
  static let statusBadge = CSFont.ui(10, .semibold)
}

extension View {
  /// The central pane's page frame: one gutter and one top/bottom inset for
  /// every panel, tabbed or not.
  func settingsPage() -> some View {
    self
      .padding(.horizontal, SettingsScale.pageGutter)
      .padding(.vertical, SettingsScale.pageVertical)
  }
}
