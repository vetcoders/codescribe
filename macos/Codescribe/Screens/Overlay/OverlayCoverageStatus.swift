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
    chip: String(localized: "Mic input is quiet"),
    sentence: String(
      localized:
        "Your speech is reaching the microphone very quietly, so words may be missed; move closer or run mic calibration."
    )
  )

  /// The compact projection's `degraded`: the speech-integrity phase is
  /// stalled, recovering or unresolved.
  static let liveTranscriptBehind = OverlayWarningCopy(
    owner: .engine,
    chip: String(localized: "Transcriber catching up"),
    sentence: String(
      localized:
        "The engine heard speech it has not transcribed yet and is running a recovery pass; this is the engine catching up, not your microphone."
    )
  )

  /// A take the engine kept but did not seal, explained from the seal-coverage
  /// receipt the reducer already projects. No acoustic judgement happens here.
  static func sealRefused(_ coverage: CsProjectedSealCoverageReceipt?) -> OverlayWarningCopy {
    guard let coverage else { return unverified() }
    switch coverage.status {
    case .incomplete:
      let missed = missedShare(coverage)
      return OverlayWarningCopy(
        owner: .engine,
        chip: String(
          localized: "Engine missed \(missed) of your speech — text kept",
          comment: "The placeholder is a share of speech, e.g. “12%” or “part”"),
        sentence: String(
          localized:
            "The engine found no words for \(missed) of the speech it detected, so it kept this text without sealing the take.",
          comment: "The placeholder is a share of speech, e.g. “12%” or “part”"))
    case .unavailable:
      return OverlayWarningCopy(
        owner: .engine,
        chip: String(localized: "Engine could not measure coverage — text kept"),
        sentence: unavailableSentence(coverage.unavailableReason))
    case .complete:
      return OverlayWarningCopy(
        owner: .engine,
        chip: String(localized: "Engine did not seal this take — text kept"),
        sentence: String(
          localized:
            "The engine measured full coverage but did not finish sealing this take, so the text is kept unsealed."
        )
      )
    case .unknown:
      return unverified()
    }
  }

  /// One whole sentence per reason: the clause and the consequence cannot be
  /// translated apart without fixing English word order.
  private static func unavailableSentence(
    _ reason: CsCoverageUnavailableReason?
  ) -> String {
    switch reason {
    case .notObserved:
      return String(
        localized:
          "The engine could not check its coverage because no acoustic measurement was taken, so it kept this text without sealing the take."
      )
    case .identityMismatch:
      return String(
        localized:
          "The engine could not check its coverage because the measurement did not match this take, so it kept this text without sealing the take."
      )
    case .invalidMeasurement:
      return String(
        localized:
          "The engine could not check its coverage because the measurement could not be used, so it kept this text without sealing the take."
      )
    case .partialObservation:
      return String(
        localized:
          "The engine could not check its coverage because the measurement covered only part of this take, so it kept this text without sealing the take."
      )
    case .unknown, nil:
      return String(
        localized:
          "The engine could not check its coverage because the measurement was unavailable, so it kept this text without sealing the take."
      )
    }
  }

  private static func unverified() -> OverlayWarningCopy {
    OverlayWarningCopy(
      owner: .engine,
      chip: String(localized: "Engine could not verify coverage — text kept"),
      sentence: String(
        localized:
          "The engine recorded no coverage measurement for this take, so it kept this text without sealing the take."
      ))
  }

  /// Share of detected speech the engine left without words, from the
  /// receipt's own sample counts.
  static func missedShare(_ coverage: CsProjectedSealCoverageReceipt) -> String {
    let speech = coverage.speechSamples
    let uncovered = speech > coverage.coveredSamples ? speech - coverage.coveredSamples : 0
    guard speech > 0, uncovered > 0 else {
      return String(localized: "part", comment: "Unquantified share of speech the engine missed")
    }
    let percent = Double(uncovered) * 100 / Double(speech)
    return percent < 1
      ? String(localized: "under 1%", comment: "Share of speech the engine missed")
      : "\(Int(percent.rounded()))%"
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
