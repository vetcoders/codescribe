import SwiftUI

// B2 — tool permissions panel: global defaults + hierarchical per-capability
// tri-state. Backed by the same registry the agent dispatcher uses
// (`listToolCapabilities`) and durable settings.json agent.permissions via the
// MCP admin bridge. Identity contract: `server:tool` / `native:name`.
//
// Every `Allow · Ask · Deny` picker sits at its own width (`fixedSize`): a
// segmented control cannot shrink below its labels, and a frame narrower than
// them lets it spill over both edges of its card. The Settings window follows
// the content minimum, so a row of three such pickers also forced the window
// wider than the screen and made it jump when this tab opened. Defaults are
// therefore one row per scope, the same shape as the tool rows below.

struct ToolPermissionsSection: View {
  @ObservedObject var model: SettingsViewModel
  @State private var searchText = ""
  /// Server whose tools are listed. View state; when the search filters it
  /// away the browser shows the first server with hits instead.
  @State private var selectedServer: String?

  private var grouped: [(server: String, items: [ToolPermissionItem])] {
    ToolPermissionGrouping.groups(
      from: model.toolCapabilities.map(ToolPermissionItem.init(capability:)),
      query: searchText
    )
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      defaultsCard

      // The real resolution order, so nobody reads "Deny wins over everything"
      // into a screen where a tool rule outranks its server's rule.
      Text(
        "A rule set for one tool outranks its server's rule, and both outrank the category defaults. Destructive tools are always blocked, and reading a path that may hold secrets always asks first."
      )
      .font(CSFont.ui(11.5))
      .foregroundStyle(Color.secondary)
      .fixedSize(horizontal: false, vertical: true)
      .padding(.top, CSSpace.control)

      if model.toolCapabilities.isEmpty {
        // Discovery spawns every configured MCP server, so the first pass
        // takes seconds: say so instead of showing an empty catalog.
        if model.toolCatalogLoading {
          loadingCapabilities
            .padding(.top, 12)
        } else {
          emptyCapabilities
            .padding(.top, 12)
        }
      } else {
        // The count is the whole catalog, not the number of individual rules.
        SettingsSectionLabel(
          String(localized: "Per-tool permissions · \(model.toolCapabilities.count)")
        )
        .padding(.top, CSSpace.section)
        ToolOverridesBrowser(
          model: model,
          groups: grouped,
          searchText: $searchText,
          selectedServer: $selectedServer
        )
        .padding(.top, CSSpace.control)
      }
    }
    .onAppear { model.reloadToolPermissions() }
  }

  private var defaultsCard: some View {
    VStack(alignment: .leading, spacing: 10) {
      Text("Defaults")
        .font(CSFont.ui(12.5, .semibold))
        .foregroundStyle(Color.primary)

      defaultRow(title: "Read data", selection: $model.readOnlyDefaultPicker)
      defaultRow(title: "Changes, processes and network", selection: $model.sideEffectDefaultPicker)
      defaultRow(title: "Unclassified tools", selection: $model.globalDefaultPicker)
    }
    .padding(CSSpace.card)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .fill(Color.primary.opacity(0.04))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
    )
  }

  private func defaultRow(title: LocalizedStringKey, selection: Binding<String>) -> some View {
    HStack(spacing: 12) {
      Text(title)
        .font(CSFont.ui(12.5, .medium))
        .foregroundStyle(Color.primary)
      Spacer(minLength: 12)
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
    HStack(spacing: 8) {
      ProgressView()
        .controlSize(.small)
      Text("Discovering tools from the MCP servers…")
        .font(CSFont.mono(11, .medium))
        .foregroundStyle(Color.secondary)
    }
    .padding(.vertical, 10)
    .accessibilityIdentifier("settings-tool-catalog-loading")
  }

  private var emptyCapabilities: some View {
    Text("No tools registered yet — open the Agent once or add an MCP server.")
      .font(CSFont.mono(11, .medium))
      .foregroundStyle(Color.secondary)
      .padding(.vertical, 10)
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

  /// Readable name shown above the raw identifier.
  var displayName: String { ToolPermissionLabels.displayName(for: name) }

  /// Only an individual rule can be cleared back to inheritance.
  var hasIndividualRule: Bool { ruleSource == "tool" }
}

/// Interface-language labels for the raw registry strings. Identifiers stay
/// verbatim in the details line; only the UI wording changes.
enum ToolPermissionLabels {
  /// `apply_patch` → "Apply patch", `mcp__dc__write_file` → "Write file".
  /// A name without separators is returned as is.
  static func displayName(for name: String) -> String {
    var base = Substring(name)
    if let range = base.range(of: "__", options: .backwards) {
      base = base[range.upperBound...]
    }
    let words = base.split(whereSeparator: { $0 == "_" || $0 == "-" }).map(String.init)
    guard let first = words.first else { return name }
    return ([first.prefix(1).uppercased() + first.dropFirst()] + words.dropFirst())
      .joined(separator: " ")
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
    case "thread": return String(localized: "Overridden for this thread")
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

  var body: some View {
    HStack(alignment: .center, spacing: 12) {
      VStack(alignment: .leading, spacing: 2) {
        Text(item.displayName)
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(Color.primary)
          .lineLimit(1)
        Text(item.identity)
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(Color.secondary)
          .lineLimit(1)
        Text(
          verbatim:
            "\(ToolPermissionLabels.origin(item.origin)) · \(ToolPermissionLabels.risk(item.risk))"
        )
        .font(CSFont.mono(10, .medium))
        .foregroundStyle(Color.secondary)
        // Stacked, not side by side: the column next to a `fixedSize` picker is
        // narrow at the minimum window width. Short nouns say where the current
        // value comes from; the action names what it does, not "inheritance".
        VStack(alignment: .leading, spacing: 2) {
          Text(ToolPermissionLabels.ruleCaption(item.ruleSource))
            .font(CSFont.ui(10.5))
            .foregroundStyle(Color.secondary)
          if item.hasIndividualRule, let restoreInheritance {
            Button(action: restoreInheritance) {
              Text("Remove rule")
                .font(CSFont.ui(10.5, .medium))
            }
            .buttonStyle(.link)
            .accessibilityIdentifier("settings-tool-restore-\(item.identity)")
          }
        }
        .padding(.top, 2)
      }
      Spacer(minLength: 8)
      Picker("Permission for \(item.displayName)", selection: $level) {
        Text("Allow", comment: "Tool permission level").tag("allow")
        Text("Ask", comment: "Tool permission level").tag("ask")
        Text("Deny", comment: "Tool permission level").tag("deny")
      }
      .labelsHidden()
      .pickerStyle(.segmented)
      .fixedSize()
    }
    .padding(.horizontal, 14)
    .padding(.vertical, 10)
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
