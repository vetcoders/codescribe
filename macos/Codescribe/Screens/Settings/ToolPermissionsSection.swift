import SwiftUI

// B2 — tool permissions panel: category defaults + hierarchical per-capability
// tri-state. Backed by the same registry the agent dispatcher uses
// (`listToolCapabilities`) and durable settings.json agent.permissions via the
// MCP admin bridge. Identity contract: `server:tool` / `native:name`.
//
// One column, two bands under two equal section titles: "Defaults" with the
// safety note that says what no setting here can lift, then "Individual tools"
// (search and source on one row, then full-width cards). The sources used to be
// a sidebar beside the cards; it took the width the tool names need, so the
// names truncated to an ellipsis ("Aicx index sta…"). The sources moved into a
// popup, the cards took the full width, and a name now wraps instead of
// truncating (Founder brief, round 12, 2026-10-10).
//
// Round 16 made the per-tool cards compact: one card owner
// (`settingsGroupedInset`), the scale's own type rungs instead of five ad hoc
// sizes, a tighter inset, a small picker, and the raw identifier dropped to the
// quietest rung while staying selectable. Both band titles are
// `SettingsSectionHeaderRow`, so neither reads weaker than the settings under
// it (Founder brief, round 16, 2026-10-18).
//
// Every `Allow · Ask · Deny` picker in the defaults sits at its own width
// (`fixedSize`): a segmented control cannot shrink below its labels, and a
// frame narrower than them lets it spill over both edges of its card. The
// Settings window follows the content minimum, so a row of three such pickers
// also forced the window wider than the screen and made it jump when this tab
// opened. Defaults are therefore one row per category; a tool card instead
// puts its picker on a row of its own at full width, which is exactly what
// frees the name's line.

struct ToolPermissionsSection: View {
  @ObservedObject var model: SettingsViewModel
  @State private var searchText = ""
  /// Source whose tools are listed. View state; when the search filters it
  /// away the browser shows the first source with hits instead.
  @State private var selectedServer: String?

  private var grouped: [(server: String, items: [ToolPermissionItem])] {
    ToolPermissionGrouping.groups(
      from: model.toolCapabilities.map(ToolPermissionItem.init(capability:)),
      query: searchText
    )
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      // Both bands carry the same header rung, so "Defaults" is no longer a
      // setting name standing in for a section title.
      SettingsSectionHeaderRow(String(localized: "Defaults"))

      defaultsCard
        .padding(.top, SettingsScale.headerGap)

      // The real resolution order, so nobody reads "Deny wins over everything"
      // into a screen where a tool rule outranks its server's rule.
      Text(
        "A rule for one tool outranks its server's rule, and both outrank these defaults. Destructive tools stay blocked, and a path that may hold secrets always asks."
      )
      .font(SettingsScale.helper)
      .foregroundStyle(Color.secondary)
      .fixedSize(horizontal: false, vertical: true)
      .padding(.top, SettingsScale.headerGap)

      if model.toolCapabilities.isEmpty {
        // Discovery spawns every configured MCP server, so the first pass
        // takes seconds: say so instead of showing an empty catalog.
        if model.toolCatalogLoading {
          loadingCapabilities
            .padding(.top, SettingsScale.rowGap)
        } else {
          emptyCapabilities
            .padding(.top, SettingsScale.rowGap)
        }
      } else {
        // No rule between the bands: two equal titles already separate them.
        // The count is the whole catalog, not the number of individual rules.
        SettingsSectionHeaderRow(String(localized: "Individual tools")) {
          Text("\(model.toolCapabilities.count) tools", comment: "Plural: count of tools")
            .font(SettingsScale.helper)
            .foregroundStyle(Color.secondary)
        }
        .padding(.top, SettingsScale.sectionGap)

        ToolOverridesBrowser(
          model: model,
          groups: grouped,
          searchText: $searchText,
          selectedServer: $selectedServer
        )
        .padding(.top, SettingsScale.headerGap)
      }
    }
    .onAppear { model.reloadToolPermissions() }
  }

  private var defaultsCard: some View {
    VStack(alignment: .leading, spacing: SettingsScale.rowGap) {
      defaultRow(title: "Read data", selection: $model.readOnlyDefaultPicker)
      defaultRow(title: "Changes, processes and network", selection: $model.sideEffectDefaultPicker)
      defaultRow(title: "Unclassified", selection: $model.globalDefaultPicker)
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .settingsGroupedInset()
  }

  private func defaultRow(title: LocalizedStringKey, selection: Binding<String>) -> some View {
    HStack(spacing: CSSpace.md) {
      Text(title)
        .font(SettingsScale.settingName)
        .foregroundStyle(Color.primary)
      Spacer(minLength: CSSpace.md)
      Picker(title, selection: selection) {
        Text("Allow", comment: "Tool permission level").tag("allow")
        Text("Ask", comment: "Tool permission level").tag("ask")
        Text("Deny", comment: "Tool permission level").tag("deny")
      }
      .labelsHidden()
      .pickerStyle(.segmented)
      .fixedSize()
    }
  }

  private var loadingCapabilities: some View {
    HStack(spacing: CSSpace.sm) {
      ProgressView()
        .controlSize(.small)
      Text("Discovering tools from the MCP servers…")
        .font(SettingsScale.helper)
        .foregroundStyle(Color.secondary)
    }
    .padding(.vertical, CSSpace.sm)
    .accessibilityIdentifier("settings-tool-catalog-loading")
  }

  private var emptyCapabilities: some View {
    Text("No tools registered yet — open the Agent once or add an MCP server.")
      .font(SettingsScale.helper)
      .foregroundStyle(Color.secondary)
      .padding(.vertical, CSSpace.sm)
  }
}

