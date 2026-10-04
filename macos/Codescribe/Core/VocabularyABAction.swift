import Foundation
import OSLog

/// No microphone, settings, dictionary, or filesystem arguments in the URL.
enum VocabularyABRequest {
  static func sample(from url: URL, labEnabled: Bool) -> UInt32? {
    guard labEnabled,
      let parts = URLComponents(url: url, resolvingAgainstBaseURL: false),
      parts.scheme == "codescribe", parts.host == "lab", parts.path == "/vocabulary-ab",
      parts.user == nil, parts.password == nil, parts.port == nil, parts.fragment == nil
    else { return nil }
    let items = parts.queryItems ?? []
    // Unknown parameters are ignored; duplicate allowed keys are ambiguous.
    let samples = items.filter { $0.name == "sample" }
    guard samples.count <= 1 else { return nil }
    guard let item = samples.first else { return 30 }
    guard let value = item.value, !value.isEmpty,
      value.utf8.allSatisfy({ (48...57).contains($0) }),
      let count = UInt32(value), (2...100).contains(count)
    else { return nil }
    return count
  }
}

@MainActor
final class VocabularyABAction {
  private(set) var job: Task<Void, Never>?
  private let worker: @Sendable (UInt32) async -> Void

  init(worker: @escaping @Sendable (UInt32) async -> Void = { sample in
    await Task.detached(priority: .utility) {
      // Errors contain no child diagnostics or dictation text.
      do { _ = try runVocabularyAb(sample: sample) }
      catch {
        Logger(subsystem: "com.vetcoders.codescribe", category: "vocabulary-ab")
          .error("Vocabulary A/B unavailable or incomplete")
      }
    }.value
  }) {
    self.worker = worker
  }

  @discardableResult
  func receive(_ url: URL, labEnabled: Bool) -> Bool {
    guard job == nil, let sample = VocabularyABRequest.sample(from: url, labEnabled: labEnabled)
    else { return false }
    job = Task {
      await worker(sample)
      job = nil
    }
    return true
  }
}
