import SwiftUI

// Row primitives for the tray dropdown. Geometry is taken straight from the
// mock: rows are 9×12 padded, 9pt-radius, 11pt icon→label gap, 18pt icon column.

/// Two mock-only tints not present in the locked token palette.
private enum TrayLocal {
    /// Submenu child + Quit label (#c7cabf) — slightly muted body text.
    static let subnote = Color(hex: 0xC7CABF)
    /// Primary-row keycap follows the operator's system accent.
    static var primaryShortcut: Color { CSColor.chromeAccent.opacity(0.78) }
}

enum TrayRowStyle {
    case plain     // transparent; subtle hover highlight
    case primary   // system accent tint + border (the ONE primary action)
    case raised    // surface-raised tint (an expanded disclosure parent)
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
    var titleColor: Color = CSColor.textBodyAlt
    var titleWeight: Font.Weight = .medium
    var shortcut: String? = nil
    var shortcutColor: Color = CSColor.textFaintAlt
    /// Expansion state of the disclosure group this row heads; `nil` for plain
    /// action rows without a chevron.
    var disclosureExpanded: Bool? = nil
    var style: TrayRowStyle = .plain
    var action: () -> Void = {}

    @State private var hovering = false

    private var fillColor: Color {
        switch style {
        case .primary: return CSColor.chromeAccent.opacity(0.13)
        case .raised:  return CSColor.surfaceRaised(0.04)
        case .plain:   return hovering ? CSColor.surfaceRaised(0.05) : .clear
        }
    }

    private var borderColor: Color {
        style == .primary ? CSColor.chromeAccent.opacity(0.24) : .clear
    }

    var body: some View {
        HStack(spacing: 11) {
            CSIconView(icon: icon, size: 13, color: iconColor ?? titleColor)
                .frame(width: 18)
            Text(title)
                .font(CSFont.ui(13, titleWeight))
                .foregroundStyle(titleColor)
                .frame(maxWidth: .infinity, alignment: .leading)
            if let shortcut {
                Text(shortcut)
                    .font(CSFont.mono(10, .medium))
                    .foregroundStyle(shortcutColor)
            }
            if let expanded = disclosureExpanded {
                CSIconView(icon: TrayDisclosureChevron.icon, size: 11, color: CSColor.textFaint)
                    .rotationEffect(
                        .degrees(TrayDisclosureChevron.rotationDegrees(expanded: expanded))
                    )
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous).fill(fillColor)
        )
        .overlay(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                .strokeBorder(borderColor, lineWidth: 1)
        )
        .contentShape(Rectangle())
        .onTapGesture(perform: action)
        .onHover { hovering = $0 }
    }
}

/// A nested disclosure child row (smaller, with the left rail in the container).
struct TrayChildRow: View {
    let title: String
    var suffix: String? = nil
    var action: () -> Void = {}

    @State private var hovering = false

    var body: some View {
        HStack(spacing: 5) {
            Text(title)
                .font(CSFont.ui(12, .medium))
                .foregroundStyle(TrayLocal.subnote)
            if let suffix {
                Text(suffix)
                    .font(CSFont.mono(10))
                    .foregroundStyle(CSColor.textFaintAlt)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 11)
        .padding(.vertical, 7)
        .background(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(hovering ? CSColor.surfaceRaised(0.05) : .clear)
        )
        .contentShape(Rectangle())
        .onTapGesture(perform: action)
        .onHover { hovering = $0 }
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
struct TrayDisclosureChildren<Content: View>: View {
    @ViewBuilder var content: Content
    var body: some View {
        VStack(spacing: 1) { content }
            .padding(.leading, 14)
            .overlay(alignment: .leading) {
                Rectangle().fill(CSColor.hairline(0.08)).frame(width: 1)
            }
            .padding(.leading, 6)
            .padding(.vertical, 2)
    }
}

/// Expose the mock-only palette so it shares the brand's hex initializer.
extension TrayRow {
    static let subnoteColor = TrayLocal.subnote
    static let primaryShortcutColor = TrayLocal.primaryShortcut
}
