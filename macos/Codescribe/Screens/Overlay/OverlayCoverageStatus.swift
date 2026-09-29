import SwiftUI

/// One overlay warning: a short chip, one sentence that says what happened and
/// why, and whose fact it is. The chip tooltip, its VoiceOver label and the
/// detail popover all read `sentence`, so the three cannot drift apart.
///
/// Owner matters because a refused seal is the engine's verdict about its own
/// coverage. Presenting it as a microphone problem sends the user to fix
/// something that is not broken.
struct OverlayWarningCopy: Equatable, Sendable {
  enum Owner: Equatable, Sendable {
    /// The input is the cause: the user can act at the microphone.
    case microphone
    /// The engine is the cause: nothing the user did produced it.
    case engine
  }

  let owner: Owner
  let chip: String
  let sentence: String

  /// Measured speech arrives below the advisory level (`AudioLevelMeter`).
  static let quietInput = OverlayWarningCopy(
    owner: .microphone,
    chip: "Mic input is quiet",
    sentence:
      "Your speech is reaching the microphone very quietly, so words may be missed; move closer or run mic calibration."
  )

  /// The compact projection's `degraded`: the speech-integrity phase is
  /// stalled, recovering or unresolved.
  static let liveTranscriptBehind = OverlayWarningCopy(
    owner: .engine,
    chip: "Transcriber catching up",
    sentence:
      "The engine heard speech it has not transcribed yet and is running a recovery pass; this is the engine catching up, not your microphone."
  )

  /// A take the engine kept but did not seal, explained from the seal-coverage
  /// receipt the reducer already projects. No acoustic judgement happens here.
  static func sealRefused(_ coverage: CsProjectedSealCoverageReceipt?) -> OverlayWarningCopy {
    let kept = "so it kept this text without sealing the take."
    guard let coverage else { return unverified(kept) }
    switch coverage.status {
    case .incomplete:
      let missed = missedShare(coverage)
      return OverlayWarningCopy(
        owner: .engine,
        chip: "Engine missed \(missed) of your speech — text kept",
        sentence: "The engine found no words for \(missed) of the speech it detected, \(kept)")
    case .unavailable:
      let reason: String
      switch coverage.unavailableReason {
      case .notObserved: reason = "no acoustic measurement was taken"
      case .identityMismatch: reason = "the measurement did not match this take"
      case .invalidMeasurement: reason = "the measurement could not be used"
      case .partialObservation: reason = "the measurement covered only part of this take"
      case .unknown, nil: reason = "the measurement was unavailable"
      }
      return OverlayWarningCopy(
        owner: .engine,
        chip: "Engine could not measure coverage — text kept",
        sentence: "The engine could not check its coverage because \(reason), \(kept)")
    case .complete:
      return OverlayWarningCopy(
        owner: .engine,
        chip: "Engine did not seal this take — text kept",
        sentence:
          "The engine measured full coverage but did not finish sealing this take, so the text is kept unsealed."
      )
    case .unknown:
      return unverified(kept)
    }
  }

  private static func unverified(_ kept: String) -> OverlayWarningCopy {
    OverlayWarningCopy(
      owner: .engine,
      chip: "Engine could not verify coverage — text kept",
      sentence: "The engine recorded no coverage measurement for this take, \(kept)")
  }

  /// Share of detected speech the engine left without words, from the
  /// receipt's own sample counts.
  static func missedShare(_ coverage: CsProjectedSealCoverageReceipt) -> String {
    let speech = coverage.speechSamples
    let uncovered = speech > coverage.coveredSamples ? speech - coverage.coveredSamples : 0
    guard speech > 0, uncovered > 0 else { return "part" }
    let percent = Double(uncovered) * 100 / Double(speech)
    return percent < 1 ? "under 1%" : "\(Int(percent.rounded()))%"
  }
}

/// A projected uncertainty, not a verdict about the words or their delivery.
struct OverlayCoverageStatus: View {
  @Environment(\.openWindow) private var openWindow
  let warning: OverlayWarningCopy
  let palette: OverlayAppearancePalette
  let canRetranscribe: Bool
  let cloudConfigured: Bool
  var diagnosticDetail: String? = nil
  let onRetranscribe: (OverlayRetranscribePass) -> Void
  @State private var presented: String?

  var body: some View {
    OverlayHoverControl(
      id: "overlay-coverage-status", title: warning.sentence, palette: palette,
      presented: $presented
    ) {
      Label(warning.chip, systemImage: warning.owner == .microphone ? "mic" : "info.circle")
        .font(CSFont.ui(11, .medium))
        .lineLimit(1)
        .truncationMode(.tail)
        .foregroundStyle(palette.processingStatus.color)
    } detail: { close in
      VStack(alignment: .leading, spacing: 10) {
        Text(warning.sentence)
          .fixedSize(horizontal: false, vertical: true)
        if warning.owner == .microphone {
          Button("Mic calibration in Settings…") {
            close()
            SettingsDeepLink.shared.present(.audio, anchor: .audioReadiness)
            openWindow(id: SettingsView.windowID)
            NSApp.activate(ignoringOtherApps: true)
          }
          .controlSize(.small)
          .accessibilityIdentifier("overlay-open-mic-calibration-settings")
        }
        if let diagnosticDetail {
          Divider()
          Text(diagnosticDetail).font(CSFont.mono(10, .medium))
        }
        if canRetranscribe {
          Text("Transcribe again")
          HStack {
            Button("Local") {
              close()
              onRetranscribe(.fullHq)
            }
            if cloudConfigured {
              Button("Cloud") {
                close()
                onRetranscribe(.cloud)
              }
            }
          }
          .buttonStyle(.borderless)
          .controlSize(.small)
          .font(CSFont.ui(11, .medium))
        }
      }
      .frame(width: 250)
    }
  }
}
