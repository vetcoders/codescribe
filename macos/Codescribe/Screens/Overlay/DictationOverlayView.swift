import SwiftUI

// The floating dictation overlay content — pixel-faithful to
// "codescribe App - Dictation Overlay.dc.html".
//
// Layout (top → bottom):
//   header      brand wordmark · status pill · Auto Paste · placement (…) menu
//   mode + meta tag chip (RECORDING/AGENT/PROCESSING/READY) · meta line
//   body        listening = waveform (live RMS level) + word-reveal transcript
//               formatted = editable finalized transcript
//   action row  recording: Finish; finalized: Copy · Insert · Format · To Agent.
//               All actions are neutral/grey; Close is the ONE red control.
//   footer      ● <engine chip from serving/preference> · meta on the right
//
// Delivery/status whispers in the footer next to the engine chip — never a
// floating pill over the action row.
struct DictationOverlayView: View {
  @ObservedObject var state: OverlayState

  // Mock-derived geometry constants (not design tokens — local to this surface).
  // The window is user-resizable; content flows to fill whatever frame it gets,
  // never narrower than `windowMinWidth`. Below `actionIconOnlyThreshold`, the
  // action row switches to fixed icon buttons so the old full-label intrinsic width
  // no longer dictates the window floor. `DictationOverlayWindow.minSize.height`
  // MUST stay ≥ the chrome + `bodyMinHeight` sum — otherwise the content column
  // overflows the window frame and GlassPanel paints its rounded background past
  // the window rect, squaring the visible corners (see DictationOverlayWindow's
  // corner note).
  private let windowMinWidth: CGFloat = 320
  private let actionIconOnlyThreshold: CGFloat = 380
  // U22 diet: the action row used to eat ~1/3 of the overlay (38pt content +
  // 10pt vertical padding + 10pt button padding). Trimmed to 30/6/6 with a
  // 12pt semibold label — the ~16pt saved is handed to the transcript via
  // `bodyMinHeight` below (lockstep, window minSize unchanged).
  private let actionRowContentHeight: CGFloat = 30
  private let actionIconButtonSize: CGFloat = 28
  // `bodyMinHeight` reserves the body floor at the min window size: the listening
  // body needs the waveform block (~46) PLUS `transcriptMinHeight` so the growing
  // transcript keeps ~3 legible lines instead of collapsing to a clipped sliver.
  // 114 → 130: the vertical space reclaimed from the slimmer action row stays
  // with the transcript. `DictationOverlayWindow.minSize.height` (300) still
  // covers chrome + this floor — the content column stays ≤ the window frame
  // (see the corner-clip note above).
  private let bodyMinHeight: CGFloat = 130
  private let transcriptMinHeight: CGFloat = 84
  private let buttonRadius: CGFloat = 10
  /// Action chrome stays put but whispers until the pointer is on the row.
  @State private var actionRowHovered = false

