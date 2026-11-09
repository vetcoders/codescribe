import AppKit
import SwiftUI

/// Developer-only Lab desk. Hidden unless `CSDeveloperSurface` is baked.
struct LabPanel: View {
  @ObservedObject var model: SettingsViewModel
  @AppStorage("codescribe.lab_mode") private var labMode = false
  /// A drag on the ledger slider persists once, at release.
  @State private var ledgerGate = SliderCommitGate()
  @State private var glassSurface: ChromeGlassSurface = .onboarding
  @State private var topScrollClip: CGFloat = 0
  var glassPreferences = ChromeGlassPreferences()

  var body: some View {
    VStack(alignment: .leading, spacing: 16) {
      SettingsPageHeader(
        SettingsSection.lab.title,
        blurb: labMode
          ? String(
            localized: "Lab mode is on. Overlay follows the tray toggle — Lab does not steal it.")
          : String(
            localized: "Open the loopback Voice Lab. Production builds never show this panel.")
      )

      // The take's word pins lead the desk, open on arrival (Maciej, 2026-10-20:
      // "jest, ale na spodzie zamiast na górze"); the switches follow.
      DictionaryWordPins(configDir: model.configDir)

      glassTuning

      // One column: every switch sits on the trailing edge of its row, so
      // the desk reads as a list and not as a staircase (Founder, 2026-10-20).
      VStack(alignment: .leading, spacing: SettingsScale.rowGap) {
        SettingsControlRow(title: String(localized: "Lab mode")) {
          Toggle("", isOn: $labMode)
            .settingsSwitch()
            .accessibilityLabel("Lab mode")
        }

        sealLaneRow

        transcriptionTracks

        dictationWitnessRow
          .disabled(!model.settings.sealLedgerEnabled || !model.settings.appleLiveEnabled)
      }

      Picker(
        "Whisper buffering",
        selection: Binding(
          get: { model.settings.whisperAdaptiveBuffer },
          set: { model.setWhisperAdaptiveBuffer($0) }
        )
      ) {
        Text("Fixed windows").tag(false)
        Text("Adaptive buffer (experimental)").tag(true)
      }
      Text(
        "Applies to the next recording. Adaptive buffering preserves phrase boundaries within bounded audio and wait limits."
      )
      .font(.caption)
      .foregroundStyle(CSColor.secondaryInk)

      Picker(
        "Text formatting",
        selection: Binding(
          get: { model.settings.formatOnDevice },
          set: { model.setFormatOnDevice($0) }
        )
      ) {
        Text("Cloud provider").tag(false)
        Text("Apple on-device (experimental)").tag(true)
      }
      Text(
        "Applies to Smart and Corrections from the next formatting pass. The on-device model runs first without a cloud credential; on failure the formatting provider is used when one is configured. Max always uses the Agent model. A .env value overrides this toggle."
      )
      .font(.caption)
      .foregroundStyle(CSColor.secondaryInk)

      Button("Open Voice Lab") {
        Task { await VoiceLabRuntime.shared.openConsole() }
      }
      .font(SettingsScale.controlLabel)
      .csLink()

      recognitionParameters

      ledgerSensitivity
    }
    .settingsPage()
    .frame(maxWidth: .infinity, alignment: .leading)
    .clipShape(LabScrollTopClipShape(topInset: topScrollClip))
    .onScrollGeometryChange(for: CGFloat.self) { geometry in
      max(0, geometry.visibleRect.minY + geometry.contentInsets.top)
    } action: { _, clip in
      topScrollClip = clip
    }
    .background(LabScrollViewportClip())
    .task { await model.refreshAdmission() }
  }

