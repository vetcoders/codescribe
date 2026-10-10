import SwiftUI

// Localized presentation of the agent-status probes. The core reports each
// row as a stable facet + state plus structured parts (count, subject,
// detail); this file turns those into interface-language text. The English
// `label` / `value` the core also carries are for logs and tests, never
// parsed here.

extension CsMcpStatusRow {
  /// Row label in the interface language. Server rows show the server name.
  var localizedLabel: String {
    switch facet {
    case .readiness: return String(localized: "Overall status", comment: "Diagnostics row label")
    case .provider: return String(localized: "Model provider", comment: "Diagnostics row label")
    case .nativeTools: return String(localized: "Native tools", comment: "Diagnostics row label")
    case .workspaceRoots:
      return String(
        localized: "Folders available to the Agent", comment: "Settings tab: Agent folders")
    case .vibecraftedRuntime:
      return String(localized: "VibeCrafted runtime", comment: "Diagnostics row label")
    case .aicxMcp: return String(localized: "AICX MCP", comment: "Diagnostics row label")
    case .loctreeMcp: return String(localized: "Loctree MCP", comment: "Diagnostics row label")
    case .prviewIntegration:
      return String(localized: "PRView integration", comment: "Diagnostics row label")
    case .mcpConfig: return String(localized: "MCP configuration", comment: "Diagnostics row label")
    case .mcpServer: return subject
    }
  }

  /// Row value in the interface language, built from the state and its parts.
  var localizedValue: String {
    switch state {
    case .ready:
      return String(
        localized: "Ready — \(subject) configured, can send requests, \(nativeToolCount)",
        comment:
          "Diagnostics verdict; first placeholder is the provider name, second a tool count phrase")
    case .accessAvailable:
      return String(
        localized: "\(subject) — can send requests",
        comment:
          "Placeholder is the provider name; a request may go out under the credentials that exist now (a key-optional provider counts), no request was made")
    case .accessUnavailable where facet == .readiness:
      return String(
        localized: "Not ready — no provider access (sign in or set \(detail))",
        comment: "Placeholder is an environment variable name")
    case .accessUnavailable:
      return String(
        localized: "\(subject) — no access (sign in or set \(detail))",
        comment: "First placeholder is the provider name, second an environment variable name")
    case .noNativeTools where facet == .readiness:
      return String(localized: "Not ready — no native tools available")
    case .noNativeTools:
      return String(localized: "No native tools available")
    case .available:
      return nativeToolCount
    case .synchronized:
      return String(
        localized: "\(Int(count ?? 0)) folders — native tools synchronized",
        comment: "Plural: number of folders the Agent may use")
    case .rootsMismatch where facet == .readiness:
      return String(localized: "Not ready — folders differ between Settings and native tools")
    case .rootsMismatch:
      return String(
        localized: "Mismatch — \(detail)", comment: "Placeholder lists both folder sets")
    case .notConfigured:
      return String(localized: "Not configured (optional)")
    case .live where facet == .prviewIntegration && !subject.isEmpty:
      return String(
        localized: "Live — \(toolCount) (server \(subject))",
        comment: "First placeholder is a tool count phrase, second a server name")
    case .live:
      return String(localized: "Live — \(toolCount)", comment: "Placeholder is a tool count phrase")
    case .failed:
      return String(localized: "Failed: \(detail)", comment: "Placeholder is an error message")
    case .disabled:
      return String(
        localized: "mcp.server.disabled", defaultValue: "Disabled", comment: "MCP server state")
    case .configured where facet == .prviewIntegration && !subject.isEmpty:
      return String(
        localized: "Configured — agent not started yet (server \(subject))",
        comment: "Placeholder is a server name")
    case .configured:
      return String(localized: "Configured — agent not started yet")
    case .error:
      return String(
        localized: "Configuration error: \(detail)", comment: "Placeholder is an error message")
    case .empty:
      return String(localized: "Configuration present, but no servers defined")
    case .missing:
      return String(localized: "No mcp.json — MCP off (optional)")
    case .note:
      return String(
        localized: "mcp.json could not be read (optional): \(detail)",
        comment: "Placeholder is an error message")
    }
  }

  private var nativeToolCount: String {
    String(localized: "\(Int(count ?? 0)) native tools", comment: "Plural: count of native tools")
  }

  private var toolCount: String {
    String(localized: "\(Int(count ?? 0)) tools", comment: "Plural: count of tools")
  }

