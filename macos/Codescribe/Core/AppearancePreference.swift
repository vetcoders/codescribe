import AppKit
import Foundation
import Observation

/// Light, dark or follow macOS. The choice is handed to AppKit as the
/// application's appearance, so every window, panel and menu follows it at
/// once; nothing in the app resolves colours any other way.
enum AppAppearanceChoice: String, CaseIterable, Identifiable {
  case system
  case light
  case dark

  var id: String { rawValue }

  /// `nil` hands control back to macOS.
  var nsAppearance: NSAppearance? {
    switch self {
    case .system: return nil
    case .light: return NSAppearance(named: .aqua)
    case .dark: return NSAppearance(named: .darkAqua)
    }
  }

  var title: String {
    switch self {
    case .system: return String(localized: "System", comment: "Appearance choice: follow macOS")
    case .light: return String(localized: "Light", comment: "Appearance choice")
    case .dark: return String(localized: "Dark", comment: "Appearance choice")
    }
  }
}

/// The one owner of the persisted appearance choice. Lives in Codescribe's
/// application domain beside the interface language, never in `settings.json`:
/// it is how this process looks, not how the core behaves.
struct AppearancePreference {
  static let key = "codescribe.appearance"

  private let defaults: UserDefaults

  init(defaults: UserDefaults = .standard) {
    self.defaults = defaults
  }

  var current: AppAppearanceChoice {
    defaults.string(forKey: Self.key).flatMap(AppAppearanceChoice.init(rawValue:)) ?? .system
  }

  func select(_ choice: AppAppearanceChoice) {
    if choice == .system {
      defaults.removeObject(forKey: Self.key)
    } else {
      defaults.set(choice.rawValue, forKey: Self.key)
    }
  }

  /// Hands the saved choice to AppKit. Called at launch, before any window,
  /// and again whenever the choice changes.
  @MainActor
  func apply(to app: NSApplication = .shared) {
    app.appearance = current.nsAppearance
  }
}

/// Candidates for the content sheet (`CSPalette.sheet`), one set per
/// appearance, for the Founder's and Maciej's click-through (2026-10-20). The
/// chosen one becomes the token's value immediately, including in windows
/// that are already open.
enum SheetShade: String, CaseIterable, Identifiable {
  case todayDark = "16161C"
  case softGraphite = "1B1B1D"
  case warmCarbon = "201F1E"
  case nightlyDark = "070707"
  case todayLight = "F6F3EE"
  case limestone = "DED9D3"
  case travertine = "D9D3CD"
  case nightlyLight = "F0F0F0"

  var id: String { rawValue }

  var hex: UInt32 { UInt32(rawValue, radix: 16) ?? 0 }

  var isDark: Bool {
    switch self {
    case .todayDark, .softGraphite, .warmCarbon, .nightlyDark: return true
    case .todayLight, .limestone, .travertine, .nightlyLight: return false
    }
  }

  static let darkChoices: [SheetShade] = [.todayDark, .softGraphite, .warmCarbon, .nightlyDark]
  static let lightChoices: [SheetShade] = [.todayLight, .limestone, .travertine, .nightlyLight]

  var title: String {
    switch self {
    case .todayDark, .todayLight:
      return String(localized: "Today", comment: "Sheet shade: the value shipped today")
    case .softGraphite: return String(localized: "Soft graphite", comment: "Sheet shade, dark")
    case .warmCarbon: return String(localized: "Warm carbon", comment: "Sheet shade, dark")
    case .limestone: return String(localized: "Limestone", comment: "Sheet shade, light")
    case .travertine: return String(localized: "Travertine", comment: "Sheet shade, light")
    case .nightlyDark, .nightlyLight:
      return String(localized: "Nightly", comment: "Sheet shade name, light and dark")
    }
  }
}

/// Persisted sheet choice, one key per appearance, in the application domain.
@Observable
final class SheetShadePreference: @unchecked Sendable {
  // UserDefaults and Observation's registrar are thread-safe. The only state
  // lives in UserDefaults; the computed properties register reads of that state
  // even when reached through a palette token or an NSColor provider.
  static let shared = SheetShadePreference()
  static let darkKey = "codescribe.sheet_shade.dark"
  static let lightKey = "codescribe.sheet_shade.light"

  @ObservationIgnored private let defaults: UserDefaults

  init(defaults: UserDefaults = .standard) {
    self.defaults = defaults
  }

  var darkShade: SheetShade {
    access(keyPath: \.darkShade)
    return stored(dark: true)
  }

  var lightShade: SheetShade {
    access(keyPath: \.lightShade)
    return stored(dark: false)
  }

  func current(dark: Bool) -> SheetShade {
    dark ? darkShade : lightShade
  }

  private func stored(dark: Bool) -> SheetShade {
    let stored = defaults.string(forKey: dark ? Self.darkKey : Self.lightKey)
      .flatMap(SheetShade.init(rawValue:))
    if let stored, stored.isDark == dark { return stored }
    return dark ? .todayDark : .todayLight
  }

  func select(_ shade: SheetShade) {
    let keyPath = shade.isDark ? \SheetShadePreference.darkShade : \.lightShade
    withMutation(keyPath: keyPath) { persist(shade) }
  }

  private func persist(_ shade: SheetShade) {
    let key = shade.isDark ? Self.darkKey : Self.lightKey
    if shade == .todayDark || shade == .todayLight {
      defaults.removeObject(forKey: key)
    } else {
      defaults.set(shade.rawValue, forKey: key)
    }
  }

}
