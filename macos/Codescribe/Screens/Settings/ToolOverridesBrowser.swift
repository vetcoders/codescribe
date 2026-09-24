import SwiftUI

/// Per-tool overrides, one server at a time: server tabs down the left, the
/// selected server's tools on the right. Vertical on purpose — a real config
/// carries ten MCP servers plus native, more than a segmented bar can label.
/// Search filters both columns; a selection the search hides falls back to
/// the first server with hits without forgetting the user's choice.
struct ToolOverridesBrowser: View {
  @ObservedObject var model: SettingsViewModel
  let groups: [(server: String, items: [ToolPermissionItem])]
  @Binding var searchText: String
  @Binding var selectedServer: String?

  var body: some View {
    let current = groups.first { $0.server == selectedServer } ?? groups.first
    VStack(alignment: .leading, spacing: CSSpace.control) {
      ToolSearchField(text: $searchText, serverCount: groups.count)

      if let current {
        HStack(alignment: .top, spacing: CSSpace.md) {
          ScrollView {
            VStack(alignment: .leading, spacing: 2) {
              ForEach(groups, id: \.server) { group in
                ToolServerTab(
                  server: group.server,
                  count: group.items.count,
                  isSelected: group.server == current.server,
                  action: { select(group.server) }
                )
              }
            }
          }
          .frame(width: 190)
          .frame(maxHeight: 360)

          ScrollView {
            LazyVStack(alignment: .leading, spacing: CSSpace.sm) {
              ForEach(current.items) { item in
                ToolCapabilityRow(item: item, level: $model[toolLevel: item.identity])
              }
            }
          }
          .frame(maxHeight: 360)
        }
      } else {
        ContentUnavailableView.search(text: searchText)
          .frame(maxWidth: .infinity, minHeight: 160)
      }
    }
  }

  private func select(_ server: String) {
    selectedServer = server
  }
}