  /// Compact one-line status for the Integrations list: exactly one message per
  /// state, so the readiness table's longer sentence is not repeated on the
  /// default screen (Founder brief, round 11, 2026-10-10).
  var localizedIntegrationStatus: String {
    switch state {
    case .notConfigured:
      return String(
        localized: "Optional · not configured",
        comment: "Integration status: the integration is optional and absent from mcp.json")
    case .configured:
      return String(
        localized: "Awaits first run",
        comment: "Integration status: configured, but the Agent has not run discovery yet")
    case .live:
      return String(
        localized: "Ready · \(Int(count ?? 0)) tools",
        comment: "Plural: integration status with the number of tools it serves")
    case .disabled:
      return String(
        localized: "mcp.server.disabled", defaultValue: "Disabled", comment: "MCP server state")
    default:
      return localizedValue
    }
  }
}

extension McpServerLine {
  /// Exactly one status per server row: the cached test verdict when there is
  /// one, otherwise the probe's runtime state — never both, so a row cannot
  /// read "Not checked" twice (Founder brief, round 11, 2026-10-10).
  var localizedSingleStatus: String {
    switch test {
    case .pending, .ok, .failed: return testText
    case .untested: return status?.localizedValue ?? testText
    }
  }

  /// Tone of the one status the row shows; a failing probe outranks a stale
  /// "not tested".
  var singleStatusTone: CsMcpRowTone {
    if status?.tone == .bad { return .bad }
    switch test {
    case .pending, .ok, .failed: return testTone
    case .untested: return status?.tone ?? .neutral
    }
  }
}

extension CsMcpRowTone {
  /// Status word shown next to the dot and read by VoiceOver.
  var label: String {
    switch self {
    case .good:
      return String(
        localized: "status.tone.good", defaultValue: "Good",
        comment: "Status dot: everything is in order")
    case .warn:
      return String(
        localized: "status.tone.warn", defaultValue: "Warning",
        comment: "Status dot: needs attention")
    case .bad:
      return String(
        localized: "status.tone.bad", defaultValue: "Error", comment: "Status dot: broken")
    case .neutral:
      return String(
        localized: "status.tone.neutral", defaultValue: "Not checked",
        comment: "Status dot: no verdict")
    }
  }
}

extension CsCapabilityRow {
  /// Tier badge in the interface language.
  var localizedTier: String {
    switch tier.lowercased() {
    case "native":
      return String(
        localized: "capability.tier.native", defaultValue: "Native",
        comment: "Capability tier badge")
    case "enhanced":
      return String(
        localized: "capability.tier.enhanced", defaultValue: "Enhanced",
        comment: "Capability tier badge")
    case "unavailable":
      return String(
        localized: "capability.tier.unavailable", defaultValue: "Unavailable",
        comment: "Capability tier badge")
    default: return tier
    }
  }

  /// One readable sentence derived from the tier and provider; the core's
  /// English reason stays available as a tooltip.
  var localizedHeadline: String {
    switch (tier.lowercased(), provider.lowercased()) {
    case ("native", _):
      return String(localized: "Built-in Codescribe tool")
    case ("enhanced", "loctree"):
      return String(localized: "Built-in tool, enriched by Loctree while it is healthy")
    case ("enhanced", "intellij"):
      return String(localized: "Built-in tool, enriched by IntelliJ for the matched project")
    case ("enhanced", _):
      return String(localized: "No built-in tool; served by an MCP server")
    default:
      return String(localized: "Unavailable — no built-in tool and no healthy MCP server")
    }
  }

  /// Qualifier shown on a row of the expanded capability list. Native rows
  /// return nil: their shared source is stated once above the list instead of
  /// repeating "Built-in Codescribe tool" on every row (Founder brief,
  /// round 11, 2026-10-10). VoiceOver still reads `localizedHeadline`.
  var localizedQualifier: String? {
    tier.lowercased() == "native" ? nil : localizedHeadline
  }

  /// Mono detail line: tool id and source, or source alone.
  var localizedDetail: String? {
    if !nativeTool.isEmpty {
      return String(
        localized: "tool: \(nativeTool) · source: \(provider)",
        comment: "Mono detail under a capability row; placeholders are identifiers")
    }
    if !provider.isEmpty {
      return String(localized: "source: \(provider)", comment: "Placeholder is an identifier")
    }
    return nil
  }
}

/// Counts per tier for the capability summary line.
struct CapabilitySummary: Equatable {
  var native = 0
  var enhanced = 0
  var unavailable = 0

  init(rows: [CsCapabilityRow]) {
    for row in rows {
      switch row.tier.lowercased() {
      case "native": native += 1
      case "enhanced": enhanced += 1
      default: unavailable += 1
      }
    }
  }