  private var glassTuning: some View {
    VStack(alignment: .leading, spacing: SettingsScale.rowGap) {
      SettingsSectionLabel(String(localized: "Glass tuning"))
      Picker("Surface", selection: $glassSurface) {
        ForEach(ChromeGlassSurface.allCases) { surface in
          Text(surface.title).tag(surface)
        }
      }
      .pickerStyle(.menu)
      .accessibilityIdentifier("lab-glass-surface")
      HStack {
        Slider(value: glassPreferences.binding(for: glassSurface), in: 0...1, step: 0.05) {
          Text("Glass tint")
        }
        .accessibilityIdentifier("lab-glass-amount")
        Text(verbatim: "\(Int(glassPreferences.tint(for: glassSurface) * 100))%")
          .monospacedDigit()
        Button("Reset glass") {
          glassPreferences.binding(for: glassSurface).wrappedValue = ChromeGlassTuning.defaultTint
        }
        .accessibilityIdentifier("lab-glass-reset")
      }
      HStack {
        Text("Less tint")
        Spacer()
        Text("More tint")
      }
      .font(SettingsScale.helper)
      .foregroundStyle(CSColor.secondaryInk)
      Text("Developer preview only. Changes appear immediately.")
        .font(SettingsScale.helper)
        .foregroundStyle(CSColor.secondaryInk)
    }
    .settingsGroupedInset()
  }

  /// The committing lane. Settings › Audio states the effective value and
  /// nothing else: turning the recorder's own precondition off is a power-user
  /// act, so the one switch that can do it lives on this desk. An env override
  /// still wins, and the switch then says so instead of lying about authority.
  @ViewBuilder
  private var sealLaneRow: some View {
    let sealLane = sealLaneControlState(model.admission)
    SettingsControlRow(
      title: String(localized: "Committing transcript fragments")
    ) {
      Toggle("", isOn: sealLaneBinding)
        .settingsSwitch()
        .disabled(!sealLane.isEnabled)
        .accessibilityLabel("Committing transcript fragments")
        .accessibilityValue(sealLaneAccessibilityValue(sealLane))
        .accessibilityIdentifier("lab-committing-transcript-fragments")
    }
    Text(sealLane.detail)
      .font(SettingsScale.helper)
      .foregroundStyle(CSColor.secondaryInk)
      .fixedSize(horizontal: false, vertical: true)
  }

  @ViewBuilder
  private var transcriptionTracks: some View {
    SettingsControlRow(title: String(localized: "Seal-ledger admission")) {
      Toggle(
        "",
        isOn: Binding(
          get: { model.settings.sealLedgerEnabled },
          set: { enabled in
            model.setSealLedgerEnabled(enabled)
            Task { await model.refreshAdmission() }
          }
        )
      )
      .settingsSwitch()
      .accessibilityLabel("Seal-ledger admission")
      .accessibilityIdentifier("lab-seal-ledger-enabled")
    }

    SettingsControlRow(
      title: String(localized: "Apple live speech"),
      subtitle: String(
        localized:
          "Applies to the next recording. Turn both off to keep Whisper output without Apple revisions or seal-ledger admission."
      )
    ) {
      Toggle(
        "",
        isOn: Binding(
          get: { model.settings.appleLiveEnabled },
          set: { enabled in
            model.setAppleLiveEnabled(enabled)
            Task { await model.refreshAdmission() }
          }
        )
      )
      .settingsSwitch()
      .accessibilityLabel("Apple live speech")
      .accessibilityIdentifier("lab-apple-live-enabled")
    }
  }

  /// Voice Lab experiment: the DictationTranscriber witness re-checks
  /// unresolved recovery debt after each sealed utterance, in the background.
  /// The effective value comes from the settings snapshot; an exported
  /// process env still overrides the switch.
  private var dictationWitnessRow: some View {
    SettingsControlRow(
      title: String(localized: "DictationTranscriber witness (experimental)"),
      subtitle: String(
        localized:
          "Re-checks words a take left unresolved, in the background after each sealed utterance. A process env value overrides this toggle."
      )
    ) {
      Toggle(
        "",
        isOn: Binding(
          get: { model.settings.dictationWitnessArmed },
          set: { model.setDictationWitnessArmed($0) }
        )
      )
      .settingsSwitch()
      .accessibilityLabel("DictationTranscriber witness (experimental)")
      .accessibilityIdentifier("lab-dictation-witness")
    }
  }

  private var sealLaneBinding: Binding<Bool> {
    Binding(
      get: { sealLaneControlState(model.admission).isOn },
      set: { armed in
        model.setSealLaneArmed(armed)
        Task { await model.refreshAdmission() }
      }
    )
  }

  private func sealLaneAccessibilityValue(_ state: SealLaneControlState) -> String {
    let value =
      state.isOn
      ? String(localized: "On", comment: "Toggle state")
      : String(localized: "Off", comment: "Toggle state")
    return state.isEnabled
      ? value
      : String(
        localized: "\(value), overridden",
        comment: "VoiceOver value: a toggle state frozen by an override")
  }

