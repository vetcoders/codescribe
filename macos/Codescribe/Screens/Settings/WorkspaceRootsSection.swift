import Foundation
import SwiftUI

/// Editable list of workspace roots the agent's `list_projects` tool scans to
/// resolve project names to absolute paths. Rows are edited locally and committed
/// through `SettingsViewModel.setAgentWorkspaceRoots` (colon-joined ->
/// `AGENT_WORKSPACE_ROOTS`). Each row shows a live "directory exists" indicator.
struct WorkspaceRootsSection: View {
  @ObservedObject var model: SettingsViewModel

  @State private var rows: [String] = []
  @State private var loaded = false

  private var isDirty: Bool {
    cleaned(rows) != cleaned(model.agentWorkspaceRoots)
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionLabel("Agent workspace roots")

      Text(
        "Directories the assistant scans for git checkouts to resolve a project name to a path (list_projects). Recursive, a few levels deep; build and hidden folders are skipped."
      )
      .font(CSFont.ui(11.5))
      .lineSpacing(2)
      .foregroundStyle(CSColor.textMutedAlt)
      .padding(.top, 8)

      VStack(spacing: 8) {
        ForEach(rows.indices, id: \.self) { index in
          rootRow(index: index)
        }
      }
      .padding(.top, 12)

      HStack(spacing: 10) {
        Button {
          rows.append("")
        } label: {
          Label("Add root", systemImage: "plus")
            .font(CSFont.ui(12, .semibold))
        }
        .csFocusRing()
        .foregroundStyle(CSColor.textBody)

        Spacer()

        Button {
          model.setAgentWorkspaceRoots(rows)
          syncFromModel()
        } label: {
          Text("Save roots")
            .font(CSFont.ui(12, .semibold))
            .foregroundStyle(isDirty ? CSColor.textHigh : CSColor.textFaint)
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
        "/path/to/checkouts",
        text: Binding(
          get: { index < rows.count ? rows[index] : "" },
          set: { if index < rows.count { rows[index] = $0 } }
        )
      )
      .textFieldStyle(.plain)
      .font(CSFont.mono(12, .regular))
      .foregroundStyle(CSColor.textBody)
      .frame(maxWidth: .infinity, alignment: .leading)

      Button {
        rows.remove(at: index)
      } label: {
        CSIconView(icon: .remove, size: 13, weight: .semibold, color: CSColor.textFaint)
      }
      .csFocusRing()
    }
    .padding(.horizontal, 11)
    .padding(.vertical, 9)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
        .fill(CSColor.surfaceRaised(0.03))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
        .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
    )
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
