import SwiftUI

// Slim evidence-first dictation overlay.
//
// Layout (top → bottom):
//   header   brand · ONE projection phase · compact waveform · timer
//   body     transcript is the product surface (listening / formatted / terminal)
//   footer   ● engine chip · transient actionable notice
//
// Removed on purpose: duplicate RECORDING/modeMeta row, full bottom Finish/Close
// action layer, and decorative body-top waveform competing with words.
//
// Authority: this view only visualizes OverlayState / projection receipts. It
// never invents transcript truth, seals, or a second recorder. Future AoT mode
// attaches to AgentChatStore (same thread owner) via existing sendToAgent — not
// a parallel chat window.
struct DictationOverlayView: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Bindable var state: OverlayState

  // Geometry constants local to this surface. The window is user-resizable;
  // content fills the frame and never goes narrower than `windowMinWidth`.
  // `DictationOverlayWindow.minSize.height` MUST stay ≥ chrome + `bodyMinHeight`
  // or GlassPanel paints past the window rect and squares the corners.
  private let windowMinWidth: CGFloat = 320
  private let bodyMinHeight: CGFloat = 130
  private let transcriptMinHeight: CGFloat = 96
  var body: some View {
    GlassPanel(cornerRadius: CSRadius.window, sitsInForest: true) {
      VStack(alignment: .leading, spacing: 0) {
        header
        hairline(0.06)
        bodySection
        hairline(0.05)
        footer
      }
    }
    .overlay(alignment: .trailing) {
      OverlayIntentRail(
        phase: state.statusText,
        intents: OverlayIntentRail.projectedIntents(for: state),
        onIntent: state.relayIntent
      )
      .padding(.trailing, CSSpace.sm)
    }
    .csFocusPolicy()
    .frame(minWidth: windowMinWidth, maxWidth: .infinity, maxHeight: .infinity)
    // Terminal corner clip (U22): GlassPanel paints its background from the
    // CONTENT column's size, not the window's. Whenever the column outgrows
    // the window frame — a mid-edge-drag beat, a stale persisted size below
    // the chrome+body sum — that background used to spill past the window
    // rect and surface as a SQUARE corner under the rounded glass. Clipping
    // the whole panel to the window-frame rounded rect closes that class of
    // regression regardless of the height arithmetic. The GlassPanel shadow
    // already falls outside the borderless window (never rendered), so this
    // clip costs nothing visually.
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.window, style: .continuous))
    .developerPowerCorner(padding: 10)
    .animation(reduceMotion ? nil : CSMotion.floatIn, value: state.toast)
    .onHover { inside in
      state.setPointerHovering(inside)
    }
    .onAppear {
      FontLoader.register()
    }
  }

  /// 1px separator matching the mock's hairline borders.
  private func hairline(_ alpha: Double) -> some View {
    CSColor.hairline(alpha).frame(height: 1)
  }

  // MARK: Header

  private var header: some View {
    ViewThatFits(in: .horizontal) {
      fullHeader
      narrowHeader
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .padding(.horizontal, 16)
    .padding(.vertical, 10)
    .background(OverlayDragHandle())
  }

  private var fullHeader: some View {
    HStack(spacing: 10) {
      // Brand block stays inert. The trailing intent rail is the overlay's
      // one control surface, including its projected Close command.
      HStack(spacing: 9) {
        ModeDot(color: CSColor.terracotta, size: 9)
          .accessibilityHidden(true)
        Text("codescribe")
          .font(CSFont.ui(15, .bold))
          .tracking(-0.3)
          .foregroundStyle(CSColor.textHigh)
          .allowsHitTesting(false)
      }
      phaseStatus(text: state.statusText)

      if state.mode == .listening || state.mode == .finalizing {
        chromeWaveform(barCount: 18)
      }

      Spacer(minLength: 4)

      sessionTimer
    }
    .fixedSize(horizontal: true, vertical: false)
  }

  /// Essential chrome only at the supported 320 pt window floor. Close, one
  /// projected phase, real level evidence, and time never collapse vertically.
  private var narrowHeader: some View {
    HStack(spacing: 7) {
      ModeDot(color: CSColor.terracotta, size: 9)
        .accessibilityHidden(true)
      phaseStatus(text: state.compactStatusText)
      if state.mode == .listening || state.mode == .finalizing {
        chromeWaveform(barCount: 10)
      }
      Spacer(minLength: 0)
      sessionTimer
    }
    .fixedSize(horizontal: true, vertical: false)
  }

  @ViewBuilder
  private func phaseStatus(text: String) -> some View {
    // One phase pill only — do not also paint RECORDING/tag/meta rows. Swap the
    // whole view type on live vs idle so repeatForever tears down after capture.
    if state.statusRippling {
      StatusPill(text: text, color: state.statusColor, rippling: true)
        .fixedSize(horizontal: true, vertical: false)
        .allowsHitTesting(false)
        .accessibilityLabel(state.statusText)
        .accessibilityIdentifier("overlay-phase-status")
    } else {
      StaticStatusPill(text: text, color: state.statusColor)
        .fixedSize(horizontal: true, vertical: false)
        .allowsHitTesting(false)
        .accessibilityLabel(state.statusText)
        .accessibilityIdentifier("overlay-phase-status")
    }
  }

  /// Audio-evidence strip in the primary bar. Amplitude/VAD only — word/PCM
  /// synchronized scrolling needs authenticated sample spans from projection
  /// receipts and is intentionally not invented here.
  private func chromeWaveform(barCount: Int) -> some View {
    WaveformView(
      barCount: barCount,
      active: state.mode == .listening && (state.audioReady || state.vadActive),
      transcribing: state.mode == .finalizing,
      indicatorMode: state.indicatorMode,
      meter: state.levelMeter,
      compact: true
    )
    .accessibilityIdentifier("overlay-chrome-waveform")
    .accessibilityLabel("Live audio level")
    .accessibilityValue(state.audioLevelAccessibilityValue)
    .allowsHitTesting(false)
  }

  /// Live `00:00` session counter — absolute reference for audio sync and lag.
  /// Lives in the primary chrome (not a second status row). Capture end freezes
  /// the stamp so the displayed value is the session's true length.
  @ViewBuilder
  private var sessionTimer: some View {
    if state.showsSessionTimer {
      TimelineView(.periodic(from: .now, by: 1)) { _ in
        Text(state.sessionTimerText)
          .csMono(11, .semibold)
          .foregroundStyle(CSColor.textFaint)
          .monospacedDigit()
      }
      .accessibilityIdentifier("overlay-session-timer")
      .accessibilityLabel("Recording time")
      .accessibilityValue(state.sessionTimerText)
    }
  }

  // MARK: Body

  private var bodySection: some View {
    Group {
      if let status = state.presentationStatus {
        presentationStatusBody(status)
          .transition(reduceMotion ? .identity : .opacity.combined(with: .offset(y: 8)))
      } else {
        switch state.mode {
        case .listening, .finalizing:
          listeningBody
            .transition(reduceMotion ? .identity : .opacity.combined(with: .offset(y: 8)))
        case .formatted:
          formattedBody
        case .noSpeech:
          noSpeechBody
            .transition(reduceMotion ? .identity : .opacity.combined(with: .offset(y: 8)))
        case .error:
          errorBody
            .transition(reduceMotion ? .identity : .opacity.combined(with: .offset(y: 8)))
        }
      }
    }
    .frame(
      maxWidth: .infinity, minHeight: bodyMinHeight, maxHeight: .infinity, alignment: .topLeading
    )
    .padding(.horizontal, 20)
    .padding(.top, 4)
    .padding(.bottom, 10)
    // Transcript content must never paint into the footer during live resize.
    .clipped()
    .animation(reduceMotion ? nil : CSMotion.floatIn, value: state.mode)
  }

  private var listeningBody: some View {
    // Transcript is the product. Audio evidence lives in the primary chrome
    // waveform; do not restack a decorative strip above the words.
    transcriptScroll
  }

  /// Native live transcript: follows the newest words until the user clicks or
  /// selects an older phrase. The `NSTextView` keeps that selection stable across
  /// ongoing stream updates, so drag selection, Cmd-C and context-menu Copy work
  /// during recording without stopping capture. A `minHeight` reserves ~2–3 lines
  /// at the window floor.
  private var transcriptScroll: some View {
    VStack(alignment: .leading, spacing: 0) {
      LiveTranscriptTextView(text: state.listeningDisplay)
        .overlay(alignment: .bottomTrailing) {
          BlinkingCaret()
            .padding(.trailing, 3)
            .allowsHitTesting(false)
        }
        .frame(minHeight: transcriptMinHeight)
        .accessibilityIdentifier("overlay-transcript-area")
    }
    .frame(maxWidth: .infinity, alignment: .leading)
  }

  private var formattedBody: some View {
    ScrollView {
      Text(state.formattedText)
        .csFont(19, .medium)
        .foregroundStyle(CSColor.textHigh)
        .lineSpacing(6)
        .textSelection(.enabled)
        .frame(maxWidth: .infinity, alignment: .topLeading)
    }
    .frame(maxWidth: .infinity, minHeight: bodyMinHeight, alignment: .topLeading)
    .accessibilityLabel("Final transcript")
    .accessibilityValue(state.formattedText)
    .accessibilityIdentifier("overlay-transcript-formatted")
  }

  /// Terminal outcome for a session that captured no usable speech. Replaces
  /// the empty editable FINAL with a calm, non-alarming notice (mic glyph +
  /// message). No Copy/Insert/Send — there is nothing to act on; the intent
  /// rail follows the projection table and keeps Retranscribe/Close only.
  private var noSpeechBody: some View {
    HStack(spacing: 12) {
      CSIconView(icon: .mic, size: 18, weight: .regular)
        .foregroundStyle(CSColor.textFaint)
      VStack(alignment: .leading, spacing: 2) {
        Text(state.noSpeechNotice)
          .csFont(15, .medium)
          .foregroundStyle(CSColor.textBody)
          .fixedSize(horizontal: false, vertical: true)
        Text("Nothing was captured this session.")
          .csMono(11, .medium)
          .foregroundStyle(CSColor.textFaint)
      }
      Spacer(minLength: 0)
    }
    .frame(maxWidth: .infinity, minHeight: bodyMinHeight, alignment: .leading)
  }

  /// Terminal outcome for a recording/transcription failure. Unlike a toast, this
  /// persists after the session aborts so the overlay does not falsely report
  /// "no speech" when the engine actually failed.
  private var errorBody: some View {
    VStack(alignment: .leading, spacing: 12) {
      HStack(spacing: 12) {
        CSIconView(icon: .error, size: 18, weight: .regular)
          .foregroundStyle(CSColor.terracotta)
        VStack(alignment: .leading, spacing: 2) {
          Text(state.errorMessage ?? "Transcription failed")
            .csFont(15, .medium)
            .foregroundStyle(CSColor.textBody)
            .fixedSize(horizontal: false, vertical: true)
          Text(state.errorLifecycleDetail)
            .csMono(11, .medium)
            .foregroundStyle(CSColor.textFaint)
        }
        Spacer(minLength: 0)
      }
    }
    .frame(maxWidth: .infinity, minHeight: bodyMinHeight, alignment: .leading)
  }

  /// Rust supplies every word and classification. The canvas only paints the
  /// status and intentionally exposes no repair button or Settings command.
  private func presentationStatusBody(_ status: OverlayPresentationStatus) -> some View {
    HStack(spacing: 12) {
      CSIconView(icon: status.isError ? .error : .success, size: 18, weight: .regular)
        .foregroundStyle(status.isError ? CSColor.terracotta : CSColor.oliveLight)
      VStack(alignment: .leading, spacing: 4) {
        Text(status.headline)
          .csFont(15, .medium)
          .foregroundStyle(CSColor.textBody)
          .fixedSize(horizontal: false, vertical: true)
        Text(status.message)
          .csMono(11, .medium)
          .foregroundStyle(CSColor.textFaint)
          .fixedSize(horizontal: false, vertical: true)
      }
      Spacer(minLength: 0)
    }
    .frame(maxWidth: .infinity, minHeight: bodyMinHeight, alignment: .leading)
    .accessibilityElement(children: .combine)
    .accessibilityIdentifier("overlay-presentation-status")
  }

  // MARK: Footer

  private var footer: some View {
    HStack(spacing: 8) {
      HStack(spacing: 6) {
        Text("●").foregroundStyle(footerEngineDot)
        // Product truth: never hardcode "local whisper". Chip = last serving
        // engine when known, else preference (Apple live default).
        Text(state.footerEngineLabel).foregroundStyle(CSColor.textFaintAlt)
        if let toast = state.toast, !toast.isEmpty {
          Text("·").foregroundStyle(CSColor.textFaintAlt)
          Text(toast)
            .foregroundStyle(CSColor.textFaintAlt)
            .lineLimit(1)
            .truncationMode(.tail)
            .accessibilityIdentifier("overlay-footer-notice")
        }
      }
      Spacer(minLength: 0)
    }
    .csMono(10, .medium)
    .padding(.horizontal, 16)
    .padding(.vertical, 7)
    .background(OverlayDragHandle())
  }

  private var footerEngineDot: Color {
    let label = state.footerEngineLabel.lowercased()
    if label.contains("apple") { return CSColor.oliveLight }
    if label.contains("whisper") { return CSColor.olive }
    return CSColor.amber
  }
}

