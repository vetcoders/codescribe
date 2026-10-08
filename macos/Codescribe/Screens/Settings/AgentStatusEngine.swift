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
  /// Provider-neutral capability matrix (native / enhanced / unavailable).
  func capabilityMatrix() -> [CsCapabilityRow]
}

// MARK: - Real engine (UniFFI bridge adapter)

/// Concrete adapter over the `CodescribeAgentStatus` bridge object. Stateless:
/// every call re-reads config truth so Swift always sees on-disk state. Injected
/// by App.swift for the live app.
final class RealAgentStatusEngine: AgentStatusEngine {
  private let status = CodescribeAgentStatus()

  func agenticReadiness() -> CsAgenticReadiness { status.agenticReadiness() }
  func mcpStatus() -> CsMcpStatusReport { status.mcpStatus() }
  func capabilityMatrix() -> [CsCapabilityRow] { status.capabilityMatrix() }
}

// MARK: - Mock engine (previews)

/// In-memory stand-in for #Preview and standalone rendering.
struct MockAgentStatusEngine: AgentStatusEngine {
  var readiness: CsAgenticReadiness = .sample
  var mcp: CsMcpStatusReport = .sample
  var matrix: [CsCapabilityRow] = CsCapabilityRow.sampleMatrix

  func agenticReadiness() -> CsAgenticReadiness { readiness }
  func mcpStatus() -> CsMcpStatusReport { mcp }
  func capabilityMatrix() -> [CsCapabilityRow] { matrix }
}

// MARK: - Bridge value helpers (preview seeds)

extension CsMcpStatusReport {
  /// Sample MCP status with a mix of live / pending servers (preview seed).
  static let sample = CsMcpStatusReport(
    configPathDisplay: "~/.codescribe/mcp.json",
    configured: true,
    rows: [
      CsMcpStatusRow(
        label: "loctree-mcp:", value: "9 tool(s)", tone: .good,
        facet: .mcpServer, state: .live, count: 9, subject: "loctree-mcp", detail: ""),
      CsMcpStatusRow(
        label: "aicx-mcp:", value: "configured (agent not started)", tone: .warn,
        facet: .mcpServer, state: .configured, count: nil, subject: "aicx-mcp", detail: ""),
      CsMcpStatusRow(
        label: "vibecrafted-mcp:", value: "failed: command not found", tone: .bad,
        facet: .mcpServer, state: .failed, count: nil, subject: "vibecrafted-mcp",
        detail: "command not found"),
    ]
  )
}

extension CsAgenticReadiness {
  /// Sample readiness: the core capability gate passes (provider + key + native
  /// tools), and the operator-tooling MCP rows are informational (preview seed).
  static let sample = CsAgenticReadiness(
    configPathDisplay: "~/.codescribe/mcp.json",
    ready: true,
    rows: [
      CsMcpStatusRow(
        label: "Agentic readiness:",
        value: "ready — OpenAI (Responses) configured, access available, 10 native tool(s)",
        tone: .good, facet: .readiness, state: .ready, count: 10,
        subject: "OpenAI (Responses)", detail: ""
      ),
      CsMcpStatusRow(
        label: "Provider:", value: "OpenAI (Responses) — access available", tone: .good,
        facet: .provider, state: .accessAvailable, count: nil, subject: "OpenAI (Responses)",
        detail: "OPENAI_API_KEY"),
      CsMcpStatusRow(
        label: "Native tools:", value: "10 tool(s) available", tone: .good,
        facet: .nativeTools, state: .available, count: 10, subject: "", detail: ""),
      CsMcpStatusRow(
        label: "Workspace roots:", value: "2 configured — native tools synchronized",
        tone: .good, facet: .workspaceRoots, state: .synchronized, count: 2, subject: "",
        detail: ""),
      CsMcpStatusRow(
        label: "Vibecrafted runtime:", value: "not configured (optional)", tone: .neutral,
        facet: .vibecraftedRuntime, state: .notConfigured, count: nil,
        subject: "vibecrafted-mcp", detail: ""),
      CsMcpStatusRow(
        label: "AICX MCP:", value: "configured — agent not started yet", tone: .warn,
        facet: .aicxMcp, state: .configured, count: nil, subject: "aicx-mcp", detail: ""),
      CsMcpStatusRow(
        label: "Loctree MCP:", value: "ready — 9 tool(s) live", tone: .good,
        facet: .loctreeMcp, state: .live, count: 9, subject: "loctree-mcp", detail: ""),
      CsMcpStatusRow(
        label: "PRView integration:", value: "not configured (optional)", tone: .neutral,
        facet: .prviewIntegration, state: .notConfigured, count: nil, subject: "", detail: ""),
    ]
  )
}

extension CsCapabilityRow {
  /// Preview seed for Settings → Agent capability matrix.
  static let sampleMatrix: [CsCapabilityRow] = [
    CsCapabilityRow(
      op: "fs.list",
      tier: "native",
      provider: "native",
      nativeTool: "list_directory",
      reason: "Native workspace-sandboxed list"
    ),
    CsCapabilityRow(
      op: "fs.search",
      tier: "enhanced",
      provider: "loctree",
      nativeTool: "search_files",
      reason: "Native search + Loctree enrichment preferred when healthy"
    ),
    CsCapabilityRow(
      op: "code.symbols",
      tier: "unavailable",
      provider: "unavailable",
      nativeTool: "",
      reason: "No native implementation and no healthy MCP provider"
    ),
  ]
}
