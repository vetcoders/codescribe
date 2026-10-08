import SwiftUI

// Agent-substrate status, rendered inside Agent Diagnostics (READ-ONLY runtime
// truth). A status screen, not an inventory: the readiness verdict with its
// prerequisite rows, the detected skill installations, then one-line summaries
// of the capability matrix and the MCP servers with their full lists collapsed
// by default. Tool permissions and server management live in the Tools and
// MCP tabs. A "Refresh" action re-probes without touching the rest of the
// panel. Degrades gracefully: a missing mcp.json shows a neutral "MCP off"
// row, never an error.

struct AgentStatusSection: View {
  @ObservedObject var model: SettingsViewModel
  @State private var showingCapabilityRows = false
  @State private var showingServerRows = false
  @State private var showingLaunchDetail = false

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      header

      // Readiness verdict + per-prerequisite rows.
      statusCard(rows: model.agentReadiness.rows)
        .padding(.top, CSSpace.control)

      installations
        .padding(.top, CSSpace.section)

      capabilities
        .padding(.top, CSSpace.section)

      mcpServers
        .padding(.top, CSSpace.section)
    }
    .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
  }

  // MARK: Header + refresh

  private var header: some View {
    HStack(spacing: 10) {
      SettingsSectionLabel(String(localized: "Agent readiness"))
      readinessPill
      Spacer(minLength: 0)
      Button {
        model.refreshAgentStatus()
        model.refreshCreatorAgentBridge()
      } label: {
        HStack(spacing: 5) {
          CSIconView(icon: .refresh, size: 11, weight: .semibold)
          Text("Refresh").font(CSFont.mono(11, .semibold))
        }
        .foregroundStyle(Color.primary)
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
        .background(
          RoundedRectangle(cornerRadius: 7, style: .continuous)
            .fill(Color.primary.opacity(0.08))
        )
        .overlay(
          RoundedRectangle(cornerRadius: 7, style: .continuous)
            .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
        )
      }
      .csFocusRing()
    }
  }

  private var readinessPill: some View {
    let ready = model.agentReadiness.ready
    let accent = ready ? CSColor.olive : CSColor.terracotta
    let accentLight = ready ? CSColor.oliveLight : CSColor.terracotta
    return Text(ready ? "Ready" : "Not ready")
      .textCase(.uppercase)
      .font(CSFont.mono(9, .semibold))
      .tracking(0.4)
      .foregroundStyle(accentLight)
      .padding(.horizontal, 8)
      .padding(.vertical, 2)
      .background(
        RoundedRectangle(cornerRadius: 6, style: .continuous)
          .fill(accent.opacity(0.12))
      )
      .overlay(
        RoundedRectangle(cornerRadius: 6, style: .continuous)
          .strokeBorder(accent.opacity(0.24), lineWidth: 1)
      )
  }

  // MARK: Detected installations

  /// One block per detected client (name + managed path), the installer's
  /// own evidence line, and the launch synchronization notice folded away as
  /// technical detail.
  private var installations: some View {
    let status = model.creatorAgentBridgeStatus
    let clients = Array(zip(status.installedClients, status.installedPaths))
    return VStack(alignment: .leading, spacing: 0) {
      SettingsSectionLabel(String(localized: "Detected installations and runtime"))
      ForEach(clients, id: \.0) { client, path in
        VStack(alignment: .leading, spacing: 2) {
          Text(verbatim: client.displayName)
            .font(CSFont.ui(12.5, .semibold))
            .foregroundStyle(Color.primary)
          Text(path.replacingOccurrences(of: NSHomeDirectory(), with: "~"))
            .font(CSFont.mono(10, .medium))
            .foregroundStyle(Color.secondary)
            .textSelection(.enabled)
        }
        .padding(.top, 8)
      }
      Text(status.detail)
        .font(CSFont.ui(11.5))
        .foregroundStyle(Color.secondary)
        .fixedSize(horizontal: false, vertical: true)
        .padding(.top, clients.isEmpty ? 4 : 8)
      if let launchDetail = model.creatorAgentBridgeLaunchDetail {
        DisclosureGroup(isExpanded: $showingLaunchDetail) {
          Text(verbatim: launchDetail)
            .font(CSFont.mono(10, .medium))
            .foregroundStyle(Color.secondary)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.top, 4)
        } label: {
          Text("Technical details")
            .font(CSFont.ui(11.5))
            .foregroundStyle(Color.secondary)
        }
        .padding(.top, 8)
        .accessibilityIdentifier("settings-diagnostics-launch-detail")
      }
    }
  }

  // MARK: Capabilities (summary + collapsed matrix)

  private var capabilities: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionLabel(String(localized: "Available tools and integrations"))
      Text(CapabilitySummary(rows: model.capabilityMatrix).line)
        .font(CSFont.ui(12.5, .semibold))
        .foregroundStyle(Color.primary)
        .padding(.top, 4)
      Text("Tool permissions are managed in the Tools tab.")
        .font(CSFont.ui(11.5))
        .foregroundStyle(Color.secondary)
        .padding(.top, 2)
      DisclosureGroup(isExpanded: $showingCapabilityRows) {
        capabilityMatrixCard.padding(.top, 8)
      } label: {
        Text("Show details")
          .font(CSFont.ui(11.5))
          .foregroundStyle(Color.secondary)
      }
      .padding(.top, 8)
      .accessibilityIdentifier("settings-diagnostics-capabilities")
    }
  }

  private var capabilityMatrixCard: some View {
    VStack(spacing: 0) {
      if model.capabilityMatrix.isEmpty {
        Text("no capability rows (refresh or start agent substrate)")
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(Color.primary)
          .padding(.horizontal, 16)
          .padding(.vertical, 12)
          .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
      } else {
        ForEach(Array(model.capabilityMatrix.enumerated()), id: \.offset) { index, row in
          if index > 0 {
            Rectangle().fill(Color.primary.opacity(0.12)).frame(height: 1)
          }
          CapabilityMatrixRow(row: row)
        }
      }
    }
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous))
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous)
        .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
    )
  }

  // MARK: MCP servers (summary + collapsed merged table)

  private var serverLines: [McpServerLine] {
    McpServerLine.merge(
      servers: model.mcpServers, statusRows: model.mcpStatus.rows,
      results: model.mcpTestResults, pending: model.mcpTestPending)
  }

  /// True when every configured server is still waiting for the first agent
  /// turn; the table then carries one note instead of repeating it per row.
  private var allServersUnstarted: Bool {
    let rows = model.mcpStatus.rows.filter { $0.facet == .mcpServer }
    return !rows.isEmpty && rows.allSatisfy { $0.state == .configured }
  }

  private var mcpServers: some View {
    let lines = serverLines
    let tested = lines.filter { $0.testTone != .neutral && $0.testTone != .warn }.count
    let issues = lines.filter { $0.testTone == .bad || $0.status?.tone == .bad }.count
    return VStack(alignment: .leading, spacing: 0) {
      SettingsSectionLabel(String(localized: "MCP servers"))
      HStack(spacing: 6) {
        Text("Configuration source:")
          .font(CSFont.ui(11.5))
          .foregroundStyle(Color.secondary)
        Text(
          model.mcpStatus.configPathDisplay.replacingOccurrences(of: NSHomeDirectory(), with: "~")
        )
        .font(CSFont.mono(10, .medium))
        .foregroundStyle(Color.secondary)
        .lineLimit(1)
        .truncationMode(.middle)
        .textSelection(.enabled)
      }
      .padding(.top, 4)
      if model.mcpServers.isEmpty {
        statusCard(rows: model.mcpStatus.rows)
          .padding(.top, 8)
      } else {
        Text(
          "Configured: \(lines.count) · Tested: \(tested) · Issues: \(issues)",
          comment: "MCP summary counts"
        )
        .font(CSFont.ui(12.5, .semibold))
        .foregroundStyle(Color.primary)
        .padding(.top, 6)
        if allServersUnstarted {
          Text("The Agent has not run yet — server status is checked on its first turn.")
            .font(CSFont.ui(11.5))
            .foregroundStyle(Color.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.top, 2)
        }
        Text("Servers are added, tested and removed in the MCP tab.")
          .font(CSFont.ui(11.5))
          .foregroundStyle(Color.secondary)
          .padding(.top, 2)
        DisclosureGroup(isExpanded: $showingServerRows) {
          serverTable(lines: lines).padding(.top, 8)
        } label: {
          Text("Show servers")
            .font(CSFont.ui(11.5))
            .foregroundStyle(Color.secondary)
        }
        .padding(.top, 8)
        .accessibilityIdentifier("settings-diagnostics-mcp-servers")
      }
    }
  }

  /// Name · runtime status · test result · status word, one line per server.
  private func serverTable(lines: [McpServerLine]) -> some View {
    VStack(spacing: 0) {
      ForEach(Array(lines.enumerated()), id: \.offset) { index, line in
        if index > 0 {
          Rectangle().fill(Color.primary.opacity(0.12)).frame(height: 1)
        }
        McpServerLineRow(line: line, hideUnstartedStatus: allServersUnstarted)
      }
    }
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous))
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous)
        .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
    )
  }

  // MARK: Status card (readiness + MCP terminal states)

  @ViewBuilder
  private func statusCard(rows: [CsMcpStatusRow]) -> some View {
    VStack(spacing: 0) {
      ForEach(Array(rows.enumerated()), id: \.offset) { index, row in
        if index > 0 {
          Rectangle().fill(Color.primary.opacity(0.12)).frame(height: 1)
        }
        AgentStatusRow(row: row)
      }
    }
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous))
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous)
        .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
    )
  }
}