  /// Named on purpose: these are capability operations (18 in this build), not
  /// the built-in tool definitions the readiness probe counts. The two numbers
  /// measure different sets and must never be presented as one
  /// (Founder brief, round 11, 2026-10-10).
  var line: String {
    String(
      localized:
        "Capabilities: \(native) native · \(enhanced) enhanced · \(unavailable) unavailable",
      comment: "Capability summary counts per tier")
  }
}

/// Counts behind the "MCP servers" summary line. "0 problems across 0 checked
/// servers is not a passed test": the line reports how many servers were
/// actually checked, and never implies readiness from an untested set
/// (Founder brief, round 11, 2026-10-10).
struct McpServerSummary: Equatable {
  var configured = 0
  var checked = 0
  var failed = 0

  init(lines: [McpServerLine]) {
    configured = lines.count
    for line in lines {
      var testFailed = false
      switch line.test {
      case .ok: checked += 1
      case .failed:
        checked += 1
        testFailed = true
      case .pending, .untested: break
      }
      if testFailed || line.status?.tone == .bad { failed += 1 }
    }
  }

  var line: String {
    let base = String(
      localized: "\(configured) configured · \(checked) checked",
      comment: "Plural: MCP servers in mcp.json and how many of them were actually checked")
    guard failed > 0 else { return base }
    return base + " · "
      + String(
        localized: "\(failed) failed", comment: "Plural: MCP servers whose last check failed")
  }

  /// Red only when a check actually failed; an unchecked set is neutral, never
  /// green and never an error.
  var tone: CsMcpRowTone {
    if failed > 0 { return .bad }
    return checked == configured && configured > 0 ? .good : .neutral
  }
}

/// One row of the Integrations list: the integration's name and exactly one
/// human status. Each row reports the single `mcp.json` entry its diagnostic
/// looks at (`serverName`), so an optional integration is never read as "any
/// configured server with a similar name" (Founder brief, round 11,
/// 2026-10-10).
struct AgentIntegrationLine: Equatable {
  let name: String
  let status: String
  let tone: CsMcpRowTone
  let serverName: String

  /// The optional operator-tooling rows of the readiness probe, in probe order.
  static func lines(rows: [CsMcpStatusRow]) -> [AgentIntegrationLine] {
    rows.filter { isIntegration($0.facet) }.map {
      AgentIntegrationLine(
        name: $0.localizedLabel, status: $0.localizedIntegrationStatus, tone: $0.tone,
        serverName: $0.subject)
    }
  }

  static func isIntegration(_ facet: CsMcpStatusFacet) -> Bool {
    switch facet {
    case .vibecraftedRuntime, .aicxMcp, .loctreeMcp, .prviewIntegration: return true
    default: return false
    }
  }
}

/// The Diagnostics summary card: one verdict line, one quiet line of key facts,
/// and the things that need attention. Derived from the live probe rows only —
/// a blocking gate failure can never render as "ready", and a warning or an
/// unchecked state never renders as an error (Founder brief, round 11,
/// 2026-10-10).
struct AgentStatusSummary: Equatable {
  struct Note: Equatable {
    let tone: CsMcpRowTone
    let text: String
  }

  let tone: CsMcpRowTone
  let headline: String
  /// Key facts, already joined; empty when nothing has been probed.
  let facts: String
  /// Errors first, then warnings. Empty when there is nothing to act on.
  let notes: [Note]

