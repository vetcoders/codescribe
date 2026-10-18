import SwiftUI

// Agent-substrate status, rendered inside Agent Diagnostics (READ-ONLY runtime
// truth). Environment state first, then what needs attention, then detail on
// demand (Founder brief, round 11, 2026-10-10): one summary card, a compact
// Integrations list, two collapsible summaries for tools and MCP servers, the
// detected installations, and one quiet "Technical details" group holding the
// paths, the installer and launch messages and the full readiness table.
//
// Nothing is dropped on the way: every row the probe reports is still reachable,
// and a real error stays visible without expanding anything. Tool permissions
// and server management live in the Tools and MCP tabs. A "Refresh" action
// re-probes without touching the rest of the panel. Degrades gracefully: a
// missing mcp.json shows a neutral "MCP off" row, never an error.

struct AgentStatusSection: View {
  @ObservedObject var model: SettingsViewModel
  @State private var showingCapabilityRows = false
  @State private var showingServerRows = false
  @State private var showingTechnicalDetails = false

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      summaryCard

      integrations
        .padding(.top, SettingsScale.sectionGap)

      toolsAndServers
        .padding(.top, SettingsScale.sectionGap)

      installations
        .padding(.top, SettingsScale.sectionGap)

      technicalDetails
        .padding(.top, SettingsScale.sectionGap)
    }
    .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
  }

  // MARK: Summary card (verdict · key facts · attention)

  private var summaryModel: AgentStatusSummary {
    AgentStatusSummary(
      readiness: model.agentReadiness, capabilities: model.capabilityMatrix,
      servers: serverLines, mcpStatus: model.mcpStatus,
      accessResolved: model.providerAccessResolved, accessError: model.providerAccessError)
  }

  private var summaryCard: some View {
    let summary = summaryModel
    return VStack(alignment: .leading, spacing: 0) {
      SettingsSectionHeaderRow(
        String(localized: "Agent status", comment: "Diagnostics section header")
      ) {
        SettingsRefreshButton(axLabel: "Refresh agent readiness") {
          model.refreshAgentStatus()
          model.refreshCreatorAgentBridge()
        }
      }
      VStack(alignment: .leading, spacing: 0) {
        statusLine(tone: summary.tone, text: summary.headline, prominent: true)
        if !summary.facts.isEmpty {
          Text(summary.facts)
            .font(SettingsScale.helper)
            .foregroundStyle(Color.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.top, 3)
            .padding(.leading, 20)
        }
        if !summary.notes.isEmpty {
          Rectangle().fill(Color.primary.opacity(0.12)).frame(height: 1)
            .padding(.vertical, SettingsScale.rowGap)
          VStack(alignment: .leading, spacing: CSSpace.xs) {
            // Says how to read the list, so a warning under "Configuration
            // ready" is not taken for a failure.
            if !summary.notesCaption.isEmpty {
              Text(summary.notesCaption)
                .font(SettingsScale.helper)
                .foregroundStyle(Color.secondary)
                .padding(.leading, 20)
            }
            ForEach(Array(summary.notes.enumerated()), id: \.offset) { _, note in
              statusLine(tone: note.tone, text: note.text, prominent: false)
            }
          }
        }
      }
      .padding(.horizontal, SettingsScale.rowInsetH)
      .padding(.vertical, SettingsScale.cardPadding)
      .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
      .clipShape(RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous))
      .overlay(
        RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous)
          .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
      )
      .padding(.top, SettingsScale.headerGap)
      .accessibilityIdentifier("settings-diagnostics-summary")
    }
  }

  /// Toned glyph + text. The glyph carries the colour, the text carries the
  /// meaning, and the accessibility label repeats the status word so VoiceOver
  /// never depends on the colour.
  private func statusLine(tone: CsMcpRowTone, text: String, prominent: Bool) -> some View {
    HStack(alignment: .firstTextBaseline, spacing: CSSpace.sm) {
      CSIconView(icon: tone.statusIcon, size: 11, weight: .semibold, color: tone.dotColor)
        .frame(width: 12, alignment: .leading)
        .accessibilityHidden(true)
      Text(text)
        .font(prominent ? SettingsScale.settingName : SettingsScale.helper)
        .foregroundStyle(Color.primary)
        .fixedSize(horizontal: false, vertical: true)
        .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
    }
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(Text(verbatim: "\(tone.label), \(text)"))
  }

  // MARK: Integrations (one row, one status)

  private var integrations: some View {
    let lines = AgentIntegrationLine.lines(rows: model.agentReadiness.rows)
    return VStack(alignment: .leading, spacing: 0) {
      SettingsSectionHeaderRow(
        String(localized: "Integrations", comment: "Diagnostics section header"))
      listCard {
        if lines.isEmpty {
          compactRow(
            name: String(
              localized: "Optional integrations",
              comment: "Diagnostics row label when no integration has been probed"),
            status: CsMcpRowTone.neutral.label, tone: .neutral, tooltip: nil)
        } else {
          ForEach(Array(lines.enumerated()), id: \.offset) { index, line in
            if index > 0 { rowDivider }
            compactRow(
              name: line.name, status: line.status, tone: line.tone,
              tooltip: line.serverName.isEmpty
                ? nil
                : String(
                  localized: "Reads the mcp.json entry \(line.serverName)",
                  comment:
                    "Tooltip on an integration row; placeholder is a server name from mcp.json"))
          }
        }
      }
      .padding(.top, SettingsScale.headerGap)
      .accessibilityIdentifier("settings-diagnostics-integrations")
    }
  }

  // MARK: Tools and servers (two independent collapsible summaries)

  private var toolsAndServers: some View {
    let capabilities = CapabilitySummary(rows: model.capabilityMatrix)
    let servers = McpServerSummary(lines: serverLines)
    return VStack(alignment: .leading, spacing: 0) {
      SettingsSectionHeaderRow(
        String(localized: "Tools and servers", comment: "Diagnostics section header"))
      listCard {
        DisclosureGroup(isExpanded: $showingCapabilityRows) {
          VStack(alignment: .leading, spacing: SettingsScale.rowGap) {
            Text("Native capabilities are served by Codescribe's built-in tools.")
              .font(SettingsScale.helper)
              .foregroundStyle(Color.secondary)
              .fixedSize(horizontal: false, vertical: true)
            capabilityMatrixCard
            Text("Tool permissions are managed in the Tools tab.")
              .font(SettingsScale.helper)
              .foregroundStyle(Color.secondary)
              .fixedSize(horizontal: false, vertical: true)
          }
          .padding(.top, SettingsScale.rowGap)
        } label: {
          summaryLabel(
            title: String(localized: "Agent tools", comment: "Diagnostics collapsible section"),
            detail: capabilities.line, tone: capabilities.unavailable > 0 ? .warn : .good)
        }
        .padding(.horizontal, SettingsScale.rowInsetH)
        .padding(.vertical, SettingsScale.cardPadding)
        .accessibilityIdentifier("settings-diagnostics-capabilities")

        rowDivider

        DisclosureGroup(isExpanded: $showingServerRows) {
          VStack(alignment: .leading, spacing: SettingsScale.rowGap) {
            if model.mcpServers.isEmpty {
              statusCard(rows: model.mcpStatus.rows)
            } else {
              serverTable(lines: serverLines)
            }
            Text("Servers are added, tested and removed in the MCP tab.")
              .font(SettingsScale.helper)
              .foregroundStyle(Color.secondary)
              .fixedSize(horizontal: false, vertical: true)
          }
          .padding(.top, SettingsScale.rowGap)
        } label: {
          summaryLabel(
            title: String(localized: "MCP servers"),
            detail: model.mcpServers.isEmpty ? mcpConfigStateText : servers.line,
            tone: model.mcpServers.isEmpty ? mcpConfigStateTone : servers.tone)
        }
        .padding(.horizontal, SettingsScale.rowInsetH)
        .padding(.vertical, SettingsScale.cardPadding)
        .accessibilityIdentifier("settings-diagnostics-mcp-servers")
      }
      .padding(.top, SettingsScale.headerGap)
    }
  }

  /// Title + one quiet count line, the label of a collapsible summary.
  private func summaryLabel(title: String, detail: String, tone: CsMcpRowTone) -> some View {
    VStack(alignment: .leading, spacing: 2) {
      Text(title)
        .font(SettingsScale.settingName)
        .foregroundStyle(Color.primary)
      HStack(spacing: CSSpace.xs) {
        CSIconView(icon: tone.statusIcon, size: 10, weight: .semibold, color: tone.dotColor)
          .accessibilityHidden(true)
        Text(detail)
          .font(SettingsScale.helper)
          .foregroundStyle(Color.secondary)
          .fixedSize(horizontal: false, vertical: true)
      }
    }
    .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(Text(verbatim: title))
    .accessibilityValue(Text(verbatim: "\(tone.label), \(detail)"))
  }

  /// The `mcp.json` state itself, shown as the MCP summary when no server is
  /// configured: missing, empty, unreadable, or the concrete read error.
  private var mcpConfigRow: CsMcpStatusRow? {
    model.mcpStatus.rows.first { $0.facet == .mcpConfig }
  }

  private var mcpConfigStateText: String {
    mcpConfigRow?.localizedValue
      ?? String(localized: "No mcp.json — MCP off (optional)")
  }

  private var mcpConfigStateTone: CsMcpRowTone { mcpConfigRow?.tone ?? .neutral }

  private var capabilityMatrixCard: some View {
    listCard {
      if model.capabilityMatrix.isEmpty {
        Text("no capability rows (refresh or start agent substrate)")
          .font(SettingsScale.settingName)
          .foregroundStyle(Color.primary)
          .padding(.horizontal, SettingsScale.rowInsetH)
          .padding(.vertical, SettingsScale.rowInsetV)
          .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
      } else {
        ForEach(Array(model.capabilityMatrix.enumerated()), id: \.offset) { index, row in
          if index > 0 { rowDivider }
          CapabilityMatrixRow(row: row)
        }
      }
    }
  }

  private var serverLines: [McpServerLine] {
    McpServerLine.merge(
      servers: model.mcpServers, statusRows: model.mcpStatus.rows,
      results: model.mcpTestResults, pending: model.mcpTestPending)
  }

  /// Name + exactly one status, one line per server.
  private func serverTable(lines: [McpServerLine]) -> some View {
    listCard {
      ForEach(Array(lines.enumerated()), id: \.offset) { index, line in
        if index > 0 { rowDivider }
        compactRow(
          name: line.name, status: line.localizedSingleStatus, tone: line.singleStatusTone,
          tooltip: nil)
      }
    }
  }

  // MARK: Detected installations

  /// One compact row per detected client. The managed paths, the installer's
  /// own evidence line and the launch synchronization notice are technical
  /// detail and live under "Technical details".
  private var installations: some View {
    let status = model.creatorAgentBridgeStatus
    return VStack(alignment: .leading, spacing: 0) {
      SettingsSectionHeaderRow(
        String(localized: "Detected installations", comment: "Diagnostics section header"))
      listCard {
        if status.installedClients.isEmpty {
          compactRow(
            name: String(
              localized: "Managed skill",
              comment: "Diagnostics row label for the agent-bridge skill installation"),
            status: String(
              localized: "Not detected", comment: "No agent client has the managed skill"),
            tone: .neutral, tooltip: nil)
        } else {
          ForEach(Array(status.installedClients.enumerated()), id: \.offset) { index, client in
            if index > 0 { rowDivider }
            installationRow(
              client: client, needsRepair: status.clientsNeedingRepair.contains(client))
          }
        }
      }
      .padding(.top, SettingsScale.headerGap)
      .accessibilityIdentifier("settings-diagnostics-installations")
    }
  }

  /// A repair need is actionable, so it stays on the default screen; the path
  /// and the installer's own message are technical detail.
  private func installationRow(client: AgentBridgeClient, needsRepair: Bool) -> some View {
    compactRow(
      name: client.displayName,
      status: needsRepair
        ? String(
          localized: "Needs repair",
          comment: "A detected installation does not match the shipped payload")
        : String(localized: "Detected", comment: "An installation of an agent client was found"),
      tone: needsRepair ? .warn : .good, tooltip: nil)
  }

  // MARK: Technical details (collapsed by default)

  private var technicalDetails: some View {
    let status = model.creatorAgentBridgeStatus
    let paths = Array(zip(status.installedClients, status.installedPaths))
    return DisclosureGroup(isExpanded: $showingTechnicalDetails) {
      VStack(alignment: .leading, spacing: SettingsScale.rowGap) {
        ForEach(paths, id: \.0) { client, path in
          Text(verbatim: "\(client.displayName): \(tilde(path))")
            .font(SettingsScale.metaKey)
            .foregroundStyle(Color.secondary)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
        }
        Text(status.detail)
          .font(SettingsScale.helper)
          .foregroundStyle(Color.secondary)
          .fixedSize(horizontal: false, vertical: true)
        if let launchDetail = model.creatorAgentBridgeLaunchDetail {
          Text(verbatim: launchDetail)
            .font(SettingsScale.metaKey)
            .foregroundStyle(Color.secondary)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityIdentifier("settings-diagnostics-launch-detail")
        }
        HStack(spacing: CSSpace.xs) {
          Text("Configuration source:")
            .font(SettingsScale.helper)
            .foregroundStyle(Color.secondary)
          Text(tilde(model.mcpStatus.configPathDisplay))
            .font(SettingsScale.metaKey)
            .foregroundStyle(Color.secondary)
            .lineLimit(1)
            .truncationMode(.middle)
            .textSelection(.enabled)
        }
        VStack(alignment: .leading, spacing: SettingsScale.headerGap) {
          SettingsSectionLabel(String(localized: "Agent readiness"))
          statusCard(rows: model.agentReadiness.rows)
        }
      }
      .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
      .padding(.top, SettingsScale.rowGap)
    } label: {
      Text("Technical details")
        .font(SettingsScale.helper)
        .foregroundStyle(Color.secondary)
    }
    .accessibilityIdentifier("settings-diagnostics-technical-details")
  }

  // MARK: Shared chrome

  private var rowDivider: some View {
    Rectangle().fill(Color.primary.opacity(0.12)).frame(height: 1)
  }

  /// Bordered list container shared by the compact sections.
  private func listCard<Content: View>(@ViewBuilder content: () -> Content) -> some View {
    VStack(spacing: 0) { content() }
      .clipShape(RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous))
      .overlay(
        RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous)
          .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
      )
  }

  /// Name on the left, exactly one toned status on the right. Long names wrap
  /// instead of clipping, and the status keeps a bounded trailing column so a
  /// long error reason cannot push the name out of the pane.
  ///
  /// This row IS the page's list pattern (Founder review, 2026-10-18): every
  /// other list on the page carries the same insets, rungs and dividers, and
  /// the two wider tables add a column rather than a second row shape.
  private func compactRow(
    name: String, status: String, tone: CsMcpRowTone, tooltip: String?
  ) -> some View {
    HStack(alignment: .firstTextBaseline, spacing: CSSpace.md) {
      Text(verbatim: name)
        .font(SettingsScale.settingName)
        .foregroundStyle(Color.primary)
        .fixedSize(horizontal: false, vertical: true)
        .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
      HStack(alignment: .firstTextBaseline, spacing: CSSpace.xs) {
        CSIconView(icon: tone.statusIcon, size: 10, weight: .semibold, color: tone.dotColor)
          .accessibilityHidden(true)
        Text(status)
          .font(SettingsScale.helper)
          .foregroundStyle(Color.primary)
          .multilineTextAlignment(.trailing)
          .fixedSize(horizontal: false, vertical: true)
      }
      .frame(minWidth: 0, maxWidth: 220, alignment: .trailing)
    }
    .padding(.horizontal, SettingsScale.rowInsetH)
    .padding(.vertical, SettingsScale.rowInsetV)
    .help(Text(verbatim: tooltip ?? status))
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(Text(verbatim: name))
    .accessibilityValue(Text(verbatim: "\(tone.label), \(status)"))
  }

  private func tilde(_ path: String) -> String {
    path.replacingOccurrences(of: NSHomeDirectory(), with: "~")
  }

  // MARK: Status card (the full readiness table, kept under technical details)

  @ViewBuilder
  private func statusCard(rows: [CsMcpStatusRow]) -> some View {
    listCard {
      ForEach(Array(rows.enumerated()), id: \.offset) { index, row in
        if index > 0 { rowDivider }
        AgentStatusRow(row: row)
      }
    }
  }
}

