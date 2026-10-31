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
  /// Rows removed since the last save, newest last, with where they sat.
  /// Undo puts the newest back; Save or Discard empties the stack.
  @State private var removed: [RemovedRoot] = []
  @State private var loaded = false
  @FocusState private var focusedRoot: Int?

  private var isDirty: Bool {
    cleaned(rows) != cleaned(model.agentWorkspaceRoots)
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionLabel(String(localized: "Allowed folders"))

      // One line: what the folders are for. What they do NOT cover — the
      // separate permissions of processes and MCP servers — is the tab's own
      // blurb and is not repeated here (Founder review, 2026-10-18).
      Text("The Agent looks for projects here, skipping hidden and build folders.")
        .font(SettingsScale.helper)
        .foregroundStyle(CSColor.secondaryInk)
        .padding(.top, SettingsScale.headerGap)

      VStack(spacing: SettingsScale.rowGap) {
        ForEach(rows.indices, id: \.self) { index in
          rootRow(index: index)
        }
      }
      .padding(.top, SettingsScale.rowGap)

      // One button family: the same chip as every other flat action in
      // Settings, not four bare labels.
      HStack(spacing: CSSpace.sm) {
        SettingsChipButton(action: pickFolder) {
          Label("Add folder…", systemImage: "plus")
            .font(SettingsScale.controlLabel)
            .foregroundStyle(Color.primary)
        }
        .help("Choose a folder to add to the list")

        if let last = removed.last {
          SettingsChipButton(action: undoRemove) {
            Label("Undo remove", systemImage: "arrow.uturn.backward")
              .font(SettingsScale.controlLabel)
              .foregroundStyle(Color.primary)
          }
          .help("Put \(last.path) back")
          .accessibilityIdentifier("settings-workspace-undo-remove")
        }

        Spacer()

        if isDirty {
          SettingsChipButton(action: syncFromModel) {
            Text("Discard changes")
              .font(SettingsScale.controlLabel)
              .foregroundStyle(CSColor.secondaryInk)
          }
          .help("Go back to the saved list")
        }

        SettingsChipButton(enabled: isDirty) {
          model.setAgentWorkspaceRoots(rows)
          syncFromModel()
        } label: {
          Text("Save changes")
            .font(SettingsScale.controlLabel)
            .foregroundStyle(isDirty ? Color.primary : CSColor.secondaryInk)
        }
      }
      .padding(.top, SettingsScale.rowGap)
    }
    .onAppear {
      guard !loaded else { return }
      loaded = true
      syncFromModel()
    }
  }

  private func rootRow(index: Int) -> some View {
    HStack(spacing: CSSpace.sm) {
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
        let path = rows.remove(at: index)
        if !path.trimmingCharacters(in: .whitespaces).isEmpty {
          removed.append(RemovedRoot(index: index, path: path))
        }
      } label: {
        CSIconView(icon: .remove, size: 13, weight: .semibold, color: CSColor.secondaryInk)
      }
      .csFocusRing()
      .help("Remove folder")
      .accessibilityLabel("Remove folder")
    }
    .padding(.horizontal, SettingsScale.rowInsetH)
    .padding(.vertical, SettingsScale.rowInsetV)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
        .fill(Color.primary.opacity(CSAlpha.fill))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
        .strokeBorder(Color.primary.opacity(CSAlpha.stroke), lineWidth: 1)
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
      .fill(valid ? CSColor.oliveLight : CSColor.amberLight)
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

  /// Reinserts the newest removed row at its old position (clamped: the list
  /// may have shrunk since). Only unsaved removals are undoable; a saved list
  /// is restored by adding the folder again.
  private func undoRemove() {
    guard let last = removed.popLast() else { return }
    rows.insert(last.path, at: min(last.index, rows.count))
  }

  private func syncFromModel() {
    removed = []
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

  struct RemovedRoot: Equatable {
    let index: Int
    let path: String
  }

  private static func directoryExists(_ path: String) -> Bool {
    guard !path.isEmpty else { return false }
    let expanded = (path as NSString).expandingTildeInPath
    var isDir: ObjCBool = false
    let exists = FileManager.default.fileExists(atPath: expanded, isDirectory: &isDir)
    return exists && isDir.boolValue
  }
}
