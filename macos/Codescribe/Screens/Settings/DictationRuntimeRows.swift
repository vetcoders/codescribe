import SwiftUI

/// Read-only rows about the last transcription (LLM truth lives on Agent › LLM
/// lanes). No readiness dot: the engine row reports what served the last take,
/// not whether the configuration looks healthy. The selected mode stays the row
/// above this card — these rows are what actually served, which is a different
/// fact and may disagree with the selection.
///
/// One compact card of two rows in the ordinary case, three in Local power
/// (Founder brief, round 13, 2026-10-10): key on the leading edge, value bold
/// on the trailing edge. Local rows rather than the shared `RuntimeRow`,
/// which styles its key in monospace — these keys are ordinary interface
/// words, and monospace is reserved here for the one technical value, the
/// model reference.
struct DictationRuntimeRows: View {
  @ObservedObject var model: SettingsViewModel

  var body: some View {
    VStack(spacing: 0) {
      LastTranscriptionRow(
        key: String(
          localized: "Engine",
          comment:
            "Engine tab row: the engine that served the last transcription of this app session"
        ),
        value: model.activeSTT)
      if let row = model.sttModelRow {
        divider
        LastTranscriptionRow(key: row.label, value: row.value, mono: true)
      }
      divider
      LastTranscriptionRow(
        key: String(
          localized: "Language",
          comment:
            "Engine tab row: the language setting handed to Apple, local Whisper and the cloud engine"
        ),
        value: model.whisperLanguageDisplay)
    }
    .settingsGroupedInset(padding: 0)
  }

  private var divider: some View {
    Rectangle().fill(Color.primary.opacity(0.12)).frame(height: 1)
  }
}

/// One row of the Last transcription card.
private struct LastTranscriptionRow: View {
  let key: String
  let value: String
  /// Only a model reference asks for monospace; it is also the only value long
  /// enough to need truncation rather than wrapping.
  var mono = false

  var body: some View {
    HStack(alignment: .firstTextBaseline, spacing: CSSpace.md) {
      Text(key)
        .font(CSFont.ui(12))
        .foregroundStyle(Color.secondary)
      Spacer(minLength: CSSpace.sm)
      valueText
        .foregroundStyle(Color.primary)
        .multilineTextAlignment(.trailing)
        .textSelection(.enabled)
    }
    .padding(.horizontal, CSSpace.card)
    .padding(.vertical, 11)
    .accessibilityElement(children: .combine)
  }

  @ViewBuilder
  private var valueText: some View {
    if mono {
      Text(verbatim: value)
        .font(CSFont.mono(11.5, .semibold))
        .lineLimit(1)
        .truncationMode(.middle)
    } else {
      // "No transcription in this app session" is a legitimate value and runs
      // long in Polish: it wraps rather than truncating.
      Text(verbatim: value)
        .font(CSFont.ui(12.5, .semibold))
        .fixedSize(horizontal: false, vertical: true)
    }
  }
}
