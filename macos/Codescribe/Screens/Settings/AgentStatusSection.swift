import SwiftUI

// Agent-substrate status, rendered inside Agent Diagnostics (READ-ONLY runtime
// truth). Surfaces the previously built-but-dead readiness + MCP status probes:
// the agentic-lane verdict (Vibecrafted + AICX + Loctree + PRView) and the
// per-server MCP status. A "Refresh" action re-probes without touching the rest
// of the panel. Degrades gracefully: a missing mcp.json shows a neutral
// "MCP off" row, never an error.

struct AgentStatusSection: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      header

      // Agentic readiness verdict + per-prerequisite rows.
      statusCard(rows: model.agentReadiness.rows)
        .padding(.top, CSSpace.control)

      SettingsSectionLabel(String(localized: "Connection details"))
        .padding(.top, CSSpace.section)
      Text(model.creatorAgentBridgeStatus.detail)
        .font(CSFont.ui(11.5))
        .foregroundStyle(Color.secondary)
        .fixedSize(horizontal: false, vertical: true)
        .padding(.top, 4)
      ForEach(model.creatorAgentBridgeStatus.installedPaths, id: \.self) { path in
        Text(path.replacingOccurrences(of: NSHomeDirectory(), with: "~"))
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(Color.secondary)
          .textSelection(.enabled)
          .padding(.top, 4)
      }
      if let launchDetail = model.creatorAgentBridgeLaunchDetail {
        Text(verbatim: launchDetail)
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(Color.secondary)
          .textSelection(.enabled)
          .fixedSize(horizontal: false, vertical: true)
          .padding(.top, 4)
      }

      SettingsSectionLabel(String(localized: "Capability matrix"))
        .padding(.top, CSSpace.section)
      Text("Native substrate vs enrichment providers (IntelliJ optional).")
        .font(CSFont.ui(11.5))
        .foregroundStyle(Color.secondary)
        .padding(.top, 4)
      capabilityMatrixCard
        .padding(.top, 8)

      SettingsSectionLabel(String(localized: "MCP servers"))
        .padding(.top, CSSpace.section)
      Text(model.mcpStatus.configPathDisplay)
        .font(CSFont.mono(10, .medium))
        .foregroundStyle(Color.secondary)
        .lineLimit(1)
        .truncationMode(.middle)
        .padding(.top, 4)
      statusCard(rows: model.mcpStatus.rows)
        .padding(.top, 8)

      // Per-server health probe. Reflects the cached Test / handshake result
      // from the MCP servers tab; purely informational and never flips the
      // readiness verdict above. Shown whole — the Diagnostics tab has room.
      if !model.mcpServers.isEmpty {
        HStack(spacing: 10) {
          SettingsSectionLabel(String(localized: "Per-server probe"))
          Spacer(minLength: 0)
          Text("\(model.mcpServers.count) configured")
            .font(CSFont.mono(10, .medium))
            .foregroundStyle(Color.secondary)
        }
        .padding(.top, CSSpace.section)
        .help("Cached initialize + tools/list result per configured server")
        statusCard(rows: probeRows)
          .padding(.top, 8)
      }
    }
    .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
  }

  // MARK: Per-server probe

  /// One row per configured server: the cached probe status (ok / fail /
  /// testing / not tested) mapped to a tone the shared status card renders.
  private var probeRows: [CsMcpStatusRow] {
    model.mcpServers.map { server in
      if model.mcpTestPending.contains(server.name) {
        return CsMcpStatusRow(
          label: server.name, value: String(localized: "testing…"), tone: .warn)
      }
      guard let result = model.mcpTestResults[server.name] else {
        return CsMcpStatusRow(
          label: server.name, value: String(localized: "not tested"), tone: .neutral)
      }
      if result.ok {
        var value = String(localized: "ok — \(Int(result.toolCount)) tools")
        if !result.serverVersion.isEmpty { value += " · v\(result.serverVersion)" }
        return CsMcpStatusRow(label: server.name, value: value, tone: .good)
      }
      return CsMcpStatusRow(
        label: server.name,
        value: String(
          localized: "fail: \(result.error)",
          comment: "The placeholder is an error message composed by the MCP probe"),
        tone: .bad)
    }
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

  // MARK: Capability matrix

  private var capabilityMatrixCard: some View {
    VStack(spacing: 0) {
      if model.capabilityMatrix.isEmpty {
        AgentStatusRow(
          row: CsMcpStatusRow(
            label: "matrix",
            value: String(localized: "no capability rows (refresh or start agent substrate)"),
            tone: .neutral
          )
        )
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

  // MARK: Status card (shared by readiness + MCP)

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

// MARK: - One status row (label · value · tone dot)

private struct AgentStatusRow: View {
  let row: CsMcpStatusRow

  var body: some View {
    HStack(spacing: 12) {
      Text(row.label)
        .font(CSFont.mono(12, .medium))
        .foregroundStyle(Color.secondary)
        .frame(width: 160, alignment: .leading)
      Text(row.value)
        .font(CSFont.ui(12.5, .semibold))
        .foregroundStyle(Color.primary)
        .lineLimit(2)
        .fixedSize(horizontal: false, vertical: true)
        .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
      Circle().fill(row.tone.dotColor).frame(width: 7, height: 7)
    }
    .padding(.horizontal, 16)
    .padding(.vertical, 12)
  }
}

// MARK: - Capability matrix row (op · tier · reason)

private struct CapabilityMatrixRow: View {
  let row: CsCapabilityRow

  var body: some View {
    HStack(alignment: .top, spacing: 12) {
      Text(row.op)
        .font(CSFont.mono(12, .medium))
        .foregroundStyle(Color.secondary)
        .frame(width: 120, alignment: .leading)
      Text(row.tier.uppercased())
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
        .frame(width: 96, alignment: .leading)
      VStack(alignment: .leading, spacing: 2) {
        Text(row.reason.isEmpty ? row.provider : row.reason)
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(Color.primary)
          .lineLimit(2)
          .fixedSize(horizontal: false, vertical: true)
        if !row.nativeTool.isEmpty {
          Text(verbatim: "tool: \(row.nativeTool) · provider: \(row.provider)")
            .font(CSFont.mono(10, .medium))
            .foregroundStyle(Color.secondary)
            .lineLimit(1)
        } else if !row.provider.isEmpty {
          Text(verbatim: "provider: \(row.provider)")
            .font(CSFont.mono(10, .medium))
            .foregroundStyle(Color.secondary)
            .lineLimit(1)
        }
      }
      .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
      Circle().fill(tierColor).frame(width: 7, height: 7).padding(.top, 5)
    }
    .padding(.horizontal, 16)
    .padding(.vertical, 12)
    .accessibilityElement(children: .combine)
    .accessibilityLabel(Text(verbatim: "\(row.op), \(row.tier)"))
    .accessibilityValue(row.reason)
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
