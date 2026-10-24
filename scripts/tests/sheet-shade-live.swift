import AppKit
import Observation
import SwiftUI

/// Standalone palette smoke: no app host, window, activation or live defaults.
/// Compile with AppearancePreference.swift, Tokens.swift, OverlayAppearance.swift
/// and OverlayColorToken.swift. Keep the executable under the central target/.
@main
struct SheetShadeLiveSmoke {
  final class Change: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0

    func record() { lock.withLock { count += 1 } }
    var value: Int { lock.withLock { count } }
  }

  static func main() throws {
    let suite = "codescribe-sheet-shade-smoke-\(UUID().uuidString)"
    let defaults = UserDefaults(suiteName: suite)!
    defer { defaults.removePersistentDomain(forName: suite) }
    let preference = SheetShadePreference(defaults: defaults)
    let changes = Change()

    func observeSheet() -> NSColor {
      withObservationTracking {
        let sheet = CSPalette.sheetColor(preference: preference)
        _ = Color(nsColor: sheet)
        return sheet
      } onChange: {
        changes.record()
      }
    }

    let initial = observeSheet()
    precondition(OverlayColorToken(initial, in: .dark).rgb == SheetShade.todayDark.hex)
    precondition(OverlayColorToken(initial, in: .light).rgb == SheetShade.todayLight.hex)

    preference.select(.warmCarbon)
    precondition(changes.value == 1, "The palette read must invalidate its observer")
    let darkChanged = observeSheet()
    precondition(OverlayColorToken(darkChanged, in: .dark).rgb == SheetShade.warmCarbon.hex)
    precondition(OverlayColorToken(darkChanged, in: .light).rgb == SheetShade.todayLight.hex)

    preference.select(.limestone)
    precondition(changes.value == 2)
    let bothChanged = observeSheet()
    precondition(OverlayColorToken(bothChanged, in: .dark).rgb == SheetShade.warmCarbon.hex)
    precondition(OverlayColorToken(bothChanged, in: .light).rgb == SheetShade.limestone.hex)
    precondition(SheetShadePreference(defaults: defaults).current(dark: true) == .warmCarbon)
    precondition(SheetShadePreference(defaults: defaults).current(dark: false) == .limestone)

    preference.select(.todayDark)
    precondition(changes.value == 3)
    precondition(defaults.object(forKey: SheetShadePreference.darkKey) == nil)
    let restored = observeSheet()
    precondition(OverlayColorToken(restored, in: .dark).rgb == SheetShade.todayDark.hex)
    precondition(OverlayColorToken(restored, in: .light).rgb == SheetShade.limestone.hex)
    preference.select(.todayLight)
    precondition(changes.value == 4)
    precondition(defaults.object(forKey: SheetShadePreference.lightKey) == nil)

    defaults.set(SheetShade.limestone.rawValue, forKey: SheetShadePreference.darkKey)
    precondition(preference.current(dark: true) == .todayDark)
    print(
      "PASS: observed palette invalidation, live light/dark colours, persistence, reset and invalid stored shade; no windows"
    )
  }
}
