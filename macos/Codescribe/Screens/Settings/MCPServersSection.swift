import SwiftUI

// Editable MCP server management, rendered inside the Engine panel below the
// read-only AgentStatusSection. Where AgentStatusSection reports discovery
// truth, THIS section writes it: add / enable-disable / remove servers in
// ~/.codescribe/mcp.json (through the atomic, unknown-field-preserving store)
// and test one on demand. A missing config degrades to an empty list + the add
// form, which creates the file on first add.

struct MCPServersSection: View {
    @ObservedObject var model: SettingsViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SettingsSectionLabel("Manage MCP servers")

            Text("Edited on disk in mcp.json. Hand edits (env, custom fields) are preserved.")
                .font(CSFont.mono(11, .medium))
                .foregroundStyle(CSColor.textFaint)
                .padding(.top, 4)

            if model.mcpServers.isEmpty {
                emptyState.padding(.top, 11)
            } else {
                VStack(spacing: 8) {
                    ForEach(model.mcpServers, id: \.name) { server in
                        MCPServerRow(
                            server: server,
                            pending: model.mcpTestPending.contains(server.name),
                            result: model.mcpTestResults[server.name],
                            onToggle: { model.toggleMcpServer(server) },
                            onTest: { model.testMcpServer(server.name) },
                            onRemove: { model.removeMcpServer(server.name) }
                        )
                    }
                }
                .padding(.top, 11)
            }

            MCPAddServerForm { name, command, args in
                model.addMcpServer(name: name, command: command, args: args)
            }
            .padding(.top, 12)
        }
    }

    private var emptyState: some View {
        HStack(spacing: 8) {
            Text("●").font(CSFont.mono(11, .medium)).foregroundStyle(CSColor.textFaint)
            Text("no MCP servers configured — add one below")
                .font(CSFont.mono(11, .medium))
                .foregroundStyle(CSColor.textFaint)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 16)
        .padding(.vertical, 14)
        .background(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .fill(CSColor.surfaceRaised(0.02))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
        )
    }
}

// MARK: - One server row (identity · command · test result · actions)

private struct MCPServerRow: View {
    let server: CsMcpServer
    let pending: Bool
    let result: CsMcpTestResult?
    let onToggle: () -> Void
    let onTest: () -> Void
    let onRemove: () -> Void

    private var accent: Color { server.enabled ? CSColor.olive : CSColor.textFaint }

    private var commandLine: String {
        server.args.isEmpty ? server.command : "\(server.command) \(server.args.joined(separator: " "))"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack(spacing: 10) {
                Circle().fill(accent.opacity(0.85)).frame(width: 7, height: 7)
                Text(server.name)
                    .font(CSFont.ui(13.5, .semibold))
                    .foregroundStyle(CSColor.textBody)
                Spacer(minLength: 0)
                enabledButton
                testButton
                removeButton
            }

            Text(commandLine)
                .font(CSFont.mono(11.5, .regular))
                .foregroundStyle(CSColor.textMutedAlt)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: .infinity, alignment: .leading)

            if !server.envKeys.isEmpty {
                Text("env: \(server.envKeys.joined(separator: ", "))")
                    .font(CSFont.mono(10, .medium))
                    .foregroundStyle(CSColor.textFaint)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            if pending {
                resultLine(text: "testing…", color: CSColor.amber)
            } else if let result {
                if result.ok {
                    resultLine(text: "ok — \(result.toolCount) tool(s)", color: CSColor.oliveLight)
                    if let identity = Self.handshakeIdentity(result) {
                        Text(identity)
                            .font(CSFont.mono(10, .medium))
                            .foregroundStyle(CSColor.textFaint)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                } else {
                    resultLine(text: "failed: \(result.error)", color: CSColor.terracottaLight)
                }
            }
        }
        .padding(.horizontal, 15)
        .padding(.vertical, 12)
        .background(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .fill(accent.opacity(0.05))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .strokeBorder(accent.opacity(0.16), lineWidth: 1)
        )
    }