// MARK: - Hierarchy model (pure, testable)

/// Lightweight projection of `CsToolCapability` so grouping/filter unit tests
/// do not need a live UniFFI registry.
struct ToolPermissionItem: Equatable, Hashable, Identifiable {
  var id: String { identity }
  let name: String
  let identity: String
  let server: String
  let origin: String
  let risk: String
  let effective: String
  /// Rule behind `effective`: `tool` (individual), `server` / `default`
  /// (inherited) or `thread`.
  let ruleSource: String

  init(
    name: String,
    identity: String,
    server: String,
    origin: String,
    risk: String,
    effective: String,
    ruleSource: String = "default"
  ) {
    self.name = name
    self.identity = identity
    self.server = server
    self.origin = origin
    self.risk = risk
    self.effective = effective
    self.ruleSource = ruleSource
  }

  init(capability: CsToolCapability) {
    self.name = capability.name
    self.identity = capability.identity
    self.server = capability.server
    self.origin = capability.origin
    self.risk = capability.risk
    self.effective = capability.effective
    self.ruleSource = capability.ruleSource
  }

  /// Readable name shown above the raw identifier. Our own tools are named in
  /// the interface language; an MCP server's tools keep the vendor's spelling.
  var displayName: String {
    ToolPermissionLabels.displayName(for: name, identity: identity)
  }

  /// The one quiet line under the name: where the tool comes from and which
  /// category decides its default — "MCP · aicx-http · Read data", or
  /// "Native · Read data" for one of ours. The rule that actually won sits
  /// opposite it, so neither fact takes a row of its own
  /// (Founder brief, round 12, 2026-10-10).
  var sourceSummary: String {
    let source = ToolPermissionLabels.origin(origin)
    let category = ToolPermissionLabels.risk(risk)
    let group = ToolPermissionGrouping.groupKey(server: server, identity: identity)
    guard group != "native", group != origin else { return "\(source) · \(category)" }
    return "\(source) · \(group) · \(category)"
  }

  /// Only an individual rule can be cleared back to inheritance.
  var hasIndividualRule: Bool { ruleSource == "tool" }
}

/// Interface-language labels for the raw registry strings. Identifiers stay
/// verbatim in the details line; only the UI wording changes.
enum ToolPermissionLabels {
  /// Names that spell themselves in lower case, so sentence-casing the first
  /// word of a tool name must leave them alone: it is always `aicx`, never
  /// `Aicx` (Founder brief, round 12, 2026-10-10). Compared case-insensitively
  /// because the registry name is the vendor's, not ours.
  private static let lowercasedNames: Set<String> = ["aicx"]

