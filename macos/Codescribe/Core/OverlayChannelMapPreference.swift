import Foundation
import Observation

/// Application chrome, like AppearancePreference: one persisted choice in the
/// app domain, with no recording, routing or core runtime configuration.
@MainActor
@Observable
final class OverlayChannelMapPreference {
  static let shared = OverlayChannelMapPreference()
  static let key = "codescribe.overlay.channel_map"

  @ObservationIgnored private let defaults: UserDefaults

  init(defaults: UserDefaults = .standard) {
    self.defaults = defaults
  }

  /// Opt-in: an existing installation has no key and keeps its earlier chrome.
  var enabled: Bool {
    access(keyPath: \.enabled)
    return defaults.bool(forKey: Self.key)
  }

  func select(_ enabled: Bool) {
    guard enabled != self.enabled else { return }
    withMutation(keyPath: \.enabled) {
      defaults.set(enabled, forKey: Self.key)
    }
  }
}
