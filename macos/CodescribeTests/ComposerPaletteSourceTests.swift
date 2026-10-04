import XCTest

@testable import Codescribe

@MainActor
final class ComposerPaletteSourceTests: XCTestCase {
  private final class Reads {
    var stamp: String? = "initial"
    var settings = 0
    var lane = 0
    var providers = 0
    var discovery = 0
  }

  private func source(_ reads: Reads) -> RealComposerPaletteSource {
    var engine = MockSettingsEngine()
    engine.composerStampProvider = { reads.stamp }
    engine.settingsLoader = {
      reads.settings += 1
      return .sample
    }
    engine.providersReader = {
      reads.providers += 1
      return CsProviderOption.sampleProviders
    }
    engine.discoveryLoader = { id in
      reads.discovery += 1
      return .sample(for: id)
    }
    return RealComposerPaletteSource(settings: engine, mcpAdmin: MockMCPAdminEngine()) {
      reads.lane += 1
      return engine.providerStore.runtimeLane(.assistive)
    }
  }

  func testWarmModelQueriesSkipEveryContextReaderAndDiscovery() {
    let reads = Reads()
    let source = source(reads)
    let first = source.entries(for: .model)
    XCTAssertFalse(first.isEmpty)
    for _ in 0..<20 { XCTAssertEqual(source.entries(for: .model), first) }
    XCTAssertEqual(reads.settings, 1)
    XCTAssertEqual(reads.lane, 1)
    XCTAssertEqual(reads.providers, 1)
    XCTAssertEqual(reads.discovery, 1)
  }

  func testChangedStampRebuildsBeforeTtlExpires() {
    let reads = Reads()
    let source = source(reads)
    _ = source.entries(for: .model)
    reads.stamp = "external-write"
    _ = source.entries(for: .model)
    XCTAssertEqual(reads.settings, 2)
    XCTAssertEqual(reads.lane, 2)
    XCTAssertEqual(reads.providers, 2)
    XCTAssertEqual(reads.discovery, 2)
  }

  func testUnknownFreshnessRefusesOldEntries() {
    let reads = Reads()
    let source = source(reads)
    _ = source.entries(for: .model)
    reads.stamp = nil
    _ = source.entries(for: .model)
    _ = source.entries(for: .model)
    XCTAssertEqual(reads.discovery, 3)
    XCTAssertEqual(reads.settings, 3)
  }

  func testMutationDuringDiscoveryCannotCertifyOldEntriesWithNewStamp() {
    let reads = Reads()
    var engine = MockSettingsEngine()
    engine.composerStampProvider = { reads.stamp }
    engine.discoveryLoader = { id in
      reads.discovery += 1
      reads.stamp = "changed-during-discovery"
      return .sample(for: id)
    }
    let source = RealComposerPaletteSource(settings: engine, mcpAdmin: MockMCPAdminEngine()) {
      engine.providerStore.runtimeLane(.assistive)
    }
    _ = source.entries(for: .model)
    _ = source.entries(for: .model)
    XCTAssertEqual(reads.discovery, 2)
    _ = source.entries(for: .model)
    XCTAssertEqual(reads.discovery, 2, "unchanged new generation can now be reused")
  }

  func testApplyingAnotherModelInvalidatesWarmCache() throws {
    let reads = Reads()
    let source = source(reads)
    _ = source.entries(for: .model)
    try source.apply(
      ComposerPaletteEntry(id: "another-model", title: "Another", subtitle: nil), for: .model)
    _ = source.entries(for: .model)
    XCTAssertEqual(reads.discovery, 2)
  }

  func testStatusEntryDoesNotWriteOrInvalidate() throws {
    let reads = Reads()
    let source = source(reads)
    _ = source.entries(for: .model)
    try source.apply(ComposerPaletteEntry(id: "", title: "Unavailable", subtitle: nil), for: .model)
    _ = source.entries(for: .model)
    XCTAssertEqual(reads.discovery, 1)
  }
}
