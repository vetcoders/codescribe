import AppKit
import SwiftUI
import Vision
import XCTest

@testable import Codescribe

@MainActor
final class OverlayIntercomMetadataTests: XCTestCase {
  override class func setUp() {
    super.setUp()
    // Vision's first request loads Apple's OCR model. Keep that fixture
    // initialization outside the per-scenario render budget; the suite still
    // includes its cost and every capture retains the same OCR assertions.
    do {
      let context = try XCTUnwrap(
        CGContext(
          data: nil, width: 32, height: 32, bitsPerComponent: 8, bytesPerRow: 0,
          space: CGColorSpaceCreateDeviceRGB(),
          bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
      let image = try XCTUnwrap(context.makeImage())
      let request = VNRecognizeTextRequest()
      request.recognitionLevel = .accurate
      request.usesLanguageCorrection = false
      try VNImageRequestHandler(cgImage: image).perform([request])
    } catch {
      XCTFail("Vision OCR fixture initialization failed: \(error)")
    }
  }

  func testOneMetadataLineUnlocksContextAndReturnsSpaceToPreview() throws {
    for scale in [CGFloat(0.8), 1, 1.6] {
      let metrics = IntercomContentMetrics(names: ["Agent"], digits: ["3"], scale: scale)
      let width = max(metrics.actionsWidth, 500 * scale)
      let header = OverlayIntercomWidgetLayout.controlHeight(scale: scale)
      let metadata = ceil(NSLayoutManager().defaultLineHeight(for: .systemFont(ofSize: 10 * scale)))
      let body = ceil(NSLayoutManager().defaultLineHeight(for: .systemFont(ofSize: 13 * scale)))
      let contextHeight = header + metadata + 6
      XCTAssertEqual(metrics.cardContent(width: width, cardHeight: contextHeight - 0.5), .header)
      XCTAssertEqual(metrics.cardContent(width: width, cardHeight: contextHeight), .context)
      // The lightweight preview needs one prose line and its route. Input
      // remains a later tier with the complete message renderer's own budget.
      let previewHeight =
        body + metadata
        + 2 * OverlayIntercomWidgetLayout.compactPreviewVerticalPadding
        + OverlayIntercomWidgetLayout.compactPreviewSpacing
      let firstPreviewHeight = contextHeight + previewHeight
      XCTAssertNil(metrics.compactPreviewLayout(width: width, cardHeight: firstPreviewHeight - 0.5))
      let preview = try XCTUnwrap(
        metrics.compactPreviewLayout(width: width, cardHeight: firstPreviewHeight))
      XCTAssertEqual(preview.height, previewHeight, accuracy: 0.01)
      XCTAssertFalse(preview.showsComposer, "The first readable message must precede its input")
      XCTAssertEqual(metrics.cardContent(width: width, cardHeight: firstPreviewHeight), .preview)
    }
  }

  func testRenderedMetadataIsOneLineWithoutExecutionCoordinates() throws {
    try renderMetadata(scale: 1, scenario: "known")
  }

  // Each native OCR capture keeps its own ten-second budget and failure
  // attribution. The same twelve cases previously shared one test budget.
  func testKnownMetadataAtSmallScale() throws {
    try renderMetadata(scale: 0.8, scenario: "known")
  }

  func testKnownMetadataAtLargeScale() throws {
    try renderMetadata(scale: 1.6, scenario: "known")
  }

  func testUnknownMetadataAtSmallScale() throws {
    try renderMetadata(scale: 0.8, scenario: "unknown")
  }

  func testUnknownMetadataAtNormalScale() throws {
    try renderMetadata(scale: 1, scenario: "unknown")
  }

  func testUnknownMetadataAtLargeScale() throws {
    try renderMetadata(scale: 1.6, scenario: "unknown")
  }

  func testLongMetadataAtSmallScale() throws {
    try renderMetadata(scale: 0.8, scenario: "long")
  }

  func testLongMetadataAtNormalScale() throws {
    try renderMetadata(scale: 1, scenario: "long")
  }

  func testLongMetadataAtLargeScale() throws {
    try renderMetadata(scale: 1.6, scenario: "long")
  }

  func testForeignLeaseMetadataAtSmallScale() throws {
    try renderMetadata(scale: 0.8, scenario: "foreign-lease")
  }

  func testForeignLeaseMetadataAtNormalScale() throws {
    try renderMetadata(scale: 1, scenario: "foreign-lease")
  }

  func testForeignLeaseMetadataAtLargeScale() throws {
    try renderMetadata(scale: 1.6, scenario: "foreign-lease")
  }

  private func renderMetadata(scale: CGFloat, scenario: String) throws {
    let context = "scenario=\(scenario), scale=\(scale)"
    let identity = AgentPlaybackIdentity(
      provider: "codex", session: "metadata", bus: "/fixture/bus")
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": identity.session,
        "lease_id": identity.leaseID, "channel": "3", "name": "Agent",
      ]))
    let conversation = OverlayConversation(
      id: owner.id, channel: "3", name: "Agent", owner: owner, messages: [])
    let state = OverlayState()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [conversation]))
    let foreign = AgentPlaybackIdentity(provider: "codex", session: "replacement", bus: "/other")
    let model =
      scenario == "unknown"
      ? nil
      : scenario == "long"
        ? String(repeating: "very-long-model-name-", count: 12) : "gpt-6.1-sol"
    let binding = AgentPlaybackBinding(
      identity: scenario == "foreign-lease" ? foreign : identity, model: model,
      repositoryName: scenario == "unknown" ? nil : "codescribe", workspace: "/Users/private-home",
      frame: .init(session: "location-must-not-be-visible", paneID: 88))
    state.applyBoundAgents(["3": binding])
    if scenario == "foreign-lease" {
      XCTAssertNil(state.conversationAgentDescriptor(conversation))
    } else {
      XCTAssertEqual(state.conversationAgentDescriptor(conversation), binding.descriptor)
    }
    let generation = state.captureGeneration
    state.conversationDrafts[conversation.id] = "Preserve draft"
    state.publishConversationText = { _, _, _, _ in XCTFail("Rendering must not publish") }
    let suite = "CodescribeTests.metadata.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let controller = OverlayIntercomWidgetController(
      state: state, textScale: TextScaleController(key: suite),
      desktopStore: OverlayIntercomDesktopStore(defaults: defaults))
    defer { controller.invalidate() }
    let width = (scenario == "long" ? CGFloat(310) : 650) * scale
    let line = ceil(NSLayoutManager().defaultLineHeight(for: .systemFont(ofSize: 10 * scale)))
    let height = OverlayIntercomWidgetLayout.controlHeight(scale: scale) + 2 * line + 9
    let host = NSHostingView(
      rootView: controller.offscreenCard(
        .init(digit: "3", conversation: conversation), width: width, height: height, scale: scale
      )
      .environment(\.csTextScale, scale)
      .environment(\.colorScheme, .dark)
      .symbolEffectsRemoved()
      // OCR measures the settled metadata, not the card's spring entrance.
      .transaction {
        $0.disablesAnimations = true
        $0.animation = nil
      }
      .background(OverlayAppearancePalette.dark.desktopBackground.color))
    host.sizingOptions = []
    host.frame = CGRect(x: 0, y: 0, width: width, height: height)
    let window = NSWindow(
      contentRect: CGRect(x: -10000, y: -10000, width: width, height: height),
      styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    defer { window.close() }
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.08))
    host.layoutSubtreeIfNeeded()
    let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
    host.cacheDisplay(in: host.bounds, to: bitmap)
    let cgImage = try XCTUnwrap(bitmap.cgImage)
    let image = NSImage(cgImage: cgImage, size: host.bounds.size)
    let attachment = XCTAttachment(image: image)
    attachment.name = "Metadata-\(scenario)-scale-\(scale)"
    attachment.lifetime = .keepAlways
    add(attachment)
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = false
    request.minimumTextHeight = 0.001
    try VNImageRequestHandler(cgImage: cgImage).perform([request])
    let lines = (request.results ?? []).compactMap { $0.topCandidates(1).first?.string }
    let metadata = lines.filter { $0.localizedCaseInsensitiveContains("codex") }
    XCTAssertEqual(metadata.count, 1, "\(context): One physical metadata row: \(lines)")
    if scenario == "known" || scenario == "long" {
      XCTAssertTrue(
        metadata.first?.localizedCaseInsensitiveContains("codescribe") == true,
        "\(context): Keep the declared repository at the end: \(lines)")
    }
    let text = lines.joined(separator: " ")
    XCTAssertFalse(text.localizedCaseInsensitiveContains("Frame"), text)
    XCTAssertFalse(text.contains("private-home"), text)
    XCTAssertFalse(text.contains("location-must"), text)
    if scenario == "foreign-lease" || scenario == "unknown" {
      XCTAssertFalse(text.contains("gpt-6.1"), "Never borrow metadata or infer a missing model")
      XCTAssertFalse(text.localizedCaseInsensitiveContains("codescribe"), text)
    }
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertEqual(state.conversationDrafts[conversation.id], "Preserve draft")
    XCTAssertTrue(state.pendingTextMessages.isEmpty)
    XCTAssertEqual(controller.interactionWindowCounts.owned, 0)
  }
}
