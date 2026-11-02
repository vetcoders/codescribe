import Foundation
import Observation

enum OverlayIntercomLayout: String, CaseIterable, Sendable {
  case horizontal
  case vertical
}

/// Settings presents one three-way choice (Maciej, 2026-10-22): the classic
/// overlay without a channel map is a peer of the two mapped layouts, not a
/// separate toggle hiding the picker. The stored pair stays `enabled` +
/// `layout`, so classic remembers the previously chosen layout.
enum OverlayIntercomPresentationMode: String, CaseIterable, Sendable {
  case classic
  case horizontal
  case vertical

  init(enabled: Bool, layout: OverlayIntercomLayout) {
    self = enabled ? (layout == .horizontal ? .horizontal : .vertical) : .classic
  }

  var layout: OverlayIntercomLayout? {
    switch self {
    case .classic: nil
    case .horizontal: .horizontal
    case .vertical: .vertical
    }
  }
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
  /// Standard builds unconditionally evaluate to false (Classic mode), ensuring
  /// a saved experimental choice from a nightly build cannot reactivate beta UI.
  var enabled: Bool {
    access(keyPath: \.enabled)
    #if CHANNEL_nightly || CHANNEL_NIGHTLY
    return defaults.bool(forKey: Self.key)
    #else
    return false
    #endif
  }

  /// The existing strip is the default. Choosing a layout never enables it
  /// implicitly or changes the document, selected agent or recording route.
  var layout: OverlayIntercomLayout {
    access(keyPath: \.layout)
    return defaults.string(forKey: Self.layoutKey).flatMap(OverlayIntercomLayout.init(rawValue:))
      ?? .horizontal
  }

  func selectLayout(_ layout: OverlayIntercomLayout) {
    #if CHANNEL_nightly || CHANNEL_NIGHTLY
    guard layout != self.layout else { return }
    withMutation(keyPath: \.layout) {
      defaults.set(layout.rawValue, forKey: Self.layoutKey)
    }
    #endif
  }

  func select(_ enabled: Bool) {
    #if CHANNEL_nightly || CHANNEL_NIGHTLY
    guard enabled != self.enabled else { return }
    withMutation(keyPath: \.enabled) {
      defaults.set(enabled, forKey: Self.key)
    }
    #endif
  }

  /// The one Settings control. Classic only disables the map; the stored
  /// layout survives, so returning to a mapped mode restores the last one.
  var mode: OverlayIntercomPresentationMode {
    #if CHANNEL_nightly || CHANNEL_NIGHTLY
    OverlayIntercomPresentationMode(enabled: enabled, layout: layout)
    #else
    .classic
    #endif
  }

  func selectMode(_ mode: OverlayIntercomPresentationMode) {
    #if CHANNEL_nightly || CHANNEL_NIGHTLY
    if let layout = mode.layout { selectLayout(layout) }
    select(mode != .classic)
    #endif
  }
}
