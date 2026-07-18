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
                switch model.section {
                case .engine:
                    EnginePanel(model: model)
                case .shortcuts:
                    ShortcutsPanel(model: model)
                case .keys:
                    KeysPanel(model: model)
                case .prompts:
                    PromptPanel(model: model)
                case .user:
                    UserPanel(model: model)
                case .voiceLab:
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
                .fill(visualState.showsActiveFill ? CSColor.terracotta : Self.inactiveDot)
                .frame(width: 7, height: 7)
            Text(item.rawValue)
                .font(CSFont.ui(13, visualState.showsActiveFill ? .semibold : .medium))
                .foregroundStyle(labelColor(item, isActive: visualState.showsActiveFill))
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                .fill(visualState.showsActiveFill ? CSColor.terracotta.opacity(0.14) : .clear)
        )
        .overlay(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                .strokeBorder(
                    visualState.showsHairline ? CSColor.terracotta.opacity(0.28) : .clear,
                    lineWidth: 1
                )
        )
        .contentShape(Rectangle())
    }

    private func labelColor(_ item: SettingsSection, isActive: Bool) -> Color {
        if isActive { return CSColor.terracottaLight }
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
            .help("Open \(target.rawValue) settings")
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
                    isKeyboardFocused ? CSColor.terracotta.opacity(0.28) : .clear,
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

#if DEBUG
#Preview("Settings — Creator") {
    SettingsView(model: SettingsViewModel.preview(.creator))
        .frame(width: 960, height: 620)
}

#Preview("Settings — Engine") {
    SettingsView(model: SettingsViewModel.preview(.engine))
        .frame(width: 960, height: 620)
}

#Preview("Settings — Keys") {
    SettingsView(model: SettingsViewModel.preview(.keys))
        .frame(width: 960, height: 620)
}

#Preview("Settings — Prompts") {
    SettingsView(model: SettingsViewModel.preview(.prompts))
        .frame(width: 960, height: 620)
}
#endif
