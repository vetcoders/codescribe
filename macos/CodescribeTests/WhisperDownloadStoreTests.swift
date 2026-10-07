import XCTest

@testable import Codescribe

@MainActor
private final class PendingWhisperDownload {
  private(set) var starts = 0
  var listener: (any CsWhisperDownloadListener)?
  private var continuation: CheckedContinuation<CsWhisperModelStatus, Error>?

  func run(_ listener: any CsWhisperDownloadListener) async throws -> CsWhisperModelStatus {
    starts += 1
    self.listener = listener
    return try await withCheckedThrowingContinuation { continuation = $0 }
  }

  func finish(available: Bool = true) {
    let pending = continuation
    continuation = nil
    listener = nil
    var status = CsWhisperModelStatus.sampleUnavailable
    status.available = available
    status.path = available ? "/fixture/whisper" : nil
    pending?.resume(returning: status)
  }

  func fail() {
    let pending = continuation
    continuation = nil
    listener = nil
    pending?.resume(throwing: NSError(domain: "OfflineWhisperFixture", code: 1))
  }
}

@MainActor
final class WhisperDownloadStoreTests: XCTestCase {
  private func makeStore(_ transfer: PendingWhisperDownload) -> WhisperDownloadStore {
    WhisperDownloadStore(
      statusProvider: { .sampleUnavailable },
      download: { listener in try await transfer.run(listener) })
  }

  func testConstructionAndRefreshNeverStartARequestedModelTransfer() async {
    let transfer = PendingWhisperDownload()
    let store = makeStore(transfer)
    store.refresh()
    await Task.yield()
    XCTAssertEqual(transfer.starts, 0)
    XCTAssertFalse(store.inFlight)
  }

  func testSettingsAndSetupCoalesceClicksIntoOneTransferAndShareProgress() async throws {
    let transfer = PendingWhisperDownload()
    let store = makeStore(transfer)
    let setup = OnboardingViewModel(
      engine: MockOnboardingEngine(progress: 4), hotkeys: MockHotkeysEngine(),
      agentStatus: MockAgentStatusEngine(), probe: MockPermissionProbe(.allGranted),
      whisperDownloadStore: store)
    let settings = SettingsViewModel(
      engine: MockSettingsEngine(), permissionProbe: MockPermissionProbe(.allGranted),
      whisperDownloadStore: store, servingStatusProvider: { nil })
    XCTAssertTrue(setup.whisperDownloadStore === settings.whisperDownloadStore)
    setup.whisperDownloadStore.start()
    settings.startWhisperDownload()
    XCTAssertTrue(store.inFlight, "The generation is claimed before the async transfer starts")
    await awaitCondition { transfer.listener != nil }
    XCTAssertEqual(transfer.starts, 1)
    let listener = try XCTUnwrap(transfer.listener)
    listener.onProgress(file: "weights", bytesDone: 25, bytesTotal: 100)
    await awaitCondition { store.fraction == 0.25 }
    XCTAssertEqual(settings.whisperDownloadFraction, 0.25)
    store.refresh()
    XCTAssertEqual(store.fraction, 0.25, "A second surface cannot erase active progress")
    listener.onComplete(path: "/callback/path")
    transfer.finish()
    await awaitCondition { !store.inFlight }
    XCTAssertTrue(store.status.available)
    XCTAssertTrue(store.detail?.contains("/fixture/whisper") == true)
    XCTAssertFalse(store.detail?.contains("/callback/path") == true)
    XCTAssertNil(store.error)
  }

  func testLateCompletionCallbackCannotOverwriteFailureAndRetryStartsANewGeneration() async throws {
    let transfer = PendingWhisperDownload()
    let store = makeStore(transfer)
    store.start()
    await awaitCondition { transfer.listener != nil }
    try XCTUnwrap(transfer.listener).onComplete(path: "/misleading/callback")
    transfer.fail()
    await awaitCondition { !store.inFlight }
    XCTAssertNotNil(store.error)
    XCTAssertNil(store.fraction)
    XCTAssertFalse(store.status.available)
    XCTAssertFalse(store.detail?.contains("/misleading/callback") == true)
    store.start()
    XCTAssertNil(store.error)
    await awaitCondition { transfer.starts == 2 }
    transfer.finish()
    await awaitCondition { !store.inFlight }
    XCTAssertTrue(store.status.available)
  }

  func testUnavailableResultIsRetryableRatherThanClaimingReady() async {
    let transfer = PendingWhisperDownload()
    let store = makeStore(transfer)
    store.start()
    await awaitCondition { transfer.listener != nil }
    transfer.finish(available: false)
    await awaitCondition { !store.inFlight }
    XCTAssertFalse(store.status.available)
    XCTAssertNotNil(store.error)
    XCTAssertNil(store.fraction)
    store.start()
    await awaitCondition { transfer.starts == 2 }
    transfer.finish()
    await awaitCondition { !store.inFlight }
    XCTAssertTrue(store.status.available)
  }

  func testSetupCanAdvanceFinishAndReleaseItsWindowModelDuringDownload() async {
    let transfer = PendingWhisperDownload()
    var store: WhisperDownloadStore? = makeStore(transfer)
    weak let retainedStore = store
    let engine = MockOnboardingEngine(progress: 4)
    var setup: OnboardingViewModel? = OnboardingViewModel(
      engine: engine, hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
      probe: MockPermissionProbe(.allGranted), whisperDownloadStore: store!,
      preferredLanguages: ["en"], processInterfaceLanguage: .english)
    var finished = false
    setup?.onFinished = { finished = true }
    store?.start()
    await awaitCondition { transfer.listener != nil }
    setup?.advance()
    XCTAssertEqual(setup?.step, .apiKey)
    setup?.finish()
    XCTAssertTrue(finished)
    XCTAssertFalse(engine.showOnboarding)
    XCTAssertTrue(store?.inFlight == true)
    setup = nil
    store = nil
    XCTAssertNotNil(retainedStore, "The transfer survives release of all window owners")
    XCTAssertEqual(transfer.starts, 1)
    transfer.finish()
    await awaitCondition { retainedStore == nil }
  }
}