  /// Spells a registry name out: `mcp__dc__write_file` → "Write file". Used
  /// for tools whose wording is not ours to write. A name without separators
  /// is returned as is.
  static func displayName(for name: String) -> String {
    var base = Substring(name)
    if let range = base.range(of: "__", options: .backwards) {
      base = base[range.upperBound...]
    }
    let words = base.split(whereSeparator: { $0 == "_" || $0 == "-" }).map(String.init)
    guard let first = words.first else { return name }
    let head =
      lowercasedNames.contains(first.lowercased())
      ? first.lowercased()
      : first.prefix(1).uppercased() + first.dropFirst()
    return ([head] + words.dropFirst()).joined(separator: " ")
  }

  /// Name of a tool for the rows. A `native:` identity is one of our own
  /// tools, so its name is interface copy ([`nativeDisplayName`]); every other
  /// identity belongs to an MCP server and keeps the vendor's own wording,
  /// spelled out of the registry name.
  static func displayName(for name: String, identity: String) -> String {
    if identity.hasPrefix("native:"), let own = nativeDisplayName(for: name) {
      return own
    }
    return displayName(for: name)
  }

  /// Interface-language name of one of our own native tools, keyed by the
  /// registry name. The name itself is identity and is never localized (R6):
  /// it stays in the `native:<name>` line under the title. `nil` means the
  /// registry grew a tool this table does not name yet, and the caller falls
  /// back to the spelled-out registry name.
  static func nativeDisplayName(for name: String) -> String? {
    switch name {
    case "apply_patch":
      return String(
        localized: "tools.native.apply_patch", defaultValue: "Apply patch",
        comment: "Native tool name: apply a patch to a file")
    case "fetch_github_file":
      return String(
        localized: "tools.native.fetch_github_file", defaultValue: "Fetch GitHub file",
        comment: "Native tool name: read one file out of a GitHub repository")
    case "get_frontmost_app":
      return String(
        localized: "tools.native.get_frontmost_app", defaultValue: "Read the active app",
        comment: "Native tool name: observe which application is in front")
    case "get_selected_text":
      return String(
        localized: "tools.native.get_selected_text", defaultValue: "Read selected text",
        comment: "Native tool name: observe the text selected in the active app")
    case "git_commit":
      return String(
        localized: "tools.native.git_commit", defaultValue: "Commit in Git",
        comment: "Native tool name: record a Git commit")
    case "git_diff":
      return String(
        localized: "tools.native.git_diff", defaultValue: "Show Git changes",
        comment: "Native tool name: read the Git diff")
    case "git_log":
      return String(
        localized: "tools.native.git_log", defaultValue: "Show Git history",
        comment: "Native tool name: read the Git log")
    case "git_status":
      return String(
        localized: "tools.native.git_status", defaultValue: "Check Git state",
        comment: "Native tool name: read the Git status")
    case "list_directory":
      return String(
        localized: "tools.native.list_directory", defaultValue: "List a folder",
        comment: "Native tool name: list what a folder holds")
    case "list_projects":
      return String(
        localized: "tools.native.list_projects", defaultValue: "List projects",
        comment: "Native tool name: enumerate the projects in the allowed folders")
    case "monitor_run":
      return String(
        localized: "tools.native.monitor_run", defaultValue: "Monitor a run",
        comment: "Native tool name: watch a long-running job")
    case "move_path":
      return String(
        localized: "tools.native.move_path", defaultValue: "Move a file or folder",
        comment: "Native tool name: move or rename a path")
    case "observe_process":
      return String(
        localized: "tools.native.observe_process", defaultValue: "Observe a process",
        comment: "Native tool name: read the output of a running process")
    case "project_build":
      return String(
        localized: "tools.native.project_build", defaultValue: "Build the project",
        comment: "Native tool name: run the project's build")
    case "project_test":
      return String(
        localized: "tools.native.project_test", defaultValue: "Run the project tests",
        comment: "Native tool name: run the project's tests")
    case "read_clipboard":
      return String(
        localized: "tools.native.read_clipboard", defaultValue: "Read the clipboard",
        comment: "Native tool name: read what the clipboard holds")
    case "read_file":
      return String(
        localized: "tools.native.read_file", defaultValue: "Read a file",
        comment: "Native tool name: read a file's contents")
    case "run_process":
      return String(
        localized: "tools.native.run_process", defaultValue: "Run a process",
        comment: "Native tool name: start a command")
    case "search_files":
      return String(
        localized: "tools.native.search_files", defaultValue: "Search in files",
        comment: "Native tool name: search the contents of files")
    case "search_threads":
      return String(
        localized: "tools.native.search_threads", defaultValue: "Search in conversations",
        comment: "Native tool name: search the Agent's own conversations")
    case "stop_process":
      return String(
        localized: "tools.native.stop_process", defaultValue: "Stop a process",
        comment: "Native tool name: end a running command")
    case "take_screenshot":
      return String(
        localized: "tools.native.take_screenshot", defaultValue: "Take a screenshot",
        comment: "Native tool name: capture the screen")
    case "transcribe_audio":
      return String(
        localized: "tools.native.transcribe_audio", defaultValue: "Transcribe a recording",
        comment: "Native tool name: turn an audio recording into text")
    case "type_text":
      return String(
        localized: "tools.native.type_text", defaultValue: "Type text",
        comment: "Native tool name: insert text into the active app")
    case "write_clipboard":
      return String(
        localized: "tools.native.write_clipboard", defaultValue: "Write to the clipboard",
        comment: "Native tool name: put text on the clipboard")
    case "write_file":
      return String(
        localized: "tools.native.write_file", defaultValue: "Write a file",
        comment: "Native tool name: write a file's contents")
    default:
      return nil
    }
  }

