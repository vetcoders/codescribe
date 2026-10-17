import SwiftUI

/// Dictation › Engine: the recognition mode first (the one editable choice),
/// then what the last transcription actually used — read-only rows sourced
/// from the runtime verdict and the settings snapshot, never hardcoded.
///
/// The mode has no section label of its own: the row below carries the one
/// label ("Recognition mode") and the page headline already says the pane is
/// about speech recognition (Founder brief, round 13, 2026-10-10).
struct DictationEngineTab: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      DictationEngineControls(model: model)

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
