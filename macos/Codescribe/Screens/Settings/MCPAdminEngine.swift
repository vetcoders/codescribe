import Foundation

// Seam between the Settings screen and the REAL MCP config store through the
// UniFFI bridge (CodescribeMcpAdmin). Unlike the read-only AgentStatusEngine,
// this surface MUTATES ~/.codescribe/mcp.json (add / update / remove) and can
// spawn a server to test it. The live app injects `RealMCPAdminEngine`; #Preview
// injects an in-memory mock so the panel renders (and its CRUD works) standalone.
//
// CRUD calls are cheap synchronous disk I/O. `testServer` is async: the handshake
// can take up to ~10s, so it runs off the main actor to keep Settings responsive.

@MainActor
protocol MCPAdminEngine {
  func listServers() throws -> [CsMcpServer]
  func addServer(_ input: CsMcpServerInput) throws
  func updateServer(name: String, input: CsMcpServerInput) throws
  func removeServer(name: String) throws
  func testServer(_ name: String) async -> CsMcpTestResult
  /// Persisted "always allow" tool grants (`server:tool` keys).
  func listToolGrants() throws -> [CsToolGrant]
  /// Revoke one grant so that tool asks for approval again.
  func revokeToolGrant(key: String) throws
  /// Durable agent.permissions snapshot.
  func getPermissionPolicy() -> CsPermissionPolicy
  func setPermissionDefaults(
    defaultLevel: String, readOnlyDefault: String, sideEffectDefault: String) throws
  func setToolPermission(identity: String, level: String) throws
  /// Drop the individual rule so the tool inherits its server rule or
  /// category default again.
  func clearToolPermission(identity: String) throws
  func setServerPermission(server: String, level: String) throws
  /// Live capabilities from the same registry the dispatcher uses.
  func listToolCapabilities() -> [CsToolCapability]
  func loadPermissionSurface() async -> (CsPermissionPolicy, [CsToolCapability])
}

extension MCPAdminEngine {
  // Default no-op surface so mocks and previews predating the grants panel
  // keep compiling; the live engine below overrides both.
  func listToolGrants() throws -> [CsToolGrant] { [] }
  func revokeToolGrant(key: String) throws {}
  func getPermissionPolicy() -> CsPermissionPolicy {
    CsPermissionPolicy(
      defaultLevel: "ask",
      readOnlyDefault: "allow",
      sideEffectDefault: "ask",
      tools: [],
      servers: []
    )
  }
  func setPermissionDefaults(
    defaultLevel: String, readOnlyDefault: String, sideEffectDefault: String
  ) throws {}
  func setToolPermission(identity: String, level: String) throws {}
  func clearToolPermission(identity: String) throws {}
  func setServerPermission(server: String, level: String) throws {}
  func listToolCapabilities() -> [CsToolCapability] { [] }
  func loadPermissionSurface() async -> (CsPermissionPolicy, [CsToolCapability]) {
    (getPermissionPolicy(), listToolCapabilities())
  }
}

// MARK: - Rejected add, translated for the form

/// What the form shows when the store refuses an add: one sentence the user can
/// act on, the field it belongs under, and the store's own words kept as the
/// technical detail. The store speaks in typed messages (`core/mcp/config_store.rs`
/// `validate_*`); this is the one place that turns them into interface copy, so
/// `Config(msg: …)` never reaches the screen.
struct MCPAddFailure: Equatable {
  enum Field: Equatable { case name, command, endpoint }

  let message: String
  let field: Field?
  /// The store's message verbatim. Equal to `message` when no translation
  /// applied, so the form can decide whether a detail is worth showing.
  let detail: String

  init(message: String, field: Field?, detail: String) {
    self.message = message
    self.field = field
    self.detail = detail
  }

  /// The bridge flattens every store error into `CsError.Config(msg:)`; mocks
  /// throw plain errors. Either way the message is what the store said.
  init(_ error: Error) {
    if case let CsError.Config(msg) = error {
      self.init(storeMessage: msg)
    } else {
      self.init(storeMessage: String(describing: error))
    }
  }

  init(storeMessage detail: String) {
    self.detail = detail
    if detail.hasPrefix("Invalid remote MCP endpoint")
      || detail.hasPrefix("Remote MCP endpoint must use http or https")
    {
      message = String(
        localized: "The server URL is invalid. Enter a full HTTP or HTTPS URL with a hostname.")
      field = .endpoint
    } else if detail.hasPrefix("Remote MCP credentials must be stored in Keychain") {
      message = String(
        localized:
          "Leave the username and password out of the server URL. Put the token in the access token field; it goes to the Keychain."
      )
      field = .endpoint
    } else if detail.hasPrefix("MCP server command is empty") {
      message = String(localized: "Enter the command that starts the server.")
      field = .command
    } else if detail.hasPrefix("MCP server name is empty")
      || detail.hasPrefix("MCP server name must not have surrounding whitespace")
    {
      message = String(localized: "Enter a server name without spaces at the start or end.")
      field = .name
    } else if detail.contains("contains unsupported characters") {
      message = String(
        localized: "Use only letters, digits, '_' or '-' in the server name.")
      field = .name
    } else if detail.contains("already exists") {
      message = String(localized: "A server with this name already exists. Choose another name.")
      field = .name
    } else {
      message = detail
      field = nil
    }
  }
}

