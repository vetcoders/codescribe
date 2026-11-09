import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Follow-up rules of the compact Context card (x9eval integration,
/// 2026-11-09): Escape around text composition and chords, outside clicks
/// beside a scrolled card, the known message count, focus visibility and the
/// closed card leaving the hit and accessibility trees.
@MainActor
final class AgentContextCardFollowUpTests: XCTestCase {
  /// Escape that ends an input-method composition belongs to the field; the
  /// card stays open and the Escape passes. The next plain Escape is the card's.
  func testEscapeWhileComposingTextLeavesTheCardToTheField() throws {
    let card = AgentContextCardPresenter()
    let (window, root) = mountLayer(card: card, size: NSSize(width: 640, height: 480))
    defer { window.close() }
    let field = NSTextView(frame: NSRect(x: 8, y: 8, width: 200, height: 40))
    root.addSubview(field)
    XCTAssertTrue(window.makeFirstResponder(field))
    card.open()
    pump(window) { self.anchor(in: root) != nil }

    field.setMarkedText(
      "e", selectedRange: NSRange(location: 1, length: 0),
      replacementRange: NSRange(location: NSNotFound, length: 0))
    XCTAssertTrue(field.hasMarkedText())
    let number = window.windowNumber
    XCTAssertTrue(card.handle(.init(windowNumber: number, kind: .escape)), "Escape passes")
    XCTAssertTrue(card.isOpen, "a composing field keeps its Escape")

    field.unmarkText()
    XCTAssertFalse(card.handle(.init(windowNumber: number, kind: .escape)))
    XCTAssertFalse(card.isOpen, "plain Escape closes the card")
  }

  /// Only plain Escape (Shift aside) is the card's; a chord passes through.
  func testOnlyPlainEscapeDismisses() throws {
    func kind(_ flags: NSEvent.ModifierFlags) throws -> AgentContextCardEvent.Kind {
      let event = try XCTUnwrap(
        NSEvent.keyEvent(
          with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0,
          windowNumber: 7, context: nil, characters: "\u{1b}",
          charactersIgnoringModifiers: "\u{1b}", isARepeat: false, keyCode: 53))
      return AgentContextCardEvent(event).kind
    }
    XCTAssertEqual(try kind([]), .escape)
    XCTAssertEqual(try kind(.shift), .escape)
    XCTAssertEqual(try kind(.command), .other)
    XCTAssertEqual(try kind(.option), .other)
    XCTAssertEqual(try kind(.control), .other)
  }

  /// On a short window the card scrolls inside itself. The outside-click rule
  /// measures the visible viewport, so a click on the conversation below it
  /// closes the card even where clipped card content would lie.
  func testClickBelowAScrolledCardIsOutsideIt() throws {
    let card = AgentContextCardPresenter()
    let height: CGFloat = 150
    let store = storeWithHistory()
    let (window, root) = mountLayer(
      card: card, store: store, size: NSSize(width: 640, height: height))
    defer { window.close() }
    card.open()
    pump(window) { self.anchor(in: root) != nil }
    let anchorView = try XCTUnwrap(anchor(in: root))
    let visible = anchorView.convert(anchorView.bounds, to: nil)
    let maxViewport = height - 2 * AgentContextCard.inset
    XCTAssertLessThanOrEqual(
      visible.height, maxViewport + 0.5, "the anchor is the viewport, not the scrolled content")
    XCTAssertGreaterThan(visible.height, 20, "the card is mounted with a real viewport")

    // Inside the viewport keeps the card; just below it (inside the window,
    // where the unclipped content would continue) closes it.
    let number = window.windowNumber
    XCTAssertTrue(
      card.handle(.init(windowNumber: number, kind: .click(NSPoint(x: visible.midX, y: visible.midY))))
    )
    XCTAssertTrue(card.isOpen)
    let below = NSPoint(x: visible.midX, y: max(1, visible.minY - 4))
    XCTAssertTrue(card.handle(.init(windowNumber: number, kind: .click(below))))
    XCTAssertFalse(card.isOpen, "a click beside the visible card closes it and passes through")
  }

  /// A count is shown when it is known, zero included; a persisted thread
  /// whose history the store has not read yet has no known count.
  func testMessageCountIsShownOnlyWhenKnown() {
    var fresh = ChatThread(title: "New", meta: "", messages: [], model: "m")
    XCTAssertNil(fresh.backendId)
    XCTAssertEqual(AgentContextCard.knownMessageCount(of: fresh), 0, "a new thread has none yet")

    fresh.backendId = "t_persisted"
    XCTAssertNil(
      AgentContextCard.knownMessageCount(of: fresh), "unloaded history is not a zero")
    fresh.messagesLoaded = true
    XCTAssertEqual(AgentContextCard.knownMessageCount(of: fresh), 0)

    let messages = [ChatMessage(role: .you, timestamp: nil, text: "Hi")]
    let loaded = ChatThread(title: "T", meta: "", messages: messages, model: "m")
    XCTAssertEqual(AgentContextCard.knownMessageCount(of: loaded), 1)
  }

  /// A pointer close or a hide never moves focus; nothing asks the Context
  /// button to take it back.
  func testPointerAndHideClosesDoNotMoveFocus() throws {
    let card = AgentContextCardPresenter()
    let (window, root) = mountLayer(card: card, size: NSSize(width: 640, height: 480))
    defer { window.close() }
    card.open()
    pump(window) { self.anchor(in: root) != nil }
    card.cardHoldsKeyboardFocus = true
    XCTAssertTrue(
      card.handle(.init(windowNumber: window.windowNumber, kind: .click(NSPoint(x: 4, y: 4)))))
    XCTAssertFalse(card.isOpen)
    XCTAssertEqual(card.keyboardFocusReturn, 0)
    XCTAssertEqual(card.accessibilityFocusReturn, 0)

    card.open()
    card.close()
    XCTAssertEqual(card.keyboardFocusReturn, 0, "the button's own close keeps focus where it is")
  }

  /// The Context button's focus outline comes from the ring style itself: a
  /// `.borderless` style closer to the button would replace it while
  /// `focusEffectDisabled` removes the native ring, leaving no visible focus.
  func testContextButtonKeepsAVisibleFocusRing() throws {
    let source = try productionSource("Screens/AgentChat/AgentContextPanel.swift")
    let start = try XCTUnwrap(source.range(of: "struct AgentContextToggle: View {"))
    let end = try XCTUnwrap(source.range(of: "// MARK: - Context card", range: start.upperBound..<source.endIndex))
    let toggle = String(source[start.lowerBound..<end.lowerBound])
    XCTAssertTrue(toggle.contains(".csFocusRing(cornerRadius: CSRadius.window)"))
    XCTAssertFalse(toggle.contains(".buttonStyle(.borderless)"), "no style shadows the ring")
  }

  /// A closing card leaves hit testing and the accessibility tree at once.
  func testClosedLayerTakesNoHitsAndNoAccessibility() throws {
    let source = try productionSource("Screens/AgentChat/AgentContextPanel.swift")
    XCTAssertTrue(source.contains(".allowsHitTesting(presenter.isOpen)"))
    XCTAssertTrue(source.contains(".accessibilityHidden(!presenter.isOpen)"))
    XCTAssertTrue(source.contains(".accessibilityAction(.escape) { presenter.close(.accessibility) }"))
  }

  // MARK: - Fixtures

  private func storeWithHistory() -> AgentChatStore {
    let messages = (0..<6).map {
      ChatMessage(role: $0.isMultiple(of: 2) ? .you : .assistant, timestamp: nil, text: "m\($0)")
    }
    var thread = ChatThread(title: "History", meta: "", messages: messages, model: "m")
    thread.updatedAt = Date(timeIntervalSince1970: 1_794_200_000)
    return AgentChatStore(engine: nil, threads: [thread])
  }

  private func mountLayer(
    card: AgentContextCardPresenter, store: AgentChatStore? = nil, size: NSSize
  ) -> (NSWindow, NSView) {
    let store = store ?? AgentChatStore(engine: nil)
    let window = NSWindow(
      contentRect: NSRect(origin: NSPoint(x: 80, y: 80), size: size),
      styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    let host = NSHostingView(
      rootView: Color.clear
        .overlay { AgentContextCardLayer(store: store, presenter: card, openToolSettings: {}) }
        .frame(width: size.width, height: size.height))
    host.frame = NSRect(origin: .zero, size: size)
    // A plain container, so a test can add an AppKit field beside the layer.
    let container = NSView(frame: NSRect(origin: .zero, size: size))
    container.addSubview(host)
    window.contentView = container
    BackgroundTestWindow.present(window)
    return (window, container)
  }

  private func pump(_ window: NSWindow, until predicate: () -> Bool) {
    let deadline = Date().addingTimeInterval(2)
    while Date() < deadline {
      RunLoop.current.run(until: Date().addingTimeInterval(0.02))
      window.displayIfNeeded()
      if predicate() { return }
    }
  }

  private func anchor(in root: NSView) -> NSView? {
    if root is AgentContextHitAnchor.AnchorView { return root }
    for child in root.subviews {
      if let found = anchor(in: child) { return found }
    }
    return nil
  }

  private func productionSource(_ relative: String) throws -> String {
    try String(
      contentsOf: URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("Codescribe").appendingPathComponent(relative),
      encoding: .utf8)
  }
}