  /// One slider over live word admission. Left refuses more identical
  /// repeats and words far from speech; right keeps more of both. The centre
  /// is the shipped engine. The take freezes the value when it starts.
  private var ledgerSensitivity: some View {
    VStack(alignment: .leading, spacing: 10) {
      SettingsSectionLabel(
        String(
          localized: "Word admission",
          comment: "Lab group: the ledger sensitivity slider"))

      VStack(alignment: .leading, spacing: 8) {
        HStack(alignment: .firstTextBaseline) {
          Text("Ledger sensitivity")
            .font(SettingsScale.settingName)
          Spacer()
          Text(verbatim: ledgerSensitivityLabel)
            .font(CSFont.ui(11, .semibold))
            .monospacedDigit()
            .foregroundStyle(CSColor.secondaryInk)
        }
        Slider(
          value: Binding(
            get: { ledgerGate.displayed(persisted: model.ledgerSensitivitySlider) },
            set: { newValue in
              if let direct = ledgerGate.valueChanged(newValue) {
                model.ledgerSensitivitySlider = direct
              }
            }
          ),
          in: -100...100,
          step: 5
        ) {
          Text("Ledger sensitivity")
        } onEditingChanged: { editing in
          if let committed = ledgerGate.editingChanged(editing) {
            model.ledgerSensitivitySlider = committed
          }
        }
        .labelsHidden()
        .tint(CSColor.chromeAccent)
        .accessibilityValue(ledgerSensitivityLabel)
        .accessibilityIdentifier("lab-ledger-sensitivity")
        HStack {
          Text("Fewer repeats")
          Spacer()
          Text("Fewer cut words")
        }
        .font(CSFont.ui(11))
        .foregroundStyle(CSColor.secondaryInk)
        HStack(alignment: .firstTextBaseline) {
          Text(
            "Too many repeated words: move it left. Too many cut words: move it right. The centre is the shipped engine. Applies to the next recording."
          )
          .font(.caption)
          .foregroundStyle(CSColor.secondaryInk)
          .fixedSize(horizontal: false, vertical: true)
          Spacer(minLength: 8)
          Button("Centre") {
            model.setLedgerSensitivity(0)
          }
          .buttonStyle(.bordered)
          .controlSize(.small)
          .disabled(model.settings.ledgerSensitivity == 0)
        }
      }
      .settingsGroupedInset()
    }
  }

  /// Signed position; mid-drag it follows the draft, not the persisted value.
  private var ledgerSensitivityLabel: String {
    let position = Int(ledgerGate.displayed(persisted: model.ledgerSensitivitySlider).rounded())
    return position > 0 ? "+\(position)" : "\(position)"
  }

  /// Experimental recognition parameters. The speech-pause limit is a
  /// product control in Agent settings; these engine timings stay in Lab.
  private var recognitionParameters: some View {
    VStack(alignment: .leading, spacing: 10) {
      SettingsSectionLabel(
        String(
          localized: "Speech recognition parameters",
          comment: "Lab group: raw recognition timings"))

      VStack(alignment: .leading, spacing: 14) {
        SettingsSecondsSlider(
          title: String(localized: "Whisper context length"),
          detail: String(
            localized:
              "How much of the recording the local Whisper model sees when it refines the transcript. Refinement runs in Local power and Cloud, never in Apple only."
          ),
          seconds: model.settings.whisperContextWindowSec,
          value: $model.whisperContextWindowSlider,
          range: 0.5...10,
          step: 0.5
        )

        SettingsSecondsSlider(
          title: String(localized: "Sentence pause"),
          detail: String(
            localized:
              "A longer gap in speech may be treated as a sentence boundary during automatic text formatting."
          ),
          seconds: model.settings.lightPlusSentencePauseSec,
          value: $model.lightPlusSentencePauseSlider,
          range: 0.3...2.0,
          step: 0.1
        )

        Text(
          "This build decodes Whisper refinement on a fixed capture-clock window, so the context length is stored but not read."
        )
        .font(CSFont.ui(11))
        .foregroundStyle(CSColor.secondaryInk)
        .fixedSize(horizontal: false, vertical: true)
      }
      .settingsGroupedInset()
    }
  }
}