// MARK: - Real engine (UniFFI bridge adapter)

final class RealMCPAdminEngine: MCPAdminEngine {
  private let admin = CodescribeMcpAdmin()

  func listServers() throws -> [CsMcpServer] { try admin.listServers() }
  func addServer(_ input: CsMcpServerInput) throws { try admin.addServer(server: input) }
  func updateServer(name: String, input: CsMcpServerInput) throws {
    try admin.updateServer(name: name, server: input)
  }
  func removeServer(name: String) throws { try admin.removeServer(name: name) }
  func listToolGrants() throws -> [CsToolGrant] { try admin.listToolGrants() }
  func revokeToolGrant(key: String) throws { try admin.revokeToolGrant(key: key) }
  func getPermissionPolicy() -> CsPermissionPolicy { admin.getPermissionPolicy() }
  func setPermissionDefaults(
    defaultLevel: String, readOnlyDefault: String, sideEffectDefault: String
  ) throws {
    try admin.setPermissionDefaults(
      defaultLevel: defaultLevel,
      readOnlyDefault: readOnlyDefault,
      sideEffectDefault: sideEffectDefault
    )
  }
  func setToolPermission(identity: String, level: String) throws {
    try admin.setToolPermission(identity: identity, level: level)
  }
  func clearToolPermission(identity: String) throws {
    try admin.clearToolPermission(identity: identity)
  }
  func setServerPermission(server: String, level: String) throws {
    try admin.setServerPermission(server: server, level: level)
  }
  func listToolCapabilities() -> [CsToolCapability] { admin.listToolCapabilities() }
  nonisolated func loadPermissionSurface() async -> (CsPermissionPolicy, [CsToolCapability]) {
    await Task.detached(priority: .userInitiated) {
      let admin = CodescribeMcpAdmin()
      return (admin.getPermissionPolicy(), admin.listToolCapabilities())
    }.value
  }

  // Spawning + handshaking a server can take up to ~10s; run it off the main
  // actor so the Settings window never freezes. A fresh stateless handle is
  // created INSIDE the detached task to avoid sending a non-Sendable object
  // across executors — only the `String` name crosses the boundary.
  func testServer(_ name: String) async -> CsMcpTestResult {
    await Task.detached { CodescribeMcpAdmin().testServer(name: name) }.value
  }
}

// MARK: - Mock engine (previews / standalone)

final class MockMCPAdminEngine: MCPAdminEngine {
  private var servers: [CsMcpServer]

  init(servers: [CsMcpServer] = CsMcpServer.samples) { self.servers = servers }

  func listServers() throws -> [CsMcpServer] { servers }

  func addServer(_ input: CsMcpServerInput) throws {
    servers.append(
      CsMcpServer(
        name: input.name, command: input.command, args: input.args,
        envKeys: [], enabled: input.enabled,
        transport: input.endpoint.isEmpty ? "stdio" : "remote",
        endpoint: input.endpoint, authRef: input.authRef
      )
    )
  }

  func updateServer(name: String, input: CsMcpServerInput) throws {
    guard let index = servers.firstIndex(where: { $0.name == name }) else { return }
    servers[index] = CsMcpServer(
      name: input.name, command: input.command, args: input.args,
      envKeys: servers[index].envKeys, enabled: input.enabled,
      transport: input.endpoint.isEmpty ? "stdio" : "remote",
      endpoint: input.endpoint, authRef: input.authRef
    )
  }

  func removeServer(name: String) throws { servers.removeAll { $0.name == name } }

  func testServer(_ name: String) async -> CsMcpTestResult {
    CsMcpTestResult(
      ok: true,
      toolCount: 7,
      serverName: "\(name).mcp.v1",
      serverVersion: "0.4.0",
      protocolVersion: "2025-06-18",
      error: ""
    )
  }
}

// MARK: - Bridge value helpers (preview seeds)

extension CsMcpServer {
  static let samples: [CsMcpServer] = [
    CsMcpServer(
      name: "loctree-mcp", command: "loctree-mcp", args: ["mcp"], envKeys: [], enabled: true,
      transport: "stdio", endpoint: "", authRef: ""),
    CsMcpServer(
      name: "aicx-mcp", command: "aicx", args: ["mcp"], envKeys: ["AICX_TOKEN"], enabled: true,
      transport: "stdio", endpoint: "", authRef: ""),
    CsMcpServer(
      name: "slack", command: "", args: [], envKeys: [], enabled: true, transport: "remote",
      endpoint: "https://connector.example/mcp", authRef: "MCP_CONNECTOR_SLACK_TOKEN"),
  ]
}