  /// Left-column label; `native` is a UI label, every server keeps its name.
  static func source(_ key: String) -> String {
    key == "native"
      ? String(localized: "Native", comment: "Tool source: built-in tools") : key
  }

  static func origin(_ raw: String) -> String {
    if raw == "native" { return source(raw) }
    if raw.hasPrefix("mcp") { return "MCP" }
    return raw
  }

  static func risk(_ raw: String) -> String {
    switch raw {
    case "read_only": return String(localized: "Read data", comment: "Tool risk class")
    case "mutating": return String(localized: "Changes", comment: "Tool risk class")
    case "process_control": return String(localized: "Processes", comment: "Tool risk class")
    case "network": return String(localized: "Network", comment: "Tool risk class")
    case "destructive": return String(localized: "Destructive", comment: "Tool risk class")
    case "unknown": return String(localized: "Unclassified", comment: "Tool risk class")
    default: return raw
    }
  }

  /// Interface-language name of a permission level (`allow` / `ask` / `deny`).
  static func level(_ raw: String) -> String {
    switch raw {
    case "allow": return String(localized: "Allow", comment: "Tool permission level")
    case "ask": return String(localized: "Ask", comment: "Tool permission level")
    case "deny": return String(localized: "Deny", comment: "Tool permission level")
    default: return raw
    }
  }

  static func ruleCaption(_ source: String) -> String {
    switch source {
    case "tool": return String(localized: "Individual rule")
    case "server": return String(localized: "Server rule")
    case "thread": return String(localized: "Overridden for this conversation")
    default: return String(localized: "Category default")
    }
  }
}

enum ToolPermissionGrouping {
  /// Group key for hierarchy: prefer the live `server` field, else the
  /// identity prefix before `:`, else `native`.
  static func groupKey(server: String, identity: String) -> String {
    let trimmed = server.trimmingCharacters(in: .whitespacesAndNewlines)
    if !trimmed.isEmpty { return trimmed }
    if let colon = identity.firstIndex(of: ":") {
      let prefix = String(identity[..<colon]).trimmingCharacters(in: .whitespacesAndNewlines)
      if !prefix.isEmpty { return prefix }
    }
    return "native"
  }

  static func matches(_ item: ToolPermissionItem, query: String) -> Bool {
    let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    guard !q.isEmpty else { return true }
    return item.name.lowercased().contains(q)
      || item.identity.lowercased().contains(q)
      || item.server.lowercased().contains(q)
      || item.origin.lowercased().contains(q)
  }

