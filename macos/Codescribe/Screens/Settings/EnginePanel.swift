import SwiftUI

// Dictation panel: everything that shapes how speech becomes text. Read-only
// runtime rows show the live STT truth (sourced from the CsSettings snapshot,
// not hardcoded); the permission matrix reflects live status. Editable owners:
// STT/layered engine controls, preview timing, and the hands-free silence
// window — all persisted through the promoted-key config router.

struct EnginePanel: View {
  @ObservedObject var model: SettingsViewModel
  @State private var advancedTimingExpanded = false
  /// Secondary blocks start collapsed — cold open shows runtime + engine only.
  @State private var whisperModelExpanded = false
  @State private var previewTimingExpanded = false
  @State private var silenceExpanded = false
  @State private var permissionsExpanded = false
  @State private var cloudPrivacyExpanded = false

  private let matrixOrder: [PermissionKind] = [
    .microphone, .accessibility, .inputMonitoring, .screenRecording,
    .speechRecognition,
  ]
  private let columns = [
    GridItem(.flexible(), spacing: 8),
    GridItem(.flexible(), spacing: 8),
  ]

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      HStack(spacing: 10) {
        EyebrowLabel(text: "Settings · \(SettingsSection.engine.title)")
        Text("RUNTIME TRUTH · READ-ONLY ROWS")
          .font(CSFont.mono(9, .medium))
          .foregroundStyle(CSColor.textMutedAlt)
          .padding(.horizontal, 8)
          .padding(.vertical, 2)
          .background(
            RoundedRectangle(cornerRadius: 6, style: .continuous)
              .fill(CSColor.surfaceRaised(0.04))
          )
          .overlay(
            RoundedRectangle(cornerRadius: 6, style: .continuous)
              .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
          )
      }

      Text("What's actually running.")
        .font(CSFont.ui(26, .bold))
        .tracking(-0.5)
        .foregroundStyle(CSColor.textHigh)
        .padding(.top, 6)

      runtimeRows
        .padding(.top, 20)
        .onAppear { model.refreshServingStatus() }

      SettingsSectionLabel("Engine controls")
        .padding(.top, 22)
      engineControls
        .padding(.top, 11)

      collapsibleSection(
        title: "Local Whisper model",
        isExpanded: $whisperModelExpanded
      ) {
        whisperDownloadSection
      }

      collapsibleSection(
        title: "Preview timing",
        isExpanded: $previewTimingExpanded
      ) {
        previewTimingSection
      }

      collapsibleSection(
        title: "Hands-free silence",
        isExpanded: $silenceExpanded
      ) {
        silenceSection
      }

      collapsibleSection(
        title: CloudPrivacyCopy.title,
        isExpanded: $cloudPrivacyExpanded
      ) {
        cloudPrivacySection
      }

      collapsibleSection(
        title: "Permission matrix",
        isExpanded: $permissionsExpanded
      ) {
        LazyVGrid(columns: columns, spacing: 8) {
          ForEach(matrixOrder) { kind in
            PermissionMatrixCell(
              kind: kind,
              state: model.permissions.state(kind),
              onStateChanged: { model.refresh() }
            )
          }
        }
      }

