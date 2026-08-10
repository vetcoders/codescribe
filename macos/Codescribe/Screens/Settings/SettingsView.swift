import Combine
import SwiftUI

// Settings window: NavigationSplitView with a truthful section rail. Available
// sections navigate and hidden sections do not render.

struct SettingsView: View {
    @StateObject private var model: SettingsViewModel

    init(model: SettingsViewModel? = nil) {
        _model = StateObject(wrappedValue: model ?? SettingsViewModel())
    }

    var body: some View {
        // Plain two-pane layout: NavigationSplitView reserved a toolbar strip above
        // the sidebar content, pushing the rail ~70px down (dead vertical space).
        // A fixed-width HStack keeps the rail flush with the titlebar.
        HStack(spacing: 0) {
            SettingsRail(model: model)
                .frame(width: 212)
            detail
        }
        .csFocusPolicy()
        .frame(minWidth: 880, maxWidth: .infinity, minHeight: 620, maxHeight: .infinity)
        .background(Self.windowGradient.ignoresSafeArea())
        .preferredColorScheme(.dark)
        .onAppear {
            model.refresh()
            consumePendingDeepLink()
        }
        .onReceive(NotificationCenter.default.publisher(for: SettingsDeepLink.pendingSectionDidChange)) { _ in
            consumePendingDeepLink()
        }
    }

    private func consumePendingDeepLink() {
        guard let target = SettingsDeepLink.consume() else { return }
        model.select(target)
    }

