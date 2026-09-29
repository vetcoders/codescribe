import SwiftUI

/// Dictation › Engine: the live STT truth (read-only rows sourced from the
/// CsSettings snapshot, not hardcoded), then the editable engine controls.
struct DictationEngineTab: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionLabel("Runtime truth · read-only rows")
      DictationRuntimeRows(model: model)
        .padding(.top, CSSpace.control)
        .onAppear { model.refreshServingStatus() }

      SettingsSectionLabel("Engine controls")
        .padding(.top, CSSpace.section)
      DictationEngineControls(model: model)
        .padding(.top, CSSpace.control)
    }
  }
}
