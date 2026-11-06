import AppKit
import SwiftUI

struct RecordingMediaSection: View {
  let media: RecordingMediaDucker

  var body: some View {
    VStack(alignment: .leading, spacing: SettingsScale.headerGap) {
      Text("Spotify is lowered by 70 percent for the whole recording and restored afterwards.")
        .font(SettingsScale.settingName)
      Text(volumeDetail)
        .font(SettingsScale.helper)
        .foregroundStyle(.secondary)
      if media.volumeState == .permissionRequired {
        HStack {
          Button("Open Automation settings") {
            if let url = URL(
              string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation")
            {
              NSWorkspace.shared.open(url)
            }
          }
          Button("Try again") { media.retry() }
        }
      } else if media.volumeState == .unavailable {
        Button("Try again") { media.retry() }
      }
      Divider()
      Text("Audio output in use")
        .font(SettingsScale.settingName)
      if !media.playbackObserved {
        Text("Audio playback status is unavailable.")
          .font(SettingsScale.helper)
          .foregroundStyle(.secondary)
      } else if media.activeOutputs.isEmpty {
        Text("No other application has an active audio output.")
          .font(SettingsScale.helper)
          .foregroundStyle(.secondary)
      } else {
        Text(verbatim: Array(Set(media.activeOutputs.map(\.name))).sorted().joined(separator: ", "))
          .font(SettingsScale.helper)
          .foregroundStyle(.secondary)
      }
    }
    .settingsGroupedInset()
    .accessibilityIdentifier("recording-media-playback")
  }

  private var volumeDetail: LocalizedStringResource {
    switch media.volumeState {
    case .unchanged:
      "Spotify keeps playing. Its volume changes only while recording."
    case .lowered:
      "Spotify is quieter while the microphone is recording."
    case .manualOverride:
      "Your manual Spotify volume change takes priority."
    case .permissionRequired:
      "Allow Codescribe to control Spotify in Privacy & Security > Automation."
    case .unavailable:
      "Spotify volume could not be adjusted. Recording continues."
    }
  }
}