    @ViewBuilder
    private var detail: some View {
        ScrollView {
            Group {
                switch model.section.destination {
                case .dictation:
                    EnginePanel(model: model)
                case .shortcuts:
                    ShortcutsPanel(model: model)
                case .providers:
                    KeysPanel(model: model)
                case .agent:
                    AgentPanel(model: model)
                case .prompts:
                    PromptPanel(model: model)
                case .user:
                    UserPanel(model: model)
                case .dictionary:
                    VoiceLabPanel(model: model)
                case .audio:
                    AudioPanel(model: model)
                case .creator:
                    CreatorPanel(model: model)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .scrollContentBackground(.hidden)
        .background(Self.windowGradient)
    }

    /// linear-gradient(135deg,#15110e,#0b0c10 55%,#0d1012) from the mock.
    static let windowGradient = LinearGradient(
        stops: [
            .init(color: Color(hex: 0x15110E), location: 0.0),
            .init(color: Color(hex: 0x0B0C10), location: 0.55),
            .init(color: Color(hex: 0x0D1012), location: 1.0)
        ],
        startPoint: .topLeading,
        endPoint: .bottomTrailing
    )
}

// MARK: - Rail

struct SettingsRailItemVisualState: Equatable {
    let showsActiveFill: Bool
    let showsHairline: Bool
}

func settingsRailItemVisualState(
    isActive: Bool,
    isKeyboardFocused: Bool
) -> SettingsRailItemVisualState {
    SettingsRailItemVisualState(
        showsActiveFill: isActive,
        showsHairline: isActive || isKeyboardFocused
    )
}

private struct SettingsRail: View {
    @ObservedObject var model: SettingsViewModel
    @FocusState private var focusedControl: FocusTarget?

    private enum FocusTarget: Hashable {
        case section(String)
        case footer
    }

    // Inactive rail dot — muted gunmetal (#3a3d44, not a brand token).
    private static let inactiveDot = Color(hex: 0x3A3D44)

    var body: some View {
        VStack(spacing: 0) {
            brand
            nav
            Spacer(minLength: 0)
            footer
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background {
            ZStack {
                SettingsView.windowGradient
                CSColor.surfaceRaised(0.015) // subtle warm lift, matches mock rail
            }
        }
        .overlay(alignment: .trailing) {
            Rectangle().fill(CSColor.hairline(0.06)).frame(width: 1)
        }
    }

    private var brand: some View {
        HStack(spacing: 9) {
            // Keep the wordmark on a single line regardless of column width.
            Wordmark(size: 15)
                .fixedSize(horizontal: true, vertical: false)
                .layoutPriority(1)
            Spacer(minLength: 0)
            Text("v\(model.appVersion)")
                .font(CSFont.mono(10, .medium))
                .foregroundStyle(CSColor.textFaintAlt)
        }
        .padding(.horizontal, 16)
        .padding(.top, 16)
        .padding(.bottom, 14)
    }

    private var nav: some View {
        VStack(spacing: 3) {
            ForEach(SettingsSection.allCases.filter { $0.availability != .hidden }) { item in
                railItem(item)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 4)
    }

    @ViewBuilder
    private func railItem(_ item: SettingsSection) -> some View {
        let isActive = model.section == item
        let isKeyboardFocused = focusedControl == .section(item.rawValue)
        switch item.availability {
        case .available:
            Button {
                model.select(item)
            } label: {
                railItemContent(
                    item,
                    visualState: settingsRailItemVisualState(
                        isActive: isActive,
                        isKeyboardFocused: isKeyboardFocused
                    )
                )
            }
            .buttonStyle(.plain)
            .focusable(true)
            .focused($focusedControl, equals: .section(item.rawValue))
            .focusEffectDisabled()
            .accessibilityAddTraits(isActive ? .isSelected : [])
        case .hidden:
            EmptyView()
        }
    }

    private func railItemContent(
        _ item: SettingsSection,
        visualState: SettingsRailItemVisualState
    ) -> some View {
        HStack(spacing: 10) {
            Circle()
                .fill(visualState.showsActiveFill ? CSColor.chromeAccent : Self.inactiveDot)
                .frame(width: 7, height: 7)
            Text(item.title)
                .font(CSFont.ui(13, visualState.showsActiveFill ? .semibold : .medium))
                .foregroundStyle(labelColor(item, isActive: visualState.showsActiveFill))
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                .fill(visualState.showsActiveFill ? CSColor.chromeAccent.opacity(0.14) : .clear)
        )
        .overlay(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                .strokeBorder(
                    visualState.showsHairline ? CSColor.chromeAccent.opacity(0.28) : .clear,
                    lineWidth: 1
                )
        )
        .contentShape(Rectangle())
    }

    private func labelColor(_ item: SettingsSection, isActive: Bool) -> Color {
        if isActive { return CSColor.chromeAccent }
        // Interactive-but-not-selected = brighter body; inert = muted.
        return item.isInteractive ? CSColor.textBody : CSColor.textMuted
    }

    @ViewBuilder
    private var footer: some View {
        let health = model.settingsHealth
        if let target = health.targetSection {
            Button {
                model.select(target)
            } label: {
                footerContent(health, isKeyboardFocused: focusedControl == .footer)
            }
            .buttonStyle(.plain)
            .focusable(true)
            .focused($focusedControl, equals: .footer)
            .focusEffectDisabled()
            .help("Open \(target.title) settings")
        } else {
            footerContent(health, isKeyboardFocused: false)
        }
    }

    private func footerContent(
        _ health: SettingsHealthState,
        isKeyboardFocused: Bool
    ) -> some View {
        HStack(spacing: 8) {
            Circle().fill(health.level.color).frame(width: 6, height: 6)
            Text(health.message)
                .font(CSFont.mono(10, .medium))
                .foregroundStyle(health.level.color)
                .lineLimit(2)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 14)
        .overlay(alignment: .top) {
            Rectangle().fill(CSColor.hairline(0.06)).frame(height: 1)
        }
        .overlay {
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                .strokeBorder(
                    isKeyboardFocused ? CSColor.chromeAccent.opacity(0.28) : .clear,
                    lineWidth: 1
                )
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
        }
    }
}

private extension SettingsHealthLevel {
    var color: Color {
        switch self {
        case .healthy: return CSColor.oliveLight
        case .degraded: return CSColor.amber
        case .offline: return CSColor.terracottaLight
        case .unknown: return CSColor.textFaint
        }
    }
}

// MARK: - Shared Settings chrome (consumed by every panel)

struct SettingsSectionLabel: View {
    let text: String
    init(_ text: String) { self.text = text }
    var body: some View {
        Text(text.uppercased())
            .font(CSFont.mono(12, .semibold))
            .tracking(0.5)
            .foregroundStyle(CSColor.textMuted)
    }
}

struct SettingsMenuLabel: View {
    let text: String
    var mono: Bool = false
    var chrome: Bool = false

    var body: some View {
        if chrome {
            content
                .padding(.horizontal, 11)
                .padding(.vertical, 7)
                .background(
                    RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                        .fill(CSColor.surfaceRaised(0.03))
                )
                .overlay(
                    RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                        .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
                )
                .contentShape(Rectangle())
        } else {
            content
        }
    }

    private var content: some View {
        HStack(spacing: 6) {
            Text(text)
                .font(mono ? CSFont.mono(12.5, .semibold) : CSFont.ui(12.5, .semibold))
                .foregroundStyle(CSColor.textHigh)
                .lineLimit(1)
            CSIconView(icon: .chevronUpDown, size: 9, weight: .semibold, color: CSColor.textFaint)
        }
    }
}

/// Read-only key/value row for runtime-truth blocks (Dictation and Providers).
struct RuntimeRow: View {
    enum Trailing {
        case none
        case dot(Color)
        case text(String, Color)
    }

    let key: String
    let value: String
    var tint: Bool = false
    var mono: Bool = false
    var trailing: Trailing = .none

    var body: some View {
        HStack(spacing: 12) {
            Text(key)
                .font(CSFont.mono(12, .medium))
                .foregroundStyle(CSColor.textMutedAlt)
                .frame(width: 160, alignment: .leading)
            Text(value)
                .font(mono ? CSFont.mono(12.5, .semibold) : CSFont.ui(12.5, .semibold))
                .foregroundStyle(mono ? CSColor.textBodyAlt : CSColor.textHigh)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: .infinity, alignment: .leading)
            trailingView
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 13)
        .background(tint ? CSColor.surfaceRaised(0.02) : Color.clear)
    }

    @ViewBuilder
    private var trailingView: some View {
        switch trailing {
        case .none:
            EmptyView()
        case .dot(let color):
            Circle().fill(color).frame(width: 7, height: 7)
        case .text(let label, let color):
            Text(label)
                .font(CSFont.mono(10, .semibold))
                .foregroundStyle(color)
        }
    }
}

#if DEBUG
#Preview("Settings — Creator") {
    SettingsView(model: SettingsViewModel.preview(.creator))
        .frame(width: 960, height: 620)
}

#Preview("Settings — Dictation") {
    SettingsView(model: SettingsViewModel.preview(.engine))
        .frame(width: 960, height: 620)
}

#Preview("Settings — Providers") {
    SettingsView(model: SettingsViewModel.preview(.keys))
        .frame(width: 960, height: 620)
}

#Preview("Settings — Prompts") {
    SettingsView(model: SettingsViewModel.preview(.prompts))
        .frame(width: 960, height: 620)
}
#endif
