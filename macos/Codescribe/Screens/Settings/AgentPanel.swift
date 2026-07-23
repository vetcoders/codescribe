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
    ]

    @ObservedObject var model: SettingsViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            EyebrowLabel(text: "Settings · \(SettingsSection.agent.title)")
            Text("How your agent works.")
                .font(CSFont.ui(26, .bold))
                .tracking(-0.5)
                .foregroundStyle(CSColor.textHigh)
                .padding(.top, 6)

            Text("Request lanes, workspace roots, and MCP servers — the runtime configuration behind agent work.")
                .font(CSFont.ui(12.5))
                .lineSpacing(2)
                .foregroundStyle(CSColor.textMutedAlt)
                .padding(.top, 8)

            runtimeRows
                .padding(.top, 20)

            SettingsSectionLabel("LLM lanes")
                .padding(.top, 22)
            LLMLanesSection(model: model)
                .padding(.top, 11)

            WorkspaceRootsSection(model: model)
                .padding(.top, 30)

            AgentStatusSection(model: model)
                .padding(.top, 30)

            MCPServersSection(model: model)
                .padding(.top, 26)
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 24)
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
