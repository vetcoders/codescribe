import SwiftUI

/// Dictation › Cloud & privacy. Three sections: the cloud status (the selected
/// mode and the stored consent record as two separate rows), what can leave
/// this Mac, and the privacy details. Copy is pinned by `CloudPrivacyCopyTests`;
/// the ASR picker on the Engine tab is the clickable grant, and Cloud never
/// displays without a granted consent record.
struct DictationCloudPrivacyTab: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(alignment: .leading, spacing: CSSpace.lg) {
      cloudStatus
      ForEach(CloudPrivacyCopy.blocks) { block in
        VStack(alignment: .leading, spacing: 6) {
          SettingsSectionLabel(block.heading)
          ForEach(block.lines, id: \.self) { line in
            prose(line)
          }
        }
      }
    }
    .settingsGroupedInset()
  }

  /// Mode and consent are two rows on purpose: a stored grant is not evidence
  /// that audio is leaving right now.
  private var cloudStatus: some View {
    VStack(alignment: .leading, spacing: 8) {
      SettingsSectionLabel(CloudPrivacyCopy.statusHeading)
      VStack(spacing: 0) {
        RuntimeRow(
          key: CloudPrivacyCopy.currentModeLabel, value: model.asrModeLabel,
          tint: true, trailing: .none)
        Rectangle().fill(Color.primary.opacity(0.12)).frame(height: 1)
        RuntimeRow(
          key: CloudPrivacyCopy.savedConsentLabel, value: consentValue,
          tint: false,
          trailing: .dot(model.cloudConsentGranted ? CSColor.oliveLight : CSColor.amber))
      }
      .clipShape(.rect(cornerRadius: CSRadius.composer))
      .overlay {
        RoundedRectangle(cornerRadius: CSRadius.composer)
          .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
      }
      prose(CloudPrivacyCopy.consentIsNotLiveEgress)
    }
  }

  private var consentValue: String {
    model.cloudConsentGranted
      ? CloudPrivacyCopy.consentGranted
      : CloudPrivacyCopy.consentNotGranted
  }

  private func prose(_ line: String) -> some View {
    Text(line)
      .font(CSFont.ui(11.5))
      .foregroundStyle(Color.secondary)
      .frame(maxWidth: .infinity, alignment: .leading)
      .fixedSize(horizontal: false, vertical: true)
  }
}
