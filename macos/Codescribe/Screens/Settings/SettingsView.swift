import Combine
import SwiftUI

// Settings window: NavigationSplitView with a truthful section rail. Available
// sections navigate, coming-soon sections are announced as such, and hidden
// sections do not render.

struct SettingsView: View {
    @StateObject private var model: SettingsViewModel

    init(model: SettingsViewModel? = nil) {
        _model = StateObject(wrappedValue: model ?? SettingsViewModel())
    }

    var body: some View {
        NavigationSplitView {
            SettingsRail(model: model)
                // Firm min/ideal/max so `.balanced` can't compress the rail below the
                // brand wordmark's width and wrap "codescribe" onto a second line.
                .navigationSplitViewColumnWidth(min: 212, ideal: 212, max: 212)
                .toolbar(removing: .sidebarToggle)
        } detail: {
            detail
        }
        .navigationSplitViewStyle(.balanced)
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
                case .creator:
                    CreatorPanel(model: model)
                case .audio:
                    // `select` cannot enter coming-soon sections. Keep this
                    // exhaustive fallback for state restoration across versions.
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

private struct SettingsRail: View {
    @ObservedObject var model: SettingsViewModel

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
        switch item.availability {
        case .available:
            Button {
                model.select(item)
            } label: {
                railItemContent(item, isActive: isActive)
            }
            .buttonStyle(.plain)
            .accessibilityAddTraits(isActive ? .isSelected : [])
        case .comingSoon:
            railItemContent(item, isActive: false, showsSoonChip: true)
                .opacity(0.85)
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("\(item.rawValue), coming soon")
                .help("Coming soon")
        case .hidden:
            EmptyView()
        }
    }

    private func railItemContent(
        _ item: SettingsSection,
        isActive: Bool,
        showsSoonChip: Bool = false
    ) -> some View {
        HStack(spacing: 10) {
            Circle()
                .fill(isActive ? CSColor.terracotta : Self.inactiveDot)
                .frame(width: 7, height: 7)
            Text(item.rawValue)
                .font(CSFont.ui(13, isActive ? .semibold : .medium))
                .foregroundStyle(labelColor(item, isActive: isActive))
            Spacer(minLength: 0)
            if showsSoonChip {
                Text("soon")
                    .font(CSFont.mono(8, .semibold))
                    .tracking(0.3)
                    .foregroundStyle(CSColor.textFaint)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(CSColor.surfaceRaised(0.04), in: Capsule())
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                .fill(isActive ? CSColor.terracotta.opacity(0.14) : .clear)
        )
        .overlay(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                .strokeBorder(isActive ? CSColor.terracotta.opacity(0.28) : .clear, lineWidth: 1)
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
                footerContent(health)
            }
            .buttonStyle(.plain)
            .help("Open \(target.rawValue) settings")
        } else {
            footerContent(health)
        }
    }

    private func footerContent(_ health: SettingsHealthState) -> some View {
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
