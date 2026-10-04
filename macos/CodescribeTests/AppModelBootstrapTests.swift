import Foundation
import XCTest

@testable import Codescribe

/// Source-only recovery contracts. Run with the integrator's full Swift gate.
@MainActor
final class AppModelBootstrapTests: XCTestCase {
  func testDefaultProductionCompositionReturnsWithComposerAlreadyConnected() throws {
    // No yield, task or delayed lookup: the production constructor must return
    // with its receiver installed. Before recovery this re-entered shared.
    let model = AppModel()

    let deliver = try XCTUnwrap(model.overlay.state.onComposerTranscript)
    let finish = try XCTUnwrap(model.overlay.state.onCaptureEnded)
    let thread = try XCTUnwrap(model.chat.selectedThreadID)
    let captureID = "bootstrap-production-" + UUID().uuidString
    admitCapture(model.chat, threadID: thread, captureID: captureID)
    defer { finish(captureID) }

    XCTAssertEqual(deliver("bootstrap words", captureID), .admitted(threadID: thread))
    XCTAssertEqual(model.chat.draft, "bootstrap words")
    XCTAssertTrue(model.chat.dictation is RealComposerDictation)
  }

  func testStandaloneOverlayConstructionDoesNotResolveAGlobalComposer() {
    let controller = OverlayController()

    XCTAssertNil(controller.state.onComposerTranscript)
    XCTAssertNil(controller.state.onCaptureEnded)
  }

  func testProductionConnectionDeliversAndReleasesOnlyTheSuppliedChat() throws {
    let owner = try makeStore(title: "Owner")
    let other = try makeStore(title: "Other")
    let ownerThread = try XCTUnwrap(owner.selectedThreadID)
    let otherThread = try XCTUnwrap(other.selectedThreadID)
    // Even the same capture ID admitted in another store cannot make that
    // store the receiver. The connection must use object ownership, not shared.
    admitCapture(owner, threadID: ownerThread, captureID: "bootstrap-capture")
    admitCapture(other, threadID: otherThread, captureID: "bootstrap-capture")
    owner.draft = "owner typed"
    other.draft = "other typed"
    let controller = OverlayController(composer: owner)
    // Exercise the callbacks installed by production construction, never
    // manually connectComposer or a copied receiver closure in the test.
    let deliver = try XCTUnwrap(controller.state.onComposerTranscript)
    let finish = try XCTUnwrap(controller.state.onCaptureEnded)

    XCTAssertEqual(deliver("spoken words", "bootstrap-capture"), .admitted(threadID: ownerThread))
    XCTAssertEqual(owner.draft, "owner typed\nspoken words")
    XCTAssertEqual(other.draft, "other typed")
    XCTAssertTrue(other.composerRecoveryDocuments.isEmpty)
    XCTAssertTrue(owner.threads.allSatisfy { $0.messages.isEmpty })
    XCTAssertTrue(other.threads.allSatisfy { $0.messages.isEmpty })

    XCTAssertEqual(deliver("spoken words", "bootstrap-capture"), .admitted(threadID: ownerThread))
    XCTAssertEqual(owner.draft, "owner typed\nspoken words", "replay must not append twice")
    finish("bootstrap-capture")
    XCTAssertFalse(owner.hasComposerCaptureRequest)
    XCTAssertNil(owner.composerCaptureHandle)
    XCTAssertTrue(other.hasComposerCaptureRequest)
    XCTAssertEqual(other.composerCaptureHandle?.captureId, "bootstrap-capture")
  }

  func testProductionConnectionRefusesForeignCaptureWithoutReleasingTheOwner() throws {
    let owner = try makeStore(title: "Owner")
    let thread = try XCTUnwrap(owner.selectedThreadID)
    admitCapture(owner, threadID: thread, captureID: "owned-capture")
    owner.draft = "keep my draft"
    let controller = OverlayController(composer: owner)
    let deliver = try XCTUnwrap(controller.state.onComposerTranscript)
    let finish = try XCTUnwrap(controller.state.onCaptureEnded)

    XCTAssertEqual(deliver("foreign words", "foreign-capture"), .retained("foreign words"))
    finish("foreign-capture")

    XCTAssertEqual(owner.draft, "keep my draft")
    XCTAssertEqual(owner.composerRecoveryDocuments.map(\.text), ["foreign words"])
    XCTAssertTrue(owner.hasComposerCaptureRequest)
    XCTAssertEqual(owner.composerCaptureHandle?.captureId, "owned-capture")
    XCTAssertTrue(owner.threads.allSatisfy { $0.messages.isEmpty })

    XCTAssertEqual(deliver("owned words", "owned-capture"), .admitted(threadID: thread))
    XCTAssertEqual(owner.draft, "keep my draft\nowned words")
    finish("owned-capture")
    XCTAssertFalse(owner.hasComposerCaptureRequest)
  }

  private func makeStore(title: String) throws -> AgentChatStore {
    let suite = "Codescribe.AppModelBootstrapTests." + UUID().uuidString
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    addTeardownBlock { UserDefaults(suiteName: suite)?.removePersistentDomain(forName: suite) }
    return AgentChatStore(
      threads: [ChatThread(title: title, meta: "now")],
      persistenceDefaults: defaults
    )
  }

  private func admitCapture(_ store: AgentChatStore, threadID: UUID, captureID: String) {
    let request = store.beginComposerCaptureRequest(threadID: threadID)
    store.completeComposerCaptureStart(
      request, live: true, handle: CsCaptureHandle(captureId: captureID)
    )
  }
}
