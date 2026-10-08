import Foundation

/// Interface copy follows the macOS per-app language preference, independently
/// of the language passed to speech recognition.
enum InterfaceLanguage: String, CaseIterable {
  case polish = "pl"
  case english = "en"

  var nativeName: String { self == .polish ? "Polski" : "English" }
  var locale: Locale { Locale(identifier: rawValue) }

  static func preferred(from languages: [String]) -> InterfaceLanguage {
    let supported = [InterfaceLanguage.english, .polish].map(\.rawValue)
    let match = Bundle.preferredLocalizations(from: supported, forPreferences: languages).first
    return match.flatMap(InterfaceLanguage.init(rawValue:)) ?? .english
  }
}

/// The one owner of the persisted interface-language choice. The setup wizard
/// and Settings › Creator both read and write through this value, so the key,
/// the preference domain and the way a choice resolves exist exactly once.
///
/// The choice is the per-app `AppleLanguages` preference macOS itself uses for
/// System Settings › General › Language & Region › Applications. It lives only
/// in Codescribe's application domain, never in `settings.json`, and the
/// running process still shows the language its bundle resolved at launch:
/// `needsRestart(for:)` says whether a saved choice is waiting for a relaunch.
struct InterfaceLanguagePreference {
  static let key = "AppleLanguages"

  private let defaults: UserDefaults
  /// Language the running process resolved its bundle with.
  let processLanguage: InterfaceLanguage
  /// Fallback when the app domain carries no explicit choice: the system order.
  private let preferredLanguages: [String]

  init(
    defaults: UserDefaults = .standard,
    preferredLanguages: [String] = Locale.preferredLanguages,
    processLanguage: InterfaceLanguage = .preferred(from: Bundle.main.preferredLocalizations)
  ) {
    self.defaults = defaults
    self.preferredLanguages = preferredLanguages
    self.processLanguage = processLanguage
  }

  /// The saved choice, or the language macOS would pick when nothing is saved.
  var current: InterfaceLanguage {
    InterfaceLanguage.preferred(
      from: defaults.stringArray(forKey: Self.key) ?? preferredLanguages)
  }

  func select(_ language: InterfaceLanguage) {
    defaults.set([language.rawValue], forKey: Self.key)
  }

  func needsRestart(for language: InterfaceLanguage) -> Bool { language != processLanguage }

  /// Flush the per-app preference before the new process resolves its bundle.
  func flush() throws {
    guard defaults.synchronize() else { throw InterfaceLanguageRestartError.unavailable }
  }
}
