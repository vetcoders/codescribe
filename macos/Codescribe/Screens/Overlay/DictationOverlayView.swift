import SwiftUI

// Slim evidence-first dictation overlay.
//
// Layout (top → bottom):
//   header   brand · ONE status pill · compact waveform · timer · Auto Paste ·
//            placement · split primary (Finish/Insert + chevron menu)
//   body     transcript is the product surface (listening / formatted / terminal)
//   footer   ● engine chip · optional canvas honesty · toast
//
// Removed on purpose: duplicate RECORDING/modeMeta row, full bottom Finish/Close
// action layer, and decorative body-top waveform competing with words.
//
// Authority: this view only visualizes OverlayState / projection receipts. It
// never invents transcript truth, seals, or a second recorder. Future AoT mode
// attaches to AgentChatStore (same thread owner) via existing sendToAgent — not
// a parallel chat window.
struct DictationOverlayView: View {
  @ObservedObject var state: OverlayState

  // Geometry constants local to this surface. The window is user-resizable;
  // content fills the frame and never goes narrower than `windowMinWidth`.
  // `DictationOverlayWindow.minSize.height` MUST stay ≥ chrome + `bodyMinHeight`
  // or GlassPanel paints past the window rect and squares the corners.
  private let windowMinWidth: CGFloat = 320
  private let bodyMinHeight: CGFloat = 130
  private let transcriptMinHeight: CGFloat = 96
  private let buttonRadius: CGFloat = 10
  private let primaryActionHeight: CGFloat = 28

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
    .csFocusPolicy()
    .background(
      OverlayKeyGate(
        editing: state.isEditingTranscript,
        onResign: { state.endTranscriptEdit() }
      )
      .frame(width: 0, height: 0)
      .allowsHitTesting(false)
    )
    .onExitCommand { state.endTranscriptEdit() }
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
    .animation(CSMotion.floatIn, value: state.toast)
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
    HStack(spacing: 10) {
      // Brand block with a LIVE dot: the orange dot sits in the window's
      // traffic-light zone and reads as a control, so it IS one — click
      // closes the overlay (same as the Close action). Hover shows the
      // familiar "×" glyph; the wordmark text stays inert.
      HStack(spacing: 9) {
        CloseDot { state.close() }
        Text("codescribe")
          .font(CSFont.ui(15, .bold))
          .tracking(-0.3)
          .foregroundStyle(CSColor.textHigh)
          .allowsHitTesting(false)
      }
      // One phase pill only — do not also paint RECORDING/tag/meta rows.
      // Swap the whole VIEW TYPE on live vs idle so the rippling animation
      // tears down instead of ticking after capture ends.
      if state.statusRippling {
        StatusPill(
          text: state.statusText,
          color: state.statusColor,
          rippling: true
        )
        .allowsHitTesting(false)
        .accessibilityIdentifier("overlay-phase-status")
      } else {
        StaticStatusPill(text: state.statusText, color: state.statusColor)
          .allowsHitTesting(false)
          .accessibilityIdentifier("overlay-phase-status")
      }

      if state.mode == .listening {
        chromeWaveform
      }

      Spacer(minLength: 4)

      sessionTimer

      if state.autoPasteControlAvailable {
        autoPasteControl
      }
      placementMenu
        .foregroundStyle(CSColor.textFaint)
      compactPrimaryAction
    }
    .padding(.horizontal, 16)
    .padding(.vertical, 10)
    .background(OverlayDragHandle())
  }

  /// Audio-evidence strip in the primary bar. Amplitude/VAD only — word/PCM
  /// synchronized scrolling needs authenticated sample spans from projection
  /// receipts and is intentionally not invented here.
  private var chromeWaveform: some View {
    WaveformView(
      barCount: 18,
      active: !state.transcribing && !state.isFinalPass && (state.audioReady || state.vadActive),
      transcribing: state.transcribing || state.isFinalPass,
      indicatorMode: state.indicatorMode,
      meter: state.levelMeter,
      compact: true
    )
    .accessibilityIdentifier("overlay-chrome-waveform")
    .accessibilityLabel("Live audio level")
    .allowsHitTesting(false)
  }

  /// Compact persisted delivery control. `ViewThatFits` keeps the literal label
  /// in normal widths and falls back to the same truthful icon/value control at
  /// the 320pt floor. Both variants share one explicit accessibility contract.
  private var autoPasteControl: some View {
    Button {
      state.setAutoPasteEnabled(!state.autoPasteEnabled)
    } label: {
      ViewThatFits(in: .horizontal) {
        autoPasteControlLabel(showTitle: true)
        autoPasteControlLabel(showTitle: false)
      }
    }
    .csFocusRing(cornerRadius: CSRadius.pill)
    .help("Auto Paste: \(state.autoPasteAccessibilityValue)")
    .accessibilityLabel("Auto Paste")
    .accessibilityValue(state.autoPasteAccessibilityValue)
    .accessibilityHint("Automatically insert completed dictation in the previous app")
    .accessibilityIdentifier("overlay-auto-paste")
  }

  private func autoPasteControlLabel(showTitle: Bool) -> some View {
    HStack(spacing: 5) {
      Image(systemName: "arrow.down.doc.fill")
        .font(.system(size: 10, weight: .semibold))
      if showTitle {
        Text("Auto Paste")
          .csMono(9, .semibold)
          .lineLimit(1)
      }
      Circle()
        .fill(state.autoPasteEnabled ? CSColor.oliveLight : CSColor.textFaint)
        .frame(width: 6, height: 6)
    }
    .foregroundStyle(CSColor.textFaint)
    .padding(.horizontal, showTitle ? 8 : 7)
    .padding(.vertical, 5)
    .background(CSColor.surfaceRaised(0.04))
    .overlay(
      Capsule().strokeBorder(CSColor.hairline(0.12), lineWidth: 1)
    )
    .clipShape(Capsule())
  }

  /// Placement config under the `…` icon: six screen anchors or free motion.
  /// Selecting an anchor exits free motion (the pick's intent is "go there");
  /// the reposition itself is orchestrated via `OverlayState.onPlacementChanged`.
  private var placementMenu: some View {
    Menu {
      Picker("Position", selection: $state.placementAnchor) {
        ForEach(OverlayAnchor.allCases) { anchor in
          Text(anchor.label).tag(anchor)
        }
      }
      .pickerStyle(.inline)
      Divider()
      Toggle("Free motion", isOn: $state.freeMotion)
    } label: {
      CSIconView(icon: .more, size: 15, weight: .medium)
    }
    .menuStyle(.button)
    .csFocusRing(cornerRadius: 8)
    .menuIndicator(.hidden)
    .fixedSize()
    .accessibilityIdentifier("overlay-placement-menu")
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
      switch state.mode {
      case .listening:
        listeningBody
          .transition(.opacity.combined(with: .offset(y: 8)))
      case .formatted:
        // TextEditor is an AppKit-backed platform view. Moving it with a SwiftUI
        // transition can leave its native text layer painting at the old frame
        // while the surrounding stack has already settled, which lets transcript
        // glyphs bleed through the action row during finalization. The FINAL body
        // swaps in place; the containing clip below is the hard sibling boundary.
        formattedBody
      case .noSpeech:
        noSpeechBody
          .transition(.opacity.combined(with: .offset(y: 8)))
      case .error:
        errorBody
          .transition(.opacity.combined(with: .offset(y: 8)))
      }
    }
    .frame(
      maxWidth: .infinity, minHeight: bodyMinHeight, maxHeight: .infinity, alignment: .topLeading
    )
    .padding(.horizontal, 20)
    .padding(.top, 4)
    .padding(.bottom, 10)
    // Platform-backed TextEditor content must never paint into the action/footer
    // siblings, including the mode-transition and live-resize frames.
    .clipped()
    .animation(CSMotion.floatIn, value: state.mode)
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
      LiveTranscriptTextView(runs: state.highlightCanvasRuns)
        .overlay(alignment: .bottomTrailing) {
          BlinkingCaret()
            .padding(.trailing, 3)
            .allowsHitTesting(false)
        }
        .frame(minHeight: transcriptMinHeight)
        .accessibilityIdentifier("overlay-transcript-area")
      if state.highlightsEnabled {
        OverlayHighlightTeachBar(
          highlights: state.highlights,
          selectedId: state.selectedHighlightId,
          onSelect: { state.selectHighlight($0) },
          onTeach: { state.sendHighlightToTeach($0) }
        )
        .padding(.top, 8)
      }
    }
    .frame(maxWidth: .infinity, alignment: .leading)
  }

  private var formattedBody: some View {
    VStack(alignment: .leading, spacing: 8) {
      if state.isEditingTranscript {
        TextEditor(
          text: Binding(
            get: { state.formattedText },
            set: { state.userEditedTranscript($0) }
          )
        )
        .csFont(19)
        .foregroundStyle(CSColor.textHigh)
        .lineSpacing(6)
        .scrollContentBackground(.hidden)
        .background(Color.clear)
        .frame(minHeight: bodyMinHeight)
        .accessibilityIdentifier("overlay-transcript-formatted")
      } else {
        Text(state.formattedText)
          .csFont(19, .medium)
          .foregroundStyle(CSColor.textHigh)
          .lineSpacing(6)
          .frame(maxWidth: .infinity, minHeight: bodyMinHeight, alignment: .topLeading)
          .contentShape(Rectangle())
          .onTapGesture { state.beginTranscriptEdit() }
          .accessibilityIdentifier("overlay-transcript-formatted")
          .help("Click to edit. The caret stays in the other app until you do.")
      }
    }
  }

  /// Terminal outcome for a session that captured no usable speech. Replaces
  /// the empty editable FINAL with a calm, non-alarming notice (mic glyph +
  /// message). No Copy/Insert/Send — there is nothing to act on; CloseDot
  /// remains the dismiss control.
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
    HStack(spacing: 12) {
      CSIconView(icon: .error, size: 18, weight: .regular)
        .foregroundStyle(CSColor.terracotta)
      VStack(alignment: .leading, spacing: 2) {
        Text(state.errorMessage ?? "Transcription failed")
          .csFont(15, .medium)
          .foregroundStyle(CSColor.textBody)
          .fixedSize(horizontal: false, vertical: true)
        Text("Recording stopped before a transcript was available.")
          .csMono(11, .medium)
          .foregroundStyle(CSColor.textFaint)
      }
      Spacer(minLength: 0)
    }
    .frame(maxWidth: .infinity, minHeight: bodyMinHeight, alignment: .leading)
  }

  // MARK: Compact primary action

  /// Split chrome control: the title runs the primary act; the chevron is a
  /// separate menu. One capsule, two hit targets. macOS Menu with a primary
  /// action treats the whole control as that action, so the chevron never opens.
  /// CloseDot stays the always-visible dismiss path; Close remains in the menu.
  @ViewBuilder
  private var compactPrimaryAction: some View {
    if let kind = state.primaryActionKind {
      splitPrimaryAction(kind: kind)
    }
  }

  private func splitPrimaryAction(kind: OverlayPrimaryActionKind) -> some View {
    let shape = RoundedRectangle(cornerRadius: buttonRadius, style: .continuous)
    return HStack(spacing: 0) {
      Button {
        performPrimaryAction(kind)
      } label: {
        Text(state.primaryActionTitle)
          .font(CSFont.ui(12, .semibold))
          .lineLimit(1)
          .padding(.leading, 10)
          .padding(.trailing, 8)
          .frame(height: primaryActionHeight)
          .contentShape(Rectangle())
      }
      .csFocusRing(cornerRadius: buttonRadius)
      .help(state.primaryActionHelp)
      .accessibilityLabel(state.primaryActionTitle)
      .accessibilityHint(state.primaryActionHelp)
      .accessibilityIdentifier("overlay-primary-action")

      Rectangle()
        .fill(CSColor.hairline(0.14))
        .frame(width: 1, height: 14)

      Menu {
        secondaryActionButtons(for: kind)
        Divider()
        Button("Close", role: .destructive) { state.close() }
      } label: {
        Image(systemName: "chevron.down")
          .font(.system(size: 9, weight: .semibold))
          .frame(width: 22, height: primaryActionHeight)
          .contentShape(Rectangle())
      }
      .menuStyle(.borderlessButton)
      .menuIndicator(.hidden)
      .frame(width: 22, height: primaryActionHeight)
      .csFocusRing(cornerRadius: 8)
      .help("More actions")
      .accessibilityLabel("More actions")
      .accessibilityIdentifier("overlay-primary-action-menu")
    }
    .foregroundStyle(CSColor.textBody)
    .background(CSColor.surfaceRaised(0.06))
    .overlay(shape.strokeBorder(CSColor.hairline(0.14), lineWidth: 1))
    .clipShape(shape)
    .fixedSize()
  }

  @ViewBuilder
  private func secondaryActionButtons(for kind: OverlayPrimaryActionKind) -> some View {
    switch kind {
    case .finish:
      if state.canCopy {
        Button("Copy") { state.copyToPasteboard() }
      }
    case .insert:
      if state.canCopy {
        Button("Copy") { state.copyToPasteboard() }
      }
      Button(OverlayActionPresentation.sendTitle) { state.sendToAgent() }
    }
  }

  private func performPrimaryAction(_ kind: OverlayPrimaryActionKind) {
    switch kind {
    case .finish: state.stop()
    case .insert: state.pasteToPreviousApp()
    }
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
      if state.showsFooterHonesty {
        Text(state.footerHonestyText)
          .foregroundStyle(CSColor.textFaintAlt)
          .accessibilityIdentifier("overlay-phase-footer")
      }
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
  #Preview("Listening") {
    DictationOverlayView(state: .previewListening())
      .padding(44)
      .background(
        LinearGradient(
          colors: [Color(hex: 0x15110E), CSColor.glassUnder],
          startPoint: .topLeading, endPoint: .bottomTrailing
        )
      )
      .preferredColorScheme(.dark)
  }

  #Preview("Transcribing") {
    // Pinned to the window's min content size so this preview doubles as the
    // min-size regression check: "transcribing…" fills the main status slot and
    // the transcript reserves ~2–3 lines instead of collapsing at the floor.
    DictationOverlayView(state: .previewTranscribing())
      .frame(width: 320, height: 260)
      .padding(44)
      .background(
        LinearGradient(
          colors: [Color(hex: 0x15110E), CSColor.glassUnder],
          startPoint: .topLeading, endPoint: .bottomTrailing
        )
      )
      .preferredColorScheme(.dark)
  }

  #Preview("No speech") {
    // Session ended without usable text: dedicated notice body, no
    // Copy/Format/Send, only Close. Pinned to the min content size so it also
    // guards the floor layout for this outcome.
    DictationOverlayView(state: .previewNoSpeech())
      .frame(width: 320, height: 260)
      .padding(44)
      .background(
        LinearGradient(
          colors: [Color(hex: 0x15110E), CSColor.glassUnder],
          startPoint: .topLeading, endPoint: .bottomTrailing
        )
      )
      .preferredColorScheme(.dark)
  }

  #Preview("Formatted") {
    DictationOverlayView(state: .previewFormatted())
      .padding(44)
      .background(
        LinearGradient(
          colors: [Color(hex: 0x15110E), CSColor.glassUnder],
          startPoint: .topLeading, endPoint: .bottomTrailing
        )
      )
      .preferredColorScheme(.dark)
  }

  #Preview("Formatted · compact chrome") {
    DictationOverlayView(state: .previewFormatted())
      .frame(width: 340, height: 260)
      .padding(44)
      .background(
        LinearGradient(
          colors: [Color(hex: 0x15110E), CSColor.glassUnder],
          startPoint: .topLeading, endPoint: .bottomTrailing
        )
      )
      .preferredColorScheme(.dark)
  }

  #Preview("Listening · scaled 1.4x") {
    // Exercises `\.csTextScale`: transcript + status render 40% larger while the
    // window chrome and paddings keep their intrinsic geometry (transcript scrolls
    // rather than forcing the panel taller).
    DictationOverlayView(state: .previewListening())
      .environment(\.csTextScale, 1.4)
      .frame(width: 470, height: 280)
      .padding(44)
      .background(
        LinearGradient(
          colors: [Color(hex: 0x15110E), CSColor.glassUnder],
          startPoint: .topLeading, endPoint: .bottomTrailing
        )
      )
      .preferredColorScheme(.dark)
  }
#endif

/// The overlay's brand dot as a real close control. It sits where macOS puts
/// traffic lights, so it honors that promise: hover swaps in the familiar "x"
/// glyph and click closes the overlay (same path as Close in the secondary menu).
/// The cursor stays the SYSTEM ARROW — real macOS window controls never switch
/// to a pointing hand, and neither does this one (U22; reverts 5415e7e's
/// pointingHand). Only the dot is live — the wordmark text is inert.
private struct CloseDot: View {
  var action: () -> Void
  @State private var hovered = false

  var body: some View {
    Button(action: action) {
      ZStack {
        ModeDot(color: CSColor.terracotta, size: 9)
        if hovered {
          Text("\u{00D7}")
            .font(.system(size: 9, weight: .heavy))
            .foregroundStyle(Color.black.opacity(0.7))
            .offset(y: -0.5)
        }
      }
      .frame(width: 16, height: 16)
      .contentShape(Circle())
    }
    .csFocusRing(cornerRadius: 8)
    .onHover { inside in
      hovered = inside
    }
    .accessibilityLabel("Close overlay")
    .accessibilityHint("Closes the dictation overlay")
  }
}
