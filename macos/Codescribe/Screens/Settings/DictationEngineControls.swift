import SwiftUI

/// One product mode selector. The row describes the selected mode only; the
/// other two are one click away. Local power's refinement readiness lives on
/// the Whisper tab, under the model it depends on (Founder review, round 2,
/// 2026-10-20).
struct DictationEngineControls: View {
  @ObservedObject var model: SettingsViewModel

  /// Gates the Cloud grant behind an explicit confirmation (CS160-F013):
  /// selecting Cloud in the menu only opens this dialog, never writes
  /// consent by itself. `SettingsOptionMenu`'s `selectedId` tracks
  /// `model.asrModeId`, so Cancel leaves the menu showing the previous mode
  /// because nothing was ever written.
  @State private var showingCloudConsentConfirmation = false

  private static let asrModeOptions = [
    SettingsMenuOption(id: "apple_only", label: String(localized: "Apple only")),
    SettingsMenuOption(id: "local_power", label: String(localized: "Local power")),
    SettingsMenuOption(id: "cloud", label: String(localized: "Cloud")),
  ]

  var body: some View {
    VStack(spacing: 8) {
      SettingsControlRow(
        title: String(
          localized: "Recognition mode",
          comment: "Engine tab row: the one editable choice, how speech becomes text"),
        subtitle: asrModeSubtitle
      ) {
        SettingsOptionMenu(
          options: Self.asrModeOptions,
          selectedId: model.asrModeId,
          currentLabel: model.asrModeLabel,
          onSelect: handleAsrModeSelection
        )
      }
    }
    .confirmationDialog(
      String(
        localized: "Send dictation audio to the cloud?",
        comment: "Cloud consent confirmation: dialog title"),
      isPresented: $showingCloudConsentConfirmation,
      titleVisibility: .visible
    ) {
      Button(
        String(
          localized: "Switch to Cloud",
          comment: "Cloud consent confirmation: button that grants consent and selects Cloud")
      ) {
        model.confirmCloudConsent()
      }
      Button("Cancel", role: .cancel) {}
    } message: {
      Text(
        String(
          localized:
            "Dictation audio will leave this Mac and be sent to the configured cloud speech provider.",
          comment: "Cloud consent confirmation: body naming the effect"))
    }
  }

  /// Intercepts Cloud: the menu only asks, `confirmCloudConsent()` (called
  /// from the dialog's Confirm button) does the write. Local power and Apple
  /// only go straight to `setAsrMode`, which also revokes a standing Cloud
  /// consent grant.
  private func handleAsrModeSelection(_ id: String) {
    if id.lowercased() == "cloud" {
      showingCloudConsentConfirmation = true
    } else {
      model.setAsrMode(id)
    }
  }

  /// One sentence for the selected mode, where there is one to say. The two
  /// on-device modes describe what runs; Cloud has no caption, because naming
  /// the consent again told nobody anything they could act on (Founder brief,
  /// round 13, 2026-10-10). That is a caption, not a warning: a consent that
  /// is actually missing never reaches this row — `asrModeId` reports Apple
  /// only until it is granted — and the Privacy tab owns the grant itself.
  private var asrModeSubtitle: String? {
    switch model.asrModeId {
    case "local_power":
      return String(
        localized:
          "Live speech recognition and correction in the background, without sending recordings.",
        comment: "ASR mode description: Local power")
    case "cloud":
      return nil
    default:
      return String(
        localized: "Live recognition by Apple.",
        comment: "ASR mode description: Apple only")
    }
  }
}