// MARK: - Status mark (dot + word, tooltip, VoiceOver)

struct StatusToneMark: View {
  let tone: CsMcpRowTone

  var body: some View {
    HStack(spacing: CSSpace.xxs) {
      Circle().fill(tone.dotColor).frame(width: 7, height: 7)
      Text(tone.label)
        .font(SettingsScale.statusBadge)
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
    HStack(alignment: .top, spacing: CSSpace.md) {
      Text(row.localizedLabel)
        .font(SettingsScale.helper)
        .foregroundStyle(Color.secondary)
        .lineLimit(2)
        .fixedSize(horizontal: false, vertical: true)
        .frame(width: 170, alignment: .leading)
      Text(row.localizedValue)
        .font(SettingsScale.settingName)
        .foregroundStyle(Color.primary)
        .lineLimit(3)
        .fixedSize(horizontal: false, vertical: true)
        .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
      StatusToneMark(tone: row.tone)
    }
    .padding(.horizontal, SettingsScale.rowInsetH)
    .padding(.vertical, SettingsScale.rowInsetV)
    .accessibilityElement(children: .combine)
    .accessibilityLabel(Text(verbatim: "\(row.localizedLabel), \(row.tone.label)"))
    .accessibilityValue(row.localizedValue)
  }
}

// MARK: - Capability matrix row (op · tier · qualifier)

private struct CapabilityMatrixRow: View {
  let row: CsCapabilityRow

