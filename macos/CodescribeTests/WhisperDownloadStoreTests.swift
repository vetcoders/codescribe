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
/// of how fast the test machine happened to run.
@MainActor
private final class StoppedClock {
  var date = Date(timeIntervalSince1970: 1_000_000)

  func advance(_ seconds: TimeInterval) { date += seconds }
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
