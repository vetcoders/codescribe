import CoreText
import SwiftUI

// Registers the bundled OFL font at runtime so it works in the app AND in
// SwiftUI Previews regardless of bundle layout. Idempotent; safe to call often.
// Text uses the system face; JetBrains Mono is the only bundled family.
@MainActor
enum FontLoader {
  nonisolated static let jetBrainsMono = "JetBrains Mono"

  private static var didRegister = false

  static func register() {
    guard !didRegister else { return }
    didRegister = true
    for name in ["JetBrainsMono"] {
      guard
        let url = Bundle.main.url(forResource: name, withExtension: "ttf")
          ?? Bundle.main.url(forResource: name, withExtension: "ttf", subdirectory: "Fonts")
      else {
        #if DEBUG
          print("[codescribe] font not found in bundle: \(name).ttf")
        #endif
        continue
      }
      var err: Unmanaged<CFError>?
      if !CTFontManagerRegisterFontsForURL(url as CFURL, .process, &err) {
        #if DEBUG
          print("[codescribe] font register skipped (likely already loaded): \(name)")
        #endif
      }
    }
  }
}