  var body: some View {
    GlassPanel(cornerRadius: CSRadius.window, sitsInForest: true) {
      VStack(alignment: .leading, spacing: 0) {
        header
        hairline(0.06)
        modeMetaRow
        bodySection
        hairline(0.06)
        actionRow
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
    HStack(spacing: 12) {
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
      // Swap the whole VIEW TYPE on live vs idle, not just a flag: the
      // animated pill (with @State + repeatForever) exists ONLY while live,
      // and is replaced by a static pill of different identity in idle/final,
      // so SwiftUI tears down its animation instead of leaving it ticking.
      if state.statusRippling {
        StatusPill(
          text: state.statusText,
          color: state.statusColor,
          rippling: true
        )
        .padding(.leading, 6)
        .allowsHitTesting(false)
        .accessibilityIdentifier("overlay-phase-status")
      } else {
        StaticStatusPill(text: state.statusText, color: state.statusColor)
          .padding(.leading, 6)
          .allowsHitTesting(false)
          .accessibilityIdentifier("overlay-phase-status")
      }
      Spacer(minLength: 0)
      if state.autoPasteControlAvailable {
        autoPasteControl
      }
      placementMenu
        .foregroundStyle(CSColor.textFaint)
    }
    .padding(.horizontal, 20)
    .padding(.vertical, 12)
    .background(OverlayDragHandle())
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

  // MARK: Mode + meta row

  private var modeMetaRow: some View {
    HStack(spacing: 10) {
      Text(state.tagText)
        .csMono(10, .semibold)
        .tracking(0.8)
        .foregroundStyle(state.tagColor)
        .padding(.horizontal, 9)
        .padding(.vertical, 3)
        .background(state.tagColor.opacity(0.1))
        .overlay(
          RoundedRectangle(cornerRadius: 6, style: .continuous)
            .strokeBorder(state.tagColor.opacity(0.28), lineWidth: 1)
        )
        .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
      Text(state.metaText)
        .csMono(11, .medium)
        .foregroundStyle(CSColor.textFaint)
      Spacer(minLength: 0)
      sessionTimer
    }
    .padding(.horizontal, 20)
    .padding(.top, 8)
    .padding(.bottom, 4)
    .background(OverlayDragHandle())
  }

  /// Live `00:00` session counter — the absolute reference for audio sync,
  /// transcription lag, and stream drift (UI_DIVERGENCE_AUDIT pkt 5). Ticks
  /// only while `.listening`; the state freezes the underlying stamp when
  /// capture stops, so the final displayed value is the session's true length.
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
    VStack(alignment: .leading, spacing: 0) {
      WaveformView(
        active: !state.transcribing && !state.isFinalPass && (state.audioReady || state.vadActive),
        transcribing: state.transcribing || state.isFinalPass,
        indicatorMode: state.indicatorMode,
        meter: state.levelMeter
      )
      .padding(.top, 4)
      .padding(.bottom, 8)
      .allowsHitTesting(false)
      .background(OverlayDragHandle())
      transcriptScroll
    }
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
      if let status = state.formatFailureStatus {
        Text(status)
          .csMono(11, .medium)
          .foregroundStyle(CSColor.textFaint)
          .accessibilityIdentifier("overlay-format-failure-status")
      }
    }
  }

  /// Terminal outcome for a session that captured no usable speech. Replaces
  /// the empty editable FINAL with a calm, non-alarming notice (mic glyph +
  /// message). No Copy/Format/Send — there is nothing to act on; only Close
  /// remains in the action row.
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

  // MARK: Action row

  /// U22 semantics: every ACTION (Finish/Copy/Insert/Format/To Agent) is a neutral
  /// grey surface — the one exception is Close, the sole destructive control,
  /// which wears `CSColor.danger` and must read as red at first glance.
  private enum ActionButtonTone {
    case neutral
    case danger
  }

  private var actionRow: some View {
    GeometryReader { proxy in
      let iconOnly = proxy.size.width < actionIconOnlyThreshold
      actionRowContent(iconOnly: iconOnly)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
    }
    .frame(height: actionRowContentHeight)
    .padding(.horizontal, 20)
    .padding(.vertical, 6)
    .contentShape(Rectangle())
    .opacity(actionRowHovered ? 1 : 0.22)
    .animation(.easeOut(duration: 0.16), value: actionRowHovered)
    .onHover { actionRowHovered = $0 }
    .accessibilityElement(children: .contain)
  }

  @ViewBuilder
  private func actionRowContent(iconOnly: Bool) -> some View {
    HStack(spacing: 8) {
      if state.mode == .listening {
        actionButton(
          title: "Finish",
          icon: "checkmark",
          tone: .neutral,
          iconOnly: iconOnly,
          action: { state.stop() }
        )
        if state.canCopy {
          actionButton(
            title: "Copy",
            icon: "doc.on.doc",
            tone: .neutral,
            iconOnly: iconOnly,
            action: { state.copyToPasteboard() }
          )
        }
      } else if state.mode == .formatted {
        if state.canCopy {
          actionButton(
            title: "Copy",
            icon: "doc.on.doc",
            tone: .neutral,
            iconOnly: iconOnly,
            action: { state.copyToPasteboard() }
          )
        }

        actionButton(
          title: state.insertActionPresentation.title,
          help: state.insertActionPresentation.help,
          icon: "arrow.down.doc.fill",
          tone: .neutral,
          iconOnly: iconOnly,
          action: { state.pasteToPreviousApp() }
        )

        if state.canRevert {
          actionButton(
            title: "Revert",
            help: "Restore the transcript from before the last format",
            icon: "arrow.uturn.backward",
            tone: .neutral,
            iconOnly: iconOnly,
            action: { state.revertFormat() }
          )
          .accessibilityIdentifier("overlay-format-revert")
        }

        manualFormatMenu(iconOnly: iconOnly)

        manualRetranscribeMenu(iconOnly: iconOnly)

        actionButton(
          title: OverlayActionPresentation.sendTitle,
          help: OverlayActionPresentation.sendHelp,
          icon: "paperplane.fill",
          tone: .neutral,
          iconOnly: iconOnly,
          action: { state.sendToAgent() }
        )
      } else if state.mode == .noSpeech {
        manualRetranscribeMenu(iconOnly: iconOnly)
      }

      Spacer(minLength: 0)

      actionButton(
        title: "Close",
        icon: "xmark",
        tone: .danger,
        iconOnly: iconOnly,
        action: { state.close() }
      )
    }
  }

  private func manualFormatMenu(iconOnly: Bool) -> some View {
    Menu {
      ForEach(OverlayActionPresentation.manualFormatLevels) { level in
        Button(level.visibleName) {
          state.formatTranscript(level: level)
        }
      }
    } label: {
      actionButtonLabel(
        title: state.isFormatting ? "Formatting..." : OverlayActionPresentation.formatTitle,
        icon: "wand.and.stars",
        tone: .neutral,
        iconOnly: iconOnly
      )
    }
    .menuStyle(.button)
    .csFocusRing(cornerRadius: 8)
    .menuIndicator(.hidden)
    .help(state.manualFormatHelp)
    .disabled(!state.canFormat)
    .opacity(state.canFormat ? 1 : 0.45)
    .accessibilityLabel(OverlayActionPresentation.formatTitle)
    .accessibilityValue(
      state.autoFormatLevel == .off
        ? "Auto Format Off" : "Auto Format \(state.autoFormatLevel.visibleName)"
    )
    .accessibilityHint(OverlayActionPresentation.formatHelp)
    .accessibilityIdentifier("overlay-format-menu")
  }

  private func manualRetranscribeMenu(iconOnly: Bool) -> some View {
    Menu {
      ForEach(OverlayRetranscribePass.allCases) { pass in
        Button(pass.visibleName) {
          state.retranscribe(pass: pass)
        }
      }
    } label: {
      actionButtonLabel(
        title: state.isRetranscribing
          ? "Retranscribing..." : OverlayActionPresentation.retranscribeTitle,
        icon: "arrow.triangle.2.circlepath",
        tone: .neutral,
        iconOnly: iconOnly
      )
    } primaryAction: {
      state.retranscribe(pass: .fullHq)
    }
    .menuStyle(.button)
    .csFocusRing(cornerRadius: 8)
    .menuIndicator(.hidden)
    .help(OverlayActionPresentation.retranscribeHelp)
    .disabled(!state.canRetranscribe)
    .opacity(state.canRetranscribe ? 1 : 0.45)
    .accessibilityLabel(OverlayActionPresentation.retranscribeTitle)
    .accessibilityHint(OverlayActionPresentation.retranscribeHelp)
    .accessibilityIdentifier("overlay-retranscribe-menu")
  }

  private func actionButton(
    title: String,
    help: String? = nil,
    icon: String,
    tone: ActionButtonTone,
    iconOnly: Bool,
    isEnabled: Bool = true,
    action: @escaping () -> Void
  ) -> some View {
    Button(action: action) {
      actionButtonLabel(title: title, icon: icon, tone: tone, iconOnly: iconOnly)
    }
    .csFocusRing(cornerRadius: 8)
    .help(help ?? title)
    .accessibilityLabel(title)
    .accessibilityHint(help ?? title)
    .disabled(!isEnabled)
    .opacity(isEnabled ? 1 : 0.45)
  }

  @ViewBuilder
  private func actionButtonLabel(
    title: String,
    icon: String,
    tone: ActionButtonTone,
    iconOnly: Bool
  ) -> some View {
    let shape = RoundedRectangle(cornerRadius: buttonRadius, style: .continuous)
    Group {
      if iconOnly {
        Image(systemName: icon)
          .font(.system(size: 12, weight: .semibold))
          .frame(width: actionIconButtonSize, height: actionIconButtonSize)
      } else {
        Text(title)
          .font(CSFont.ui(12, .semibold))
          .padding(.horizontal, 13)
          .padding(.vertical, 6)
      }
    }
    .foregroundStyle(actionForeground(tone))
    .background(actionBackground(tone))
    .overlay {
      if let border = actionBorder(tone) {
        shape.strokeBorder(border, lineWidth: 1)
      }
    }
    .clipShape(shape)
  }

  private func actionForeground(_ tone: ActionButtonTone) -> Color {
    switch tone {
    case .neutral: return CSColor.textBody
    case .danger: return CSColor.textHigh
    }
  }

  private func actionBackground(_ tone: ActionButtonTone) -> Color {
    switch tone {
    case .neutral: return CSColor.surfaceRaised(0.04)
    case .danger: return CSColor.danger
    }
  }

  private func actionBorder(_ tone: ActionButtonTone) -> Color? {
    switch tone {
    case .neutral: return CSColor.hairline(0.12)
    case .danger: return nil
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
      Text(state.footerRight)
        .foregroundStyle(CSColor.textFaintAlt)
        .accessibilityIdentifier("overlay-phase-footer")
    }
    .csMono(10, .medium)
    .padding(.horizontal, 20)
    .padding(.vertical, 8)
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
    // Pinned to the window's min content size (320×300) so this preview doubles as
    // the min-size regression check: "transcribing…" fills the main status slot and
    // the transcript reserves ~2–3 lines instead of collapsing at the floor.
    DictationOverlayView(state: .previewTranscribing())
      .frame(width: 320, height: 300)
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
      .frame(width: 320, height: 300)
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

  #Preview("Formatted · icon actions") {
    DictationOverlayView(state: .previewFormatted())
      .frame(width: 340, height: 300)
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
      .frame(width: 470, height: 330)
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
/// glyph and click closes the overlay (same path as the Close action button).
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
