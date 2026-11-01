import Combine
import XCTest

@testable import Codescribe

@MainActor
final class FormatterKeywordsTests: XCTestCase {
  /// Fixes 3dea75ef4 and 5aac8c9df; kills M3b-04 (sync reload) and M3b-06
  /// (extension-only dispatch), on both successful and refused writes.
  func testM3bKeywordsSaveAwaitsExistentialSnapshotOnSuccessAndRefusal() async {
    for refusesWrite in [false, true] {
      var synchronousReads = 0
      var asynchronousReads = 0
      var writes: [[String]] = []
      let engine: any SettingsEngine = MockSettingsEngine(
        setFormatterKeywordsObserver: { terms in
          writes.append(terms)
          if refusesWrite { throw CsError.Config(msg: "M3b refused write") }
          return ["Vetcoders"]
        },
        settingsLoader: {
          synchronousReads += 1
          var snapshot = CsSettings.sample
          snapshot.formatterKeywords = ["synchronous stale snapshot"]
          return snapshot
        },
        settingsAsyncLoader: {
          await Task.yield()
          asynchronousReads += 1
          var snapshot = CsSettings.sample
          snapshot.formatterKeywords = ["Codescribe", "LibraxisAI"]
          return snapshot
        }
      )
      let model = SettingsViewModel(engine: engine, permissionProbe: MockPermissionProbe())
      let readsAfterInitialization = synchronousReads
      XCTAssertEqual(model.formatterKeywords, ["synchronous stale snapshot"])

      do {
        let stored = try await model.saveFormatterKeywords(["  Vetcoders  "])
        XCTAssertFalse(refusesWrite, "a refused write must propagate its error")
        XCTAssertEqual(stored, ["Vetcoders"])
      } catch {
        XCTAssertTrue(refusesWrite, "the successful write unexpectedly failed: \(error)")
      }

      XCTAssertEqual(writes, [["  Vetcoders  "]])
      XCTAssertEqual(asynchronousReads, 1, "the existential must call its async witness")
      XCTAssertEqual(synchronousReads, readsAfterInitialization, "post-save I/O used the sync path")
      XCTAssertEqual(model.formatterKeywords, ["Codescribe", "LibraxisAI"])
    }
  }

  func testSettingsEngineExistentialUsesAsyncSnapshotOverride() async {
    var syncLoads = 0
    var asyncLoads = 0
    let engine: any SettingsEngine = MockSettingsEngine(
      settingsLoader: {
        syncLoads += 1
        return .sample
      },
      settingsAsyncLoader: {
        asyncLoads += 1
        var settings = CsSettings.sample
        settings.formatterKeywords = ["Codescribe"]
        return settings
      }
    )

    let snapshot = await engine.loadSettingsAsync()

    XCTAssertEqual(snapshot.formatterKeywords, ["Codescribe"])
    XCTAssertEqual(asyncLoads, 1)
    XCTAssertEqual(syncLoads, 0, "the existential must not select the MainActor synchronous default")
  }

  func testSaveFormatterKeywordsReplacesProjectionWithNormalizedSnapshotAndNotifiesScope()
    async throws
  {
    var persistedSettings = CsSettings.sample
    var capturedInput: [[String]] = []
    let invalidation = ConfigurationInvalidation()
    var invalidationEdges: [ConfigurationInvalidation.Edge] = []
    let cancellable = invalidation.edges(excluding: self).sink { edge in
      invalidationEdges.append(edge)
    }
    _ = cancellable

    let engine = MockSettingsEngine(
      setFormatterKeywordsObserver: { draft in
        capturedInput.append(draft)
        let normalized = ["Codescribe", "Vetcoders", "UniFFI"]
        persistedSettings.formatterKeywords = normalized
        return normalized
      },
      settingsLoader: { persistedSettings }
    )
    let model = SettingsViewModel(
      engine: engine,
      permissionProbe: MockPermissionProbe(),
      configurationInvalidation: invalidation
    )

    XCTAssertEqual(model.formatterKeywords, [String]())

    let stored = try await model.saveFormatterKeywords([
      "  Codescribe ", "vetcoders", "codescribe", "UniFFI",
    ])

    XCTAssertEqual(
      capturedInput,
      [["  Codescribe ", "vetcoders", "codescribe", "UniFFI"]]
    )
    XCTAssertEqual(stored, ["Codescribe", "Vetcoders", "UniFFI"])
    XCTAssertEqual(model.formatterKeywords, ["Codescribe", "Vetcoders", "UniFFI"])
    XCTAssertEqual(invalidationEdges, [.settingsWritten])
  }

  func testRefusedFormatterKeywordsSaveKeepsPersistedSnapshotUntouchedAndSurfacesError() async {
    var persistedSettings = CsSettings.sample
    persistedSettings.formatterKeywords = ["Codescribe", "LibraxisAI"]

    let engine = MockSettingsEngine(
      setFormatterKeywordsObserver: { _ in
        throw CsError.Config(
          msg: "formatter_keywords supports at most 100 entries (received 101)"
        )
      },
      settingsLoader: { persistedSettings }
    )
    let model = SettingsViewModel(
      engine: engine,
      permissionProbe: MockPermissionProbe()
    )

    XCTAssertEqual(model.formatterKeywords, ["Codescribe", "LibraxisAI"])

    let oversized = (1...101).map { "Term\($0)" }
    do {
      _ = try await model.saveFormatterKeywords(oversized)
      XCTFail("Expected oversized formatter_keywords list to be refused")
    } catch {
      XCTAssertTrue(
        error.userFacingMessage.contains("100"),
        "Refusal message must explain the limit: \(error.userFacingMessage)"
      )
    }

    XCTAssertEqual(
      model.formatterKeywords,
      ["Codescribe", "LibraxisAI"],
      "Failed save must leave the persisted snapshot projection intact"
    )
  }

  func testAutomaticFormattingSelectorProjectsStoredMaxAsSmartAndRefusesMaxWrites() {
    var persistedSettings = CsSettings.sample
    persistedSettings.formattingLevel = "max"
    var writes: [(String, String)] = []

    let engine = MockSettingsEngine(
      settingsLoader: { persistedSettings },
      updateConfigObserver: { key, value in
        writes.append((key, value))
        if key == "FORMATTING_LEVEL" {
          persistedSettings.formattingLevel = value
        }
      }
    )
    let model = SettingsViewModel(
      engine: engine,
      permissionProbe: MockPermissionProbe()
    )

    XCTAssertEqual(model.automaticFormattingLevel, .smart)

    model.setFormattingLevel("max")
    XCTAssertTrue(writes.isEmpty)
    XCTAssertNotNil(model.lastError)

    model.setFormattingLevel("correction")
    XCTAssertEqual(writes.map(\.0), ["FORMATTING_LEVEL"])
    XCTAssertEqual(writes.map(\.1), ["correction"])
    XCTAssertEqual(model.automaticFormattingLevel, .correction)
  }
}
