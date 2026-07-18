import Foundation

// Seam between the Settings screen and the REAL MCP config store through the
// UniFFI bridge (CodescribeMcpAdmin). Unlike the read-only AgentStatusEngine,
// this surface MUTATES ~/.codescribe/mcp.json (add / update / remove) and can
// spawn a server to test it. The live app injects `RealMCPAdminEngine`; #Preview
// injects an in-memory mock so the panel renders (and its CRUD works) standalone.
//
// CRUD calls are cheap synchronous disk I/O. `testServer` is async: the handshake
// can take up to ~10s, so it runs off the main actor to keep Settings responsive.

protocol MCPAdminEngine {
    func listServers() throws -> [CsMcpServer]
    func addServer(_ input: CsMcpServerInput) throws
    func updateServer(name: String, input: CsMcpServerInput) throws
    func removeServer(name: String) throws
    func testServer(_ name: String) async -> CsMcpTestResult
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
                envKeys: [], enabled: input.enabled
            )
        )
    }

    func updateServer(name: String, input: CsMcpServerInput) throws {
        guard let index = servers.firstIndex(where: { $0.name == name }) else { return }
        servers[index] = CsMcpServer(
            name: input.name, command: input.command, args: input.args,
            envKeys: servers[index].envKeys, enabled: input.enabled
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
        CsMcpServer(name: "loctree-mcp", command: "loctree-mcp", args: ["mcp"], envKeys: [], enabled: true),
        CsMcpServer(name: "aicx-mcp", command: "aicx", args: ["mcp"], envKeys: ["AICX_TOKEN"], enabled: true),
        CsMcpServer(name: "prview", command: "prview", args: ["mcp"], envKeys: [], enabled: false)
    ]
}
