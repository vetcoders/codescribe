import SwiftUI

// Agent panel: the one owner of request lanes and local agent substrate.
// Storage keys and bridge calls remain unchanged; this view only gives the
// existing controls a dedicated navigation destination.
struct AgentPanel: View {
    static let ownedCapabilities: Set<SettingsPanelCapability> = [
        .llmLanes,
        .workspaceRoots,
        .agentStatus,
        .mcpServers,
        .toolPermissions,
    ]

    @ObservedObject var model: SettingsViewModel

    /// One page at a time. Five independent subsystems (lanes, roots,
    /// capabilities, tool permissions, MCP servers — the last two alone are
    /// ~800 lines) used to stack into a single scroll where finding anything
    /// meant wheeling past four subsystems you did not come for. The rail's
    /// tree addresses each directly; this view renders only what was asked for.
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            EyebrowLabel(text: "Settings · \(SettingsSection.agent.title)\(pageEyebrow)")
            Text("How your agent works.")
                .font(CSFont.ui(26, .bold))
                .tracking(-0.5)
                .foregroundStyle(CSColor.textHigh)
                .padding(.top, 6)

            Text("Request lanes, workspace roots, tool permissions, and MCP servers — the runtime configuration behind agent work.")
                .font(CSFont.ui(12.5))
                .lineSpacing(2)
                .foregroundStyle(CSColor.textMutedAlt)
                .padding(.top, 8)

            // Resolved LLM truth stays on every page: it is the answer to "which
            // provider am I actually talking to", and it was the one block worth
            // re-reading regardless of which subsystem you came to change.
            runtimeRows
                .padding(.top, 20)

            page
                .padding(.top, 24)
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 24)
    }

    private var pageEyebrow: String {
        model.page.map { " · \($0.title)" } ?? ""
    }

    @ViewBuilder
    private var page: some View {
        switch model.page {
        case .agentWorkspace:
            WorkspaceRootsSection(model: model)
        case .agentStatus:
            AgentStatusSection(model: model)
        case .agentTools:
            ToolPermissionsSection(model: model)
        case .agentMcp:
            MCPServersSection(model: model)
        default:
            // `.agentLanes` and the nil route (deep link that named no page).
            VStack(alignment: .leading, spacing: 0) {
                SettingsSectionLabel("LLM lanes")
                LLMLanesSection(model: model)
                    .padding(.top, 11)
            }
        }
    }

    // MARK: - Resolved LLM truth (read-only)

    private var runtimeRows: some View {
        VStack(spacing: 0) {
            RuntimeRow(
                key: "AI formatting",
                value: model.formattingDescription,
                tint: true,
                trailing: .none
            )
            divider
            ForEach(LLMLane.allCases) { lane in
                let laneModel = model.llmLane(lane)
                RuntimeRow(
                    key: "\(lane.title) endpoint",
                    value: laneModel.resolvedEndpoint,
                    tint: false,
                    mono: true,
                    trailing: .none
                )
                divider
                RuntimeRow(
                    key: "\(lane.title) model",
                    value: laneModel.resolvedModel,
                    tint: true,
                    mono: true,
                    trailing: .none
                )
                if lane != LLMLane.allCases.last {
                    divider
                }
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 13, style: .continuous)
                .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
        )
    }

    private var divider: some View {
        Rectangle().fill(CSColor.hairline(0.05)).frame(height: 1)
    }
}

#if DEBUG
#Preview("Agent panel") {
    ScrollView { AgentPanel(model: .preview(.agent)) }
        .frame(width: 720, height: 900)
        .background(SettingsView.windowGradient)
        .preferredColorScheme(.dark)
}
#endif
