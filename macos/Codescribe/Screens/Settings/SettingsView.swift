import SwiftUI

// Settings window: NavigationSplitView with the 212px section rail (Creator · Keys
// · Prompts · Engine · Audio · Voice Lab · User) and a scrolling detail panel.
// Creator · Keys · Prompts · Engine are interactive. Pixel-faithful to
// "codescribe App - Settings.dc.html".

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
            // Honour a one-shot deep-link (e.g. onboarding routing to MCP setup)
            // so the window lands on the requested section instead of the default.
            if let target = SettingsDeepLink.consume() {
                model.select(target)
            }
        }
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
                default:
                    // Creator is the default interactive section; inert rail
                    // items never switch the detail away from these panels.
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
            ForEach(SettingsSection.allCases) { item in
                railItem(item)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 4)
    }

    @ViewBuilder
    private func railItem(_ item: SettingsSection) -> some View {
        let isActive = model.section == item
        HStack(spacing: 10) {
            Circle()
                .fill(isActive ? CSColor.terracotta : Self.inactiveDot)
                .frame(width: 7, height: 7)
            Text(item.rawValue)
                .font(CSFont.ui(13, isActive ? .semibold : .medium))
                .foregroundStyle(labelColor(item, isActive: isActive))
            Spacer(minLength: 0)
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
        .onTapGesture { model.select(item) }
        .opacity(item.isInteractive || isActive ? 1 : 0.85)
        .accessibilityAddTraits(isActive ? [.isButton, .isSelected] : .isButton)
    }

    private func labelColor(_ item: SettingsSection, isActive: Bool) -> Color {
        if isActive { return CSColor.terracottaLight }
        // Interactive-but-not-selected = brighter body; inert = muted.
        return item.isInteractive ? CSColor.textBody : CSColor.textMuted
    }

    private var footer: some View {
        HStack(spacing: 8) {
            Circle().fill(CSColor.oliveLight).frame(width: 6, height: 6)
            Text("all systems healthy")
                .font(CSFont.mono(10, .medium))
                .foregroundStyle(CSColor.textFaint)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 14)
        .overlay(alignment: .top) {
            Rectangle().fill(CSColor.hairline(0.06)).frame(height: 1)
        }
    }
}

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
