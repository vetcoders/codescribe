import XCTest

@testable import Codescribe

@MainActor
private final class PendingSettingsSnapshot {
  var settings: CsSettings = .sample
  var reads = 0
  var continuation: CheckedContinuation<CsProviderAccessSnapshot, Error>?

  func acquire() async throws -> CsProviderAccessSnapshot {
    reads += 1
    return try await withCheckedThrowingContinuation { continuation = $0 }
  }

  func resolve(endpoint: String?, accountErrors: [String: String] = [:]) {
    var lane = CsSttLane.sampleFile
    lane.endpoint = endpoint
    lane.apiKeySet = true
    let pending = continuation
    continuation = nil
    pending?.resume(
      returning: CsProviderAccessSnapshot(
        providers: CsProviderOption.sampleProviders, accountErrors: accountErrors,
        keyStatus: .sampleAllSet, sttLanes: [lane, .sampleLive], revision: 0))
  }
}

@MainActor
final class SettingsSnapshotOrderingTests: XCTestCase {
  private func makeModel(_ snapshot: PendingSettingsSnapshot) -> SettingsViewModel {
    let store = MockProviderStore()
    var engine = MockSettingsEngine(
      settingsLoader: { snapshot.settings }, providerStore: store,
      updateConfigObserver: { key, value in
        if key == "STT_FILE_ENDPOINT" { snapshot.settings.sttFileEndpoint = value }
      })
    engine.providerAccessSnapshotLoader = { try await snapshot.acquire() }
    return SettingsViewModel(
      engine: engine, permissionProbe: MockPermissionProbe(),
      runtimeLlmLaneProvider: { store.runtimeLane($0) })
  }

  func testOldAccessSnapshotCannotReplaceNewerCommittedSttEndpoint() async {
    let snapshot = PendingSettingsSnapshot()
    let oldEndpoint = "https://old.example.test/v1/audio/transcriptions"
    let newEndpoint = "https://new.example.test/v1/audio/transcriptions"
    snapshot.settings.sttFileEndpoint = oldEndpoint
    let model = makeModel(snapshot)
    model.refreshProviderAccess()
    await awaitCondition { snapshot.continuation != nil }
    model.setSttLaneEndpoint("file", newEndpoint)
    XCTAssertEqual(snapshot.settings.sttFileEndpoint, newEndpoint)
    XCTAssertEqual(model.sttLanes.first?.endpoint, newEndpoint)
    snapshot.resolve(endpoint: oldEndpoint)
    await awaitCondition { snapshot.continuation != nil }
    XCTAssertEqual(snapshot.reads, 2, "one follow-up replaces the invalidated read")
    XCTAssertEqual(
      model.sttLanes.first?.endpoint, newEndpoint,
      "an older credential result must not paint an obsolete endpoint")
    snapshot.resolve(endpoint: newEndpoint)
    await awaitCondition { !model.providerAccessPending }
    XCTAssertEqual(model.sttLanes.first?.endpoint, newEndpoint)
    XCTAssertNil(model.providerAccessError)
  }

  func testAccountErrorLeavesProviderRegistryAndIndependentSttKeyVisible() async {
    let snapshot = PendingSettingsSnapshot()
    let model = makeModel(snapshot)
    let provider = CsProviderOption.sampleProviders[1]
    model.refreshProviderAccess()
    await awaitCondition { snapshot.continuation != nil }
    snapshot.resolve(
      endpoint: "https://speech.example.test/v1/audio/transcriptions",
      accountErrors: [provider.id: "Account access unavailable"])
    await awaitCondition { !model.providerAccessPending }
    XCTAssertTrue(model.providerAccessResolved)
    XCTAssertNil(model.providerAccessError, "a per-account error is not a whole-store failure")
    XCTAssertEqual(model.providers.map(\.id), CsProviderOption.sampleProviders.map(\.id))
    XCTAssertEqual(model.providerAccountErrors[provider.id], "Account access unavailable")
    XCTAssertTrue(model.sttLanes.first?.apiKeySet == true)
    model.refreshProviderAccess()
    await awaitCondition { snapshot.continuation != nil }
    snapshot.resolve(endpoint: "https://speech.example.test/v1/audio/transcriptions")
    await awaitCondition { !model.providerAccessPending }
    XCTAssertTrue(
      model.providerAccountErrors.isEmpty, "successful recovery clears the account error")
    XCTAssertTrue(model.sttLanes.first?.apiKeySet == true)
  }
}
