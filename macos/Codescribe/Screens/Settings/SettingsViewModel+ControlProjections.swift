import Foundation

/// Key-path projections for the Dictation sliders and preset picker and the
/// tool-permission pickers, so the controls bind as `$model.bufferDelaySlider`
/// instead of rebuilding closure bindings on every body pass. Presentation
/// adapters only: reads come from the same snapshot and every write goes
/// through the existing setter.
extension SettingsViewModel {
  /// "Custom" is a no-op by `applyPreviewTimingPreset`'s own contract: the
  /// sliders are the custom editor, and the preset reads back as Custom once
  /// they leave every named preset.
  var previewPresetPicker: PreviewTimingPreset {
    get { previewTimingPreset }
    set { applyPreviewTimingPreset(newValue) }
  }

  var bufferDelaySlider: Double {
    get { Double(previewTimingConfiguration.values.bufferDelayMs) }
    set { setPreviewBufferDelayMs(UInt64(newValue.rounded())) }
  }

  var typingCpsSlider: Double {
    get { Double(previewTimingConfiguration.values.typingCps) }
    set { setPreviewTypingCps(Float(newValue)) }
  }

  var emitWordsSlider: Double {
    get { Double(previewTimingConfiguration.values.emitWordsMax) }
    set { setPreviewEmitWordsMax(UInt64(newValue.rounded())) }
  }

  var interimSecondsSlider: Double {
    get { Double(previewTimingConfiguration.values.interimSeconds) }
    set { setPreviewInterimSeconds(Float(newValue)) }
  }

  var toggleSilenceSlider: Double {
    get { Double(settings.toggleSilenceSec) }
    set { setToggleSilenceSeconds(Float(newValue)) }
  }

  var whisperContextWindowSlider: Double {
    get { Double(settings.whisperContextWindowSec) }
    set { setWhisperContextWindowSeconds(Float(newValue)) }
  }

  var readOnlyDefaultPicker: String {
    get { permissionPolicy.readOnlyDefault }
    set { setPermissionDefault(kind: .readOnly, level: newValue) }
  }

  var sideEffectDefaultPicker: String {
    get { permissionPolicy.sideEffectDefault }
    set { setPermissionDefault(kind: .sideEffect, level: newValue) }
  }

  var globalDefaultPicker: String {
    get { permissionPolicy.defaultLevel }
    set { setPermissionDefault(kind: .global, level: newValue) }
  }

  /// Per-tool override for one `server:tool` identity, read from the registry
  /// snapshot the rows are built from. An identity the last reload dropped
  /// reads as no selection until its row goes away.
  subscript(toolLevel identity: String) -> String {
    get { toolCapabilities.first { $0.identity == identity }?.effective ?? "" }
    set { setToolPermission(identity: identity, level: newValue) }
  }
}