/// Word-reveal caret: 8×18 terracotta block, softpulsing on a 1s cycle (mock).
private struct BlinkingCaret: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion

  var body: some View {
    if reduceMotion {
      caret.opacity(1)
    } else {
      AnimatedOverlayCaret()
    }
  }

  private var caret: some View {
    RoundedRectangle(cornerRadius: 1, style: .continuous)
      .fill(CSColor.terracotta)
      .frame(width: 7, height: 15)
      .padding(.bottom, 3)
  }
}

private struct AnimatedOverlayCaret: View {
  @State private var on = false

  var body: some View {
    RoundedRectangle(cornerRadius: 1, style: .continuous)
      .fill(CSColor.terracotta)
      .frame(width: 7, height: 15)
      .padding(.bottom, 3)
      .opacity(on ? 1 : 0.7)
      .onAppear {
        withAnimation(.easeInOut(duration: 1).repeatForever(autoreverses: true)) {
          on = true
        }
      }
  }
}

#if DEBUG
  @ViewBuilder
  private func overlayPreviewCanvas<Content: View>(
    width: CGFloat? = nil,
    height: CGFloat? = nil,
    @ViewBuilder content: () -> Content
  ) -> some View {
    content()
      .frame(width: width, height: height)
      .padding(CSSpace.previewInset)
      .background(CSColor.windowWash)
      .preferredColorScheme(.dark)
  }

  #Preview("Listening") {
    overlayPreviewCanvas {
      DictationOverlayView(state: .previewListening())
    }
  }

  #Preview("Transcribing") {
    // Pinned to the window's min content size so this preview doubles as the
    // min-size regression check: "transcribing…" fills the main status slot and
    // the transcript reserves ~2–3 lines instead of collapsing at the floor.
    overlayPreviewCanvas(width: 320, height: 260) {
      DictationOverlayView(state: .previewTranscribing())
    }
  }

  #Preview("No speech") {
    // Session ended without usable text: dedicated notice body, no
    // Copy/Format/Send, only Close. Pinned to the min content size so it also
    // guards the floor layout for this outcome.
    overlayPreviewCanvas(width: 320, height: 260) {
      DictationOverlayView(state: .previewNoSpeech())
    }
  }

  #Preview("Formatted") {
    overlayPreviewCanvas {
      DictationOverlayView(state: .previewFormatted())
    }
  }

  #Preview("Formatted · compact chrome") {
    overlayPreviewCanvas(width: 340, height: 260) {
      DictationOverlayView(state: .previewFormatted())
    }
  }

  #Preview("Listening · scaled 1.4x") {
    // Exercises `\.csTextScale`: transcript + status render 40% larger while the
    // window chrome and paddings keep their intrinsic geometry (transcript scrolls
    // rather than forcing the panel taller).
    overlayPreviewCanvas(width: 470, height: 280) {
      DictationOverlayView(state: .previewListening())
        .environment(\.csTextScale, 1.4)
    }
  }
#endif
