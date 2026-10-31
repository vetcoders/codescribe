import AppKit
import XCTest

@testable import Codescribe

/// No window ordering, global swizzling, physical pointer movement or general clipboard.
@MainActor
enum OverlayLostCopySupport {
  final class KeyPanel: NSPanel {
    var keyFocus = true
    override var isKeyWindow: Bool { keyFocus }
  }

  final class Editor: NSTextView {
    var copiedSelections: [String] = []
    override func copy(_ sender: Any?) {
      copiedSelections.append((string as NSString).substring(with: selectedRange()))
    }
  }

  /// Prevent NSTextView/AppKit drag tracking from waiting for a real mouse-up.
  final class ClickSink: NSView {
    var mouseDowns = 0
    override func mouseDown(with event: NSEvent) { mouseDowns += 1 }
  }

  static func panel() -> KeyPanel {
    let panel = KeyPanel(
      contentRect: NSRect(x: -10000, y: -10000, width: 320, height: 200),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    return panel
  }

  static func commandC(in panel: NSPanel) throws -> NSEvent {
    try XCTUnwrap(NSEvent.keyEvent(
      with: .keyDown, location: .zero, modifierFlags: .command, timestamp: 0,
      windowNumber: panel.windowNumber, context: nil, characters: "c",
      charactersIgnoringModifiers: "c", isARepeat: false, keyCode: 8))
  }

  static func field<Value>(_ name: String, in object: Any, as type: Value.Type) -> Value? {
    var mirror: Mirror? = Mirror(reflecting: object)
    while let current = mirror {
      if let value = current.children.first(where: { $0.label == name })?.value as? Value {
        return value
      }
      mirror = current.superclassMirror
    }
    return nil
  }

  /// Reflection lets the same test compile on the generation without the
  /// Intercom registry. A missing registry is an ordinary assertion failure.
  static func registry(in panel: NSPanel) throws -> NSHashTable<NSView> {
    try XCTUnwrap(field("selectableTextRegions", in: panel, as: NSHashTable<NSView>.self),
      "Both panel types must own a weak registry of their mounted bubble regions")
  }

  static func descendants<View: NSView>(_ type: View.Type, in root: NSView) -> [View] {
    (root as? View).map { [$0] } ?? root.subviews.flatMap { descendants(type, in: $0) }
  }
}
