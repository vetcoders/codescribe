import SwiftUI

/// Dictation › Permissions: the live macOS permission matrix.
struct DictationPermissionsTab: View {
  @ObservedObject var model: SettingsViewModel

  private static let matrixOrder: [PermissionKind] = [
    .microphone, .accessibility, .inputMonitoring, .screenRecording,
    .speechRecognition,
  ]
  private static let columns = [
    GridItem(.flexible(), spacing: 8),
    GridItem(.flexible(), spacing: 8),
  ]

  var body: some View {
    LazyVGrid(columns: Self.columns, spacing: 8) {
      ForEach(Self.matrixOrder) { kind in
        PermissionMatrixCell(
          kind: kind,
          state: model.permissions.state(kind),
          onStateChanged: { model.refresh() }
        )
      }
    }
  }
}
