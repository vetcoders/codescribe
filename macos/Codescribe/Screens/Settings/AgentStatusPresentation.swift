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
    case .live where namesOperatorServer:
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
    case .configured where namesOperatorServer:
      return String(
        localized: "Configured — agent not started yet (server \(subject))",
        comment: "Placeholder is a server name")
    case .configured:
      return String(localized: "Configured — agent not started yet")
    case .reachable where namesOperatorServer:
      return String(
        localized:
          "Connection test passed — \(toolCount), not registered by the agent yet (server \(subject))",
        comment: "First placeholder is a tool count phrase, second a server name")
    case .reachable:
      return String(
        localized: "Connection test passed — \(toolCount), not registered by the agent yet",
        comment: "Placeholder is a tool count phrase")
    case .unreachable:
      return String(
        localized: "Connection test failed: \(detail)", comment: "Placeholder is an error message")
    case .liveLastTestFailed where namesOperatorServer:
      return String(
        localized:
          "Live — \(toolCount) registered; last connection test failed: \(detail) (server \(subject))",
        comment:
          "First placeholder is a tool count phrase, second an error message, third a server name")
    case .liveLastTestFailed:
      return String(
        localized: "Live — \(toolCount) registered; last connection test failed: \(detail)",
        comment: "First placeholder is a tool count phrase, second an error message")
    case .failedLastTestPassed where namesOperatorServer:
      return String(
        localized:
          "Registration failed: \(detail); last connection test passed — \(toolCount) (server \(subject))",
        comment:
          "First placeholder is an error message, second a tool count phrase, third a server name")
    case .failedLastTestPassed:
      return String(
        localized: "Registration failed: \(detail); last connection test passed — \(toolCount)",
        comment: "First placeholder is an error message, second a tool count phrase")
    case .unverified:
      return String(
        localized: "Not detected (optional) — identity unknown for: \(detail)",
        comment:
          "Placeholder lists configured MCP server names whose identity is unknown until they are tested")
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

  /// Operator-tool rows name the configured server that supplied the evidence.
  private var namesOperatorServer: Bool {
    switch facet {
    case .vibecraftedRuntime, .aicxMcp, .loctreeMcp, .prviewIntegration: return !subject.isEmpty
    default: return false
    }
  }

  private var nativeToolCount: String {
    String(localized: "\(Int(count ?? 0)) native tools", comment: "Plural: count of native tools")
  }

  private var toolCount: String {
    String(localized: "\(Int(count ?? 0)) tools", comment: "Plural: count of tools")
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

  var line: String {
    String(
      localized: "Native: \(native) · Enhanced: \(enhanced) · Unavailable: \(unavailable)",
      comment: "Capability summary counts")
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
