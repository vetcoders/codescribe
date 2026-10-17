import SwiftUI

/// Per-tool overrides, one source at a time, in one column: a search field, a
/// "Source" popup, then the selected source's tools at the full width of the
/// pane. The sources used to be a sidebar beside the cards — a real config
/// carries ten MCP servers plus native, more than a segmented bar can label —
/// but the column it took is the column the tool names need, so it became a
/// popup instead (Founder brief, round 12, 2026-10-10). The popup carries the
/// same names and counts the sidebar did.
///
/// Search filters the whole catalog; a selection the search hides falls back to
/// the first source with hits without forgetting the user's choice.
struct ToolOverridesBrowser: View {
  @ObservedObject var model: SettingsViewModel
  let groups: [(server: String, items: [ToolPermissionItem])]
  @Binding var searchText: String
  @Binding var selectedServer: String?

  var body: some View {
    let current = groups.first { $0.server == selectedServer } ?? groups.first
    VStack(alignment: .leading, spacing: CSSpace.control) {
      ToolSearchField(text: $searchText)

      if let current {
        HStack(spacing: CSSpace.md) {
          Text(Self.sourceRowTitle)
            .font(CSFont.ui(12.5, .medium))
            .foregroundStyle(Color.primary)
          Spacer(minLength: 12)
          // Reads the resolved source, writes the user's choice: a source the
          // search hid shows the fallback without overwriting what they picked.
          Picker(
            Self.sourceRowTitle,
            selection: Binding(
              get: { current.server },
              set: { selectedServer = $0 }
            )
          ) {
            ForEach(groups, id: \.server) { group in
              Text(verbatim: Self.sourceLabel(server: group.server, count: group.items.count))
                .tag(group.server)
            }
          }
          .labelsHidden()
          .pickerStyle(.menu)
          // No `fixedSize` here, unlike the segmented defaults: a popup can
          // shrink, and a long server name must compress rather than push the
          // Settings window past the screen.
          .accessibilityIdentifier("settings-tool-source")
        }

        ScrollView {
          LazyVStack(alignment: .leading, spacing: CSSpace.sm) {
            ForEach(current.items) { item in
              ToolCapabilityRow(
                item: item, level: $model[toolLevel: item.identity],
                restoreInheritance: { model.clearToolPermission(identity: item.identity) })
            }
          }
          .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(maxHeight: 420)
      } else {
        ContentUnavailableView.search(text: searchText)
          .frame(maxWidth: .infinity, minHeight: 160)
      }
    }
  }

  /// Label of the popup row. Named once so the visible label and the picker's
  /// accessibility label cannot drift apart.
  static var sourceRowTitle: String {
    String(localized: "Source", comment: "Tool permissions: which tool source is listed")
  }

  /// One popup entry: the source's own name plus how many tools it carries —
  /// "Native (26)". `native` is a UI label, every server keeps its name.
  static func sourceLabel(server: String, count: Int) -> String {
    "\(ToolPermissionLabels.source(server)) (\(count))"
  }
}
