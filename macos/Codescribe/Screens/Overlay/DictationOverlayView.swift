import AppKit
import SwiftUI

// Slim evidence-first dictation overlay.
//
// Layout (top → bottom):
//   header   brand · ONE projection phase · compact waveform · timer
//   body     transcript is the product surface (listening / formatted / terminal)
//   floating actions over the transcript; no footer inset or reserved band
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
  @Environment(\.colorScheme) private var colorScheme
  @AppStorage(DictationOverlayGate.labModeDefaultsKey) private var labMode = false
  @State private var closeDotHovered = false
  @Namespace private var bottomChromeNamespace
  @State private var actions = OverlayActionsPresentation()
  @FocusState private var actionsFocused: Bool
  @State private var pointerInsideOverlay = false
  @State private var overlayVisible = false
  @Bindable var state: OverlayState

  // Geometry constants local to this surface. The window is user-resizable;
  // content fills the frame and never goes narrower than `windowMinWidth`.
  // `DictationOverlayWindow.minSize.height` MUST stay ≥ chrome + `bodyMinHeight`
  // or GlassPanel paints past the window rect and squares the corners.
  private let windowMinWidth: CGFloat = 320
  private let bodyMinHeight: CGFloat = 130
  private let transcriptMinHeight: CGFloat = 96
  private var palette: OverlayAppearancePalette {
    OverlayAppearancePalette.resolve(colorScheme)
  }
  private var showsDiagnostics: Bool {
    DeveloperSurface.isPowerModeEnabled(labMode: labMode)
  }

  var body: some View {
    OverlayCanvasSurface(palette: palette) {
      sharedChromeContainer(
        OverlayIntentRail(
          phase: state.statusText,
          intents: OverlayIntentRail.projectedIntents(for: state),
          palette: palette,
          footerEngineLabel: state.footerEngineLabel,
          footerNotice: state.toast,
          history: state.documentHistory,
          historyAvailable: state.terminal,
          currentRevision: state.revision,
          formatLevel: state.autoFormatLevel,
          onIntent: state.relayIntent,
          onRetranscribe: { state.retranscribe(pass: $0) },
          onRestore: state.restoreDocumentRevision,
          onHistoryRequest: state.loadDocumentHistory,
          onFormatOnce: { state.formatTranscript(at: $0) },
          onFocusChange: { if $0 { actions.interact() } },
          onDismiss: { actions.dismiss() },
          onInteraction: { actions.interact() }
        )
      )
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
    .animation(reduceMotion ? nil : CSMotion.floatIn, value: state.toast)
    .onHover { inside in
      pointerInsideOverlay = inside
      state.setPointerHovering(inside)
    }
    .onAppear {
      FontLoader.register()
    }
  }

  @ViewBuilder
  private func sharedChromeContainer<IntentRail: View>(
    _ intentRail: IntentRail
  ) -> some View {
    if #available(macOS 26.0, *) {
      GlassEffectContainer(spacing: 0) {
        canvasStack(intentRail)
      }
    } else {
      canvasStack(intentRail)
    }
  }

  private func canvasStack<IntentRail: View>(_ intentRail: IntentRail) -> some View {
    VStack(spacing: 0) {
      header
      hairline(0.06)
      if !state.isCollapsed,
        let label = OverlayActionsPresentation.finishingLabel(
          mode: state.mode, transcribing: state.transcribing, terminal: state.terminal)
      {
        Text(label)
          .csMono(10, .medium)
          .foregroundStyle(palette.processingStatus.color)
          .accessibilityIdentifier("overlay-finishing")
          .allowsHitTesting(false)
      }
      bodySection
        .frame(height: state.isCollapsed ? 0 : nil)
        .clipped()
        .opacity(state.isCollapsed ? 0 : 1)
        .allowsHitTesting(!state.isCollapsed)
        .accessibilityHidden(state.isCollapsed)
    }
    .overlay(alignment: .bottom) {
      if !state.isCollapsed {
        HStack(spacing: 6) {
          OverlayEvidenceChip(
            state: state, palette: palette, actionsOpen: actions.phase == .open,
            glassNamespace: bottomChromeNamespace
          )
          .layoutPriority(-1)
          HStack(spacing: 2) {
            Button {
              actions.toggle()
            } label: {
              HStack(spacing: 4) {
                Image(systemName: OverlayControlSymbols.actions)
                if let label = OverlayActionsPresentation.pillLabel(
                  phase: actions.phase, notice: state.toast)
                {
                  Text(label)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: 200, alignment: .leading)
                }
              }
              .font(.system(size: 11, weight: .medium))
              .foregroundStyle(palette.primaryText.color)
              .padding(.horizontal, actions.phase == .open ? 0 : 10)
              .frame(
                minWidth: actions.phase == .open
                  ? nil : OverlayResizeChrome.actionsWidth(narrow: actions.phase != .hover))
              .frame(height: OverlayResizeChrome.actionsHeight)
              .fixedSize(horizontal: true, vertical: true)
              .contentShape(Capsule())
              .overlay(alignment: .topTrailing) {
                if state.hasRecoverableSupersededWork && actions.phase != .open {
                  Circle()
                    .fill(palette.processingStatus.color)
                    .frame(width: 5, height: 5)
                    .accessibilityHidden(true)
                    .accessibilityIdentifier("overlay-retained-work-badge")
                }
              }
            }
            .buttonStyle(.plain)
            .focusable()
            .focused($actionsFocused)
            .help(actions.phase == .open ? "Hide actions" : "Show actions")
            .accessibilityLabel("Actions")
            .accessibilityValue(actions.phase == .open ? "Open" : "Collapsed")
            .accessibilityHint(
              state.hasRecoverableSupersededWork
                ? "Previous take available. Open actions to copy or discard it."
                : "Show or hide transcript tools"
            )
            .accessibilityIdentifier("overlay-tools-handle")
            if actions.phase == .open {
              intentRail
            }
          }
          .padding(.vertical, actions.phase == .open ? 2 : 0)
          .padding(.horizontal, actions.phase == .open ? 10 : 0)
          .fixedSize(horizontal: false, vertical: true)
          .modifier(OverlayActionsSurface(palette: palette, glassNamespace: bottomChromeNamespace))
          .contentShape(Capsule())
          .onHover { actions.pointerChanged($0) }
          .onChange(of: actionsFocused) { _, focused in
            if focused { actions.interact() }
          }
          .onExitCommand { actions.dismiss() }
          .animation(reduceMotion ? nil : .easeOut(duration: 0.15), value: actions.phase)
          .transaction { transaction in
            if reduceMotion {
              transaction.animation = nil
              transaction.disablesAnimations = true
            }
          }
          .task(id: actions.hideDeadline) {
            guard let deadline = actions.hideDeadline else { return }
            do {
              try await ContinuousClock().sleep(until: deadline)
            } catch { return }
            guard !Task.isCancelled else { return }
            actions.expire()
          }
        }
        // Glass is confined to each capsule, before the bar's clear margins.
        // The AppKit edge intercept and existing header/body drag regions stay in place.
        .frame(maxWidth: .infinity, alignment: .center)
        .padding(.horizontal, OverlayResizeChrome.actionsBottomInset)
        .padding(.bottom, OverlayResizeChrome.actionsBottomInset)
      } else if let label = OverlayActionsPresentation.finishingLabel(
        mode: state.mode, transcribing: state.transcribing, terminal: state.terminal)
      {
        // The folded bar keeps its height; the passive wait label uses its
        // bottom center without touching the header timer or capture state.
        Text(label)
          .csMono(10, .medium)
          .foregroundStyle(palette.processingStatus.color)
          .padding(.horizontal, 4)
          .background(palette.desktopBackground.color, in: Capsule())
          .padding(.bottom, 2)
          .accessibilityIdentifier("overlay-finishing")
          .allowsHitTesting(false)
      }
    }
    .overlay(alignment: .bottom) {
      if !state.isCollapsed {
        // The container claims this entire bar and its vertical margin before
        // SwiftUI hit testing, then tracks .bottom with the edge resize cursor.
        Capsule()
          .fill(palette.primaryText.color.opacity(0.3))
          .frame(
            width: OverlayResizeChrome.gripSize.width, height: OverlayResizeChrome.gripSize.height
          )
          .padding(.bottom, OverlayResizeChrome.gripBottomInset)
          .allowsHitTesting(false)
          .accessibilityHidden(true)
      }
    }
    .overlay {
      if !state.isCollapsed {
        HStack {
          Capsule().frame(width: 3, height: 28)
          Spacer()
          Capsule().frame(width: 3, height: 28)
        }
        .foregroundStyle(palette.primaryText.color)
        .padding(.horizontal, 5)
        .opacity(OverlayResizeChrome.sideIndicatorOpacity(pointerInside: pointerInsideOverlay))
        .animation(
          OverlayResizeChrome.sideIndicatorAnimation(reduceMotion: reduceMotion),
          value: pointerInsideOverlay
        )
        .transaction { transaction in
          if reduceMotion {
            transaction.animation = nil
            transaction.disablesAnimations = true
          }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
      }
    }
    .onChange(of: state.isCollapsed) { _, collapsed in
      if collapsed { actions.reset() }
    }
    .onChange(of: state.captureGeneration) { _, _ in actions.reset() }
  }

  /// 1px separator matching the mock's hairline borders.
  private func hairline(_ alpha: Double) -> some View {
    palette.border.color.opacity(alpha / max(palette.border.alpha, 0.001)).frame(height: 1)
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
    // Drag region BETWEEN the content and the chrome. `OverlayHeaderChrome` is
    // `glassEffect` on macOS 26, and a glass surface is hit-testable: with the
    // region attached after the modifier it sat under the glass, so every
    // header point outside the brand block answered `NSHostingView` and the
    // window's drag intercept never fired (Founder, build 849: "header chrome
    // overlaya nadal nie oferuje drag area"). Falsifier:
    // OverlayResizeHitTests.testHeaderIsAWindowDragHandleAcrossItsWidth.
    .background { OverlayWindowDragRegion(identifier: "overlay-header-drag-region") }
    .modifier(OverlayHeaderChrome(palette: palette))
    // The cached panel survives orderOut. Observe its window outside
    // ViewThatFits so hidden header candidates cannot compete for visibility.
    .background {
      OverlayRenderVisibility { visible in
        guard overlayVisible != visible else { return }
        var transaction = Transaction(animation: nil)
        transaction.disablesAnimations = true
        withTransaction(transaction) { overlayVisible = visible }
      }
      .frame(width: 0, height: 0)
    }
  }

  private var fullHeader: some View { justifiedHeader(compact: false) }

  private var narrowHeader: some View { justifiedHeader(compact: true) }

  private func justifiedHeader(compact: Bool) -> some View {
    HStack(spacing: compact ? 6 : 10) {
      HStack(spacing: 5) {
        Button {
          state.relayIntent(.close)
        } label: {
          ModeDot(
            color: palette.statusToken(for: state.mode).color,
            size: compact ? 5.25 : 7
          )
          .overlay {
            if closeDotHovered {
              OverlayCloseCross()
                .stroke(palette.desktopBackground.color, style: StrokeStyle(lineWidth: 1))
                .accessibilityHidden(true)
            }
          }
          .scaleEffect(closeDotHovered ? 10.0 / 7.0 : 1)
          // 24 pt hit target without moving the dot: the shape reaches past the
          // circle, the layout keeps the pre-b83e95538 position (Founder, 25 IX).
          .contentShape(Circle().inset(by: compact ? -9.375 : -8.5))
        }
        .buttonStyle(.plain)
        .onHover { closeDotHovered = $0 }
        .animation(.easeOut(duration: 0.12), value: closeDotHovered)
        // Never the panel's initial key view: the transcript canvas keeps the
        // preselection, and Space/Return cannot close the overlay by accident.
        .focusable(false)
        .help(OverlayIntent.close.helpText)
        .accessibilityLabel(OverlayIntent.close.accessibilityLabel)
        .accessibilityIdentifier("overlay-brand-close-dot")

        Text("codescribe")
          .font(CSFont.ui(compact ? 12 : 15, .bold))
          .tracking(-0.3)
          .foregroundStyle(palette.primaryText.color)
          .allowsHitTesting(false)
      }
      .fixedSize()
      .accessibilityElement(children: .contain)
      .accessibilityIdentifier("overlay-header-leading")
      .background {
        OverlayWindowDragRegion(identifier: "overlay-header-inert-drag-region")
      }

      chromeWaveform(barCount: compact ? 10 : 34)
        .frame(minWidth: compact ? 12 : 100, maxWidth: .infinity)
        .layoutPriority(-1)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("overlay-header-center")

      HStack(spacing: compact ? 4 : 8) {
        if showsDiagnostics && state.compactProjection?.degraded == true {
          Image(systemName: "exclamationmark.bubble.fill")
            .foregroundStyle(.orange)
            .help("Detected speech is not fully transcribed")
            .accessibilityLabel("Detected speech is not fully transcribed")
            .accessibilityIdentifier("overlay-acoustic-warning")
        }
        if let error = state.expansionPreferenceError {
          Image(systemName: "exclamationmark.triangle.fill")
            .foregroundStyle(.orange)
            .help(error)
            .accessibilityLabel(error)
            .accessibilityIdentifier("overlay-preference-save-error")
        }
        autoPasteControl
        sessionTimer
          .allowsHitTesting(false)
        OverlayPlacementMenu(state: state, palette: palette)
        Button {
          state.toggleCollapsed()
        } label: {
          Image(systemName: state.isCollapsed ? "chevron.down" : "chevron.up")
            .font(.system(size: 11, weight: .semibold))
            .foregroundStyle(palette.mutedText.color)
            .frame(width: 22, height: 22)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(state.isCollapsed ? "Expand transcript" : "Collapse to recording bar")
        .accessibilityLabel(state.isCollapsed ? "Expand transcript" : "Collapse transcript")
        .accessibilityIdentifier("overlay-collapse-toggle")
      }
      .fixedSize()
      .accessibilityElement(children: .contain)
      .accessibilityIdentifier("overlay-header-trailing")
    }
  }

  /// Compact header auto-paste control: discrete icon + indicator dot.
  private var autoPasteControl: some View {
    Button {
      state.setAutoPasteEnabled(!state.autoPasteEnabled)
    } label: {
      HStack(spacing: 4) {
        Image(
          systemName: state.autoPasteEnabled
            ? OverlayControlSymbols.autoPasteOn : OverlayControlSymbols.autoPasteOff
        )
        .font(.system(size: 10, weight: .semibold))
        Circle()
          .fill(state.autoPasteEnabled ? CSColor.oliveLight : CSColor.textFaint)
          .frame(width: 5, height: 5)
      }
      .foregroundStyle(palette.mutedText.color)
      .padding(.horizontal, 6)
      .padding(.vertical, 4)
      .background(CSColor.surfaceRaised(0.04))
      .overlay(Capsule().strokeBorder(palette.border.color, lineWidth: 1))
      .clipShape(Capsule())
    }
    .buttonStyle(.plain)
    .disabled(!state.autoPasteControlAvailable)
    .help("Auto-paste: \(state.autoPasteEnabled ? "On" : "Off")")
    .accessibilityLabel("Auto-paste toggle")
    .accessibilityValue(state.autoPasteEnabled ? "On" : "Off")
    .accessibilityIdentifier("overlay-auto-paste")
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
      inactiveColor: palette.border.color,
      compact: true,
      stretches: true
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
    if state.showsSessionTimer && !state.isCollapsed && overlayVisible {
      TimelineView(.animation(minimumInterval: 1, paused: state.sessionTimerPaused)) { _ in
        Text(state.sessionTimerText)
          .csMono(11, .semibold)
          .foregroundStyle(palette.mutedText.color)
          .monospacedDigit()
      }
      .accessibilityIdentifier("overlay-session-timer")
      .accessibilityLabel("Recording time")
      .accessibilityValue(state.sessionTimerText)
    }
  }

  // MARK: Body

  private var bodySection: some View {
    VStack(alignment: .leading, spacing: CSSpace.sm) {
      // One permanent transcript surface. Status and lifecycle never replace
      // its contents; the engine may replace them only via its projection.
      transcriptScroll
      if let status = state.presentationStatus {
        presentationStatusBody(status)
          .transition(reduceMotion ? .identity : .opacity.combined(with: .offset(y: 8)))
      } else {
        switch state.mode {
        case .listening, .finalizing:
          EmptyView()
        case .formatted:
          revisionStatusRow
        case .coverageRefused:
          revisionStatusRow
          if showsDiagnostics {
            coverageRefusedBody
              .transition(reduceMotion ? .identity : .opacity.combined(with: .offset(y: 8)))
          }
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
    .background { OverlayWindowDragRegion(identifier: "overlay-body-drag-region") }
    // Transcript content stays inside the body during live resize.
    .clipped()
    .animation(reduceMotion ? nil : CSMotion.floatIn, value: state.mode)
  }

  /// Native live transcript: follows the newest words until the user clicks or
  /// selects an older phrase. The `NSTextView` keeps that selection stable across
  /// ongoing stream updates, so drag selection, Cmd-C and context-menu Copy work
  /// during recording without stopping capture. A `minHeight` reserves ~2–3 lines
  /// at the window floor.
  private var transcriptScroll: some View {
    VStack(alignment: .leading, spacing: 0) {
      LiveTranscriptTextView(
        text: state.canvasText,
        isEditable: state.isTranscriptEditable,
        appearance: palette.appearance,
        onEditingChanged: { editing in
          if editing { state.beginTranscriptEdit() } else { state.endTranscriptEdit() }
        },
        onTextChange: { state.updateRevisionDraft($0) },
        onCancelEdit: { state.discardRevisionDraft() }
      )
      .modifier(OverlayScrollEdgeEffects())
      .overlay(alignment: .bottomTrailing) {
        // The decorative caret yields to the real insertion point while the
        // canvas is being edited.
        if !state.isEditingTranscript {
          BlinkingCaret(animating: overlayVisible && state.animatesTranscriptCaret)
            .padding(.trailing, 3)
            .allowsHitTesting(false)
        }
      }
      .frame(minHeight: transcriptMinHeight)
      .accessibilityIdentifier("overlay-transcript-area")
      .accessibilityHint(
        state.isTranscriptEditable
          ? "Click to edit. Edits stay local until committed to the transcript ledger."
          : ""
      )
    }
    .frame(maxWidth: .infinity, alignment: .leading)
  }

  /// Ledger truth under the canvas: what the bytes on screen ARE — a local
  /// draft, a revision in flight, or the reducer's projection — plus the last
  /// commit failure. Same states as T15's editor status.
  private var revisionStatusRow: some View {
    VStack(alignment: .leading, spacing: CSSpace.xxs) {
      HStack(spacing: CSSpace.xs) {
        if state.formatterCommitPending {
          ProgressView()
            .controlSize(.small)
          Text("Formatting revision…")
        } else if state.revisionCommitPending {
          ProgressView()
            .controlSize(.small)
          Text("Committing revision…")
        } else if state.isRevisionDraftDirty {
          Image(systemName: "pencil.line")
          Text("Draft · not committed")
        } else {
          Image(systemName: state.mode == .coverageRefused ? "doc.text" : "checkmark.seal")
          Text(state.mode == .coverageRefused ? "Unsealed transcript" : "Ledger projection")
        }
      }
      .csMono(10, .semibold)
      .foregroundStyle(
        state.isRevisionDraftDirty ? CSColor.terracotta : palette.mutedText.color
      )
      .accessibilityElement(children: .combine)
      .accessibilityIdentifier("overlay-revision-status")

      // `recoveryFailure` joins the chain because a failed recovery keeps the
      // retained item: the user needs the full sentence, not just the footer
      // chip. This row is `.formatted`-only, so the persisting footer notice
      // remains the surface that covers a live capture.
      if let error = state.revisionCommitError ?? state.formatterError
        ?? state.recoveryFailure
      {
        Label(error, systemImage: "exclamationmark.triangle")
          .csMono(10, .medium)
          .foregroundStyle(CSColor.terracotta)
          .lineLimit(2)
          .accessibilityIdentifier("overlay-revision-error")
      }
    }
    .allowsHitTesting(false)
  }

  /// Terminal outcome for a session that captured no usable speech. Replaces
  /// the empty editable FINAL with a calm, non-alarming notice (mic glyph +
  /// message). No Copy/Insert/Send — there is nothing to act on; the intent
  /// rail follows the projection table and keeps Retranscribe/Close only.
  private var noSpeechBody: some View {
    HStack(spacing: 12) {
      CSIconView(icon: .mic, size: 18, weight: .regular)
        .foregroundStyle(palette.mutedText.color)
      VStack(alignment: .leading, spacing: 2) {
        Text(state.noSpeechNotice)
          .csFont(15, .medium)
          .foregroundStyle(palette.bodyText.color)
          .fixedSize(horizontal: false, vertical: true)
        Text("Nothing was captured this session.")
          .csMono(11, .medium)
          .foregroundStyle(palette.mutedText.color)
      }
      Spacer(minLength: 0)
    }
    .frame(maxWidth: .infinity, minHeight: bodyMinHeight, alignment: .leading)
  }

  /// Terminal outcome for a take the ledger settled without accepting its
  /// acoustic coverage. The words stay on the canvas above, untouched: this
  /// Voice Lab row explains why they carry no seal. Hiding this diagnostic
  /// never changes the projected delivery permissions or the retained text.
  ///
  /// Persistent by construction — it is painted from state, not scheduled
  /// like a toast — and combined into one accessibility element so VoiceOver
  /// reads the refusal and its consequence as a single sentence rather than
  /// two orphaned fragments.
  private var coverageRefusedBody: some View {
    HStack(spacing: 12) {
      CSIconView(icon: .warning, size: 18, weight: .regular)
        .foregroundStyle(palette.processingStatus.color)
      VStack(alignment: .leading, spacing: 2) {
        Text(state.coverageRefusalNotice ?? OverlayState.defaultCoverageRefusalNotice)
          .csFont(15, .medium)
          .foregroundStyle(palette.bodyText.color)
          .fixedSize(horizontal: false, vertical: true)
        Text(state.coverageRefusalDetail)
          .csMono(11, .medium)
          .foregroundStyle(palette.mutedText.color)
          .fixedSize(horizontal: false, vertical: true)
      }
      Spacer(minLength: 0)
    }
    .frame(maxWidth: .infinity, minHeight: bodyMinHeight, alignment: .leading)
    .accessibilityElement(children: .combine)
    .accessibilityIdentifier("overlay-coverage-refused")
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
          // "Transcription failed" is only true when there is nothing to show.
          // A take whose words exist but whose handover did not land is a
          // delivery failure, and saying otherwise buries a recoverable
          // transcript under a verdict about the audio.
          Text(
            state.errorMessage
              ?? (state.retainedComposerDelivery != nil
                ? "Delivery interrupted — the transcript is still here"
                : state.activeText.isEmpty ? "Transcription failed" : "Delivery interrupted")
          )
          .csFont(15, .medium)
          .foregroundStyle(palette.bodyText.color)
          .fixedSize(horizontal: false, vertical: true)
          Text(state.errorLifecycleDetail)
            .csMono(11, .medium)
            .foregroundStyle(palette.mutedText.color)
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
}

/// Only reports the hosting window's visibility; it owns no capture state.
private struct OverlayRenderVisibility: NSViewRepresentable {
  let onChange: (Bool) -> Void

  func makeNSView(context: Context) -> VisibilityView {
    let view = VisibilityView()
    view.onChange = onChange
    return view
  }

  func updateNSView(_ nsView: VisibilityView, context: Context) {
    nsView.onChange = onChange
  }

  static func dismantleNSView(_ nsView: VisibilityView, coordinator: ()) {
    NotificationCenter.default.removeObserver(nsView)
    nsView.onChange = nil
  }

  final class VisibilityView: NSView {
    var onChange: ((Bool) -> Void)?

    override func viewDidMoveToWindow() {
      super.viewDidMoveToWindow()
      NotificationCenter.default.removeObserver(self)
      if let window {
        NotificationCenter.default.addObserver(
          self, selector: #selector(visibilityChanged(_:)),
          name: NSWindow.didChangeOcclusionStateNotification, object: window)
      }
      publishVisibility()
    }

    @objc private func visibilityChanged(_ notification: Notification) {
      publishVisibility()
    }

    private func publishVisibility() {
      // Window attachment can happen during a SwiftUI update. Read the current
      // window on the next actor turn, so an old notification cannot revive it.
      Task { @MainActor [weak self] in
        guard let self else { return }
        onChange?(window?.isVisible == true && window?.occlusionState.contains(.visible) == true)
      }
    }
  }
}

private struct OverlayHeaderChrome: ViewModifier {
  let palette: OverlayAppearancePalette

  @ViewBuilder
  func body(content: Content) -> some View {
    if #available(macOS 26.0, *) {
      content.glassEffect(
        .regular,
        in: RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
      )
    } else {
      content.background(
        palette.surfaceTint.color,
        in: RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
      )
    }
  }
}

private struct OverlayCloseCross: Shape {
  func path(in rect: CGRect) -> Path {
    let inset = min(rect.width, rect.height) * 0.28
    var path = Path()
    path.move(to: CGPoint(x: rect.minX + inset, y: rect.minY + inset))
    path.addLine(to: CGPoint(x: rect.maxX - inset, y: rect.maxY - inset))
    path.move(to: CGPoint(x: rect.maxX - inset, y: rect.minY + inset))
    path.addLine(to: CGPoint(x: rect.minX + inset, y: rect.maxY - inset))
    return path
  }
}

private struct OverlayScrollEdgeEffects: ViewModifier {
  @ViewBuilder
  func body(content: Content) -> some View {
    if #available(macOS 26.0, *) {
      content.scrollEdgeEffectStyle(.soft, for: [.top, .bottom])
    } else {
      content
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
  }

  @ViewBuilder
  private func dockPreviewRow(
    _ title: String,
    light: OverlayState,
    dark: OverlayState
  ) -> some View {
    Text(title)
      .font(.headline)
    overlayPreviewCanvas(width: 320, height: 260) {
      DictationOverlayView(state: light)
    }
    .preferredColorScheme(.light)
    overlayPreviewCanvas(width: 320, height: 260) {
      DictationOverlayView(state: dark)
    }
    .preferredColorScheme(.dark)
  }

  #Preview("Dock matrix · 320 pt") {
    ScrollView {
      VStack(spacing: CSSpace.section) {
        dockPreviewRow("Listening", light: .previewListening(), dark: .previewListening())
        dockPreviewRow(
          "Listening · evidence", light: .previewListeningWithEvidence(),
          dark: .previewListeningWithEvidence())
        dockPreviewRow(
          "Finalizing", light: .previewTranscribing(), dark: .previewTranscribing())
        dockPreviewRow("Formatted", light: .previewFormatted(), dark: .previewFormatted())
        dockPreviewRow("No speech", light: .previewNoSpeech(), dark: .previewNoSpeech())
        dockPreviewRow("Error", light: .previewError(), dark: .previewError())
      }
      .padding()
    }
  }

  #Preview("Listening") {
    Group {
      overlayPreviewCanvas {
        DictationOverlayView(state: .previewListening())
      }
      .preferredColorScheme(.light)

      overlayPreviewCanvas {
        DictationOverlayView(state: .previewListening())
      }
      .preferredColorScheme(.dark)
    }
  }

  #Preview("Listening · evidence") {
    // A refused Whisper alternative sits beside the canvas as secondary,
    // non-committed text; the canvas keeps the committed words only.
    overlayPreviewCanvas(width: 360, height: 300) {
      DictationOverlayView(state: .previewListeningWithEvidence())
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
