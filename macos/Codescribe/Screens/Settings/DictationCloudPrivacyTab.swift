import SwiftUI

/// Dictation › Cloud & privacy (C2 — copy pinned by CloudPrivacyCopyTests; the
/// ASR picker on the Engine tab is the clickable grant. Cloud never displays
/// without a granted consent record.)
struct DictationCloudPrivacyTab: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      ForEach(CloudPrivacyCopy.lines, id: \.self) { line in
        Text(line)
          .font(CSFont.ui(11.5))
          .foregroundStyle(CSColor.textMutedAlt)
          .frame(maxWidth: .infinity, alignment: .leading)
          .fixedSize(horizontal: false, vertical: true)
      }
      Text(
        model.cloudConsentGranted
          ? "Cloud consent is granted. Audio may leave this Mac only in Cloud mode."
          : "Cloud consent is not granted. The picker stays on Apple only."
      )
      .font(CSFont.ui(11.5, .medium))
      .foregroundStyle(model.cloudConsentGranted ? CSColor.oliveLight : CSColor.amber)
      Text("Endpoints and keys live on Providers › Speech-to-text Cloud Service.")
        .font(CSFont.ui(11.5))
        .foregroundStyle(CSColor.textMutedAlt)
    }
    .csSettingsCard()
  }
}
