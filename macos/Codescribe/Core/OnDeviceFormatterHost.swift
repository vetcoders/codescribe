import Foundation
import FoundationModels

/// Swift owns the system model; Rust invokes this synchronous callback from
/// its blocking pool. Each request gets a new model session and the sealed
/// instructions and user message are passed through without modification.
final class OnDeviceFormatterHost: CsOnDeviceFormatter, Sendable {
  typealias AvailabilityProbe = @Sendable () -> String?
  typealias Generate = @Sendable (String, String) async throws -> String

  private let availabilityProbe: AvailabilityProbe
  private let generate: Generate

  init(
    availabilityProbe: @escaping AvailabilityProbe = OnDeviceFormatterHost.availabilityReason,
    generate: @escaping Generate = OnDeviceFormatterHost.generateText
  ) {
    self.availabilityProbe = availabilityProbe
    self.generate = generate
  }

  func format(instructions: String, userMessage: String) -> CsOnDeviceFormatOutcome {
    assert(!Thread.isMainThread, "On-device formatting must run off the main thread")
    if let reason = availabilityProbe() {
      return .unavailable(message: reason)
    }

    let result = OutcomeBox()
    let semaphore = DispatchSemaphore(value: 0)
    Task.detached { [generate] in
      let outcome: CsOnDeviceFormatOutcome
      do {
        let text = try await generate(instructions, userMessage)
        outcome = .formatted(text: text)
      } catch {
        outcome = Self.mapError(error)
      }
      result.store(outcome)
      semaphore.signal()
    }
    semaphore.wait()
    return result.take()
  }

  private static func availabilityReason() -> String? {
    guard #available(macOS 26.0, *) else {
      return "Foundation Models requires macOS 26 or newer"
    }
    switch SystemLanguageModel.default.availability {
    case .available: return nil
    case .unavailable(let reason): return "System model unavailable: \(reason)"
    }
  }

  private static func generateText(instructions: String, userMessage: String) async throws -> String {
    guard #available(macOS 26.0, *) else {
      throw FormatterUnavailable()
    }
    let session = LanguageModelSession(instructions: instructions)
    let response = try await session.respond(
      to: userMessage,
      options: GenerationOptions(temperature: 0.2)
    )
    return response.content
  }

  static func mapError(_ error: Error) -> CsOnDeviceFormatOutcome {
    let message = String(describing: error)
    if #available(macOS 27.0, *), let modelError = error as? LanguageModelError {
      switch modelError {
      case .unsupportedLanguageOrLocale: return .unsupportedLanguage(message: message)
      case .refusal, .guardrailViolation: return .refused(message: message)
      case .contextSizeExceeded: return .contextExceeded(message: message)
      default: return .failed(message: message)
      }
    }
    return .failed(message: message)
  }
}

private struct FormatterUnavailable: Error {}

/// The detached task is the sole writer; the waiting callback is the reader.
/// The lock gives Swift's strict concurrency checker an explicit handoff.
private final class OutcomeBox: @unchecked Sendable {
  private let lock = NSLock()
  private var outcome: CsOnDeviceFormatOutcome?

  func store(_ value: CsOnDeviceFormatOutcome) {
    lock.lock()
    outcome = value
    lock.unlock()
  }

  func take() -> CsOnDeviceFormatOutcome {
    lock.lock()
    defer { lock.unlock() }
    return outcome ?? .failed(message: "On-device formatter completed without a result")
  }
}
