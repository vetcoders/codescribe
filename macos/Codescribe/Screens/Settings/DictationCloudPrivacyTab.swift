import SwiftUI

/// Dictation › Cloud & privacy. Two short informational sections plus the
/// rest on demand: the cloud status (the selected mode and the stored consent
/// record as two separate rows), two scannable egress rows (Audio, Text), and
/// the full privacy details behind a collapsed disclosure — kept, not cut.
/// Copy is pinned by `CloudPrivacyCopyTests`; the ASR picker on the Engine tab
/// is the clickable grant, and Cloud never displays without a granted consent
/// record.
struct DictationCloudPrivacyTab: View {
  @ObservedObject var model: SettingsViewModel
  @State private var showingPrivacyDetails = false

  var body: some View {
    VStack(alignment: .leading, spacing: CSSpace.lg) {
      cloudStatus
      egress
      privacyDetails
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
        rowDivider
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

  /// The center of the screen: one row per egress surface, each with a single
  /// condition line, built to be scanned rather than read.
  private var egress: some View {
    VStack(alignment: .leading, spacing: 8) {
      SettingsSectionLabel(CloudPrivacyCopy.egressHeading)
      VStack(spacing: 0) {
        egressRow(
          icon: .mic,
          title: CloudPrivacyCopy.egressAudioTitle,
          detail: CloudPrivacyCopy.egressAudioDetail)
        rowDivider
        egressRow(
          icon: .agent,
          title: CloudPrivacyCopy.egressTextTitle,
          detail: CloudPrivacyCopy.egressTextDetail)
      }
      .clipShape(.rect(cornerRadius: CSRadius.composer))
      .overlay {
        RoundedRectangle(cornerRadius: CSRadius.composer)
          .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
      }
    }
  }

  private func egressRow(icon: CSIcon, title: String, detail: String) -> some View {
    HStack(alignment: .top, spacing: 12) {
      CSIconView(icon: icon, size: 13, color: CSColor.textTertiary)
        .padding(.top, 2)
      VStack(alignment: .leading, spacing: 3) {
        Text(title)
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(Color.primary)
        Text(detail)
          .font(CSFont.ui(11.5))
          .foregroundStyle(Color.secondary)
          .fixedSize(horizontal: false, vertical: true)
      }
      .frame(maxWidth: .infinity, alignment: .leading)
    }
    .padding(.horizontal, 16)
    .padding(.vertical, 11)
  }

  /// Everything the long version said still lives here, split into short
  /// headed subsections — hidden by default, never removed.
  private var privacyDetails: some View {
    VStack(alignment: .leading, spacing: 6) {
      DisclosureGroup(isExpanded: $showingPrivacyDetails) {
        VStack(alignment: .leading, spacing: CSSpace.md) {
          ForEach(CloudPrivacyCopy.detailBlocks) { block in
            VStack(alignment: .leading, spacing: 4) {
              Text(block.heading)
                .font(CSFont.ui(11.5, .semibold))
                .foregroundStyle(Color.primary)
              ForEach(block.lines, id: \.self) { line in
                prose(line)
              }
            }
          }
        }
        .padding(.top, 8)
      } label: {
        Text(CloudPrivacyCopy.detailsHeading)
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(Color.primary)
      }
      prose(CloudPrivacyCopy.detailsCaption)
      Button(CloudPrivacyCopy.configureCloudServices) {
        // `.keys` is the Providers section (its panel destination is
        // `.providers`); the anchor scrolls to Cloud transcription.
        SettingsDeepLink.shared.present(.keys, anchor: .providersCloudTranscription)
      }
      .buttonStyle(.link)
      .font(CSFont.ui(11.5, .medium))
      .padding(.top, 2)
    }
  }

  private var rowDivider: some View {
    Rectangle().fill(Color.primary.opacity(0.12)).frame(height: 1)
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
