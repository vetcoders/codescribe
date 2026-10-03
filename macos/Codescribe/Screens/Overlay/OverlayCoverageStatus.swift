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
      let reason: String
      switch coverage.unavailableReason {
      case .notObserved: reason = "no acoustic measurement was taken"
      case .identityMismatch: reason = "the measurement did not match this take"
      case .invalidMeasurement: reason = "the measurement could not be used"
      case .partialObservation: reason = "the measurement covered only part of this take"
      case .unknown, nil: reason = "the measurement was unavailable"
      }
      return OverlayWarningCopy(
        owner: .coverage,
        chip: "Speech coverage not measured",
        sentence: "Speech coverage was not measured because \(reason).")
    case .complete:
      return OverlayWarningCopy(
        owner: .coverage,
        chip: "Take not sealed yet",
        sentence:
          "Speech coverage was measured as complete, but this take has no terminal seal."
      )
    case .unknown:
      return unverified()
    }
  }

  private static func unverified() -> OverlayWarningCopy {
    OverlayWarningCopy(
      owner: .coverage,
      chip: "No coverage measurement for this take",
      sentence: "No coverage measurement was recorded for this take.")
  }

  /// Convert only with the measured capture clock; never assume a device rate.
  private static func incomplete(
    _ coverage: CsProjectedSealCoverageReceipt, sampleRateHz: UInt32?
  ) -> OverlayWarningCopy {
    let ranges = coverage.uncoveredSpeechRanges.sorted { $0.sampleStart < $1.sampleStart }
    guard !ranges.isEmpty, ranges.allSatisfy({ $0.sampleEnd > $0.sampleStart }) else {
      return OverlayWarningCopy(
        owner: .coverage,
        chip: "Text verification incomplete",
        sentence:
          "Text verification is incomplete; this receipt provides no usable uncovered speech interval. You can review and recover the available text."
      )
    }
    guard let sampleRateHz, sampleRateHz > 0 else {
      return OverlayWarningCopy(
        owner: .coverage,
        chip: "Speech coverage incomplete",
        sentence:
          "Some measured speech is not covered by committed text; its timing is unavailable for this take. You can review and recover the available text."
      )
    }
    let rate = UInt64(sampleRateHz)
    let seconds = ranges.reduce(0.0) { $0 + Double($1.sampleEnd - $1.sampleStart) / Double(rate) }
    let duration =
      seconds < 0.1
      ? "under 0.1"
      : String(
        format: "%.1f", locale: Locale(identifier: "en_US_POSIX"), seconds)
    let positions = ranges.map { timestamp($0.sampleStart / rate) }.joined(separator: ", ")
    let intervals = ranges.map { range in
      let end = range.sampleEnd / rate + (range.sampleEnd % rate == 0 ? 0 : 1)
      return "\(timestamp(range.sampleStart / rate))–\(timestamp(end))"
    }.joined(separator: ", ")
    return OverlayWarningCopy(
      owner: .coverage,
      chip: "\(duration) s of speech not covered · \(positions)",
      sentence:
        "Committed text does not cover measured speech at \(intervals); you can review and recover the available text."
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
    HStack(spacing: 8) {
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
        }
        .frame(width: 250)
      }
      if canRetranscribe {
        HStack(spacing: 6) {
          Text("Transcribe again")
          Button("Local") { onRetranscribe(.fullHq) }
          if cloudConfigured {
            Button("Cloud") { onRetranscribe(.cloud) }
          }
        }
        .buttonStyle(.borderless)
        .controlSize(.small)
        .font(CSFont.ui(11, .medium))
        .accessibilityIdentifier("overlay-retranscribe-offer")
      }
    }
  }
}
