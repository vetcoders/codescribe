import Foundation
import Observation

enum OverlayIntercomLayout: String, CaseIterable, Sendable {
  case horizontal
  case vertical
}

/// Application chrome, like AppearancePreference: one persisted choice in the
/// app domain, with no recording, routing or core runtime configuration.
@MainActor
@Observable
final class OverlayChannelMapPreference {
  static let shared = OverlayChannelMapPreference()
  static let key = "codescribe.overlay.channel_map"
  static let layoutKey = "codescribe.overlay.intercom_layout"

  @ObservationIgnored private let defaults: UserDefaults

  init(defaults: UserDefaults = .standard) {
    self.defaults = defaults
  }

  /// Opt-in: an existing installation has no key and keeps its earlier chrome.
  var enabled: Bool {
    access(keyPath: \.enabled)
    return defaults.bool(forKey: Self.key)
  }

  /// The existing strip is the default. Choosing a layout never enables it
  /// implicitly or changes the document, selected agent or recording route.
  var layout: OverlayIntercomLayout {
    access(keyPath: \.layout)
    return defaults.string(forKey: Self.layoutKey).flatMap(OverlayIntercomLayout.init(rawValue:))
      ?? .horizontal
  }

  func selectLayout(_ layout: OverlayIntercomLayout) {
    guard layout != self.layout else { return }
    withMutation(keyPath: \.layout) {
      defaults.set(layout.rawValue, forKey: Self.layoutKey)
    }
  }

  func select(_ enabled: Bool) {
    guard enabled != self.enabled else { return }
    withMutation(keyPath: \.enabled) {
      defaults.set(enabled, forKey: Self.key)
    }
  }
}
