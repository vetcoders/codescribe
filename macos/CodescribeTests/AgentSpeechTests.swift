import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class AgentSpeechTests: XCTestCase {
  private final class Engine: AgentChatEngine {
    var unavailable: String?
    var spoken: [String] = []
    var failure: Error?
    var stopCount = 0
    func isAvailable() -> Bool { true }
    func availabilityDetail() -> String? { nil }
    func speechAvailability() -> String? { unavailable }
    func speak(text: String) async throws {
      if let failure { throw failure }
      spoken.append(text)
    }
    func stopSpeaking() { stopCount += 1 }
    func generateThreadTitle(_ text: String) async throws -> String? { nil }
    func cancelReply(threadId: String) -> Bool { false }
    func streamReply(
      _ text: String, threadId: String, attachmentPaths: [String],
      onDelta: @escaping @MainActor (String) -> Void,
      onReasoning: @escaping @MainActor (String) -> Void,
      onToolExecuting: @escaping @MainActor (String, String) -> Void,
      onToolResult: @escaping @MainActor (String, String, Bool, String) -> Void
    ) async throws -> String { text }
  }

  func testSpeakSendsAssistantTextAndClearsPlayingState() async {
    let engine = Engine()
    let store = AgentChatStore(engine: engine)
    await store.speak(ChatMessage(role: .assistant, timestamp: "now", text: "Hello"))
    XCTAssertEqual(engine.spoken, ["Hello"])
    XCTAssertNil(store.speakingMessageID)
    XCTAssertNil(store.speechError)
  }

  func testUnavailableSpeechPreservesExactReasonAndNeverCallsVendor() async {
    let engine = Engine()
    engine.unavailable = "unavailable: Anthropic has no speech endpoint"
    let store = AgentChatStore(engine: engine)
    XCTAssertEqual(store.speechUnavailableReason, engine.unavailable)
    await store.speak(ChatMessage(role: .assistant, timestamp: "now", text: "Hello"))
    XCTAssertEqual(store.speechError, engine.unavailable)
    XCTAssertTrue(engine.spoken.isEmpty)
  }

  func testSpeechFailureIsVisibleAndStopReachesEngine() async {
    let engine = Engine()
    engine.failure = NSError(
      domain: "Speech", code: 1,
      userInfo: [
        NSLocalizedDescriptionKey: "Speech request failed (HTTP 403)."
      ])
    let store = AgentChatStore(engine: engine)
    await store.speak(ChatMessage(role: .assistant, timestamp: "now", text: "Hello"))
    XCTAssertEqual(store.speechError, "Speech request failed (HTTP 403).")
    XCTAssertNil(store.speakingMessageID)
    store.stopSpeaking()
    XCTAssertEqual(engine.stopCount, 1)
  }

  func testSpeakButtonIsPresentWithoutHoverAndDisabledWithAvailabilityReason() throws {
    let engine = Engine()
    engine.unavailable = "unavailable: xAI has no signed-in account or API key"
    let store = AgentChatStore(engine: engine)
    let button = AssistantSpeechButton(
      messageID: UUID(), isSpeaking: false,
      unavailableReason: store.speechUnavailableReason, action: {}
    )
    XCTAssertTrue(button.isDisabled)
    XCTAssertEqual(button.help, engine.unavailable)
    // Render the actual control without a hover event. A hidden hover-only
    // action would produce no button pixels. AX children aren't materialized
    // by headless SwiftUI hosts, so use the rendered image for presence.
    let renderer = ImageRenderer(content: button.padding(8))
    let image = try XCTUnwrap(renderer.cgImage)
    XCTAssertGreaterThan(image.width, 20)
    XCTAssertGreaterThan(image.height, 10)
    let bitmap = NSBitmapImageRep(cgImage: image)
    let hasVisiblePixels = (0..<bitmap.pixelsWide).contains { x in
      (0..<bitmap.pixelsHigh).contains { y in
        (bitmap.colorAt(x: x, y: y)?.alphaComponent ?? 0) > 0.01
      }
    }
    XCTAssertTrue(hasVisiblePixels)
  }

  func testStopRemainsEnabledIfAvailabilityChangesDuringPlayback() {
    let button = AssistantSpeechButton(
      messageID: UUID(), isSpeaking: true,
      unavailableReason: "Account signed out", action: {}
    )
    XCTAssertFalse(button.isDisabled)
  }
}
