import SwiftUI

// Dictation panel: everything that shapes how speech becomes text, one tab per
// concern. The Engine tab shows the live STT truth (read-only rows sourced from
// the CsSettings snapshot, not hardcoded) and the engine controls; the other
// tabs used to be collapsibles stacked under it. Editable owners: STT/layered
// engine controls, preview timing, and the hands-free silence window — all
// persisted through the promoted-key config router.

struct EnginePanel: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    SettingsTabbedPane(model: model, section: .engine) {
      switch model.currentTab {
      case .dictationWhisper:
        DictationWhisperModelTab(model: model)
      case .dictationPreview:
        DictationPreviewTimingTab(model: model)
      case .dictationHandsFree:
        DictationHandsFreeTab(model: model)
      case .dictationPrivacy:
        DictationCloudPrivacyTab(model: model)
      case .dictationPermissions:
        DictationPermissionsTab(model: model)
      default:
        // `.dictationEngine`; other sections' tabs cannot reach this panel.
        DictationEngineTab(model: model)
      }
    }
  }
}

#if DEBUG
  #Preview("Dictation panel") {
    EnginePanel(model: .preview(.engine))
      .frame(width: 720, height: 620)
      .background(CSColor.windowWash)
      .preferredColorScheme(.dark)
  }
#endif
