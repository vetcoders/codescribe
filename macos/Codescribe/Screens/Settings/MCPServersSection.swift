import SwiftUI

// Editable MCP server management on the Agent › MCP tab. Where the
// Diagnostics tab reports discovery truth, THIS section writes it: add /
// enable-disable / remove servers in ~/.codescribe/mcp.json (through the
// atomic, unknown-field-preserving store) and run a one-off handshake on
// demand. A missing config degrades to an empty list + the add form, which
// creates the file on first add.
//
// The screen reads as a list of servers, not as a config dump: a card shows
// the name, the configured state (enabled / disabled is a config flag, not a
// live connection), the result of the last handshake and two actions. The
// launch command, URL, environment keys, authentication, permission rule and
// handshake identity live behind a per-card disclosure, and the on-disk
// mechanics behind the tab-level "Technical details".

struct MCPServersSection: View {
  @ObservedObject var model: SettingsViewModel
  @State private var confirmingClear = false
  @State private var showingTechnicalDetails = false

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      if model.mcpServers.isEmpty {
        emptyState
      } else {
        VStack(spacing: 8) {
          ForEach(model.mcpServers, id: \.name) { server in
            MCPServerRow(
              server: server,
              pending: model.mcpTestPending.contains(server.name),
              result: model.mcpTestResults[server.name],
              permissionLevel: model.mcpServerPermissionLevel(server.name),
              onToggle: { model.toggleMcpServer(server) },
              onTest: { model.testMcpServer(server.name) },
              onRemove: { model.requestMcpServerRemoval(server.name) }
            )
          }
        }
      }

      MCPAddServerForm { name, command, args, endpoint, token in
        model.addMcpServer(
          name: name, command: command, args: args,
          endpoint: endpoint, token: token
        )
      }
      .padding(.top, 12)

      DisclosureGroup(isExpanded: $showingTechnicalDetails) {
        VStack(alignment: .leading, spacing: 6) {
          Text("Edited on disk in mcp.json. Hand edits (env, custom fields) are preserved.")
            .font(CSFont.mono(11, .medium))
            .foregroundStyle(Color.secondary)
            .fixedSize(horizontal: false, vertical: true)
          Text(verbatim: "~/.codescribe/mcp.json")
            .font(CSFont.mono(10, .medium))
            .foregroundStyle(Color.secondary)
            .textSelection(.enabled)
          Button(role: .destructive) {
            confirmingClear = true
          } label: {
            Text("Move MCP configuration to Trash…")
              .font(CSFont.mono(10.5, .semibold))
              .foregroundStyle(CSColor.danger)
          }
          .csFocusRing()
          .padding(.top, 6)
          .accessibilityHint("Moves only mcp.json to Trash after confirmation.")
        }
        .padding(.top, 6)
      } label: {
        Text("Technical details")
          .font(CSFont.ui(11.5))
          .foregroundStyle(Color.secondary)
      }
      .padding(.top, 14)
      .accessibilityIdentifier("settings-mcp-technical-details")
    }
    .alert("Move MCP configuration to Trash?", isPresented: $confirmingClear) {
      Button("Cancel", role: .cancel) {}
      Button("Move mcp.json to Trash", role: .destructive) {
        model.clearMcpConfiguration()
      }
    } message: {
      Text(
        "Moves only ~/.codescribe/mcp.json to Trash. Recordings, transcripts, threads, preferences, and API keys stay untouched."
      )
    }
    // Removing one server is the same class of action as clearing the file:
    // it edits mcp.json and deletes the server's Keychain token, with no undo.
    // The row's Remove button only asks; the alert names the server and the
    // consequence, Cancel and Escape leave the configuration as it was.
    .alert(
      Text("Remove \(model.mcpRemovalCandidate ?? "") from MCP servers?"),
      isPresented: Binding(
        get: { model.mcpRemovalCandidate != nil },
        set: { presented in if !presented { model.cancelMcpServerRemoval() } }
      ),
      presenting: model.mcpRemovalCandidate
    ) { name in
      Button("Cancel", role: .cancel) { model.cancelMcpServerRemoval() }
      Button("Remove server", role: .destructive) { model.confirmMcpServerRemoval(name) }
    } message: { name in
      Text(
        "Removes \(name) from mcp.json and deletes its Keychain token. The Agent loses this server's tools until you add it again."
      )
    }
  }

  private var emptyState: some View {
    VStack(alignment: .leading, spacing: 6) {
      Text("No MCP servers yet — this is optional.")
        .font(CSFont.ui(12.5, .semibold))
        .foregroundStyle(Color.primary)
      Text(
        "MCP servers extend the Agent with extra tools like code search, PR review, or web search. Add your first server below, or skip it and wire one any time."
      )
      .font(CSFont.mono(11, .medium))
      .foregroundStyle(Color.secondary)
      .fixedSize(horizontal: false, vertical: true)
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .padding(.horizontal, 16)
    .padding(.vertical, 14)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .fill(Color.primary.opacity(0.04))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
    )
  }
}