      HStack(spacing: 8) {
        Text("●").font(CSFont.mono(11, .medium)).foregroundStyle(CSColor.olive)
        Text("runtime rows reflect the live engine — changes apply on the next recording session")
          .font(CSFont.mono(11, .medium))
          .foregroundStyle(CSColor.textFaint)
      }
      .padding(.top, 16)
    }
    .padding(.horizontal, 28)
    .padding(.vertical, 24)
  }

  /// Section chrome: label is the disclosure chevron host; body mounts only when open.
  private func collapsibleSection<Content: View>(
    title: String,
    isExpanded: Binding<Bool>,
    @ViewBuilder content: @escaping () -> Content
  ) -> some View {
    DisclosureGroup(isExpanded: isExpanded) {
      content()
        .padding(.top, 11)
    } label: {
      SettingsSectionLabel(title)
    }
    .tint(CSColor.chromeAccent)
    .padding(.top, 22)
  }

  // MARK: Runtime key/value rows (STT truth only — LLM truth lives in Providers)

  private var runtimeRows: some View {
    VStack(spacing: 0) {
      RuntimeRow(
        key: "Active STT", value: model.activeSTT,
        tint: true, trailing: .dot(model.sttHealthy ? CSColor.oliveLight : CSColor.amber))
      divider
      RuntimeRow(
        key: "STT model (preference)", value: model.sttModelDescription,
        tint: false, mono: true, trailing: .none)
      divider
      RuntimeRow(
        key: "Whisper language", value: model.whisperLanguageCode,
        tint: true, mono: true, trailing: .none)
    }
    .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
    .overlay(
      RoundedRectangle(cornerRadius: 13, style: .continuous)
        .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
    )
  }

  private var divider: some View {
    Rectangle().fill(CSColor.hairline(0.05)).frame(height: 1)
  }

  // MARK: Engine controls

  /// Selectable engines. "onnx" is deliberately NOT exposed (experimental,
  /// frozen); "auto" defers to the core policy (Apple live when available).
  private static let sttEngineOptions: [(id: String, label: String)] = [
    ("auto", "Auto"),
    ("apple", "Apple (live)"),
    ("whisper", "Whisper (Candle)"),
  ]

  private static let asrModeOptions: [(id: String, label: String)] = [
    ("apple_only", "Apple only"),
    ("local_power", "Local power"),
    ("cloud", "Cloud"),
  ]

  private var engineControls: some View {
    VStack(spacing: 8) {
      if let note = model.sttEngineTruthNote {
        Text(note)
          .font(CSFont.ui(11.5))
          .foregroundStyle(CSColor.amber)
          .frame(maxWidth: .infinity, alignment: .leading)
          .padding(.bottom, 2)
      }
      SettingsControlRow(
        title: "ASR mode",
        subtitle:
          "Apple only = live Apple without Layer 1. Local power = Apple-first with mandatory on-device Whisper refinement. Cloud uses its consent-gated provider, not local Whisper."
      ) {
        Menu {
          ForEach(Self.asrModeOptions, id: \.id) { option in
            Button {
              model.setAsrMode(option.id)
            } label: {
              if option.id == model.asrModeId {
                Label(option.label, systemImage: "checkmark")
              } else {
                Text(option.label)
              }
            }
          }
        } label: {
          SettingsMenuLabel(text: model.asrModeLabel)
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
      }
      SettingsControlRow(
        title: "STT engine",
        subtitle:
          "Auto/Apple = immediate Apple text; Local power continuously repairs it with Whisper. Whisper = direct local engine."
      ) {
        Menu {
          ForEach(Self.sttEngineOptions, id: \.id) { option in
            Button {
              model.setSttEngine(option.id)
            } label: {
              if option.id == model.sttEngineId {
                Label(option.label, systemImage: "checkmark")
              } else {
                Text(option.label)
              }
            }
          }
        } label: {
          SettingsMenuLabel(text: model.sttEngineLabel)
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
      }
      if model.asrModeId == "local_power" {
        SettingsControlRow(
          title: localWhisperRuntimeTitle,
          subtitle: localWhisperRuntimeSubtitle
        ) {
          HStack(spacing: 8) {
            Text(localWhisperRuntimeLabel)
              .font(CSFont.mono(11, .medium))
              .foregroundStyle(localWhisperRuntimeColor)
            if model.localWhisperRuntimeState == .livePatchingConfigurationMismatch {
              Button("Repair") {
                model.repairLocalWhisperLivePatching()
              }
              .buttonStyle(.bordered)
              .controlSize(.small)
            } else {
              Button("Recheck") {
                model.recheckLocalWhisperRuntime()
              }
              .buttonStyle(.bordered)
              .controlSize(.small)
            }
          }
        }
      }
      SettingsControlRow(
        title: "Whole-session final pass",
        subtitle: wholeSessionFinalPassSubtitle(asrModeId: model.asrModeId)
      ) {
        Text("Off")
          .font(CSFont.mono(11, .medium))
          .foregroundStyle(CSColor.textMutedAlt)
      }
    }
  }

  private var localWhisperRuntimeTitle: String {
    switch model.localWhisperRuntimeState {
    case .directEngineReady, .directEngineNotReady:
      return "Local Whisper engine"
    default:
      return "Live Whisper refinement"
    }
  }

  private var localWhisperRuntimeLabel: String {
    switch model.localWhisperRuntimeState {
    case .directEngineReady: return "Ready"
    case .directEngineNotReady, .livePatchingNotReady: return "Not ready"
    case .livePatchingConfigured: return "Required · configured"
    case .livePatchingConfigurationMismatch: return "Degraded"
    case .notSelected: return "Not selected"
    }
  }

  private var localWhisperRuntimeSubtitle: String {
    switch model.localWhisperRuntimeState {
    case .directEngineReady:
      return
        "The validated local FP16 bundle serves transcription directly; no parallel Apple patcher runs."
    case .directEngineNotReady:
      return
        "The local FP16 bundle is missing or invalid. Direct Whisper cannot serve the next take."
    case .livePatchingConfigured:
      return
        "Apple paints immediately; the validated local FP16 model repairs the same audio spans during the take."
    case .livePatchingNotReady:
      return
        "The local FP16 bundle is missing or invalid. Local power is explicitly not ready until validation passes."
    case .livePatchingConfigurationMismatch:
      return
        "Local power requires the runtime phase1 arming token, but persisted readback is disarmed. Repair before the next take."
    case .notSelected:
      return "Local Whisper is not the selected provider."
    }
  }

  private var localWhisperRuntimeColor: Color {
    switch model.localWhisperRuntimeState {
    case .directEngineReady, .livePatchingConfigured:
      return CSColor.oliveLight
    case .directEngineNotReady, .livePatchingNotReady, .livePatchingConfigurationMismatch:
      return CSColor.amber
    case .notSelected:
      return CSColor.textMutedAlt
    }
  }

  // MARK: Local Whisper download (public DMG is slim — model is opt-in)

  private var whisperDownloadSection: some View {
    let status = model.localWhisperStatus
    return VStack(alignment: .leading, spacing: 10) {
      SettingsControlRow(
        title: "Install state",
        subtitle: whisperInstallSubtitle(status)
      ) {
        Text(whisperInstallLabel(status))
          .font(CSFont.mono(11, .medium))
          .foregroundStyle(status.available ? CSColor.oliveLight : CSColor.amber)
      }

      if model.whisperDownloadInFlight {
        VStack(alignment: .leading, spacing: 6) {
          if let fraction = model.whisperDownloadFraction {
            ProgressView(value: fraction)
              .progressViewStyle(.linear)
              .tint(CSColor.chromeAccent)
          } else {
            ProgressView()
              .controlSize(.small)
          }
          Text(model.whisperDownloadDetail ?? "Downloading…")
            .font(CSFont.mono(10.5, .medium))
            .foregroundStyle(CSColor.textFaint)
            .lineLimit(2)
        }
      } else if !status.available {
        SettingsControlRow(
          title: "Download Whisper",
          subtitle: whisperDownloadSubtitle(status)
        ) {
          Button("Download") {
            model.startWhisperDownload()
          }
          .buttonStyle(.borderedProminent)
          .controlSize(.small)
          .tint(CSColor.chromeAccent)
        }
      } else if !status.embedded {
        // On-disk / cache — offer re-check, not re-download spam.
        SettingsControlRow(
          title: "Local path",
          subtitle: status.path ?? status.modelId
        ) {
          Button("Recheck") {
            model.refreshWhisperModelStatus()
          }
          .buttonStyle(.bordered)
          .controlSize(.small)
        }
      } else {
        Text("This build embeds Whisper (fat SKU). Runtime download is not required.")
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(CSColor.textFaint)
      }
    }
  }

  private func whisperInstallLabel(_ status: CsWhisperModelStatus) -> String {
    if status.embedded { return "Embedded" }
    if status.available { return "Installed" }
    return "Not installed"
  }

  private func whisperInstallSubtitle(_ status: CsWhisperModelStatus) -> String {
    if status.embedded {
      return "Baked into this fat build · \(status.modelId)"
    }
    if status.available {
      return "Ready for Whisper engine · \(status.modelId)"
    }
    if model.asrModeId == "local_power" {
      return "Missing or invalid FP16 bundle · Local power is not ready"
    }
    return "Required by the direct Whisper engine or Local power live refinement"
  }

  private func whisperDownloadSubtitle(_ status: CsWhisperModelStatus) -> String {
    if model.asrModeId == "local_power" {
      return
        "Required local FP16 model (\(status.sizeHint)). Missing or invalid weights keep Local power not ready."
    }
    return "Local FP16 model (\(status.sizeHint)) for direct Whisper or Local power refinement."
  }

  // MARK: Preview timing (overlay pacing — writes the existing promoted keys)

  private var previewTimingSection: some View {
    VStack(alignment: .leading, spacing: 10) {
      Picker("Preview timing preset", selection: presetBinding) {
        ForEach(PreviewTimingPreset.allCases) { preset in
          Text(preset.rawValue).tag(preset)
        }
      }
      .pickerStyle(.segmented)
      .labelsHidden()

      Text(previewSummary)
        .font(CSFont.mono(10.5, .medium))
        .foregroundStyle(CSColor.textFaint)

      DisclosureGroup(isExpanded: $advancedTimingExpanded) {
        VStack(spacing: 8) {
          timingSlider(
            title: "Buffer delay",
            value: bufferDelayBinding,
            range: 0...1500,
            step: 1,
            valueLabel: "\(model.previewTimingConfiguration.values.bufferDelayMs) ms"
          )
          timingSlider(
            title: "Typing speed",
            value: typingCpsBinding,
            range: 5...180,
            step: 0.1,
            valueLabel: String(
              format: "%.1f cps",
              model.previewTimingConfiguration.values.typingCps
            )
          )
          timingSlider(
            title: "Words per tick",
            value: emitWordsBinding,
            range: 1...10,
            step: 1,
            valueLabel: "\(model.previewTimingConfiguration.values.emitWordsMax)"
          )
          timingSlider(
            title: "Interim cadence",
            value: interimBinding,
            range: 1...30,
            step: 0.1,
            valueLabel: String(
              format: "%.1f s",
              model.previewTimingConfiguration.values.interimSeconds
            )
          )
        }
        .padding(.top, 10)
      } label: {
        Text("Advanced")
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(CSColor.textBody)
      }
      .tint(CSColor.chromeAccent)
    }
    .padding(15)
    .background(card)
    .overlay(cardBorder)
  }

  private var presetBinding: Binding<PreviewTimingPreset> {
    Binding(
      get: { model.previewTimingPreset },
      set: { preset in
        if preset == .custom {
          advancedTimingExpanded = true
        } else {
          advancedTimingExpanded = false
          model.applyPreviewTimingPreset(preset)
        }
      }
    )
  }

  private var bufferDelayBinding: Binding<Double> {
    Binding(
      get: { Double(model.previewTimingConfiguration.values.bufferDelayMs) },
      set: { model.setPreviewBufferDelayMs(UInt64($0.rounded())) }
    )
  }

  private var typingCpsBinding: Binding<Double> {
    Binding(
      get: { Double(model.previewTimingConfiguration.values.typingCps) },
      set: { model.setPreviewTypingCps(Float($0)) }
    )
  }

  private var emitWordsBinding: Binding<Double> {
    Binding(
      get: { Double(model.previewTimingConfiguration.values.emitWordsMax) },
      set: { model.setPreviewEmitWordsMax(UInt64($0.rounded())) }
    )
  }

  private var interimBinding: Binding<Double> {
    Binding(
      get: { Double(model.previewTimingConfiguration.values.interimSeconds) },
      set: { model.setPreviewInterimSeconds(Float($0)) }
    )
  }

  private var previewSummary: String {
    guard model.previewTimingConfiguration.overlayEnabled else {
      return "Preview off · committed transcripts are unchanged"
    }
    let values = model.previewTimingConfiguration.values
    return String(
      format: "%llu ms · %.1f cps · %llu words · %.1f s interim",
      values.bufferDelayMs,
      values.typingCps,
      values.emitWordsMax,
      values.interimSeconds
    )
  }

  private func timingSlider(
    title: String,
    value: Binding<Double>,
    range: ClosedRange<Double>,
    step: Double,
    valueLabel: String
  ) -> some View {
    VStack(alignment: .leading, spacing: 6) {
      HStack {
        Text(title)
          .font(CSFont.ui(12, .medium))
          .foregroundStyle(CSColor.textMutedAlt)
        Spacer(minLength: 0)
        Text(valueLabel)
          .font(CSFont.mono(10.5, .semibold))
          .foregroundStyle(CSColor.textBody)
      }
      Slider(value: value, in: range, step: step)
        .tint(CSColor.chromeAccent)
    }
  }

  // MARK: Cloud & privacy (C2 — copy pinned by CloudPrivacyCopyTests; the
  // ASR picker above is the clickable grant. Cloud never displays without
  // a granted consent record.)

  private var cloudPrivacySection: some View {
    VStack(alignment: .leading, spacing: 12) {
      ForEach(CloudPrivacyCopy.lines, id: \.self) { line in
        Text(line)
          .font(CSFont.ui(11.5))
          .foregroundStyle(CSColor.textMutedAlt)
          .frame(maxWidth: .infinity, alignment: .leading)
          .fixedSize(horizontal: false, vertical: true)
      }
      Text(
        model.cloudConsentGranted
          ? "Cloud consent is granted. Audio may leave this Mac only in Cloud mode."
          : "Cloud consent is not granted. The picker stays on Apple only."
      )
      .font(CSFont.ui(11.5, .medium))
      .foregroundStyle(model.cloudConsentGranted ? CSColor.oliveLight : CSColor.amber)
      SttEndpointRow(
        current: model.sttEndpoint,
        onSave: { model.setSttEndpoint($0) }
      )
      SettingsUrlRow(
        title: "Gateway session URL",
        keyLabel: "CODESCRIBE_ASR_GATEWAY_URL",
        current: model.asrGatewayUrl,
        placeholder: "https://…/session",
        help:
          "Session-mint endpoint for live Cloud Layer 1. Not the WSS socket. Clearing restores unset.",
        onSave: { model.setAsrGatewayUrl($0) }
      )
    }
    .padding(15)
    .background(card)
    .overlay(cardBorder)
  }

  // MARK: Hands-free silence (Apple engine epoch — TOGGLE_SILENCE_SEC)

  private var silenceSection: some View {
    VStack(alignment: .leading, spacing: 10) {
      HStack {
        VStack(alignment: .leading, spacing: 4) {
          Text("Hands-free silence")
            .font(CSFont.ui(13, .semibold))
            .foregroundStyle(CSColor.textBody)
          Text(
            "Rest the Apple engine after this much silence; the next speech edge wakes a fresh epoch so Whisper can patch the sealed span"
          )
          .font(CSFont.ui(11.5))
          .foregroundStyle(CSColor.textMutedAlt)
        }
        Spacer(minLength: 12)
        Text(String(format: "%.1f s", model.settings.toggleSilenceSec))
          .font(CSFont.mono(11, .semibold))
          .foregroundStyle(CSColor.textBody)
      }
      Slider(value: silenceBinding, in: 0.5...30, step: 0.5)
        .tint(CSColor.chromeAccent)
        .accessibilityLabel("Hands-free silence duration")
        .accessibilityValue(String(format: "%.1f seconds", model.settings.toggleSilenceSec))
    }
    .padding(15)
    .background(card)
    .overlay(cardBorder)
  }

  private var silenceBinding: Binding<Double> {
    Binding(
      get: { Double(model.settings.toggleSilenceSec) },
      set: { model.setToggleSilenceSeconds(Float($0)) }
    )
  }

  private var card: some ShapeStyle {
    CSColor.surfaceRaised(0.025)
  }

  private var cardBorder: some View {
    RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
      .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
  }
}

// MARK: - Permission matrix cell

private struct PermissionMatrixCell: View {
  let kind: PermissionKind
  let state: PermissionState
  /// Re-probe hook fired after an in-app permission request resolves.
  var onStateChanged: (() -> Void)? = nil

  private var granted: Bool { state.isGranted }
  private var accent: Color { granted ? CSColor.olive : CSColor.terracotta }
  private var accentLight: Color { granted ? CSColor.oliveLight : CSColor.terracottaLight }

  var body: some View {
    HStack(spacing: 10) {
      CSIconView(
        icon: granted ? .success : .warning, size: 11, weight: .semibold, color: accentLight)
      Text(kind.rawValue)
        .font(CSFont.ui(12.5, .medium))
        .foregroundStyle(CSColor.textBodyAlt)
        .frame(maxWidth: .infinity, alignment: .leading)
      Text(granted ? "granted" : state.label)
        .font(CSFont.mono(10, .semibold))
        .foregroundStyle(accentLight)
    }
    .padding(.horizontal, 14)
    .padding(.vertical, 11)
    .background(
      RoundedRectangle(cornerRadius: 10, style: .continuous)
        .fill(accent.opacity(0.08))
    )
    .overlay(
      RoundedRectangle(cornerRadius: 10, style: .continuous)
        .strokeBorder(accent.opacity(0.2), lineWidth: 1)
    )
    .contentShape(Rectangle())
    .onTapGesture {
      guard !granted else { return }
      // Speech Recognition can be requested straight from the app while
      // undetermined — the system dialog grants the app's own TCC
      // identity, which the bridge child inherits. Once determined,
      // macOS never re-prompts, so fall through to the deep link.
      // Same grant path as onboarding: in-app dialog while undetermined,
      // System Settings deep-link once macOS will no longer re-prompt.
      if state == .notDetermined, kind.supportsInAppPermissionRequest {
        kind.requestInApp { _ in onStateChanged?() }
      } else {
        kind.openSystemSettings()
      }
    }
  }
}

#if DEBUG
  #Preview("Dictation panel") {
    ScrollView { EnginePanel(model: .preview(.engine)) }
      .frame(width: 720, height: 620)
      .background(SettingsView.windowGradient)
      .preferredColorScheme(.dark)
  }
#endif
