import SwiftUI

/// Dictation › Engine: the recognition mode first (the one editable choice),
/// then what the last transcription actually used — read-only rows sourced
/// from the runtime verdict and the settings snapshot, never hardcoded.
struct DictationEngineTab: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionLabel(
        String(localized: "Recognition mode", comment: "Engine tab section: the mode picker"))
      DictationEngineControls(model: model)
        .padding(.top, CSSpace.control)

      SettingsSectionLabel(
        String(
          localized: "Last transcription",
          comment: "Engine tab section: read-only rows about the last transcription")
      )
      .padding(.top, CSSpace.section)
      DictationRuntimeRows(model: model)
        .padding(.top, CSSpace.control)
        .onAppear { model.refreshServingStatus() }
    }
  }
}
