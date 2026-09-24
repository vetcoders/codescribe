import SwiftUI

// B2 — tool permissions panel: global defaults + hierarchical per-capability
// tri-state. Backed by the same registry the agent dispatcher uses
// (`listToolCapabilities`) and durable settings.json agent.permissions via the
// MCP admin bridge. Identity contract: `server:tool` / `native:name`.

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
      SettingsSectionLabel("Tool permissions")

      Text(
        "Allow · Ask · Deny. Defaults: read-only allow, side-effectful ask. "
          + "\"Always allow\" from the approval card writes the same identity key."
      )
      .font(CSFont.mono(11, .medium))
      .foregroundStyle(CSColor.textFaint)
      .padding(.top, 4)

      defaultsCard
        .padding(.top, CSSpace.control)

      if model.toolCapabilities.isEmpty {
        emptyCapabilities
          .padding(.top, 12)
      } else {
        SettingsSectionLabel("Tool overrides · \(model.toolCapabilities.count)")
          .padding(.top, CSSpace.section)
        ToolOverridesBrowser(
          groups: grouped,
          searchText: $searchText,
          selectedServer: $selectedServer,
          onLevel: { identity, level in
            model.setToolPermission(identity: identity, level: level)
          }
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
        .foregroundStyle(CSColor.textBody)

      HStack(spacing: 12) {
        defaultPicker(
          title: "Read-only",
          selection: Binding(
            get: { model.permissionPolicy.readOnlyDefault },
            set: { model.setPermissionDefault(kind: .readOnly, level: $0) }
          )
        )
        defaultPicker(
          title: "Side effects",
          selection: Binding(
            get: { model.permissionPolicy.sideEffectDefault },
            set: { model.setPermissionDefault(kind: .sideEffect, level: $0) }
          )
        )
        defaultPicker(
          title: "Global / unknown",
          selection: Binding(
            get: { model.permissionPolicy.defaultLevel },
            set: { model.setPermissionDefault(kind: .global, level: $0) }
          )
        )
      }
    }
    .padding(CSSpace.card)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(
      RoundedRectangle(cornerRadius: 11, style: .continuous)
        .fill(CSColor.surfaceRaised(0.02))
    )
    .overlay(
      RoundedRectangle(cornerRadius: 11, style: .continuous)
        .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
    )
  }

  private func defaultPicker(title: String, selection: Binding<String>) -> some View {
    VStack(alignment: .leading, spacing: 4) {
      Text(title)
        .font(CSFont.mono(10, .medium))
        .foregroundStyle(CSColor.textFaint)
      Picker(title, selection: selection) {
        Text("Allow").tag("allow")
        Text("Ask").tag("ask")
        Text("Deny").tag("deny")
      }
      .labelsHidden()
      .pickerStyle(.segmented)
      .frame(maxWidth: 180)
    }
  }

  private var emptyCapabilities: some View {
    Text("No tools registered yet — open the agent once or add an MCP server.")
      .font(CSFont.mono(11, .medium))
      .foregroundStyle(CSColor.textFaint)
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

  init(
    name: String,
    identity: String,
    server: String,
    origin: String,
    risk: String,
    effective: String
  ) {
    self.name = name
    self.identity = identity
    self.server = server
    self.origin = origin
    self.risk = risk
    self.effective = effective
  }

  init(capability: CsToolCapability) {
    self.name = capability.name
    self.identity = capability.identity
    self.server = capability.server
    self.origin = capability.origin
    self.risk = capability.risk
    self.effective = capability.effective
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
  let onLevel: @MainActor (String) -> Void

  var body: some View {
    HStack(alignment: .center, spacing: 12) {
      VStack(alignment: .leading, spacing: 2) {
        Text(item.name)
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(CSColor.textBody)
          .lineLimit(1)
        Text(item.identity)
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.textFaint)
          .lineLimit(1)
        Text("\(item.origin) · \(item.risk)")
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(CSColor.textFaint)
      }
      Spacer(minLength: 8)
      Picker(
        "Level",
        selection: Binding(
          get: { item.effective },
          set: { onLevel($0) }
        )
      ) {
        Text("Allow").tag("allow")
        Text("Ask").tag("ask")
        Text("Deny").tag("deny")
      }
      .labelsHidden()
      .pickerStyle(.segmented)
      .frame(width: 180)
    }
    .padding(.horizontal, 14)
    .padding(.vertical, 10)
    .background(
      RoundedRectangle(cornerRadius: 10, style: .continuous)
        .fill(CSColor.surfaceRaised(0.02))
    )
    .overlay(
      RoundedRectangle(cornerRadius: 10, style: .continuous)
        .strokeBorder(CSColor.hairline(0.06), lineWidth: 1)
    )
  }
}
