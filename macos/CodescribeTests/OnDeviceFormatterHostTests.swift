import Foundation
import FoundationModels
import XCTest

@testable import Codescribe

final class OnDeviceFormatterHostTests: XCTestCase {
  private final class ResultBox: @unchecked Sendable {
    var value: CsOnDeviceFormatOutcome?
  }

  private func formatOffMain(
    _ formatter: OnDeviceFormatterHost, instructions: String, message: String
  ) -> CsOnDeviceFormatOutcome {
    let box = ResultBox()
    let done = DispatchSemaphore(value: 0)
    DispatchQueue.global().async {
      box.value = formatter.format(instructions: instructions, userMessage: message)
      done.signal()
    }
    done.wait()
    return box.value ?? .failed(message: "Test worker did not return")
  }

  func testFakeModelReceivesInstructionsAndMessageVerbatim() {
    let formatter = OnDeviceFormatterHost(
      availabilityProbe: { nil },
      generate: { instructions, message in "\(instructions)|\(message)" }
    )
    let outcome = formatOffMain(
      formatter, instructions: "sealed\n\n prompt", message: "[Language: pl]\n\nIwo"
    )
    guard case .formatted(let text) = outcome else {
      return XCTFail("Expected formatted text")
    }
    XCTAssertEqual(text, "sealed\n\n prompt|[Language: pl]\n\nIwo")
  }

  func testUnavailableSkipsTheModel() {
    let formatter = OnDeviceFormatterHost(
      availabilityProbe: { "model not ready" },
      generate: { _, _ in "unexpected" }
    )
    let outcome = formatOffMain(formatter, instructions: "prompt", message: "text")
    guard case .unavailable(let message) = outcome else {
      return XCTFail("Expected unavailable")
    }
    XCTAssertEqual(message, "model not ready")
  }

  func testUnknownErrorIsFailed() {
    let error = NSError(domain: "FormatterTest", code: 7)
    guard case .failed(let message) = OnDeviceFormatterHost.mapError(error) else {
      return XCTFail("Expected failed")
    }
    XCTAssertTrue(message.contains("FormatterTest"))
  }

  func testLanguageModelErrorCategories() {
    guard #available(macOS 27.0, *) else { return }
    let unsupported = LanguageModelError.unsupportedLanguageOrLocale(
      .init(languageCode: .init("pl"), debugDescription: "unsupported")
    )
    let refusal = LanguageModelError.refusal(
      .init(explanation: "refused", debugDescription: "refused")
    )
    let guardrail = LanguageModelError.guardrailViolation(
      .init(debugDescription: "guardrail")
    )
    let context = LanguageModelError.contextSizeExceeded(
      .init(contextSize: 8192, tokenCount: 9000, debugDescription: "too long")
    )
    let timeout = LanguageModelError.timeout(.init(debugDescription: "timeout"))

    guard case .unsupportedLanguage = OnDeviceFormatterHost.mapError(unsupported) else {
      return XCTFail("Unsupported language should retain its category")
    }
    guard case .refused = OnDeviceFormatterHost.mapError(refusal) else {
      return XCTFail("Refusal should retain its category")
    }
    guard case .refused = OnDeviceFormatterHost.mapError(guardrail) else {
      return XCTFail("Guardrail should retain its category")
    }
    guard case .contextExceeded = OnDeviceFormatterHost.mapError(context) else {
      return XCTFail("Context overflow should retain its category")
    }
    guard case .failed = OnDeviceFormatterHost.mapError(timeout) else {
      return XCTFail("Timeout should be a general failure")
    }
  }
}
