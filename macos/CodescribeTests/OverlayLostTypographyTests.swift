import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayLostTypographyTests: XCTestCase {
  func testBothRichBodyBranchesAndBothBudgetsReadOneBaseline() throws {
    _ = try OverlayLostBehaviorSource.bodyFontSize()
    let content = try OverlayLostBehaviorSource.section(
      OverlayLostBehaviorSource.overlay("OverlayConversationView.swift"),
      from: "struct OverlayConversationMessageContent: View {",
      to: "struct OverlayConversationView: View {")
    try OverlayLostBehaviorSource.ordered(
      ["OversizedMessageBody(fullText: message.bodyText)", "MarkdownText(",
        "raw: head, size: Self.bodyFontSize", "} else {", "MarkdownText(",
        "raw: message.bodyText, size: Self.bodyFontSize"], in: content)
    let layout = try OverlayLostBehaviorSource.overlay("OverlayIntercomWidgetLayout.swift")
    let preview = try OverlayLostBehaviorSource.section(
      layout, from: "func compactPreviewLayout(", to: "func fittedCardHeight(")
    let minimum = try OverlayLostBehaviorSource.section(
      layout, from: "static func minimumReadableConversationHeight(",
      to: "static func rowCornerRadius(")
    for budget in [preview, minimum] {
      XCTAssertTrue(
        OverlayLostBehaviorSource.compact(budget).contains(
          ".systemFont(ofSize: OverlayConversationMessageContent.bodyFontSize * scale)"))
    }
  }

  func testRenderedRichBodyTracksTheDeclaredBaselineAtEveryScale() throws {
    let size = try OverlayLostBehaviorSource.bodyFontSize()
    let text = "A paragraph with **bold** and `code`.\n\nAnother paragraph wrapping at a narrow width."
    let message = OverlayConversationMessage(
      id: "o2-typography", kind: .user, text: text, order: 1,
      emittedAt: "2026-10-31T12:00:00Z", owner: nil, recipients: [], deliveryID: nil,
      replyTo: nil, unsolicited: false, playback: nil, busPath: "")
    for scale in [CGFloat(1), 1.5, 2] {
      for width in [CGFloat(180), 360] {
        let actual = NSHostingView(
          rootView: OverlayConversationMessageContent(message: message, palette: .dark)
            .environment(\.csTextScale, scale).frame(width: width))
        let oracle = NSHostingView(
          rootView: MarkdownText(raw: text, size: size, bodyColor: OverlayAppearancePalette.dark.primaryText.color)
            .fixedSize(horizontal: false, vertical: true)
            .environment(\.csTextScale, scale).frame(width: width))
        XCTAssertGreaterThan(actual.fittingSize.height, 0)
        XCTAssertEqual(actual.fittingSize.height, oracle.fittingSize.height, accuracy: 1,
          "The painted body must follow a baseline change at width \(width), scale \(scale)")
      }
    }
  }

  func testMinimumReadableHeightTracksTheRichBodyFont() throws {
    let size = try OverlayLostBehaviorSource.bodyFontSize()
    for scale in [CGFloat(1), 1.5, 2] {
      let layout = NSLayoutManager()
      let body = ceil(layout.defaultLineHeight(for: .systemFont(ofSize: size * scale)))
      let metadata = ceil(layout.defaultLineHeight(for: .systemFont(ofSize: 10 * scale)))
      // Two prose lines, two metadata lines, bubble padding and list anchors.
      let expected = 2 * body + 2 * metadata + 10 + 24 + 16 + 12 + 1
      XCTAssertEqual(
        OverlayIntercomWidgetLayout.minimumReadableConversationHeight(scale: scale),
        expected, accuracy: 0.001)
    }
  }

  func testCompactPreviewAdmissionChangesAtTheSharedFontBoundary() throws {
    let size = try OverlayLostBehaviorSource.bodyFontSize()
    for scale in [CGFloat(1), 1.5, 2] {
      let metrics = IntercomContentMetrics(names: ["Astra"], digits: ["3"], scale: scale)
      let layout = NSLayoutManager()
      let body = ceil(layout.defaultLineHeight(for: .systemFont(ofSize: size * scale)))
      let metadata = ceil(layout.defaultLineHeight(for: .systemFont(ofSize: 10 * scale)))
      let width = max(900, metrics.actionsWidth + 100)
      let chrome = OverlayIntercomWidgetLayout.controlHeight(scale: scale)
        + OverlayIntercomWidgetLayout.contextStripHeight(scale: scale)
      let boundary = chrome + body + metadata + 8 + 5
      XCTAssertNil(metrics.compactPreviewLayout(width: width, cardHeight: boundary - 0.5))
      let admitted = try XCTUnwrap(
        metrics.compactPreviewLayout(width: width, cardHeight: boundary))
      XCTAssertEqual(admitted.height, body + metadata + 8 + 5, accuracy: 0.001)
      XCTAssertFalse(admitted.showsComposer, "A single preview line cannot admit the full composer")
    }
  }

  func testMarkdownLinksAndExistingBubbleActionsRemainWired() throws {
    let url = try XCTUnwrap(URL(string: "https://example.com/o2"))
    let attributed = MarkdownText.inlineAttributed(
      "**bold** `code` [report](https://example.com/o2)", fontSize: 10,
      baseFont: .body, baseColor: .primary)
    XCTAssertEqual(attributed.runs.compactMap(\.link), [url])
    XCTAssertTrue(attributed.runs.filter { $0.link != nil }.allSatisfy { $0.underlineStyle != nil })
    XCTAssertTrue(attributed.runs.contains { $0.inlinePresentationIntent?.contains(.stronglyEmphasized) == true })
    XCTAssertTrue(attributed.runs.contains { $0.inlinePresentationIntent?.contains(.code) == true })
    let source = try OverlayLostBehaviorSource.overlay("OverlayConversationView.swift")
    for action in ["CopyMessageButton(", "onReply", "onControl"] {
      XCTAssertTrue(source.contains(action), "Typography must retain \(action)")
    }
    let markdown = try OverlayLostBehaviorSource.read("Screens/AgentChat/ChatComponents.swift")
    XCTAssertTrue(markdown.contains("MarkdownLinkText(attributed: attr, lineSpacing: lineSpacing)"))
  }
}
