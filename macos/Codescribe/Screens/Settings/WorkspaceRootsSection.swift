import Foundation
import SwiftUI

/// Editable list of the folders the Agent may reach. One setting serves two
/// consumers: the path policy of every file/terminal tool and the
/// `list_projects` scan, so the UI shows one neutral list — some entries are
/// project checkouts, others (data dirs, /tmp) are plain access grants. Rows
/// are edited locally and committed through
/// `SettingsViewModel.setAgentWorkspaceRoots`. Each row shows a live
/// "directory exists" indicator.
struct WorkspaceRootsSection: View {
  @ObservedObject var model: SettingsViewModel

  /// Sample path, not copy: it must read the same in every language.
  private static let rootPlaceholder = "/path/to/checkouts"

  @State private var rows: [String] = []
  @State private var loaded = false
  @FocusState private var focusedRoot: Int?

  private var isDirty: Bool {
    cleaned(rows) != cleaned(model.agentWorkspaceRoots)
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionLabel(String(localized: "Allowed folders"))

      Text(
        "The Agent looks for projects and Git repositories in these folders. It also searches subfolders, but skips hidden folders and build directories."
      )
      .font(CSFont.ui(11.5))
      .lineSpacing(2)
      .foregroundStyle(Color.secondary)
      .padding(.top, 8)

      VStack(spacing: 8) {
        ForEach(rows.indices, id: \.self) { index in
          rootRow(index: index)
        }
      }
      .padding(.top, 12)

      HStack(spacing: 10) {
        Button(action: pickFolder) {
          Label("Add folder…", systemImage: "plus")
            .font(CSFont.ui(12, .semibold))
        }
        .csFocusRing()
        .foregroundStyle(Color.primary)
        .help("Choose a folder to add to the list")

        Spacer()

        Button {
          model.setAgentWorkspaceRoots(rows)
          syncFromModel()
        } label: {
          Text("Save changes")
            .font(CSFont.ui(12, .semibold))
            .foregroundStyle(isDirty ? Color.primary : Color.secondary)
        }
        .csFocusRing()
        .disabled(!isDirty)
      }
      .padding(.top, 12)
    }
    .onAppear {
      guard !loaded else { return }
      loaded = true
      syncFromModel()
    }
  }

  private func rootRow(index: Int) -> some View {
    HStack(spacing: 10) {
      existsDot(for: rows[index])
      TextField(
        Self.rootPlaceholder,
        text: Binding(
          get: { index < rows.count ? rows[index] : "" },
          set: { if index < rows.count { rows[index] = $0 } }
        )
      )
      .textFieldStyle(.plain)
      .focused($focusedRoot, equals: index)
      .font(CSFont.mono(12, .regular))
      .foregroundStyle(Color.primary)
      .frame(maxWidth: .infinity, alignment: .leading)

      Button {
        rows.remove(at: index)
      } label: {
        CSIconView(icon: .remove, size: 13, weight: .semibold, color: Color.secondary)
      }
      .csFocusRing()
      .help("Remove folder")
      .accessibilityLabel("Remove folder")
    }
    .padding(.horizontal, 11)
    .padding(.vertical, 9)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
        .fill(Color.primary.opacity(0.06))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
        .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
    )
    .overlay {
      CSFocusOutline(isFocused: focusedRoot == index, cornerRadius: CSRadius.input)
    }
  }

  /// Green when the (tilde-expanded) path is an existing directory, amber
  /// otherwise — the tool will silently skip a root that does not resolve.
  private func existsDot(for path: String) -> some View {
    let trimmed = path.trimmingCharacters(in: .whitespaces)
    let valid = Self.directoryExists(trimmed)
    return Circle()
      .fill(valid ? CSColor.oliveLight : CSColor.amber)
      .frame(width: 7, height: 7)
  }

  /// The ellipsis on the button promises a dialog: a directory picker whose
  /// choice lands as a new editable row, tilde-abbreviated like the defaults.
  /// Nothing is saved until Save changes.
  private func pickFolder() {
    let panel = NSOpenPanel()
    panel.canChooseDirectories = true
    panel.canChooseFiles = false
    panel.allowsMultipleSelection = false
    panel.canCreateDirectories = false
    panel.prompt = String(localized: "Add folder", comment: "Folder picker confirm button")
    panel.message = String(localized: "Choose a folder the Agent may read and write")
    guard panel.runModal() == .OK, let url = panel.url else { return }
    let path = (url.path as NSString).abbreviatingWithTildeInPath
    if !rows.contains(path) {
      rows.append(path)
    }
  }

  private func syncFromModel() {
    rows = model.agentWorkspaceRoots
    // Mirror of the runtime default (DEFAULT_AGENT_WORKSPACE_ROOT): with no
    // configured roots the tool really scans the app's own data dir.
    if rows.isEmpty { rows = ["~/.codescribe"] }
  }

  private func cleaned(_ input: [String]) -> [String] {
    input
      .map { $0.trimmingCharacters(in: .whitespaces) }
      .filter { !$0.isEmpty }
  }

  private static func directoryExists(_ path: String) -> Bool {
    guard !path.isEmpty else { return false }
    let expanded = (path as NSString).expandingTildeInPath
    var isDir: ObjCBool = false
    let exists = FileManager.default.fileExists(atPath: expanded, isDirectory: &isDir)
    return exists && isDir.boolValue
  }
}
