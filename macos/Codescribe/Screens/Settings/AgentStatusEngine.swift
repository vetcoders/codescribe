import Foundation

// Seam between the Settings screen and the REAL codescribe agent-status probes
// through the UniFFI bridge (CodescribeAgentStatus). Read-only: it reports the
// agentic-lane readiness verdict and the MCP server status. Mirrors the
// SettingsEngine seam so #Preview can inject deterministic data while the live
// app injects `RealAgentStatusEngine`.
//
// Nothing here mutates config — MCP editing is a separate cut. Both bridge calls
// are synchronous, cheap on-disk reads (parse mcp.json + merge the last runtime
// discovery), so there are no Rust callbacks to hop onto the main actor.

/// Read-only agent-substrate status surface the Settings screen consumes.
protocol AgentStatusEngine {
    /// Agentic-lane readiness (Vibecrafted + AICX + Loctree + PRView).
    func agenticReadiness() -> CsAgenticReadiness
    /// Basic-lane MCP config + runtime status. Missing mcp.json → neutral row.
    func mcpStatus() -> CsMcpStatusReport
}

// MARK: - Real engine (UniFFI bridge adapter)

/// Concrete adapter over the `CodescribeAgentStatus` bridge object. Stateless:
/// every call re-reads config truth so Swift always sees on-disk state. Injected
/// by App.swift for the live app.
final class RealAgentStatusEngine: AgentStatusEngine {
    private let status = CodescribeAgentStatus()

    func agenticReadiness() -> CsAgenticReadiness { status.agenticReadiness() }
    func mcpStatus() -> CsMcpStatusReport { status.mcpStatus() }
}

// MARK: - Mock engine (previews)

/// In-memory stand-in for #Preview and standalone rendering.
struct MockAgentStatusEngine: AgentStatusEngine {
    var readiness: CsAgenticReadiness = .sample
    var mcp: CsMcpStatusReport = .sample

    func agenticReadiness() -> CsAgenticReadiness { readiness }
    func mcpStatus() -> CsMcpStatusReport { mcp }
}

// MARK: - Bridge value helpers (preview seeds)

extension CsMcpStatusReport {
    /// Sample MCP status with a mix of live / pending servers (preview seed).
    static let sample = CsMcpStatusReport(
        configPathDisplay: "~/.codescribe/mcp.json",
        rows: [
            CsMcpStatusRow(label: "loctree-mcp:", value: "9 tool(s)", tone: .good),
            CsMcpStatusRow(label: "aicx-mcp:", value: "configured (agent not started)", tone: .warn),
            CsMcpStatusRow(label: "vibecrafted-mcp:", value: "failed: command not found", tone: .bad)
        ]
    )
}

extension CsAgenticReadiness {
    /// Sample readiness with one blocking prerequisite (preview seed).
    static let sample = CsAgenticReadiness(
        configPathDisplay: "~/.codescribe/mcp.json",
        ready: false,
        rows: [
            CsMcpStatusRow(
                label: "Agentic readiness:",
                value: "not ready — 1 prerequisite(s) missing",
                tone: .bad
            ),
            CsMcpStatusRow(label: "Vibecrafted runtime:", value: "ready — 3 tool(s) live", tone: .good),
            CsMcpStatusRow(label: "AICX MCP:", value: "configured — agent not started yet", tone: .warn),
            CsMcpStatusRow(label: "Loctree MCP:", value: "ready — 9 tool(s) live", tone: .good),
            CsMcpStatusRow(
                label: "PRView integration:",
                value: "missing (required) — no PRView MCP server or local command found",
                tone: .bad
            )
        ]
    )
}
