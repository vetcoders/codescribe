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

/// Hand-wound clock, so a transfer rate is a fact of the fixture rather than
/// of how fast the test machine happened to run. The store hands it to the
/// progress sink, which stamps events off the main actor, so it is locked
/// rather than isolated.
private final class StoppedClock: @unchecked Sendable {
  private let lock = NSLock()
  private var stored = Date(timeIntervalSince1970: 1_000_000)

  var date: Date { lock.withLock { stored } }

  func advance(_ seconds: TimeInterval) { lock.withLock { stored += seconds } }
}

@MainActor
final class WhisperDownloadStoreTests: XCTestCase {
  /// 1539 MB: the shape of the real weights file, in whole megabytes.
  private static let weightsBytes: Int64 = 1539 * 1_048_576

  private func makeStore(_ transfer: PendingWhisperDownload) -> WhisperDownloadStore {
    WhisperDownloadStore(
      statusProvider: { .sampleUnavailable },
      download: { listener in try await transfer.run(listener) })
  }

  private func makeStore(_ transfer: PendingWhisperDownload, clock: StoppedClock)
    -> WhisperDownloadStore
  {
    WhisperDownloadStore(
      statusProvider: { .sampleUnavailable },
      download: { listener in try await transfer.run(listener) },
      now: { clock.date })
  }

  func testConstructionAndRefreshNeverStartARequestedModelTransfer() async {
    let transfer = PendingWhisperDownload()
    let store = makeStore(transfer)
    store.refresh()
    await Task.yield()
    XCTAssertEqual(transfer.starts, 0)
    XCTAssertFalse(store.inFlight)
  }