    /// Compact identity advertised by the server in the `initialize` handshake:
    /// name · version · protocol. Nil when the server exposed none of them.
    static func handshakeIdentity(_ result: CsMcpTestResult) -> String? {
        var parts: [String] = []
        if !result.serverName.isEmpty { parts.append(result.serverName) }
        if !result.serverVersion.isEmpty { parts.append("v\(result.serverVersion)") }
        if !result.protocolVersion.isEmpty { parts.append("proto \(result.protocolVersion)") }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    private func resultLine(text: String, color: Color) -> some View {
        Text(text)
            .font(CSFont.mono(11, .semibold))
            .foregroundStyle(color)
            .lineLimit(2)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var enabledButton: some View {
        Button(action: onToggle) {
            Text(server.enabled ? "enabled" : "disabled")
                .font(CSFont.mono(10, .semibold))
                .foregroundStyle(server.enabled ? CSColor.oliveLight : CSColor.textFaint)
                .padding(.horizontal, 9)
                .padding(.vertical, 5)
                .background(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .fill(accent.opacity(0.10))
                )
                .overlay(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .strokeBorder(accent.opacity(0.22), lineWidth: 1)
                )
        }
        .buttonStyle(.plain)
        .help(server.enabled ? "Disable this server" : "Enable this server")
    }

    private var testButton: some View {
        Button(action: onTest) {
            Text("Test")
                .font(CSFont.mono(10, .semibold))
                .foregroundStyle(pending ? CSColor.textFaint : CSColor.textBodyAlt)
                .padding(.horizontal, 10)
                .padding(.vertical, 5)
                .background(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .fill(CSColor.surfaceRaised(0.04))
                )
                .overlay(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
                )
        }
        .buttonStyle(.plain)
        .disabled(pending)
        .help("Spawn the server and list its tools")
    }

    private var removeButton: some View {
        Button(action: onRemove) {
            CSIconView(icon: .delete, size: 11, weight: .semibold, color: CSColor.terracottaLight)
                .frame(width: 28, height: 26)
                .background(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .fill(CSColor.surfaceRaised(0.04))
                )
                .overlay(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
                )
        }
        .buttonStyle(.plain)
        .help("Remove this server from mcp.json")
    }
}

// MARK: - Add-server form

private struct MCPAddServerForm: View {
    let onAdd: (_ name: String, _ command: String, _ args: [String]) -> Void

    @State private var name: String = ""
    @State private var command: String = ""
    @State private var argsText: String = ""

    private var canAdd: Bool {
        !name.trimmingCharacters(in: .whitespaces).isEmpty
            && !command.trimmingCharacters(in: .whitespaces).isEmpty
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            Text("ADD SERVER")
                .font(CSFont.mono(10, .semibold))
                .tracking(0.5)
                .foregroundStyle(CSColor.textMuted)

            field(placeholder: "name (e.g. prview)", text: $name, mono: true)
            field(placeholder: "command (e.g. prview)", text: $command, mono: true)
            field(placeholder: "args, space-separated (e.g. mcp)", text: $argsText, mono: true)

            HStack {
                Spacer(minLength: 0)
                Button(action: submit) {
                    Text("Add")
                        .font(CSFont.ui(12, .semibold))
                        .foregroundStyle(canAdd ? CSColor.oliveLight : CSColor.textFaint)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 8)
                        .background(
                            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                                .fill(CSColor.olive.opacity(canAdd ? 0.14 : 0.05))
                        )
                        .overlay(
                            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                                .strokeBorder(CSColor.olive.opacity(canAdd ? 0.28 : 0.10), lineWidth: 1)
                        )
                }
                .buttonStyle(.plain)
                .disabled(!canAdd)
            }
        }
        .padding(.horizontal, 15)
        .padding(.vertical, 13)
        .background(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .fill(CSColor.surfaceRaised(0.03))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
        )
    }

    private func field(placeholder: String, text: Binding<String>, mono: Bool) -> some View {
        TextField(placeholder, text: text)
            .textFieldStyle(.plain)
            .font(mono ? CSFont.mono(12, .regular) : CSFont.ui(12, .regular))
            .foregroundStyle(CSColor.textBody)
            .padding(.horizontal, 11)
            .padding(.vertical, 8)
            .background(
                RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                    .fill(CSColor.surfaceRaised(0.03))
            )
            .overlay(
                RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                    .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
            )
            .onSubmit(submit)
    }

    private func submit() {
        guard canAdd else { return }
        let args = argsText
            .split(whereSeparator: { $0 == " " || $0 == "\t" })
            .map(String.init)
        onAdd(
            name.trimmingCharacters(in: .whitespaces),
            command.trimmingCharacters(in: .whitespaces),
            args
        )
        name = ""
        command = ""
        argsText = ""
    }
}
