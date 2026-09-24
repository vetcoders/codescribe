import Foundation

/// Key-path projections for the Dictation sliders and preset picker, so the
/// controls bind as `$model.bufferDelaySlider` instead of rebuilding closure
/// bindings on every body pass. Presentation adapters only: reads come from the
/// same snapshot and every write goes through the existing promoted-key setter.
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
}