private struct LabScrollTopClipShape: Shape {
  var topInset: CGFloat

  func path(in rect: CGRect) -> Path {
    let y = min(max(0, topInset), rect.height)
    return Path(
      CGRect(x: rect.minX, y: y, width: rect.width, height: max(0, rect.height - y)))
  }
}

/// Clips the enclosing settings scroll view below the 52 pt titlebar accessory
/// (`window.contentLayoutRect`) so scrolled Lab controls and word pins never
/// bleed behind the transparent header glass.
private struct LabScrollViewportClip: NSViewRepresentable {
  func makeCoordinator() -> Coordinator { Coordinator() }

  func makeNSView(context: Context) -> ClipAttachmentView {
    let view = ClipAttachmentView()
    view.onAttach = { [weak coordinator = context.coordinator] scrollView in
      coordinator?.attach(to: scrollView)
    }
    return view
  }

  func updateNSView(_ nsView: ClipAttachmentView, context: Context) {
    nsView.attachWhenReady()
    context.coordinator.refreshMask()
  }

  static func dismantleNSView(_ nsView: ClipAttachmentView, coordinator: Coordinator) {
    coordinator.detach()
  }

  @MainActor
  final class Coordinator {
    private weak var scrollView: NSScrollView?
    private let maskLayer = CAShapeLayer()
    private var frameObserver: NSObjectProtocol?
    private var windowObserver: NSObjectProtocol?

    func attach(to target: NSScrollView) {
      if scrollView === target {
        refreshMask()
        return
      }
      detach()
      scrollView = target
      target.wantsLayer = true
      target.postsFrameChangedNotifications = true
      frameObserver = NotificationCenter.default.addObserver(
        forName: NSView.frameDidChangeNotification,
        object: target,
        queue: .main
      ) { [weak self] _ in
        MainActor.assumeIsolated { self?.refreshMask() }
      }
      if let window = target.window {
        windowObserver = NotificationCenter.default.addObserver(
          forName: NSWindow.didResizeNotification,
          object: window,
          queue: .main
        ) { [weak self] _ in
          MainActor.assumeIsolated { self?.refreshMask() }
        }
      }
      refreshMask()
    }

    func detach() {
      if let frameObserver { NotificationCenter.default.removeObserver(frameObserver) }
      if let windowObserver { NotificationCenter.default.removeObserver(windowObserver) }
      frameObserver = nil
      windowObserver = nil
      if scrollView?.layer?.mask === maskLayer {
        scrollView?.layer?.mask = nil
      }
      scrollView = nil
    }

    func refreshMask() {
      guard let scrollView, let layer = scrollView.layer else { return }
      let bounds = scrollView.bounds
      guard bounds.width > 0, bounds.height > 0 else { return }
      let layoutTopInset: CGFloat =
        if let window = scrollView.window {
          scrollView.isFlipped
            ? max(0, scrollView.convert(window.contentLayoutRect, from: nil).minY)
            : max(0, bounds.height - scrollView.convert(window.contentLayoutRect, from: nil).maxY)
        } else {
          0
        }
      let topInset = max(scrollView.contentInsets.top, layoutTopInset)
      guard topInset > 0 else {
        if layer.mask === maskLayer { layer.mask = nil }
        return
      }
      let clipHeight = max(0, bounds.height - topInset)
      let clipY = layer.isGeometryFlipped ? bounds.minY + topInset : bounds.minY
      let clipRect = CGRect(x: bounds.minX, y: clipY, width: bounds.width, height: clipHeight)
      maskLayer.frame = bounds
      maskLayer.path = CGPath(rect: clipRect, transform: nil)
      if layer.mask !== maskLayer {
        layer.mask = maskLayer
      }
    }
  }

  @MainActor
  final class ClipAttachmentView: NSView {
    var onAttach: ((NSScrollView) -> Void)?

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func viewDidMoveToWindow() {
      super.viewDidMoveToWindow()
      attachWhenReady()
    }

    func attachWhenReady() {
      Task { @MainActor [weak self] in
        await Task.yield()
        guard let self, let scrollView = enclosingScrollView else { return }
        onAttach?(scrollView)
      }
    }
  }
}
