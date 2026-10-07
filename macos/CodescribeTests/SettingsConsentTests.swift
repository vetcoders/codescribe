import XCTest

@testable import Codescribe

@MainActor
final class SettingsConsentTests: XCTestCase {
  private func makeModel(onboarding: Bool, reads: @escaping () -> Void) -> SettingsViewModel {
    var engine = MockSettingsEngine(onboarding: onboarding)
    engine.providerAccessSnapshotLoader = {
      reads()
      return CsProviderAccessSnapshot(
        providers: CsProviderOption.sampleProviders, accountErrors: [:], keyStatus: .sampleAllSet,
        sttLanes: [.sampleFile, .sampleLive], revision: 0)
    }
    return SettingsViewModel(
      engine: engine, permissionProbe: MockPermissionProbe(.allGranted),
      whisperDownloadStore: WhisperDownloadStore(
        statusProvider: { .sampleUnavailable }, download: { _ in .sampleUnavailable }),
      servingStatusProvider: { nil })
  }

  func testUnfinishedSetupKeepsGenericRefreshAndOtherSectionsCredentialPassive() async {
    var reads = 0
    let model = makeModel(onboarding: true) { reads += 1 }
    XCTAssertFalse(
      model.keyStatus.llmOpenaiApiKeySet, "Construction cannot read the engine's saved key")
    model.refresh()
    model.refreshForCurrentSection()
    for section in [SettingsSection.audio, .engine, .shortcuts, .agent, .creator] {
      model.select(section)
      model.refreshForCurrentSection()
    }
    for _ in 0..<12 { await Task.yield() }
    XCTAssertEqual(reads, 0)
    XCTAssertFalse(model.providerAccessResolved)
    model.select(SettingsSection.keys)
    await awaitCondition { model.providerAccessResolved }
    XCTAssertEqual(reads, 1, "The explained Providers surface owns the cold read")
    XCTAssertTrue(model.keyStatus.llmOpenaiApiKeySet)
  }

  func testCanonicalCompletedSetupAllowsRestorationWithoutSelectingProviders() async {
    var reads = 0
    let model = makeModel(onboarding: false) { reads += 1 }
    XCTAssertFalse(model.providerAccessResolved)
    model.refresh()
    await awaitCondition { model.providerAccessResolved }
    XCTAssertEqual(reads, 1)
    XCTAssertFalse(model.needsOnboarding)
  }
}