  /// Filtered, then grouped by server, tools sorted by name within each group,
  /// groups sorted by server name (native first when present).
  static func groups(
    from items: [ToolPermissionItem],
    query: String
  ) -> [(server: String, items: [ToolPermissionItem])] {
    let filtered = items.filter { matches($0, query: query) }
    var buckets: [String: [ToolPermissionItem]] = [:]
    for item in filtered {
      let key = groupKey(server: item.server, identity: item.identity)
      buckets[key, default: []].append(item)
    }
    for key in buckets.keys {
      buckets[key]?.sort {
        if $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedSame {
          return $0.identity.localizedCaseInsensitiveCompare($1.identity) == .orderedAscending
        }
        return $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending
      }
    }
    return buckets.keys.sorted { a, b in
      if a == "native" { return true }
      if b == "native" { return false }
      return a.localizedCaseInsensitiveCompare(b) == .orderedAscending
    }.compactMap { key in
      guard let items = buckets[key], !items.isEmpty else { return nil }
      return (server: key, items: items)
    }
  }
}

// MARK: - Capability row

struct ToolCapabilityRow: View {
  let item: ToolPermissionItem
  @Binding var level: String
  /// Clears the individual rule so the tool inherits again; nil hides the action.
  var restoreInheritance: (() -> Void)? = nil

  /// Name and identifier in full, for the card's tooltip. The name itself is
  /// never shortened on screen any more, so this is the shortcut to the pair,
  /// not a rescue for a truncated line. Glue, not copy (R3).
  private var fullIdentification: String { "\(item.displayName) · \(item.identity)" }

  var body: some View {
    // One column: the name owns a full-width line and wraps, the quiet facts
    // sit under it, and the picker takes a row of its own. The name used to
    // share its line with a `fixedSize` segmented picker, which left it too
    // little room and truncated it to "Aicx index sta…"
    // (Founder brief, round 12, 2026-10-10).
    //
    // Round 16 pulled the three facts together: name, the rule that decided
    // the level and the category sit in one block at two type rungs, the raw
    // identifier drops to the quietest rung under them — still whole and now
    // selectable — and the picker follows at `controlSize(.small)`.
    VStack(alignment: .leading, spacing: CSSpace.xs) {
      VStack(alignment: .leading, spacing: 2) {
        HStack(alignment: .firstTextBaseline, spacing: CSSpace.md) {
          Text(item.displayName)
            .font(SettingsScale.settingName)
            .foregroundStyle(Color.primary)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
          // Where the level below comes from, in the same words the defaults
          // band uses: "Category default", "Server rule" or "Individual rule".
          // At the helper rung, not a rung below it, so a tool that follows
          // its category says so as plainly as the name is written. Capped so
          // a long translation wraps instead of eating the name's line.
          Text(ToolPermissionLabels.ruleCaption(item.ruleSource))
            .font(SettingsScale.helper)
            .foregroundStyle(Color.secondary)
            .multilineTextAlignment(.trailing)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: 150, alignment: .trailing)
        }
        Text(verbatim: item.sourceSummary)
          .font(SettingsScale.helper)
          .foregroundStyle(Color.secondary)
          .fixedSize(horizontal: false, vertical: true)
        // The technical identity, in the background but never hidden: the
        // scale's quietest mono rung, whole, and selectable so it can be
        // copied into a config or a bug report.
        Text(item.identity)
          .font(SettingsScale.metaKey)
          .foregroundStyle(Color.secondary)
          .textSelection(.enabled)
          .fixedSize(horizontal: false, vertical: true)
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      .help(fullIdentification)

      Picker("Permission for \(item.displayName)", selection: $level) {
        Text("Allow", comment: "Tool permission level").tag("allow")
        Text("Ask", comment: "Tool permission level").tag("ask")
        Text("Deny", comment: "Tool permission level").tag("deny")
      }
      .labelsHidden()
      .pickerStyle(.segmented)
      .frame(maxWidth: .infinity)
      .controlSize(.small)

      // The action names what it does, not "inheritance", and only a tool that
      // actually carries an individual rule can drop one. One button family:
      // the same chip every other flat Settings action uses.
      if item.hasIndividualRule, let restoreInheritance {
        SettingsChipButton(action: restoreInheritance) {
          Text("Remove rule")
            .font(SettingsScale.controlLabel)
            .foregroundStyle(Color.primary)
        }
        .accessibilityIdentifier("settings-tool-restore-\(item.identity)")
      }
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .settingsGroupedInset(padding: CSSpace.sm)
  }
}
