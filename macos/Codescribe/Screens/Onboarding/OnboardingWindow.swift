import AppKit
import SwiftUI

// First-run wizard window host. Codescribe is an LSUIElement accessory, so the
// wizard is a standalone AppKit window (not a SwiftUI `Settings`/`WindowGroup`
// scene) that App.swift presents at launch when `shouldShowOnboarding()` is true.
//
// Single-instance note: the flock(2) onboarding session lock from the excised
// AppKit wizard (git 37efe51^:app/ui/onboarding/session.rs) is deliberately NOT
// re-implemented. AppDelegate already enforces one live instance at launch
// (`isDuplicateInstance` → terminate in applicationWillFinishLaunching), so a
// second concurrent wizard cannot exist and the advisory lock is redundant here.

@MainActor
final class OnboardingWindowController {
  private var window: NSWindow?
  private let engine: OnboardingEngine
  private let applyInterfaceLanguage: (@escaping @MainActor () -> Void) async throws -> Void

  init(
    engine: OnboardingEngine,
    applyInterfaceLanguage: @escaping (@escaping @MainActor () -> Void) async throws -> Void
  ) {
    self.engine = engine
    self.applyInterfaceLanguage = applyInterfaceLanguage
  }

  /// Present the wizard only when the live gate says onboarding is due.
  func presentIfNeeded() {
    guard engine.shouldShowOnboarding() else { return }
    present()
  }

  /// Build (once) and front the wizard window. Idempotent — a second call just
  /// re-fronts the existing window.
  func present() {
    if window == nil {
      let model = OnboardingViewModel(engine: engine)
      model.onApplyInterfaceLanguage = applyInterfaceLanguage
      model.onFinished = { [weak self] in self?.close() }
      let hosting = NSHostingController(rootView: OnboardingView(model: model))
      let window = NSWindow(contentViewController: hosting)
      window.title = model.windowTitle
      window.setContentSize(NSSize(width: 720, height: 620))
      window.styleMask = [.titled, .closable, .fullSizeContentView]
      window.titlebarAppearsTransparent = true
      window.isOpaque = false
      window.backgroundColor = .clear
      window.isReleasedWhenClosed = false
      window.center()
      self.window = window
    }
    NSApp.activate(ignoringOtherApps: true)
    window?.makeKeyAndOrderFront(nil)
  }

  private func close() {
    window?.close()
    window = nil
  }
}

/// Only the noninteractive chapter header owns window dragging. The scrollable
/// step and footer retain their normal selection and control hit testing.
struct OnboardingDragRegion: NSViewRepresentable {
  func makeNSView(context: Context) -> OnboardingDragView {
    let view = OnboardingDragView()
    view.setAccessibilityIdentifier("onboarding-window-drag-region")
    view.setAccessibilityElement(false)
    return view
  }

  func updateNSView(_ nsView: OnboardingDragView, context: Context) {}
}

final class OnboardingDragView: NSView {
  override var isOpaque: Bool { false }
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
  override func mouseDown(with event: NSEvent) {
    window?.performDrag(with: event)
  }
}