  var body: some View {
    HStack(alignment: .top, spacing: CSSpace.md) {
      Text(row.op)
        .font(SettingsScale.metaKey)
        .foregroundStyle(Color.secondary)
        .frame(width: 110, alignment: .leading)
      Text(row.localizedTier.uppercased())
        .font(SettingsScale.statusBadge)
        .tracking(0.3)
        .foregroundStyle(tierColor)
        .padding(.horizontal, CSSpace.xs)
        .padding(.vertical, 2)
        .background(
          RoundedRectangle(cornerRadius: 5, style: .continuous)
            .fill(tierColor.opacity(0.12))
        )
        .overlay(
          RoundedRectangle(cornerRadius: 5, style: .continuous)
            .strokeBorder(tierColor.opacity(0.28), lineWidth: 1)
        )
        .frame(width: 100, alignment: .leading)
      VStack(alignment: .leading, spacing: 2) {
        if let qualifier = row.localizedQualifier {
          Text(qualifier)
            .font(SettingsScale.settingName)
            .foregroundStyle(Color.primary)
            .lineLimit(2)
            .fixedSize(horizontal: false, vertical: true)
        }
        if let detail = row.localizedDetail {
          Text(detail)
            .font(SettingsScale.helper)
            .foregroundStyle(Color.secondary)
            .lineLimit(1)
        }
      }
      .frame(minWidth: 0, maxWidth: .infinity, alignment: .leading)
    }
    .padding(.horizontal, SettingsScale.rowInsetH)
    .padding(.vertical, SettingsScale.rowInsetV)
    .help(row.reason)
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

// MARK: - Tone → color and glyph

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

  /// Glyph for the places that show an icon instead of a dot. A ready state is
  /// a thin check, never a filled badge, so green cannot visually outweigh a
  /// warning (Founder brief, round 11, 2026-10-10).
  var statusIcon: CSIcon {
    switch self {
    case .good: return .check
    case .warn: return .warning
    case .bad: return .error
    case .neutral: return .circleEmpty
    }
  }
}
