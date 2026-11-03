import AppKit
import QuartzCore
import SwiftUI
import XCTest

@testable import Codescribe

/// A draft in flight survives a text-scale change: the letters may rewrap or
/// scroll, but they must stay painted and editable, and a draft that fits one
/// row again after the change must get its single row back.
@MainActor
final class ComposerTextScaleMidDraftTests: XCTestCase {
  @MainActor
  private final class Fixture {
    let host: NSHostingView<AnyView>
    let window: NSWindow
    private(set) var editor: NSTextView!
    private(set) var scroll: NSScrollView!
    private var draftText: String
    private let maximumEditorHeight: CGFloat?

    init(text: String, width: CGFloat = 360, maximumEditorHeight: CGFloat? = nil) throws {
      draftText = text
      self.maximumEditorHeight = maximumEditorHeight
      host = NSHostingView(rootView: AnyView(EmptyView()))
      host.sizingOptions = []
      host.frame = NSRect(x: 0, y: 0, width: width, height: 200)
      window = NSWindow(
        contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      BackgroundTestWindow.present(window)
      apply(scale: 1)
      func find(_ view: NSView) -> NSTextView? {
        if let text = view as? NSTextView, text.isEditable { return text }
        return view.subviews.lazy.compactMap { find($0) }.first
      }
      editor = try XCTUnwrap(find(host))
      scroll = try XCTUnwrap(editor.enclosingScrollView)
    }

    /// Re-renders the SAME composer at a new text scale; the AnyView wrapper
    /// keeps structural identity, so the composer's own @State survives the
    /// way it does when the user moves the app-wide scale slider mid-draft.
    func apply(scale: CGFloat) {
      let draft = Binding(
        get: { [weak self] in self?.draftText ?? "" },
        set: { [weak self] in self?.draftText = $0 })
      host.rootView = AnyView(
        OverlayConversationComposer(
          palette: .dark, draft: draft, maximumEditorHeight: maximumEditorHeight,
          sending: false, onSubmit: { XCTFail("Geometry must never send") }
        )
        .environment(\.csTextScale, scale))
      settle()
    }

    func settle() {
      for _ in 0..<4 {
        host.layoutSubtreeIfNeeded()
        scroll?.layoutSubtreeIfNeeded()
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.005))
      }
    }

    var draft: String { draftText }

    /// The first character's laid-out line fragment in clip coordinates.
    func firstLineRect() throws -> NSRect {
      let manager = try XCTUnwrap(editor.layoutManager)
      let container = try XCTUnwrap(editor.textContainer)
      manager.ensureLayout(for: container)
      let line = manager.lineFragmentRect(forGlyphAt: 0, effectiveRange: nil)
        .offsetBy(dx: editor.textContainerOrigin.x, dy: editor.textContainerOrigin.y)
      return scroll.contentView.convert(line, from: editor)
    }

    /// True when `point` (clip coordinates) is inside the clip and not cut
    /// away by the corner mask, i.e. the user can actually see pixels there.
    func paints(_ point: NSPoint) -> Bool {
      let clip = scroll.contentView
      guard clip.bounds.insetBy(dx: -1, dy: -1).contains(point) else { return false }
      guard let mask = clip.layer?.mask as? CAShapeLayer, let path = mask.path else { return true }
      return path.contains(clip.convertToLayer(point))
    }
  }

  func testScaleUpMidDraftKeepsTheDraftPaintedAndEditable() throws {
    let text = "Jeden wiersz tekstu rośnie razem z literami."
    let fixture = try Fixture(text: text)
    defer { fixture.window.close() }
    XCTAssertTrue(fixture.window.makeFirstResponder(fixture.editor))

    fixture.apply(scale: 1.6)

    XCTAssertEqual(fixture.editor.string, text, "Scaling must never drop draft characters")
    XCTAssertEqual(fixture.editor.font?.pointSize, ComposerControlMetrics.textSize * 1.6)
    let line = try fixture.firstLineRect()
    XCTAssertGreaterThan(line.height, 0, "The first line must still have laid-out glyphs")
    XCTAssertTrue(
      fixture.paints(NSPoint(x: line.minX + 2, y: line.midY)),
      "The draft's first line must stay painted after the letters grow")

    fixture.editor.insertText("!", replacementRange: NSRange(location: 0, length: 0))
    fixture.settle()
    XCTAssertEqual(fixture.draft, "!" + text, "The scaled editor must keep publishing edits")
  }

  func testScaleRoundTripReturnsAShortDraftToTheWritableMiddle() throws {
    let fixture = try Fixture(text: "krótko")
    defer { fixture.window.close() }
    let container = try XCTUnwrap(fixture.editor.textContainer)
    XCTAssertEqual(
      container.exclusionPaths.count, 2,
      "A short draft starts in the writable middle between the corner controls")

    fixture.apply(scale: 1.6)
    fixture.apply(scale: 1)

    XCTAssertEqual(fixture.editor.string, "krótko")
    XCTAssertEqual(
      container.exclusionPaths.count, 2,
      "A draft that fits one row at the restored scale gets its single row back")
    let line = try fixture.firstLineRect()
    XCTAssertTrue(
      fixture.paints(NSPoint(x: line.minX + 2, y: line.midY)),
      "The returned first row must be painted, not masked away")
  }

  func testCompactWidgetScaleUpKeepsTheDraftPainted() throws {
    let fixture = try Fixture(
      text: "Wiersz w kompaktowym widgecie.", maximumEditorHeight: 120)
    defer { fixture.window.close() }

    fixture.apply(scale: 1.6)

    XCTAssertEqual(fixture.editor.string, "Wiersz w kompaktowym widgecie.")
    let line = try fixture.firstLineRect()
    XCTAssertGreaterThan(line.height, 0)
    XCTAssertTrue(
      fixture.paints(NSPoint(x: line.minX + 2, y: line.midY)),
      "The compact composer must keep its draft painted after the letters grow")
  }
}