  init(
    readiness: CsAgenticReadiness, capabilities: [CsCapabilityRow], servers: [McpServerLine],
    mcpStatus: CsMcpStatusReport, accessResolved: Bool, accessError: String?
  ) {
    let rows = readiness.rows
    func row(_ facet: CsMcpStatusFacet) -> CsMcpStatusRow? {
      rows.first { $0.facet == facet }
    }

    // ---- Verdict. The gate decides; nothing else may overrule it. ----
    if let accessError {
      tone = .bad
      headline = String(
        localized: "Provider access could not be checked: \(accessError)",
        comment: "Diagnostics summary headline; placeholder is an error message")
    } else if !accessResolved || rows.isEmpty {
      tone = .neutral
      headline = String(
        localized: "Not checked yet",
        comment: "Diagnostics summary headline before the first probe has run")
    } else if readiness.ready {
      tone = .good
      headline = String(
        localized: "Configuration ready", comment: "Diagnostics summary headline: the gate passes")
    } else {
      tone = .bad
      headline =
        row(.readiness)?.localizedValue
        ?? String(
          localized: "Not ready", comment: "Diagnostics summary headline: the gate fails")
    }

    // ---- Key facts. A missing probe drops its fragment instead of showing 0. ----
    var facts: [String] = []
    if let provider = row(.provider)?.subject, !provider.isEmpty {
      facts.append(
        String(
          localized: "Model: \(provider)",
          comment: "Diagnostics key fact; placeholder is a provider display name"))
    }
    if let tools = row(.nativeTools)?.count {
      facts.append(
        String(
          localized: "\(Int(tools)) built-in tools",
          comment: "Plural: how many tools are compiled into Codescribe"))
    }
    if let roots = row(.workspaceRoots)?.count {
      facts.append(
        String(
          localized: "\(Int(roots)) Agent folders",
          comment: "Plural: how many folders the Agent may use"))
    }
    self.facts = facts.joined(separator: " · ")

    // ---- Attention. Aggregates only; the detail stays in its own section. ----
    let integrations = rows.filter { AgentIntegrationLine.isIntegration($0.facet) }
    let failedIntegrations = integrations.filter { $0.state == .failed }.count
    let awaiting = integrations.filter { $0.state == .configured }.count
    let serverSummary = McpServerSummary(lines: servers)
    let unavailable = CapabilitySummary(rows: capabilities).unavailable
    let configRow =
      (mcpStatus.rows.first { $0.facet == .mcpConfig }) ?? row(.mcpConfig)

    var errors: [Note] = []
    var warnings: [Note] = []
    if failedIntegrations > 0 {
      errors.append(
        Note(
          tone: .bad,
          text: String(
            localized: "\(failedIntegrations) integrations reported an error",
            comment: "Plural: optional integrations whose discovery failed")))
    }
    if serverSummary.failed > 0 {
      errors.append(
        Note(
          tone: .bad,
          text: String(
            localized: "\(serverSummary.failed) MCP servers failed their last check",
            comment: "Plural: MCP servers whose last check failed")))
    }
    if let configRow, configRow.state == .error {
      errors.append(Note(tone: .bad, text: configRow.localizedValue))
    }
    if awaiting > 0 {
      warnings.append(
        Note(
          tone: .warn,
          text: String(
            localized: "\(awaiting) integrations await their first run",
            comment: "Plural: optional integrations configured but not yet discovered")))
    }
    if unavailable > 0 {
      warnings.append(
        Note(
          tone: .warn,
          text: String(
            localized: "\(unavailable) capabilities have no provider",
            comment: "Plural: capability operations no native tool or MCP server can serve")))
    }
    if let configRow, configRow.state == .note {
      warnings.append(Note(tone: configRow.tone, text: configRow.localizedValue))
    }
    notes = errors + warnings
  }
}

/// One merged line of the MCP server table: runtime status from the probe and
/// the cached per-server test, joined by server name.
struct McpServerLine: Equatable {
  enum Test: Equatable {
    case pending
    case untested
    case ok(toolCount: Int, version: String)
    case failed(String)
  }

  let name: String
  let status: CsMcpStatusRow?
  let test: Test

  var testText: String {
    switch test {
    case .pending: return String(localized: "Testing…", comment: "MCP server test state")
    case .untested: return String(localized: "Not tested", comment: "MCP server test state")
    case .ok(let toolCount, let version):
      let base = String(
        localized: "OK — \(toolCount) tools", comment: "Plural: MCP server test result")
      return version.isEmpty ? base : base + " · v\(version)"
    case .failed(let error):
      return String(localized: "Failed: \(error)", comment: "Placeholder is an error message")
    }
  }

  var testTone: CsMcpRowTone {
    switch test {
    case .pending: return .warn
    case .untested: return .neutral
    case .ok: return .good
    case .failed: return .bad
    }
  }

  /// Join probe rows with configured servers and cached test results.
  static func merge(
    servers: [CsMcpServer], statusRows: [CsMcpStatusRow],
    results: [String: CsMcpTestResult], pending: Set<String>
  ) -> [McpServerLine] {
    let byName = Dictionary(
      statusRows.filter { $0.facet == .mcpServer }.map { ($0.subject, $0) },
      uniquingKeysWith: { first, _ in first })
    return servers.map { server in
      let test: Test
      if pending.contains(server.name) {
        test = .pending
      } else if let result = results[server.name] {
        test =
          result.ok
          ? .ok(toolCount: Int(result.toolCount), version: result.serverVersion)
          : .failed(result.error)
      } else {
        test = .untested
      }
      return McpServerLine(name: server.name, status: byName[server.name], test: test)
    }
  }
}
