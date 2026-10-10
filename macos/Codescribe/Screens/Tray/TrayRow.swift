import SwiftUI

// Row primitives for the tray dropdown. Top-level geometry is taken straight
// from the mock: rows are 9×12 padded, 9pt-radius, 11pt icon→label gap, 18pt
// icon column. Rows nested inside an expanded disclosure take the denser
// `TrayRowScale.child` numbers below.

/// The one tray tint that is not a fixed hex: the primary-row keycap follows
/// the system accent. Secondary text uses the shared `CSColor`
/// ramp (`textMuted` for child rows) so the tray cannot
/// drift off the locked palette.
private enum TrayLocal {
  static var primaryShortcut: Color { CSColor.chromeAccent.opacity(0.78) }
}

enum TrayRowStyle {
  case plain  // transparent; subtle hover highlight
  case primary  // system accent tint + border (the ONE primary action)
  case raised  // surface-raised tint (an expanded disclosure parent)
}

/// Vertical rhythm and type scale of a tray row.
///
/// Founder brief, round 17, 2026-10-10: a top-level section must visually
/// dominate its own contents, so the collapsed tray keeps its geometry
/// untouched (`top`) while every row living inside an expanded disclosure
/// drops onto one denser, calmer scale (`child`). Click areas stay above the
/// 22pt the menu bar expects: 12pt text plus 5pt padding is a 25pt row.
enum TrayRowScale {
  /// The collapsed tray's own scale. Never densified.
  case top
  /// Rows nested under an expanded disclosure parent.
  case child

  var titleSize: CGFloat { self == .top ? 13 : 12 }
  /// Label colour. Weight stays `.medium` on both scales: the hierarchy comes
  /// from size and colour, so a nested row reads as legible as the
  /// `TrayChildRow` beside it.
  var titleColor: Color { self == .top ? CSColor.textBodyAlt : CSColor.textMuted }
  var iconSize: CGFloat { self == .top ? 13 : 12 }
  /// Fixed icon column: labels align whatever glyph the row carries.
  var iconColumn: CGFloat { self == .top ? 18 : 16 }
  var iconGap: CGFloat { self == .top ? 11 : 9 }
  var keycapSize: CGFloat { self == .top ? 10 : 9 }
  var chevronSize: CGFloat { self == .top ? 11 : 10 }
  var horizontalPadding: CGFloat { self == .top ? 12 : 11 }
  var verticalPadding: CGFloat { self == .top ? 9 : 5 }
  var cornerRadius: CGFloat { self == .top ? CSRadius.input : CSRadius.chip }
}

/// The one disclosure idiom shared by every expandable tray row: a single
/// glyph (`chevron.right`) pointing right when collapsed, rotated to point
/// down when expanded — the standard macOS disclosure gesture.
enum TrayDisclosureChevron {
  static let icon: CSIcon = .chevronRight
  static let animation = Animation.easeOut(duration: 0.18)
  static func rotationDegrees(expanded: Bool) -> Double { expanded ? 90 : 0 }
}

/// A standard tray action row: icon · label · optional shortcut / chevron.
struct TrayRow: View {
  let icon: CSIcon
  var iconColor: Color? = nil
  let title: String
  /// `nil` takes the scale's own label colour, so a nested row reads calmer
  /// than the section heading it sits under.
  var titleColor: Color? = nil
  var titleWeight: Font.Weight = .medium
  var shortcut: String? = nil
  var shortcutColor: Color = CSColor.textFaintAlt
  /// Expansion state of the disclosure group this row heads; `nil` for plain
  /// action rows without a chevron.
  var disclosureExpanded: Bool? = nil
  var style: TrayRowStyle = .plain
  var scale: TrayRowScale = .top
  var action: () -> Void = {}

  @State private var hovering = false

  private var resolvedTitleColor: Color { titleColor ?? scale.titleColor }

  private var fillColor: Color {
    switch style {
    case .primary: return CSColor.accentWash.opacity(0.13)
    case .raised: return CSColor.surfaceRaised(0.04)
    case .plain: return hovering ? CSColor.surfaceRaised(0.05) : .clear
    }
  }

  private var borderColor: Color {
    style == .primary ? CSColor.chromeAccent.opacity(0.24) : .clear
  }

