import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class ConversationLocalFileLinksTests: XCTestCase {
  private func render(_ text: String) -> AttributedString {
    MarkdownText.inlineAttributed(text, fontSize: 14, baseFont: .body, baseColor: .primary)
  }

  private func links(_ text: AttributedString) -> [(String, URL)] {
    text.runs.compactMap { run in
      run.link.map { (String(text[run.range].characters), $0) }
    }
  }

  func testPlainUnicodePathsKeepCopyTextAndBecomeFileLinks() throws {
    let raw = "Żółć: /tmp/zażółć/raport.md:12:3, potem /Volumes/vc-workspace/wynik.json."
    let text = render(raw)
    XCTAssertEqual(String(text.characters), raw)
    let found = links(text)
    XCTAssertEqual(found.count, 2)
    XCTAssertEqual(found[0].0, "/tmp/zażółć/raport.md:12:3")
    XCTAssertEqual(found[0].1.path, "/tmp/zażółć/raport.md")
    XCTAssertEqual(found[1].1.path, "/Volumes/vc-workspace/wynik.json")
    XCTAssertTrue(found.allSatisfy { $0.1.isFileURL })
  }

  func testInlineCodePathsWithSpacesAndHomeAreOpenable() throws {
    let text = render("`/tmp/My Report (final).md:8` oraz `~/.codescribe/attachments/obraz.png`")
    let found = links(text)
    XCTAssertEqual(found.count, 2)
    XCTAssertEqual(found[0].0, "/tmp/My Report (final).md:8")
    XCTAssertEqual(found[0].1.path, "/tmp/My Report (final).md")
    XCTAssertEqual(found[1].1.path, NSHomeDirectory() + "/.codescribe/attachments/obraz.png")
  }

  func testMarkdownLocalDestinationsResolveSpacesAndLineSuffix() throws {
    let text = render("[Raport](/tmp/My%20Report.md:42) i [WWW](https://example.com/a/b)")
    let found = links(text)
    XCTAssertEqual(found.count, 2)
    XCTAssertEqual(found[0].0, "Raport")
    XCTAssertEqual(found[0].1.path, "/tmp/My Report.md")
    XCTAssertTrue(found[0].1.isFileURL)
    XCTAssertEqual(found[1].1.absoluteString, "https://example.com/a/b")
  }

  func testFileURLAndAttachmentWireLineUseSameOpeningPath() throws {
    let text = render(
      "[attachment] image.png (image/png): /tmp/stored_image.png\nfile:///tmp/My%20Report.pdf")
    let found = links(text)
    XCTAssertEqual(found.count, 2)
    XCTAssertEqual(found[0].1.path, "/tmp/stored_image.png")
    XCTAssertEqual(found[1].1.path, "/tmp/My Report.pdf")
  }

  func testURLPathFragmentsAndRelativeCodeAreNotLocalLinks() {
    let text = render("https://example.com/a/b `core/file.rs` x/y 1/2")
    XCTAssertTrue(links(text).allSatisfy { !$0.1.isFileURL })
    XCTAssertNil(ConversationLocalFileLinks.fileURL(for: "https://example.com/a"))
    XCTAssertNil(ConversationLocalFileLinks.fileURL(for: "file://remote-host/tmp/a"))
    XCTAssertNil(ConversationLocalFileLinks.fileURL(for: "//remote-host/tmp/a"))
    XCTAssertNil(ConversationLocalFileLinks.fileURL(for: "/tmp/a\nmalformed"))
  }

  func testSystemOpeningGetsOneLiteralFileURLWithoutShellExpansion() throws {
    let path = "/tmp/$(touch unwanted) `literal` ; report.png"
    let url = try XCTUnwrap(ConversationLocalFileLinks.fileURL(for: path))
    XCTAssertEqual(url.path, path)
    XCTAssertEqual(ConversationLocalFileLinks.urlForOpening(url), url)
    XCTAssertEqual(
      ConversationLocalFileLinks.urlForOpening(try XCTUnwrap(URL(string: "/tmp/a.md:9"))).path,
      "/tmp/a.md")
  }
}
