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
//
// Round 16: "Add server" is an ordinary section title above its card, not an
// uppercase mono eyebrow inside it; the card actions are the one Settings
// chip; and the form sits on the shared card with the pane's own spacing
// (Founder brief, round 16, 2026-10-18). Round 20 dropped the empty-state card
// that used to sit above the form when there were no servers yet: the form
// already is the only action, so the pane opens straight on it (UI-28).

struct MCPServersSection: View {
  @ObservedObject var model: SettingsViewModel
  @State private var confirmingClear = false
  @State private var showingTechnicalDetails = false

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      // No empty-state card any more (UI-28, round 20): with no servers, the
      // pane opens straight on the "Add server" form below instead of a card
      // that only restated what the form already offers.
      if !model.mcpServers.isEmpty {
        VStack(spacing: CSSpace.xs) {
          ForEach(model.mcpServers, id: \.name) { server in
            MCPServerRow(
              server: server,
              pending: model.mcpTestPending.contains(server.name),
              result: model.mcpTestResults[server.name],
              permissionLevel: model.mcpServerPermissionLevel(server.name),
              onToggle: { model.toggleMcpServer(server) },
              onTest: { model.testMcpServer(server.name) },
              onRemove: { model.requestMcpServerRemoval(server.name) },
              authorizationPending: model.mcpAuthorizationPending.contains(server.name),
              authNotice: model.mcpAuthNotices[server.name],
              onAuthorize: { model.authorizeMcpServer(server) },
              onSave: { command, args, endpoint, token, oauth in
                model.updateMcpServer(
                  server, command: command, args: args, endpoint: endpoint, token: token,
                  oauth: oauth)
              }
            )
          }
        }
      }

      // The form's title is a section title above its card, like every other
      // section of the pane — never an uppercase eyebrow inside the card.
      SettingsSectionHeaderRow(
        String(
          localized: "Add server",
          comment: "MCP servers: header of the form that adds a server")
      )
      .padding(.top, model.mcpServers.isEmpty ? 0 : SettingsScale.sectionGap)

      MCPServerForm { name, command, args, endpoint, token, oauth in
        model.addMcpServer(
          name: name, command: command, args: args,
          endpoint: endpoint, token: token, oauth: oauth
        )
      }
      .padding(.top, SettingsScale.headerGap)

