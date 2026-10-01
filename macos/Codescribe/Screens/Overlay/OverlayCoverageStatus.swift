import SwiftUI

/// One overlay warning: a short chip and one sentence describing the detected
/// event and, when measured, its place in the take. The chip tooltip, its
/// VoiceOver label and the detail popover all read `sentence`, so the three
/// cannot drift apart.
///
/// Seal coverage reports an observation, without assigning a cause or claiming
/// delivery. Microphone and live-transcription advisories remain separate.
struct OverlayWarningCopy: Equatable, Sendable {
  enum Owner: Equatable, Sendable {
    /// The input is the cause: the user can act at the microphone.
    case microphone
    /// The engine is the cause: nothing the user did produced it.
    case engine
    /// An acoustic coverage observation; no cause is assigned.
    case coverage
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

  /// A take without a seal, explained from the seal-coverage
  /// receipt the reducer already projects. No acoustic judgement happens here.
  static func sealRefused(
    _ coverage: CsProjectedSealCoverageReceipt?, sampleRateHz: UInt32? = nil
  ) -> OverlayWarningCopy {
    guard let coverage else { return unverified() }
    switch coverage.status {
    case .incomplete:
      return incomplete(coverage, sampleRateHz: sampleRateHz)
    case .unavailable:
      return OverlayWarningCopy(
        owner: .coverage,
        chip: String(localized: "Speech coverage not measured"),
        sentence: unavailableSentence(coverage.unavailableReason))
    case .complete:
      return OverlayWarningCopy(
        owner: .coverage,
        chip: String(localized: "Take not sealed yet"),
        sentence: String(
          localized:
            "Speech coverage was measured as complete, but this take has no terminal seal."
        )
      )
    case .unknown:
      return unverified()
    }
  }

  /// One whole sentence per reason: the reason is a clause, and a clause
  /// cannot be translated apart from its sentence without fixing English
  /// word order.
  private static func unavailableSentence(
    _ reason: CsCoverageUnavailableReason?
  ) -> String {
    switch reason {
    case .notObserved:
      return String(
        localized: "Speech coverage was not measured because no acoustic measurement was taken.")
    case .identityMismatch:
      return String(
        localized:
          "Speech coverage was not measured because the measurement did not match this take.")
    case .invalidMeasurement:
      return String(
        localized: "Speech coverage was not measured because the measurement could not be used.")
    case .partialObservation:
      return String(
        localized:
          "Speech coverage was not measured because the measurement covered only part of this take."
      )
    case .unknown, nil:
      return String(
        localized: "Speech coverage was not measured because the measurement was unavailable.")
    }
  }

  private static func unverified() -> OverlayWarningCopy {
    OverlayWarningCopy(
      owner: .coverage,
      chip: String(localized: "No coverage measurement for this take"),
      sentence: String(localized: "No coverage measurement was recorded for this take."))
  }

  /// Convert only with the measured capture clock; never assume a device rate.
  private static func incomplete(
    _ coverage: CsProjectedSealCoverageReceipt, sampleRateHz: UInt32?
  ) -> OverlayWarningCopy {
    let ranges = coverage.uncoveredSpeechRanges.sorted { $0.sampleStart < $1.sampleStart }
    guard let sampleRateHz, sampleRateHz > 0, !ranges.isEmpty,
      ranges.allSatisfy({ $0.sampleEnd > $0.sampleStart })
    else {
      return OverlayWarningCopy(
        owner: .coverage,
        chip: String(localized: "Speech had no words"),
        sentence: String(
          localized:
            "Codescribe detected speech that no recognizer turned into words; its timing is unavailable for this take."
        )
      )
    }
    let rate = UInt64(sampleRateHz)
    let seconds = ranges.reduce(0.0) { $0 + Double($1.sampleEnd - $1.sampleStart) / Double(rate) }
    let positions = ranges.map { timestamp($0.sampleStart / rate) }.joined(separator: ", ")
    let intervals = ranges.map { range in
      let end = range.sampleEnd / rate + (range.sampleEnd % rate == 0 ? 0 : 1)
      return "\(timestamp(range.sampleStart / rate))–\(timestamp(end))"
    }.joined(separator: ", ")
    return OverlayWarningCopy(
      owner: .coverage,
      chip: durationChip(seconds: seconds, positions: positions),
      sentence: String(
        localized:
          "Codescribe heard speech at \(intervals) that no recognizer turned into words — it may have been cut off, noise or the microphone.",
        comment:
          "The placeholder is a list of time ranges within the take, e.g. “0:12–0:14, 0:58–1:00”")
    )
  }

  /// Two whole phrases: “under 0.1” is wording, not a number, so it cannot be
  /// slotted into the measured phrase. The measured figure is pre-formatted.
  private static func durationChip(seconds: Double, positions: String) -> String {
    guard seconds >= 0.1 else {
      return String(
        localized: "under 0.1 s of speech had no words · \(positions)",
        comment:
          "“s” is seconds. The placeholder is a list of positions in the take, e.g. “0:12, 0:58”")
    }
    let duration = String(format: "%.1f", locale: Locale(identifier: "en_US_POSIX"), seconds)
    return String(
      localized: "\(duration) s of speech had no words · \(positions)",
      comment:
        "“s” is seconds. First placeholder is a pre-formatted duration, e.g. “1.3”; second is a list of positions in the take, e.g. “0:12, 0:58”"
    )
  }

  private static func timestamp(_ seconds: UInt64) -> String {
    let remainder = seconds % 60
    return "\(seconds / 60):\(remainder < 10 ? "0" : "")\(remainder)"
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