  var body: some View {
    Button(action: action) {
      HStack(spacing: scale.iconGap) {
        CSIconView(icon: icon, size: scale.iconSize, color: iconColor ?? resolvedTitleColor)
          .frame(width: scale.iconColumn)
        Text(title)
          .font(CSFont.ui(scale.titleSize, titleWeight))
          .foregroundStyle(resolvedTitleColor)
          .frame(maxWidth: .infinity, alignment: .leading)
        if let shortcut {
          Text(shortcut)
            .font(CSFont.mono(scale.keycapSize, .medium))
            .foregroundStyle(shortcutColor)
        }
        if let expanded = disclosureExpanded {
          CSIconView(
            icon: TrayDisclosureChevron.icon, size: scale.chevronSize, color: CSColor.textFaint
          )
          .rotationEffect(
            .degrees(TrayDisclosureChevron.rotationDegrees(expanded: expanded))
          )
        }
      }
      .padding(.horizontal, scale.horizontalPadding)
      .padding(.vertical, scale.verticalPadding)
      .background(
        RoundedRectangle(cornerRadius: scale.cornerRadius, style: .continuous).fill(fillColor)
      )
      .overlay(
        RoundedRectangle(cornerRadius: scale.cornerRadius, style: .continuous)
          .strokeBorder(borderColor, lineWidth: 0.5)
      )
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .csFocusOutline(cornerRadius: scale.cornerRadius)
    .accessibilityLabel(title)
    .accessibilityValue(accessibilityValue)
    .onHover { hovering = $0 }
  }

  /// Disclosure state when the row heads a group, otherwise the keycap.
  private var accessibilityValue: String {
    guard let expanded = disclosureExpanded else { return shortcut ?? "" }
    return expanded
      ? String(localized: "Expanded", comment: "Accessibility value: a disclosure row is open")
      : String(localized: "Collapsed", comment: "Accessibility value: a disclosure row is closed")
  }
}

/// A nested disclosure child row (smaller, with the left rail in the container).
struct TrayChildRow: View {
  let title: String
  var suffix: String? = nil
  var action: (() -> Void)? = nil

  @State private var hovering = false

  var body: some View {
    if let action {
      Button(action: action) { label }
        .buttonStyle(.plain)
        .csFocusOutline()
        .onHover { hovering = $0 }
    } else {
      label
    }
  }

  private var label: some View {
    HStack(spacing: 5) {
      Text(title)
        .font(CSFont.ui(TrayRowScale.child.titleSize, .medium))
        .foregroundStyle(CSColor.textMuted)
        .lineLimit(1)
        .truncationMode(.tail)
      if let suffix {
        Text(suffix)
          .font(CSFont.mono(TrayRowScale.child.keycapSize))
          .foregroundStyle(CSColor.textFaintAlt)
      }
      Spacer(minLength: 0)
    }
    .padding(.horizontal, TrayRowScale.child.horizontalPadding)
    .padding(.vertical, TrayRowScale.child.verticalPadding)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.chip, style: .continuous)
        .fill(hovering ? CSColor.surfaceRaised(0.05) : .clear)
    )
    .contentShape(Rectangle())
  }
}

/// A transcript-history child row: a fixed-width time column plus the snippet.
///
/// Founder brief, round 17, 2026-10-10: the time is a separate, stable element,
/// so it keeps a monospaced-digit column of its own width and never shifts when
/// a neighbouring snippet is longer. The snippet takes the remaining width and
/// truncates with one trailing ellipsis, so a long transcript can never widen
/// the 300pt menu.
struct TrayHistoryRow: View {
  let time: String
  let snippet: String
  var action: (() -> Void)? = nil

  /// "HH:mm" in the keycap mono face, with headroom for a wider locale clock.
  static let timeColumn: CGFloat = 34

  @State private var hovering = false

  var body: some View {
    if let action {
      Button(action: action) { label }
        .buttonStyle(.plain)
        .csFocusOutline()
        .onHover { hovering = $0 }
    } else {
      label
    }
  }

  private var label: some View {
    HStack(spacing: 8) {
      Text(time)
        .font(CSFont.mono(TrayRowScale.child.keycapSize, .medium))
        .monospacedDigit()
        .foregroundStyle(CSColor.textFaintAlt)
        .lineLimit(1)
        .frame(width: Self.timeColumn, alignment: .leading)
      Text(snippet)
        .font(CSFont.ui(TrayRowScale.child.titleSize, .medium))
        .foregroundStyle(CSColor.textMuted)
        .lineLimit(1)
        .truncationMode(.tail)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
    .padding(.horizontal, TrayRowScale.child.horizontalPadding)
    .padding(.vertical, TrayRowScale.child.verticalPadding)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.chip, style: .continuous)
        .fill(hovering ? CSColor.surfaceRaised(0.05) : .clear)
    )
    .contentShape(Rectangle())
  }
}

/// Hairline group separator (transparent margins per the mock).
struct TrayDivider: View {
  var top: CGFloat = 5
  var bottom: CGFloat = 5
  var body: some View {
    Rectangle()
      .fill(CSColor.hairline(0.07))
      .frame(height: 1)
      .padding(.horizontal, 6)
      .padding(.top, top)
      .padding(.bottom, bottom)
  }
}

/// Indented container for disclosure children: left rail + 14pt inset.
///
/// Founder brief, round 17, 2026-10-10: children sit flush against each other
/// so an expansion reads as one block under its heading; the gap that separates
/// top-level groups stays with `TrayDivider`.
struct TrayDisclosureChildren<Content: View>: View {
  @ViewBuilder var content: Content
  var body: some View {
    VStack(spacing: 0) { content }
      .padding(.leading, 14)
      .overlay(alignment: .leading) {
        Rectangle().fill(CSColor.hairline(0.08)).frame(width: 1)
      }
      .padding(.leading, 6)
      .padding(.vertical, 2)
  }
}

/// The accent-derived keycap tint so it shares the brand's accent source.
extension TrayRow {
  static let primaryShortcutColor = TrayLocal.primaryShortcut
}