// MARK: - One server card (name · state · last test · actions · details)

private struct MCPServerRow: View {
  let server: CsMcpServer
  let pending: Bool
  let result: CsMcpTestResult?
  /// The server-wide permission rule from the live policy, nil when the
  /// server inherits the category defaults.
  let permissionLevel: String?
  let onToggle: () -> Void
  let onTest: () -> Void
  let onRemove: () -> Void

  @State private var showingDetails = false

  private var isRemote: Bool { server.transport == "remote" }

  private var accent: Color {
    guard server.enabled else { return Color.secondary }
    if pending { return CSColor.amber }
    if let result { return result.ok ? CSColor.olive : CSColor.terracotta }
    return Color.secondary
  }

  private var commandLine: String {
    server.args.isEmpty
      ? server.command
      : "\(server.command) \(server.args.joined(separator: " "))"
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 9) {
      HStack(spacing: 10) {
        Circle().fill(accent.opacity(0.85)).frame(width: 7, height: 7)
        Text(server.name)
          .font(CSFont.ui(13.5, .semibold))
          .foregroundStyle(Color.primary)
        Spacer(minLength: 0)
        enabledButton
        testButton
        removeButton
      }

      lastTestLine

      if server.name == "desktop-commander" {
        // No hardcoded per-level counts here: the Tools tab renders them from
        // the live registry. A frozen literal drifts from the policy it
        // claims to describe (review P2-12).
        Text(
          "Terminal and process tools always require Allow once. Commands and paths remain constrained to Agent workspace roots."
        )
        .font(CSFont.ui(11, .regular))
        .foregroundStyle(CSColor.amber)
        .fixedSize(horizontal: false, vertical: true)
      }

      DisclosureGroup(isExpanded: $showingDetails) {
        details.padding(.top, 6)
      } label: {
        Text("Details")
          .font(CSFont.ui(11))
          .foregroundStyle(Color.secondary)
      }
    }
    .padding(.horizontal, 15)
    .padding(.vertical, 12)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .fill(accent.opacity(0.05))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(accent.opacity(0.16), lineWidth: 1)
    )
  }

  // MARK: Last handshake

  /// One line about the most recent one-off handshake. It is a test result,
  /// never a live connection indicator: the Agent spawns servers per turn.
  @ViewBuilder private var lastTestLine: some View {
    if pending {
      resultLine(text: String(localized: "Checking the connection…"), color: CSColor.amber)
    } else if let result {
      if result.ok {
        resultLine(
          text: String(localized: "Last test: passed · \(Int(result.toolCount)) tools"),
          color: CSColor.oliveLight
        )
      } else {
        resultLine(text: String(localized: "Last test: failed"), color: CSColor.terracotta)
      }
    } else {
      resultLine(text: String(localized: "Connection not tested"), color: Color.secondary)
    }
  }

  private func resultLine(text: String, color: Color) -> some View {
    Text(text)
      .font(CSFont.mono(11, .semibold))
      .foregroundStyle(color)
      .lineLimit(2)
      .frame(maxWidth: .infinity, alignment: .leading)
  }

  // MARK: Details (collapsed by default)

  private var details: some View {
    VStack(alignment: .leading, spacing: 5) {
      detailRow(
        String(localized: "Transport"),
        isRemote
          ? String(localized: "HTTP connection") : String(localized: "Local process"))
      if isRemote {
        detailRow(String(localized: "Server URL"), server.endpoint)
        detailRow(
          String(localized: "Authentication"),
          server.authRef.isEmpty
            ? String(localized: "No authentication") : String(localized: "Token in Keychain"))
      } else {
        detailRow(String(localized: "Launch command"), commandLine)
      }
      if !server.envKeys.isEmpty {
        detailRow(
          String(localized: "Environment variables"), server.envKeys.joined(separator: ", "))
      }
      if let permissionLevel {
        detailRow(
          String(localized: "Permission rule"), ToolPermissionLabels.level(permissionLevel))
      }
      if let result, !pending {
        if result.ok, let identity = Self.handshakeIdentity(result) {
          detailRow(String(localized: "Server identity"), identity)
        }
        if !result.ok, !result.error.isEmpty {
          detailRow(String(localized: "Error details"), result.error)
        }
      }
    }
  }

  private func detailRow(_ label: String, _ value: String) -> some View {
    HStack(alignment: .firstTextBaseline, spacing: 8) {
      Text(label)
        .font(CSFont.ui(11, .medium))
        .foregroundStyle(Color.secondary)
        .frame(width: 150, alignment: .leading)
      Text(verbatim: value)
        .font(CSFont.mono(10.5, .regular))
        .foregroundStyle(Color.primary)
        .textSelection(.enabled)
        .fixedSize(horizontal: false, vertical: true)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
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

  // MARK: Actions

  /// The configured state. Clicking flips the `enabled` flag in mcp.json; it
  /// never connects or disconnects anything by itself.
  private var enabledButton: some View {
    Button(action: onToggle) {
      HStack(spacing: 5) {
        CSIconView(
          icon: .power, size: 8, weight: .semibold,
          color: server.enabled ? CSColor.oliveLight : Color.secondary)
        Text(
          server.enabled
            ? String(localized: "mcp.server.enabled", defaultValue: "Enabled")
            : String(localized: "mcp.server.disabled", defaultValue: "Disabled"))
      }
      .font(CSFont.mono(10, .semibold))
      .foregroundStyle(server.enabled ? CSColor.oliveLight : Color.secondary)
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
    .csFocusRing()
    .help(server.enabled ? "Disable this server" : "Enable this server")
  }

  private var testButton: some View {
    Button(action: onTest) {
      Text("Test", comment: "Button label: run a connection test")
        .font(CSFont.mono(10, .semibold))
        .foregroundStyle(pending ? Color.secondary : Color.primary)
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
        .background(
          RoundedRectangle(cornerRadius: 6, style: .continuous)
            .fill(Color.primary.opacity(0.08))
        )
        .overlay(
          RoundedRectangle(cornerRadius: 6, style: .continuous)
            .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
        )
    }
    .csFocusRing()
    .disabled(pending)
    .help("Spawn the server and list its tools")
  }

  private var removeButton: some View {
    Button(action: onRemove) {
      CSIconView(icon: .delete, size: 11, weight: .semibold, color: CSColor.terracotta)
        .frame(width: 28, height: 26)
        .background(
          RoundedRectangle(cornerRadius: 6, style: .continuous)
            .fill(Color.primary.opacity(0.08))
        )
        .overlay(
          RoundedRectangle(cornerRadius: 6, style: .continuous)
            .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
        )
    }
    .csFocusRing()
    .accessibilityLabel(Text("Remove", comment: "Button label: remove an MCP server"))
    .accessibilityHint("Asks for confirmation before removing the server from mcp.json.")
    .help("Remove this server from mcp.json…")
  }
}

// MARK: - Add-server form

private struct MCPAddServerForm: View {
  /// Returns nil on success, otherwise the store's refusal translated for the
  /// form. A failed add keeps every field as typed so the fix is one edit away,
  /// and the message sits under the field it names.
  let onAdd:
    (
      _ name: String, _ command: String, _ args: [String],
      _ endpoint: String, _ token: String
    ) -> MCPAddFailure?

  @State private var remote = false
  @State private var name: String = ""
  @State private var command: String = ""
  @State private var argsText: String = ""
  @State private var endpoint: String = ""
  @State private var token: String = ""
  @State private var addError: MCPAddFailure?
  @FocusState private var focusedField: Field?

  private enum Field { case name, endpoint, token, command, args }

  /// The field a refusal points at, if the form has one for it.
  private static func field(for failure: MCPAddFailure.Field?) -> Field? {
    switch failure {
    case .name: return .name
    case .command: return .command
    case .endpoint: return .endpoint
    case nil: return nil
    }
  }

  private var canAdd: Bool {
    !name.trimmingCharacters(in: .whitespaces).isEmpty
      && (remote
        ? endpoint.trimmingCharacters(in: .whitespaces).hasPrefix("http")
        : !command.trimmingCharacters(in: .whitespaces).isEmpty)
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 9) {
      Text("Add server", comment: "MCP servers: header of the form that adds a server")
        .textCase(.uppercase)
        .font(CSFont.mono(10, .semibold))
        .tracking(0.5)
        .foregroundStyle(Color.secondary)

      Picker("Transport", selection: $remote) {
        Text("Local process").tag(false)
        Text("HTTP connection").tag(true)
      }
      .pickerStyle(.segmented)
      .labelsHidden()

      labeledField("Server name", placeholder: "e.g. prview", text: $name, focus: .name)
      if remote {
        labeledField(
          "Server URL", placeholder: "https://…/mcp", text: $endpoint, focus: .endpoint)
        VStack(alignment: .leading, spacing: 4) {
          fieldLabel("Access token (optional)")
          // The label is the field's accessibility name; the visible caption
          // above stays a plain Text so the chrome matches the other fields.
          SecureField(text: $token, prompt: nil) { Text("Access token (optional)") }
            .labelsHidden()
            .focused($focusedField, equals: .token)
            .settingsInputChrome(isFocused: focusedField == .token)
            .onSubmit(submit)
          Text("The token is stored in the macOS Keychain, never in mcp.json.")
            .font(CSFont.ui(10.5))
            .foregroundStyle(Color.secondary)
            .fixedSize(horizontal: false, vertical: true)
        }
      } else {
        labeledField(
          "Launch command", placeholder: "e.g. prview", text: $command, focus: .command)
        labeledField(
          "Command arguments", placeholder: "e.g. mcp", text: $argsText, focus: .args)
      }

      // A refusal without a field of its own (store I/O, an unknown message)
      // still shows under the form; field-specific ones render under their
      // field inside `labeledField`.
      if let addError, Self.field(for: addError.field) == nil {
        errorLine(addError)
      }

      HStack {
        Spacer(minLength: 0)
        SettingsSaveButton(title: "Add", enabled: canAdd, action: submit)
      }
    }
    .padding(.horizontal, 15)
    .padding(.vertical, 13)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .fill(Color.primary.opacity(0.06))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
    )
  }

  private func fieldLabel(_ title: LocalizedStringKey) -> some View {
    Text(title)
      .font(CSFont.ui(11, .medium))
      .foregroundStyle(Color.secondary)
  }

  /// A field with its caption. The caption is also the field's accessibility
  /// name (`TextField(title…)` with the label hidden), so VoiceOver reads
  /// "Server name" and not the placeholder or the typed text. A refusal that
  /// names this field renders right under it.
  private func labeledField(
    _ title: LocalizedStringKey, placeholder: LocalizedStringKey, text: Binding<String>,
    focus: Field
  ) -> some View {
    VStack(alignment: .leading, spacing: 4) {
      fieldLabel(title)
      TextField(title, text: text, prompt: Text(placeholder))
        .labelsHidden()
        .focused($focusedField, equals: focus)
        .settingsInputChrome(isFocused: focusedField == focus)
        .onSubmit(submit)
      if let addError, Self.field(for: addError.field) == focus {
        errorLine(addError)
      }
    }
  }

  /// The user sentence in the accent colour; the store's own words, when they
  /// differ, stay one hover away instead of on the screen.
  private func errorLine(_ failure: MCPAddFailure) -> some View {
    Text(verbatim: failure.message)
      .font(CSFont.mono(10.5, .medium))
      .foregroundStyle(CSColor.terracotta)
      .textSelection(.enabled)
      .fixedSize(horizontal: false, vertical: true)
      .help(failure.detail == failure.message ? "" : failure.detail)
      .accessibilityIdentifier("settings-mcp-add-error")
  }

  private func submit() {
    guard canAdd else { return }
    let args =
      argsText
      .split(whereSeparator: { $0 == " " || $0 == "\t" })
      .map(String.init)
    addError = onAdd(
      name.trimmingCharacters(in: .whitespaces),
      remote ? "" : command.trimmingCharacters(in: .whitespaces),
      remote ? [] : args,
      remote ? endpoint.trimmingCharacters(in: .whitespaces) : "",
      remote ? token : ""
    )
    guard addError == nil else {
      if let field = Self.field(for: addError?.field) { focusedField = field }
      return
    }
    name = ""
    command = ""
    argsText = ""
    endpoint = ""
    token = ""
  }
}