// MARK: - Status mark (dot + word, tooltip, VoiceOver)

struct StatusToneMark: View {
  let tone: CsMcpRowTone

  var body: some View {
    HStack(spacing: 5) {
      Circle().fill(tone.dotColor).frame(width: 7, height: 7)
      Text(tone.label)
        .font(CSFont.mono(10, .medium))
        .foregroundStyle(Color.secondary)
        .lineLimit(1)
    }
    .help(tone.label)
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(tone.label)
  }
}

// MARK: - One status row (label · value · status mark)

private struct AgentStatusRow: View {
  let row: CsMcpStatusRow

  var body: some View {
    HStack(alignment: .top, spacing: 12) {
      Text(row.localizedLabel)
        .font(CSFont.mono(12, .medium))
        .foregroundStyle(Color.secondary)
        .lineLimit(2)
        .fixedSize(horizontal: false, vertical: true)
        .frame(width: 190, alignment: .leading)
      Text(row.localizedValue)
        .font(CSFont.ui(12.5, .semibold))
        .foregroundStyle(Color.primary)
        .lineLimit(3)
        .fixedSize(horizontal: false, vertical: true)
        .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
      StatusToneMark(tone: row.tone)
    }
    .padding(.horizontal, 16)
    .padding(.vertical, 12)
    .accessibilityElement(children: .combine)
    .accessibilityLabel(Text(verbatim: "\(row.localizedLabel), \(row.tone.label)"))
    .accessibilityValue(row.localizedValue)
  }
}

