import Combine
import XCTest

@testable import Codescribe

@MainActor
final class FormatterKeywordsTests: XCTestCase {
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
