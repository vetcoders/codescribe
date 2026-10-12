import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class ThreadRailNativeTests: XCTestCase {
  func testNativeSearchFiltersWhileTypingAndEscapeRestoresList() throws {
    let rows = [ChatThread(title: "Atlas", meta: ""), ChatThread(title: "Lena", meta: "")]
    let store = AgentChatStore(engine: nil, threads: rows)
    let host = NSHostingView(rootView: ThreadRail(store: store))
    let window = NSWindow(
      contentRect: NSRect(x: 60, y: 60, width: 300, height: 500),
      styleMask: [.titled], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    window.makeKeyAndOrderFront(nil)
    defer { window.close() }
    settle(host)
    let field = try XCTUnwrap(find(NSSearchField.self, in: host))
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
    editor.doCommand(by: #selector(NSResponder.cancelOperation(_:)))
    settle(host)
    XCTAssertEqual(field.stringValue, "")
    XCTAssertEqual(table.numberOfRows, initialCount)
    XCTAssertTrue(window.firstResponder === editor, "First Escape clears; it keeps search focus")
    editor.doCommand(by: #selector(NSResponder.moveDown(_:)))
    settle(host)
    XCTAssertEqual(store.selectedThreadID, rows[1].id)
    editor.doCommand(by: #selector(NSResponder.moveDown(_:)))
    settle(host)
    XCTAssertEqual(store.selectedThreadID, rows[1].id, "Down at the end must not wrap")
    let request = store.composerFocusRequest
    editor.doCommand(by: #selector(NSResponder.cancelOperation(_:)))
    settle(host)
    XCTAssertNotEqual(store.composerFocusRequest, request, "Second Escape returns to the composer")
  }

  func testNativeListSelectionOpensThreadAndExternalSelectionReturns() throws {
    let rows = [ChatThread(title: "Atlas", meta: ""), ChatThread(title: "Lena", meta: "")]
    let store = AgentChatStore(engine: nil, threads: rows)
    let host = NSHostingView(rootView: ThreadRail(store: store))
    let window = NSWindow(
      contentRect: NSRect(x: 60, y: 60, width: 300, height: 500),
      styleMask: [.titled], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    window.orderFrontRegardless()
    defer { window.close() }
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
