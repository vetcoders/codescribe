import SwiftUI

// Agent-substrate status, rendered inside the Engine panel (READ-ONLY runtime
// truth). Surfaces the previously built-but-dead readiness + MCP status probes:
// the agentic-lane verdict (Vibecrafted + AICX + Loctree + PRView) and the
// per-server MCP status. A "Refresh" action re-probes without touching the rest
// of the panel. Degrades gracefully: a missing mcp.json shows a neutral
// "MCP off" row, never an error.

struct AgentStatusSection: View {
    @ObservedObject var model: SettingsViewModel

    /// Collapsed by default: the per-server health probe is an informational
    /// drill-down, not a readiness input, so it stays out of the way until asked.
    @State private var probesExpanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header

            // Agentic readiness verdict + per-prerequisite rows.
            statusCard(rows: model.agentReadiness.rows)
                .padding(.top, 11)

            SettingsSectionLabel("MCP servers")
                .padding(.top, 22)
            Text(model.mcpStatus.configPathDisplay)
                .font(CSFont.mono(10, .medium))
                .foregroundStyle(CSColor.textFaint)
                .lineLimit(1)
                .truncationMode(.middle)
                .padding(.top, 4)
            statusCard(rows: model.mcpStatus.rows)
                .padding(.top, 8)

            // Collapsible per-server health probe. Reflects the cached Test /
            // handshake result from the management section below; purely
            // informational and never flips the readiness verdict above.
            if !model.mcpServers.isEmpty {
                probeDisclosure
                    .padding(.top, 13)
            }
        }
    }

    // MARK: Per-server probe (collapsible)

    /// One row per configured server: the cached probe status (ok / fail /
    /// testing / not tested) mapped to a tone the shared status card renders.
    private var probeRows: [CsMcpStatusRow] {
        model.mcpServers.map { server in
            if model.mcpTestPending.contains(server.name) {
                return CsMcpStatusRow(label: server.name, value: "testing…", tone: .warn)
            }
            guard let result = model.mcpTestResults[server.name] else {
                return CsMcpStatusRow(label: server.name, value: "not tested", tone: .neutral)
            }
            if result.ok {
                var value = "ok — \(result.toolCount) tool(s)"
                if !result.serverVersion.isEmpty { value += " · v\(result.serverVersion)" }
                return CsMcpStatusRow(label: server.name, value: value, tone: .good)
            }
            return CsMcpStatusRow(label: server.name, value: "fail: \(result.error)", tone: .bad)
        }
    }

    private var probeDisclosure: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.easeOut(duration: 0.18)) { probesExpanded.toggle() }
            } label: {
                HStack(spacing: 7) {
                    CSIconView(
                        icon: probesExpanded ? .chevronDown : .chevronRight,
                        size: 10, weight: .semibold, color: CSColor.textMuted
                    )
                    Text("Per-server probe")
                        .font(CSFont.mono(11, .semibold))
                        .foregroundStyle(CSColor.textMutedAlt)
                    Spacer(minLength: 0)
                    Text("\(model.mcpServers.count) configured")
                        .font(CSFont.mono(10, .medium))
                        .foregroundStyle(CSColor.textFaint)
                }
            }
            .buttonStyle(.plain)
            .help("Cached initialize + tools/list result per configured server")

            if probesExpanded {
                statusCard(rows: probeRows)
                    .padding(.top, 8)
            }
        }
    }

    // MARK: Header + refresh

    private var header: some View {
        HStack(spacing: 10) {
            SettingsSectionLabel("Agent readiness")
            readinessPill
            Spacer(minLength: 0)
            Button {
                model.refreshAgentStatus()
            } label: {
                HStack(spacing: 5) {
                    CSIconView(icon: .refresh, size: 11, weight: .semibold)
                    Text("Refresh").font(CSFont.mono(11, .semibold))
                }
                .foregroundStyle(CSColor.textBodyAlt)
                .padding(.horizontal, 10)
                .padding(.vertical, 5)
                .background(
                    RoundedRectangle(cornerRadius: 7, style: .continuous)
                        .fill(CSColor.surfaceRaised(0.04))
                )
                .overlay(
                    RoundedRectangle(cornerRadius: 7, style: .continuous)
                        .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
                )
            }
            .buttonStyle(.plain)
        }
    }

    private var readinessPill: some View {
        let ready = model.agentReadiness.ready
        let accent = ready ? CSColor.olive : CSColor.terracotta
        let accentLight = ready ? CSColor.oliveLight : CSColor.terracottaLight
        return Text(ready ? "READY" : "NOT READY")
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

    // MARK: Status card (shared by readiness + MCP)

    @ViewBuilder
    private func statusCard(rows: [CsMcpStatusRow]) -> some View {
        VStack(spacing: 0) {
            ForEach(Array(rows.enumerated()), id: \.offset) { index, row in
                if index > 0 {
                    Rectangle().fill(CSColor.hairline(0.05)).frame(height: 1)
                }
                AgentStatusRow(row: row)
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 13, style: .continuous)
                .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
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
                .foregroundStyle(CSColor.textMutedAlt)
                .frame(width: 160, alignment: .leading)
            Text(row.value)
                .font(CSFont.ui(12.5, .semibold))
                .foregroundStyle(CSColor.textHigh)
                .lineLimit(2)
                .frame(maxWidth: .infinity, alignment: .leading)
            Circle().fill(row.tone.dotColor).frame(width: 7, height: 7)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
    }
}

// MARK: - Tone → color

extension CsMcpRowTone {
    /// Map the UI-agnostic core tone onto concrete brand tokens.
    var dotColor: Color {
        switch self {
        case .good: return CSColor.oliveLight
        case .warn: return CSColor.amber
        case .bad: return CSColor.terracottaLight
        case .neutral: return CSColor.textFaint
        }
    }
}