      DisclosureGroup(isExpanded: $showingTechnicalDetails) {
        // The group takes its content's width; without this frame the pane
        // centres it under the form (Founder, 2026-10-23 01:20).
        VStack(alignment: .leading, spacing: CSSpace.xs) {
          Text("Edited on disk in mcp.json. Hand edits (env, custom fields) are preserved.")
            .font(SettingsScale.helper)
            .foregroundStyle(CSColor.secondaryInk)
            .fixedSize(horizontal: false, vertical: true)
          Text(verbatim: "~/.codescribe/mcp.json")
            .font(CSFont.mono(10, .medium))
            .foregroundStyle(CSColor.secondaryInk)
            .textSelection(.enabled)
          Button(role: .destructive) {
            confirmingClear = true
          } label: {
            Text("Move MCP configuration to Trash…")
              .font(SettingsScale.controlLabel)
              .foregroundStyle(CSColor.dangerLight)
          }
          .csFocusRing()
          .padding(.top, CSSpace.xs)
          .accessibilityHint("Moves only mcp.json to Trash after confirmation.")
        }
        .padding(.top, CSSpace.xs)
        .frame(maxWidth: .infinity, alignment: .leading)
      } label: {
        Text("Technical details")
          .font(SettingsScale.helper)
          .foregroundStyle(CSColor.secondaryInk)
      }
      .padding(.top, SettingsScale.sectionGap)
      .accessibilityIdentifier("settings-mcp-technical-details")
    }
    .alert("Move MCP configuration to Trash?", isPresented: $confirmingClear) {
      Button("Cancel", role: .cancel) {}
      Button("Move mcp.json to Trash", role: .destructive) {
        model.clearMcpConfiguration()
      }
    } message: {
      Text(
        "Moves only ~/.codescribe/mcp.json to Trash. Recordings, transcripts, conversations, preferences, and API keys stay untouched."
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
}

// MARK: - One server card (name · state · last test · actions · details)

struct MCPServerRow: View {
  let server: CsMcpServer
  let pending: Bool
  let result: CsMcpTestResult?
  /// The server-wide permission rule from the live policy, nil when the
  /// server inherits the category defaults.
  let permissionLevel: String?
  let onToggle: () -> Void
  let onTest: () -> Void
  let onRemove: () -> Void
  let authorizationPending: Bool
  let authNotice: String?
  let onAuthorize: () -> Void
  let onSave: (String, [String], String, String, Bool) -> MCPAddFailure?

  @State private var showingDetails = false
  @State private var isEditing = false

  init(
    server: CsMcpServer, pending: Bool, result: CsMcpTestResult?,
    permissionLevel: String?, initiallyShowingDetails: Bool = false,
    onToggle: @escaping () -> Void, onTest: @escaping () -> Void,
    onRemove: @escaping () -> Void, authorizationPending: Bool = false,
    authNotice: String? = nil, onAuthorize: @escaping () -> Void = {},
    onSave: @escaping (String, [String], String, String, Bool) -> MCPAddFailure? = {
      _, _, _, _, _ in nil
    }
  ) {
    self.server = server
    self.pending = pending
    self.result = result
    self.permissionLevel = permissionLevel
    self.onToggle = onToggle
    self.onTest = onTest
    self.onRemove = onRemove
    self.authorizationPending = authorizationPending
    self.authNotice = authNotice
    self.onAuthorize = onAuthorize
    self.onSave = onSave
    _showingDetails = State(initialValue: initiallyShowingDetails)
  }

  private var isRemote: Bool { server.transport == "remote" }

  private var accent: Color {
    guard server.enabled else { return CSColor.secondaryInk }
    if pending { return CSColor.amberLight }
    if let result { return result.ok ? CSColor.olive : CSColor.dangerLight }
    return CSColor.secondaryInk
  }

  private var commandLine: String {
    server.args.isEmpty
      ? server.command
      : "\(server.command) \(server.args.joined(separator: " "))"
  }

  var body: some View {
    VStack(alignment: .leading, spacing: CSSpace.xs) {
      HStack(alignment: .top, spacing: CSSpace.sm) {
        // A setting name inside a card: the "Add server" title above the form
        // is the section rung, and a card's own name never matches it.
        Text(server.name)
          .font(SettingsScale.settingName)
          .foregroundStyle(Color.primary)
          .fixedSize(horizontal: false, vertical: true)
          .layoutPriority(1)
        Spacer(minLength: CSSpace.sm)
        HStack(spacing: CSSpace.xs) {
          enabledToggle
          if server.oauth && isRemote {
            SettingsChipButton(action: onAuthorize) {
              Text(
                authorizationPending
                  ? String(localized: "Signing in…")
                  : (server.authRef.isEmpty
                    ? String(localized: "Sign in") : String(localized: "Sign in again"))
              )
              .font(SettingsScale.controlLabel)
            }
            .disabled(authorizationPending)
          }
          testButton
          SettingsChipButton(action: { isEditing.toggle() }) {
            Text("Edit").font(SettingsScale.controlLabel)
          }
          .accessibilityIdentifier("settings-mcp-edit-\(server.name)")
          removeButton
        }
        .fixedSize(horizontal: true, vertical: false)
      }

      if isEditing {
        MCPServerForm(server: server, onCancel: { isEditing = false }) {
          _, command, args, endpoint, token, oauth in
          let failure = onSave(command, args, endpoint, token, oauth)
          if failure == nil { isEditing = false }
          return failure
        }
      }

      if let authNotice, !authNotice.isEmpty {
        resultLine(text: authNotice, color: CSColor.secondaryInk)
      }

      DisclosureGroup(isExpanded: $showingDetails) {
        details.padding(.top, CSSpace.xs)
      } label: {
        HStack(spacing: CSSpace.sm) {
          lastTestLine
            .layoutPriority(1)
          Spacer(minLength: CSSpace.sm)
          HStack(spacing: 5) {
            Text("Details")
              .font(SettingsScale.helper)
            Image(systemName: "chevron.right")
              .font(.system(size: 8, weight: .semibold))
              .rotationEffect(.degrees(showingDetails ? 90 : 0))
              .accessibilityHidden(true)
          }
          .foregroundStyle(CSColor.secondaryInk)
          .fixedSize(horizontal: true, vertical: false)
        }
      }
      .disclosureGroupStyle(MCPServerRowDisclosureStyle())
    }
    .padding(SettingsScale.cardPadding)
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
      resultLine(text: String(localized: "Checking the connection…"), color: CSColor.amberLight)
    } else if let result {
      if result.ok {
        resultLine(
          text: String(localized: "Last test: passed · \(Int(result.toolCount)) tools"),
          color: CSColor.oliveLight
        )
      } else {
        resultLine(text: String(localized: "Last test: failed"), color: CSColor.dangerLight)
      }
    } else {
      resultLine(text: String(localized: "Connection not tested"), color: CSColor.secondaryInk)
    }
  }

  private func resultLine(text: String, color: Color) -> some View {
    Text(text)
      .font(CSFont.ui(11, .semibold))
      .foregroundStyle(color)
      .lineLimit(2)
      .frame(maxWidth: .infinity, alignment: .leading)
  }

  // MARK: Details (collapsed by default)

  private var details: some View {
    VStack(alignment: .leading, spacing: 5) {
      if server.name == "desktop-commander" {
        // No hardcoded per-level counts here: the Tools tab renders them from
        // the live registry. A frozen literal drifts from the policy it
        // claims to describe (review P2-12).
        Text(
          "Terminal and process tools always require Allow once. Commands and paths remain constrained to Agent workspace roots."
        )
        .font(CSFont.ui(11, .regular))
        .foregroundStyle(CSColor.amberLight)
        .fixedSize(horizontal: false, vertical: true)
        .padding(.bottom, 2)
      }
      detailRow(
        String(localized: "Transport"),
        isRemote
          ? String(localized: "HTTP connection") : String(localized: "Local process"))
      if isRemote {
        detailRow(String(localized: "Server URL"), server.endpoint)
        detailRow(
          String(localized: "Authentication"),
          server.oauth
            ? (server.authRef.isEmpty
              ? String(localized: "OAuth — not signed in")
              : String(localized: "OAuth — signed in"))
            : (server.authRef.isEmpty
              ? String(localized: "No authentication") : String(localized: "Token in Keychain")))
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
        .foregroundStyle(CSColor.secondaryInk)
        .frame(width: 150, alignment: .leading)
      Text(verbatim: value)
        .font(CSFont.mono(11, .regular))
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

  // The configured state is a switch. Test, Edit and Remove stay actions in
  // the shared Settings chip family.

  /// The configured state. Clicking flips the `enabled` flag in mcp.json; it
  /// never connects or disconnects anything by itself. The switch is the one
  /// Settings switch (`settingsSwitch()`), whose label is hidden, so the state
  /// word stays visible beside it and the server name labels it for VoiceOver.
  private var enabledToggle: some View {
    HStack(spacing: 5) {
      Toggle(
        isOn: Binding(
          get: { server.enabled },
          set: { enabled in
            if enabled != server.enabled { onToggle() }
          }
        )
      ) {
        Text(verbatim: server.name)
      }
      .settingsSwitch()
      Text(
        server.enabled
          ? String(localized: "mcp.server.enabled", defaultValue: "Enabled")
          : String(localized: "mcp.server.disabled", defaultValue: "Disabled")
      )
      .font(SettingsScale.controlLabel)
      .accessibilityHidden(true)
    }
    .help(server.enabled ? "Disable this server" : "Enable this server")
  }

  private var testButton: some View {
    SettingsChipButton(enabled: !pending, action: onTest) {
      Text("Test", comment: "Button label: run a connection test")
        .font(SettingsScale.controlLabel)
        .foregroundStyle(pending ? CSColor.secondaryInk : Color.primary)
    }
    .help("Spawn the server and list its tools")
  }

  private var removeButton: some View {
    SettingsChipButton(action: onRemove) {
      // A glyph has no cap height to pad against: the square matches the line
      // height of the two labelled chips beside it, so all three are one row.
      CSIconView(icon: .delete, size: 11, weight: .semibold, color: CSColor.dangerLight)
        .frame(width: 14, height: 14)
    }
    .accessibilityLabel(Text("Remove", comment: "Button label: remove an MCP server"))
    .accessibilityHint("Asks for confirmation before removing the server from mcp.json.")
    .help("Remove this server from mcp.json…")
  }
}

/// Keeps the compact handshake summary on the left and the disclosure affordance
/// on the right while allowing the technical body to use the card's full width.
private struct MCPServerRowDisclosureStyle: DisclosureGroupStyle {
  func makeBody(configuration: Configuration) -> some View {
    VStack(alignment: .leading, spacing: 0) {
      Button {
        configuration.isExpanded.toggle()
      } label: {
        configuration.label
          .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .contentShape(Rectangle())
      .csFocusRing()
      .accessibilityElement(children: .combine)
      .accessibilityValue(configuration.isExpanded ? Text("Expanded") : Text("Collapsed"))

      if configuration.isExpanded {
        configuration.content
      }
    }
  }
}

// MARK: - Add-server form

private struct MCPServerForm: View {
  /// Returns nil on success, otherwise the store's refusal translated for the
  /// form. A failed add keeps every field as typed so the fix is one edit away,
  /// and the message sits under the field it names.
  let onAdd:
    (
      _ name: String, _ command: String, _ args: [String],
      _ endpoint: String, _ token: String, _ oauth: Bool
    ) -> MCPAddFailure?
  let server: CsMcpServer?
  let onCancel: (() -> Void)?

  private struct Argument: Identifiable {
    let id = UUID()
    var value: String
  }

  @State private var remote = false
  @State private var name: String = ""
  @State private var command: String = ""
  @State private var arguments: [Argument] = []
  @State private var endpoint: String = ""
  @State private var token: String = ""
  @State private var oauth = false
  @State private var addError: MCPAddFailure?
  @FocusState private var focusedField: Field?

  private enum Field { case name, endpoint, token, command, args }

  init(
    server: CsMcpServer? = nil, onCancel: (() -> Void)? = nil,
    onAdd: @escaping (String, String, [String], String, String, Bool) -> MCPAddFailure?
  ) {
    self.server = server
    self.onCancel = onCancel
    self.onAdd = onAdd
    _remote = State(initialValue: server?.transport == "remote")
    _name = State(initialValue: server?.name ?? "")
    _command = State(initialValue: server?.command ?? "")
    _arguments = State(initialValue: (server?.args ?? []).map { Argument(value: $0) })
    _endpoint = State(initialValue: server?.endpoint ?? "")
    _oauth = State(initialValue: server?.oauth ?? false)
  }

  /// The field a refusal points at, if the form has one for it.
  private static func field(for failure: MCPAddFailure.Field?) -> Field? {
    switch failure {
    case .name: return .name
    case .command: return .command
    case .endpoint: return .endpoint
    case nil: return nil
    }
  }

  /// Add is live as soon as the form has anything in it. The store is the one
  /// validator (`core/mcp/config_store.rs` `validate_*`): a bad name, an empty
  /// command or a non-HTTP endpoint come back as a refusal the form can show
  /// under the right field, instead of a button that stays grey without a word.
  private var canAdd: Bool {
    !name.isEmpty || !(remote ? endpoint : command).isEmpty
  }

  var body: some View {
    // No title inside the card: `MCPServersSection` puts "Add server" above it
    // as an ordinary section title (Founder brief, round 16, 2026-10-18).
    VStack(alignment: .leading, spacing: SettingsScale.rowGap) {
      Picker("Transport", selection: $remote) {
        Text("Local process").tag(false)
        Text("HTTP connection").tag(true)
      }
      .pickerStyle(.segmented)
      .labelsHidden()
      .controlSize(.small)

      labeledField("Server name", placeholder: "e.g. prview", text: $name, focus: .name)
        .disabled(server != nil)
      if remote {
        labeledField(
          "Server URL", placeholder: "https://…/mcp", text: $endpoint, focus: .endpoint)
        Toggle(String(localized: "Sign in with OAuth"), isOn: $oauth)
          .toggleStyle(.checkbox)
          .font(SettingsScale.helper)
        if oauth {
          Text("Save the server, then use Sign in on its row — the browser opens like for provider accounts.")
            .font(SettingsScale.helper)
            .foregroundStyle(CSColor.secondaryInk)
            .fixedSize(horizontal: false, vertical: true)
        } else {
          VStack(alignment: .leading, spacing: CSSpace.xxs) {
            fieldLabel("Access token (optional)")
            // The label is the field's accessibility name; the visible caption
            // above stays a plain Text so the chrome matches the other fields.
            SecureField(text: $token, prompt: nil) { Text("Access token (optional)") }
              .labelsHidden()
              .focused($focusedField, equals: .token)
              .settingsInputChrome(isFocused: focusedField == .token)
              .onSubmit(submit)
            Text("The token is stored in the macOS Keychain, never in mcp.json.")
              .font(SettingsScale.helper)
              .foregroundStyle(CSColor.secondaryInk)
              .fixedSize(horizontal: false, vertical: true)
            if server?.authRef.isEmpty == false {
              Text("Leave the token empty to keep the saved token.")
                .font(SettingsScale.helper)
                .foregroundStyle(CSColor.secondaryInk)
            }
          }
        }
      } else {
        labeledField(
          "Launch command", placeholder: "e.g. /usr/local/bin/prview", text: $command,
          focus: .command)
        Text("Enter only the executable here. Add each command argument separately below.")
          .font(SettingsScale.helper)
          .foregroundStyle(CSColor.secondaryInk)
        fieldLabel("Command arguments")
        ForEach($arguments) { $argument in
          HStack {
            TextField("Argument", text: $argument.value)
              .settingsInputChrome(isFocused: focusedField == .args)
              .focused($focusedField, equals: .args)
            SettingsChipButton(action: { arguments.removeAll { $0.id == argument.id } }) {
              Image(systemName: "minus")
            }
            .accessibilityLabel("Remove argument")
          }
        }
        SettingsChipButton(action: { arguments.append(Argument(value: "")) }) {
          Text("Add argument").font(SettingsScale.controlLabel)
        }
      }

      // A refusal without a field of its own (store I/O, an unknown message)
      // still shows under the form; field-specific ones render under their
      // field inside `labeledField`.
      if let addError, Self.field(for: addError.visibleField(remote: remote)) == nil {
        errorLine(addError)
      }

      HStack {
        Spacer(minLength: 0)
        if let onCancel {
          SettingsChipButton(action: onCancel) { Text("Cancel") }
        }
        SettingsSaveButton(title: server == nil ? "Add" : "Save", enabled: canAdd, action: submit)
      }
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .settingsGroupedInset()
  }

  private func fieldLabel(_ title: LocalizedStringKey) -> some View {
    Text(title)
      .font(SettingsScale.helper)
      .foregroundStyle(CSColor.secondaryInk)
  }

  /// A field with its caption. The caption is also the field's accessibility
  /// name (`TextField(title…)` with the label hidden), so VoiceOver reads
  /// "Server name" and not the placeholder or the typed text. A refusal that
  /// names this field renders right under it.
  ///
  /// A long launch command, argument list or URL is not clipped: the field
  /// keeps one line and scrolls its own content, and the full value is one
  /// tooltip away while the field is not focused.
  private func labeledField(
    _ title: LocalizedStringKey, placeholder: LocalizedStringKey, text: Binding<String>,
    focus: Field
  ) -> some View {
    VStack(alignment: .leading, spacing: CSSpace.xxs) {
      fieldLabel(title)
      TextField(title, text: text, prompt: Text(placeholder))
        .labelsHidden()
        .focused($focusedField, equals: focus)
        .settingsInputChrome(isFocused: focusedField == focus)
        .onSubmit(submit)
        .help(text.wrappedValue)
      if let addError, Self.field(for: addError.visibleField(remote: remote)) == focus {
        errorLine(addError)
      }
    }
  }

  /// The user sentence in the error colour; the store's own words, when they
  /// differ, stay one hover away instead of on the screen.
  private func errorLine(_ failure: MCPAddFailure) -> some View {
    Text(verbatim: failure.message)
      .font(SettingsScale.helper)
      .foregroundStyle(CSColor.dangerLight)
      .textSelection(.enabled)
      .fixedSize(horizontal: false, vertical: true)
      .help(failure.detail == failure.message ? "" : failure.detail)
      .accessibilityIdentifier("settings-mcp-add-error")
  }

  private func submit() {
    guard canAdd else { return }
    // The name goes through untouched: the store rejects surrounding
    // whitespace rather than trimming it, and the form shows that refusal.
    addError = onAdd(
      name,
      remote ? "" : command.trimmingCharacters(in: .whitespaces),
      remote ? [] : arguments.map(\.value),
      remote ? endpoint.trimmingCharacters(in: .whitespaces) : "",
      remote && !oauth ? token : "",
      remote && oauth
    )?.forTransport(remote: remote, endpoint: endpoint)
    guard addError == nil else {
      if let field = Self.field(for: addError?.visibleField(remote: remote)) {
        focusedField = field
      }
      return
    }
    name = ""
    command = ""
    arguments = []
    endpoint = ""
    token = ""
    oauth = false
  }
}