// MARK: - One merged MCP server line

private struct McpServerLineRow: View {
  let line: McpServerLine
  let hideUnstartedStatus: Bool

  private var statusText: String? {
    guard let status = line.status else { return nil }
    if hideUnstartedStatus && status.state == .configured { return nil }
    return status.localizedValue
  }

  var body: some View {
    let statusText = statusText
    HStack(alignment: .top, spacing: 12) {
      Text(verbatim: line.name)
        .font(CSFont.mono(12, .medium))
        .foregroundStyle(Color.secondary)
        .lineLimit(1)
        .frame(width: 190, alignment: .leading)
      VStack(alignment: .leading, spacing: 2) {
        if let statusText {
          Text(statusText)
            .font(CSFont.ui(12.5, .semibold))
            .foregroundStyle(Color.primary)
            .lineLimit(2)
            .fixedSize(horizontal: false, vertical: true)
        }
        Text(line.testText)
          .font(statusText == nil ? CSFont.ui(12.5, .semibold) : CSFont.ui(11.5))
          .foregroundStyle(statusText == nil ? Color.primary : Color.secondary)
          .lineLimit(2)
          .fixedSize(horizontal: false, vertical: true)
      }
      .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
      StatusToneMark(tone: line.status?.tone == .bad ? .bad : line.testTone)
    }
    .padding(.horizontal, 16)
    .padding(.vertical, 12)
    .accessibilityElement(children: .combine)
    .accessibilityLabel(Text(verbatim: line.name))
    .accessibilityValue(
      Text(verbatim: [statusText, line.testText].compactMap { $0 }.joined(separator: ", ")))
  }
}

// MARK: - Capability matrix row (op · tier · headline)

private struct CapabilityMatrixRow: View {
  let row: CsCapabilityRow

  var body: some View {
    HStack(alignment: .top, spacing: 12) {
      Text(row.op)
        .font(CSFont.mono(12, .medium))
        .foregroundStyle(Color.secondary)
        .frame(width: 120, alignment: .leading)
      Text(row.localizedTier.uppercased())
        .font(CSFont.mono(10, .semibold))
        .tracking(0.3)
        .foregroundStyle(tierColor)
        .padding(.horizontal, 7)
        .padding(.vertical, 2)
        .background(
          RoundedRectangle(cornerRadius: 5, style: .continuous)
            .fill(tierColor.opacity(0.12))
        )
        .overlay(
          RoundedRectangle(cornerRadius: 5, style: .continuous)
            .strokeBorder(tierColor.opacity(0.28), lineWidth: 1)
        )
        .frame(width: 110, alignment: .leading)
      VStack(alignment: .leading, spacing: 2) {
        Text(row.localizedHeadline)
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(Color.primary)
          .lineLimit(2)
          .fixedSize(horizontal: false, vertical: true)
        if let detail = row.localizedDetail {
          Text(detail)
            .font(CSFont.mono(10, .medium))
            .foregroundStyle(Color.secondary)
            .lineLimit(1)
        }
      }
      .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
      Circle().fill(tierColor).frame(width: 7, height: 7).padding(.top, 5)
        .help(row.reason)
    }
    .padding(.horizontal, 16)
    .padding(.vertical, 12)
    .accessibilityElement(children: .combine)
    .accessibilityLabel(Text(verbatim: "\(row.op), \(row.localizedTier)"))
    .accessibilityValue(row.localizedHeadline)
  }

  private var tierColor: Color {
    switch row.tier.lowercased() {
    case "native": return CSColor.oliveLight
    case "enhanced": return CSColor.amber
    case "unavailable": return CSColor.terracotta
    default: return Color.secondary
    }
  }
}

// MARK: - Tone → color

extension CsMcpRowTone {
  /// Map the UI-agnostic core tone onto concrete brand tokens.
  var dotColor: Color {
    switch self {
    case .good: return CSColor.oliveLight
    case .warn: return CSColor.amber
    case .bad: return CSColor.terracotta
    case .neutral: return Color.secondary
    }
  }
}