  func testOpeningTheSetupModelStepNeverStartsARequestedModelTransfer() async {
    let transfer = PendingWhisperDownload()
    let store = makeStore(transfer)
    let setup = OnboardingViewModel(
      engine: MockOnboardingEngine(progress: 4), hotkeys: MockHotkeysEngine(),
      agentStatus: MockAgentStatusEngine(), probe: MockPermissionProbe(.allGranted),
      whisperDownloadStore: store)
    XCTAssertEqual(setup.step, .localModel)
    setup.refreshForCurrentStep()
    await Task.yield()
    XCTAssertEqual(transfer.starts, 0, "Opening the step reads status; the download stays opt-in")
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

  func testCancellingStopsTheRunningGenerationAndOffersTheDownloadAgain() async throws {
    let transfer = PendingWhisperDownload()
    let store = makeStore(transfer)
    store.start()
    await awaitCondition { transfer.listener != nil }
    let listener = try XCTUnwrap(transfer.listener)
    listener.onProgress(file: "weights.safetensors", bytesDone: 25, bytesTotal: 100)
    await awaitCondition { store.fraction == 0.25 }

    store.cancel()
    XCTAssertTrue(store.cancelling)
    XCTAssertTrue(listener.isCancelled(), "the running generation is the one asked to stop")
    // Rust reports a stopped transfer as a failed call after clearing the partial.
    transfer.fail()
    await awaitCondition { !store.inFlight }

    XCTAssertNil(store.error, "a stop the user asked for is not a failure to report")
    XCTAssertNil(store.detail)
    XCTAssertNil(store.fraction)
    XCTAssertFalse(store.cancelling)
    XCTAssertFalse(store.status.available, "the model is back to not installed")

    store.start()
    await awaitCondition { transfer.starts == 2 }
    XCTAssertFalse(
      try XCTUnwrap(transfer.listener).isCancelled(),
      "a cancelled generation cannot stop the one started after it")
  }

  func testProgressArrivingAfterCancelLeavesTheStoppingMessageAlone() async throws {
    let transfer = PendingWhisperDownload()
    let store = makeStore(transfer)
    store.start()
    await awaitCondition { transfer.listener != nil }
    let listener = try XCTUnwrap(transfer.listener)
    listener.onProgress(file: "weights.safetensors", bytesDone: 25, bytesTotal: 100)
    await awaitCondition { store.fraction == 0.25 }

    store.cancel()
    let stopping = try XCTUnwrap(store.detail)
    XCTAssertNil(store.fraction, "the bar goes indeterminate the moment a stop is asked for")

    // Callbacks the transfer thread had already queued, plus anything it emits
    // before it notices the flag, must not repaint a download the user ended.
    listener.onProgress(file: "weights.safetensors", bytesDone: 60, bytesTotal: 100)
    listener.onProgress(file: "weights.safetensors", bytesDone: 95, bytesTotal: 100)
    for _ in 0..<50 { await Task.yield() }
    XCTAssertEqual(store.detail, stopping, "the stop message owns the line until the outcome")
    XCTAssertNil(store.fraction)

    transfer.fail()
    await awaitCondition { !store.inFlight }
    XCTAssertNil(store.detail)
    XCTAssertNil(store.error)
  }

  func testRateIsMeasuredFromEventTimestampsNotFromWhenTheUiCatchesUp() async throws {
    let transfer = PendingWhisperDownload()
    let clock = StoppedClock()
    let store = makeStore(transfer, clock: clock)
    store.start()
    await awaitCondition { transfer.listener != nil }
    let listener = try XCTUnwrap(transfer.listener)

    listener.onProgress(
      file: "tokenizer.json", bytesDone: 1_048_576, bytesTotal: Self.weightsBytes)
    await awaitCondition { store.detail?.contains("1 / 1539 MB") == true }

    // One megabyte one second later: 1 MB/s, whatever the main actor is doing.
    clock.advance(1)
    listener.onProgress(
      file: "tokenizer.json", bytesDone: 2 * 1_048_576, bytesTotal: Self.weightsBytes)
    // The repaint lands three seconds late. Nothing between the callback and
    // here awaits, so the consumer cannot have run yet.
    clock.advance(3)

    await awaitCondition { store.detail?.contains("MB/s") == true }
    let detail = try XCTUnwrap(store.detail)
    let rate = 1.0.formatted(.number.precision(.fractionLength(1)).grouping(.never))
    XCTAssertTrue(
      detail.hasPrefix("tokenizer.json · 2 / 1539 MB · \(rate) MB/s · about "),
      "consumption time would have read 1 MB over four seconds, got: \(detail)")
  }

  func testCancelIsANoOpWhileNoTransferIsRunning() {
    let transfer = PendingWhisperDownload()
    let store = makeStore(transfer)
    store.cancel()
    XCTAssertFalse(store.cancelling)
    XCTAssertEqual(transfer.starts, 0)
  }

  func testProgressReportsRateAndTimeLeftOnceTwoSamplesAreApart() async throws {
    let transfer = PendingWhisperDownload()
    let clock = StoppedClock()
    let store = makeStore(transfer, clock: clock)
    store.start()
    await awaitCondition { transfer.listener != nil }
    let listener = try XCTUnwrap(transfer.listener)

    listener.onProgress(
      file: "weights.safetensors", bytesDone: 20 * 1_048_576, bytesTotal: Self.weightsBytes)
    await awaitCondition { store.detail?.contains("20 / 1539 MB") == true }
    XCTAssertEqual(
      store.detail, "weights.safetensors · 20 / 1539 MB",
      "one sample says nothing about speed, so rate and time left stay out")

    clock.advance(1)
    listener.onProgress(
      file: "weights.safetensors", bytesDone: 33 * 1_048_576, bytesTotal: Self.weightsBytes)
    await awaitCondition { store.detail?.contains("MB/s") == true }
    let detail = try XCTUnwrap(store.detail)
    // The decimal separator belongs to the host locale; the number does not.
    let rate = 13.0.formatted(.number.precision(.fractionLength(1)).grouping(.never))
    XCTAssertTrue(
      detail.hasPrefix("weights.safetensors · 33 / 1539 MB · \(rate) MB/s · about "),
      "13 MB in one second is 13 MB/s, got: \(detail)")
    XCTAssertTrue(detail.contains("2"), "1506 MB left at 13 MB/s is about two minutes: \(detail)")
  }

  func testTheProgressLineDropsMembersItCannotHonestlyFill() {
    XCTAssertEqual(
      WhisperDownloadStore.progressLine(
        file: "config.json", bytesDone: 1_048_576, bytesTotal: 2 * 1_048_576,
        bytesPerSecond: nil),
      "config.json · 1 / 2 MB")
    XCTAssertEqual(
      WhisperDownloadStore.progressLine(
        file: "config.json", bytesDone: 1_048_576, bytesTotal: -1, bytesPerSecond: nil),
      "config.json · 1 MB",
      "a server without Content-Length leaves no total to print")
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
    await awaitCondition { finished }
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

  func testCancelledRequiredModelBlocksFinishUntilAValidModelessLaneIsChosen() async {
    let transfer = PendingWhisperDownload()
    let store = makeStore(transfer)
    let engine = MockOnboardingEngine(progress: 8)
    engine.lane = .localPower
    let setup = OnboardingViewModel(
      engine: engine, hotkeys: MockHotkeysEngine(), agentStatus: MockAgentStatusEngine(),
      probe: MockPermissionProbe(.allGranted), whisperDownloadStore: store,
      preferredLanguages: ["en"], processInterfaceLanguage: .english)
    var finished = false
    setup.onFinished = { finished = true }
    store.start()
    await awaitCondition { transfer.listener != nil }
    store.cancel()
    transfer.fail()
    await awaitCondition { !store.inFlight }
    XCTAssertEqual(store.readiness, .cancelled)
    setup.finish()
    await awaitCondition { !setup.finishPending }
    XCTAssertEqual(setup.dictationReadiness.localModelBlocker, .cancelled)
    XCTAssertFalse(finished)
    XCTAssertTrue(engine.showOnboarding)

    engine.lane = .appleOnly
    setup.finish()
    await awaitCondition { finished }
    XCTAssertTrue(finished, "an explicit working lane does not require unused Whisper weights")
    XCTAssertFalse(engine.showOnboarding)
    XCTAssertEqual(store.readiness, .cancelled, "Finish never starts or substitutes a model")
  }
}
