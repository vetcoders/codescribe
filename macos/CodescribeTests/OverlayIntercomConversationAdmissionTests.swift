import AppKit
import SwiftUI
import Vision
import XCTest

@testable import Codescribe

@MainActor
final class OverlayIntercomConversationAdmissionTests: XCTestCase {
  // Founder correction, 2026-10-26 12:26Z and 12:41Z: a grouped card
  // reveals header -> context -> messages -> input as its size permits.
  // Monika, 2026-11-07: the 13 pt text / content-sized mini bar is the contract.
  // The card is as tall as the first readable preview needs (context strip,
  // one body line at `bodyFontSize`, its route line and the preview's padding),
  // so it stays below the composer gate at every scale; a fixed height tuned
  // to the old 10 pt body no longer fits one line at 0.8.
  func testGroupedCardBelowComposerGateRendersOnlyLatestPreview() throws {
    // The title row never shrinks below 30 pt and the context strip sits under
    // it again (6ca5ea7e3), so at scale 0.8 a 100 pt fixture is an 80 pt card:
    // 34 pt left of the 36 pt a one-line preview needs. 120 pt admits the
    // preview at every scale and stays below the composer gate.
    for scale in [CGFloat(0.8), 1, 1.6] {
      try renderCard(scale: scale, height: 120, expectsComposer: false)
    }
  }

  func testGroupedCardAboveComposerGateKeepsHistoryAndSharedDraft() throws {
    for scale in [CGFloat(0.8), 1, 1.6] {
      try renderCard(scale: scale, expectsComposer: true)
    }
  }

  func testOpenedCardKeepsFullConversationAndSharedDraft() throws {
    try renderCard(scale: 1, expanded: true, expectsComposer: true)
  }

  private func renderCard(
    scale: CGFloat, expanded: Bool = false, height: CGFloat = 500, expectsComposer: Bool
  ) throws {
    let identity = AgentPlaybackIdentity(
      provider: "codex", session: "compact-admission", bus: "/fixture/bus")
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": identity.session,
        "lease_id": identity.leaseID, "channel": "3", "name": "Agent",
      ]))
    let messages = ["Earlier amber sentence", "**Latest olive sentence**"].enumerated().map {
      index, text in
      OverlayConversationMessage(
        id: "reply-\(index)", kind: .reply, text: text, order: UInt64(index),
        emittedAt: "2026-10-25T18:00:00Z", owner: owner, recipients: [], deliveryID: nil,
        replyTo: nil, unsolicited: false, playback: nil, busPath: identity.bus)
    }
    let conversation = OverlayConversation(
      id: owner.id, channel: "3", name: "Agent", owner: owner, messages: messages)
    let state = OverlayState()
    state.applyChannelRoster([
      .init(
        channel: "3", audience: "Agent", provider: "codex", providerSessionId: identity.session,
        open: false, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true,
        unreadSinceUnixMs: nil)
    ])
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [conversation]))
    state.conversationDrafts[conversation.id] = "Preserve shared draft"
    state.publishConversationText = { _, _, _, _ in
      XCTFail("Rendering must not send")
      return .empty
    }
    let generation = state.captureGeneration
    let keyBefore = NSApp.keyWindow
    let suite = "CodescribeTests.compact-conversation.\(UUID().uuidString)"
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    let controller = OverlayIntercomWidgetController(
      state: state, textScale: TextScaleController(key: suite),
      desktopStore: OverlayIntercomDesktopStore(defaults: defaults))
    defer { controller.invalidate() }
    let width = 600 * scale
    let height = height * scale
    let host = NSHostingView(
      rootView: controller.offscreenCard(
        .init(digit: "3", conversation: conversation), width: width, height: height, scale: scale,
        expanded: expanded
      )
      .environment(\.csTextScale, scale).environment(\.colorScheme, .dark)
      .symbolEffectsRemoved().transaction { $0.disablesAnimations = true }
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
    RunLoop.main.run(until: Date().addingTimeInterval(0.15))
    host.layoutSubtreeIfNeeded()
    let editors = descendants(host).compactMap { $0 as? NSTextView }.filter(\.isEditable)
    if expectsComposer {
      XCTAssertEqual(editors.count, 1)
      XCTAssertEqual(editors.first?.string, "Preserve shared draft")
    } else {
      XCTAssertTrue(editors.isEmpty, "A card below the gate remains a passive preview")
    }
    let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
    host.cacheDisplay(in: host.bounds, to: bitmap)
    let cgImage = try XCTUnwrap(bitmap.cgImage)
    let attachment = XCTAttachment(image: NSImage(cgImage: cgImage, size: host.bounds.size))
    attachment.name = "Closed-rich-preview-scale-\(scale)"
    attachment.lifetime = .keepAlways
    add(attachment)
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = false
    try VNImageRequestHandler(cgImage: cgImage).perform([request])
    let text = (request.results ?? []).compactMap { $0.topCandidates(1).first?.string }.joined(
      separator: " ")
    XCTAssertEqual(
      text.localizedCaseInsensitiveContains("Earlier amber sentence"), expectsComposer, text)
    XCTAssertFalse(text.contains("**"), "Markdown must render, not expose its source")
    XCTAssertTrue(text.localizedCaseInsensitiveContains("Latest olive sentence"), text)
    XCTAssertTrue(
      text.contains(String(localized: "You")), "The route must identify the recipient: \(text)")
    XCTAssertEqual(state.conversationDrafts[conversation.id], "Preserve shared draft")
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(state.pendingTextMessages.isEmpty)
    XCTAssertEqual(controller.interactionWindowCounts.owned, 0)
    XCTAssertTrue(NSApp.keyWindow === keyBefore)
  }

  private func descendants(_ view: NSView) -> [NSView] {
    [view] + view.subviews.flatMap(descendants)
  }
}
