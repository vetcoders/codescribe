import XCTest

@testable import Codescribe

/// P2-001: operator-tool Diagnostics rows name the configured server behind
/// the evidence and keep a connection test distinct from agent registration.
@MainActor
final class McpDiagnosticsPresentationTests: XCTestCase {
  private func row(
    facet: CsMcpStatusFacet, state: CsMcpStatusState, count: UInt32? = nil,
    subject: String = "", detail: String = ""
  ) -> CsMcpStatusRow {
    CsMcpStatusRow(
      label: "raw:", value: "raw english", tone: .warn,
      facet: facet, state: state, count: count, subject: subject, detail: detail)
  }

  func testOperatorRowsNameTheSelectedServer() {
    XCTAssertEqual(
      row(facet: .loctreeMcp, state: .live, count: 12, subject: "loctree-http").localizedValue,
      "Live — 12 tools (server loctree-http)")
    XCTAssertEqual(
      row(facet: .aicxMcp, state: .configured, subject: "aicx").localizedValue,
      "Configured — agent not started yet (server aicx)")
  }

  func testConnectionTestStatesNeverReadAsRegistration() {
    XCTAssertEqual(
      row(facet: .loctreeMcp, state: .reachable, count: 3, subject: "code-map").localizedValue,
      "Connection test passed — 3 tools, not registered by the agent yet (server code-map)")
    XCTAssertEqual(
      row(facet: .mcpServer, state: .reachable, count: 3, subject: "code-map").localizedValue,
      "Connection test passed — 3 tools, not registered by the agent yet")
    XCTAssertEqual(
      row(facet: .aicxMcp, state: .unreachable, subject: "aicx", detail: "Connection refused")
        .localizedValue,
      "Connection test failed: Connection refused")
  }

  func testUnknownIdentityIsUncertaintyNotAbsence() {
    XCTAssertEqual(
      row(facet: .loctreeMcp, state: .unverified, detail: "memory-box").localizedValue,
      "Not detected (optional) — identity unknown for: memory-box")
    XCTAssertEqual(
      row(facet: .loctreeMcp, state: .notConfigured).localizedValue,
      "Not configured (optional)")
  }
}
