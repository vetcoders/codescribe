import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class ThreadRailNativeTests: XCTestCase {
  func testNativeSearchFiltersWhileTypingAndEscapeRestoresList() throws {
    let rows = [ChatThread(title: "Atlas", meta: ""), ChatThread(title: "Lena", meta: "")]
    let store = AgentChatStore(engine: nil, threads: rows)
    let previous = UserDefaults.standard.object(forKey: "AgentChat.sidebarExpanded.v1")
    UserDefaults.standard.set(true, forKey: "AgentChat.sidebarExpanded.v1")
    let (window, host) = mount(store)
    defer {
      window.close()
      restoreSidebar(previous)
    }
    settle(host)
    let windowRoot = try XCTUnwrap(window.contentView?.superview)
    let field = try XCTUnwrap(find(NSSearchField.self, in: windowRoot))
    let table = try XCTUnwrap(find(NSTableView.self, in: host), "System List must own the rows")
    let initialCount = table.numberOfRows
    XCTAssertGreaterThanOrEqual(initialCount, 2)
    XCTAssertTrue(window.makeFirstResponder(field))
    let editor = try XCTUnwrap(field.currentEditor() as? NSTextView)
    editor.insertText("Atlas", replacementRange: NSRange(location: NSNotFound, length: 0))
    settle(host)
    XCTAssertEqual(field.stringValue, "Atlas")
    XCTAssertLessThan(table.numberOfRows, initialCount, "Search must filter before Return")
    XCTAssertGreaterThan(table.numberOfRows, 0)
    try press(53, characters: "\u{1b}", in: window)
    settle(host)
    XCTAssertEqual(field.stringValue, "")
    XCTAssertEqual(table.numberOfRows, initialCount)
    XCTAssertTrue(window.firstResponder === editor, "First Escape clears; it keeps search focus")
    try press(125, characters: "\u{f701}", in: window)
    settle(host)
    XCTAssertEqual(store.selectedThreadID, rows[1].id)
    try press(125, characters: "\u{f701}", in: window)
    settle(host)
    XCTAssertEqual(store.selectedThreadID, rows[1].id, "Down at the end must not wrap")
    let request = store.composerFocusRequest
    try press(53, characters: "\u{1b}", in: window)
    settle(host)
    XCTAssertNotEqual(store.composerFocusRequest, request, "Second Escape returns to the composer")
  }

  func testNativeListSelectionOpensThreadAndExternalSelectionReturns() throws {
    let rows = [ChatThread(title: "Atlas", meta: ""), ChatThread(title: "Lena", meta: "")]
    let store = AgentChatStore(engine: nil, threads: rows)
    let previous = UserDefaults.standard.object(forKey: "AgentChat.sidebarExpanded.v1")
    UserDefaults.standard.set(true, forKey: "AgentChat.sidebarExpanded.v1")
    let (window, host) = mount(store)
    defer {
      window.close()
      restoreSidebar(previous)
    }
    settle(host)
    let table = try XCTUnwrap(find(NSTableView.self, in: host))
    let initialSelection = store.selectedThreadID
    let initialRow = table.selectedRow
    // Section headers cannot be selected. Try each native row until the
    // system's selection callback opens the other actual thread.
    for row in 0..<table.numberOfRows where row != initialRow {
      table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
      settle(host)
      if store.selectedThreadID != initialSelection { break }
    }
    XCTAssertNotEqual(store.selectedThreadID, initialSelection)
    store.select(try XCTUnwrap(initialSelection))
    settle(host)
    XCTAssertEqual(table.selectedRow, initialRow)
  }

  func testSearchKeysLeaveImeAndModifiedNavigationNative() {
    for key in [UInt16(53), 125] {
      XCTAssertEqual(
        ThreadSearchKeyDisposition.resolve(
          keyCode: key, modifiers: [], hasMarkedText: true, queryIsEmpty: false), .native)
      for flag in [NSEvent.ModifierFlags.command, .control, .option, .shift] {
        XCTAssertEqual(
          ThreadSearchKeyDisposition.resolve(
            keyCode: key, modifiers: flag, hasMarkedText: false, queryIsEmpty: false), .native)
      }
    }
    XCTAssertEqual(
      ThreadSearchKeyDisposition.resolve(
        keyCode: 36, modifiers: [], hasMarkedText: false, queryIsEmpty: false), .native)
  }

  private func mount(_ store: AgentChatStore) -> (NSWindow, NSView) {
    let controller = NSHostingController(rootView: AgentChatView(store: store))
    let window = NSWindow(contentViewController: controller)
    window.isReleasedWhenClosed = false
    window.styleMask.formUnion([.titled, .closable, .resizable, .fullSizeContentView])
    window.setContentSize(NSSize(width: 840, height: 600))
    window.makeKeyAndOrderFront(nil)
    return (window, controller.view)
  }

  private func press(_ key: UInt16, characters: String, in window: NSWindow) throws {
    let event = try XCTUnwrap(
      NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
        windowNumber: window.windowNumber, context: nil, characters: characters,
        charactersIgnoringModifiers: characters, isARepeat: false, keyCode: key))
    NSApp.postEvent(event, atStart: true)
    let next = try XCTUnwrap(
      NSApp.nextEvent(
        matching: .keyDown, until: Date().addingTimeInterval(0.1),
        inMode: .default, dequeue: true))
    NSApp.sendEvent(next)
  }

  private func restoreSidebar(_ previous: Any?) {
    if let previous {
      UserDefaults.standard.set(previous, forKey: "AgentChat.sidebarExpanded.v1")
    } else {
      UserDefaults.standard.removeObject(forKey: "AgentChat.sidebarExpanded.v1")
    }
  }

  private func settle(_ host: NSView) {
    let deadline = Date().addingTimeInterval(0.12)
    repeat {
      RunLoop.main.run(until: Date().addingTimeInterval(0.01))
      host.layoutSubtreeIfNeeded()
    } while Date() < deadline
  }

  private func find<T: NSView>(_ type: T.Type, in view: NSView) -> T? {
    if let match = view as? T { return match }
    for child in view.subviews {
      if let match = find(type, in: child) { return match }
    }
    return nil
  }
}
