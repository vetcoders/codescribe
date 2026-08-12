import Foundation
import OSLog

/// Build-time analytics contract. B3 has not been approved, so the production
/// domain stays empty and the feature is inert even if a local preference is
/// toggled. Tests and development probes inject an explicit domain.
struct ActivationPingConfiguration: Equatable {
  static let production = ActivationPingConfiguration(
    endpoint: URL(string: "https://plausible.io/api/event")!,
    domain: ""
  )

  let endpoint: URL
  let domain: String

  var isEnabled: Bool {
    !domain.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
  }
}

protocol ActivationPingTransport {
  func data(for request: URLRequest) async throws -> (Data, URLResponse)
}

struct URLSessionActivationPingTransport: ActivationPingTransport {
  func data(for request: URLRequest) async throws -> (Data, URLResponse) {
    try await URLSession.shared.data(for: request)
  }
}

enum ActivationPingOutcome: Equatable {
  case notConsented
  case disabled
  case alreadySent
  case inFlight
  case sent
  case failed
}

/// Sends one content-free activation event after a successful dictation.
///
/// The API deliberately accepts no transcript argument: callers can report only
/// the fact of success. Consent and the successful-send receipt are persisted in
/// UserDefaults; consent defaults to false when the key is absent.
@MainActor
final class ActivationPing {
  static let optInDefaultsKey = "analytics.activationPing.optIn.v1"
  static let sentDefaultsKey = "analytics.firstSuccessfulDictation.sent.v1"
  static let shared = ActivationPing()

  private static let logger = Logger(
    subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
    category: "activation-analytics"
  )

  private let configuration: ActivationPingConfiguration
  private let defaults: UserDefaults
  private let transport: ActivationPingTransport
  private let appVersion: () -> String
  private let operatingSystem: () -> String
  private var sendInFlight = false

  init(
    configuration: ActivationPingConfiguration = .production,
    defaults: UserDefaults = .standard,
    transport: ActivationPingTransport = URLSessionActivationPingTransport(),
    appVersion: @escaping () -> String = {
      Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
        ?? "unknown"
    },
    operatingSystem: @escaping () -> String = {
      ProcessInfo.processInfo.operatingSystemVersionString
    }
  ) {
    self.configuration = configuration
    self.defaults = defaults
    self.transport = transport
    self.appVersion = appVersion
    self.operatingSystem = operatingSystem
  }

  func recordFirstSuccessfulDictation() async -> ActivationPingOutcome {
    guard defaults.bool(forKey: Self.optInDefaultsKey) else { return .notConsented }
    guard configuration.isEnabled else { return .disabled }
    guard !defaults.bool(forKey: Self.sentDefaultsKey) else { return .alreadySent }
    guard !sendInFlight else { return .inFlight }

    sendInFlight = true
    defer { sendInFlight = false }

    do {
      let request = try makeRequest()
      let (_, response) = try await transport.data(for: request)
      guard let http = response as? HTTPURLResponse,
        (200..<300).contains(http.statusCode)
      else {
        Self.logger.error("activation ping rejected by analytics endpoint")
        return .failed
      }
      defaults.set(true, forKey: Self.sentDefaultsKey)
      Self.logger.info("first successful dictation activation ping sent")
      return .sent
    } catch {
      Self.logger.error(
        "activation ping failed: \(error.localizedDescription, privacy: .public)"
      )
      return .failed
    }
  }

  private func makeRequest() throws -> URLRequest {
    let version = appVersion()
    let payload: [String: Any] = [
      "name": "first_successful_dictation",
      "url": "https://\(configuration.domain)/app/activation",
      "domain": configuration.domain,
      "props": [
        "app_version": version,
        "os": operatingSystem(),
      ],
    ]
    var request = URLRequest(url: configuration.endpoint)
    request.httpMethod = "POST"
    request.setValue("application/json", forHTTPHeaderField: "Content-Type")
    request.setValue("Codescribe/\(version)", forHTTPHeaderField: "User-Agent")
    request.httpBody = try JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys])
    return request
  }
}
