import Foundation

/// Formatter Keywords: preferred vocabulary that Correction, Smart and Max
/// receive as context. The saved list is `settings.formatterKeywords`, read
/// from the one runtime snapshot; `FormatterKeywordsSection` holds only its
/// unsaved draft. Dictionary rules (variant → canonical) and the speech
/// recognizer's vocabulary are separate stores and are not touched here.
extension SettingsViewModel {
  /// Saved, normalized Keywords: the list the next formatting request gets.
  var formatterKeywords: [String] { settings.formatterKeywords }

  /// Replace the saved Keywords with `terms` and return the stored,
  /// normalized list. The bridge write runs off the main actor. The settings
  /// owner refuses an invalid list whole and the error is thrown; the
  /// projection is re-read in both cases, so Settings never shows a list
  /// that is not on disk.
  func saveFormatterKeywords(_ terms: [String]) async throws -> [String] {
    // Without a core (bare preview) nothing can be written: the saved list stands.
    guard let engine else { return formatterKeywords }
    do {
      let stored = try await engine.setFormatterKeywordsAsync(keywords: terms)
      let reloaded = await engine.loadSettingsAsync()
      applyLoadedSettings(reloaded)
      configurationInvalidation?.settingsWritten(by: self)
      return stored
    } catch {
      let reloaded = await engine.loadSettingsAsync()
      applyLoadedSettings(reloaded)
      throw error
    }
  }
}
