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

  /// One page at a time, and the page IS the content. Five independent
  /// subsystems (lanes, roots, capabilities, tool permissions, MCP servers —
  /// the last two alone are ~800 lines) used to stack into a single scroll.
  /// The first paginated cut kept the monolith's headline and the 7-row
  /// resolved-LLM table as a preamble on EVERY page, so each page rendered
  /// below a screenful of old world — pagination in the rail, monolith in
  /// the pane. Now each page owns its headline and body; the resolved-truth
  /// table lives only where it is the subject (LLM lanes), below the editors.
  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      EyebrowLabel(text: "Settings · \(SettingsSection.agent.title) · \(current.title)")
      Text(headline)
        .font(CSFont.ui(26, .bold))
        .tracking(-0.5)
        .foregroundStyle(CSColor.textHigh)
        .padding(.top, 6)

      Text(blurb)
        .font(CSFont.ui(12.5))
        .lineSpacing(2)
        .foregroundStyle(CSColor.textMutedAlt)
        .padding(.top, 8)

      page
        .padding(.top, 20)
    }
    .padding(.horizontal, 28)
    .padding(.vertical, 24)
  }

  /// The nil route (a deep link that named no page) lands on lanes,
  /// mirroring the `page` switch below. Pages of other sections cannot
  /// reach this panel, so they clamp to lanes too.
  private var current: SettingsPage {
    switch model.page {
    case .agentWorkspace, .agentStatus, .agentTools, .agentMcp:
      return model.page ?? .agentLanes
    default:
      return .agentLanes
    }
  }

  private var headline: String {
    switch current {
    case .agentWorkspace: return "Workspace roots."
    case .agentStatus: return "Capabilities."
    case .agentTools: return "Tool permissions."
    case .agentMcp: return "MCP servers."
    default: return "Request lanes."
    }
  }

  private var blurb: String {
    switch current {
    case .agentWorkspace:
      return "Directories the agent may read and write. Everything outside them is out of reach."
    case .agentStatus:
      return "What the local agent substrate can currently do, and why."
    case .agentTools:
      return "Allow, ask, or deny — per tool. Deny wins over everything."
    case .agentMcp:
      return "External MCP servers the agent can call, and their transports."
    default:
      return
        "Provider, endpoint, and model per request path. The resolved runtime truth is below the editors."
    }
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
        LLMLanesSection(model: model)

        // The answer to "which provider am I actually talking to",
        // placed after the editors as read-only proof of what resolved.
        SettingsSectionLabel("Resolved runtime truth")
          .padding(.top, 24)
        runtimeRows
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
