import Foundation
import Security

protocol LicenseKeychainStoring: Sendable {
  func load() throws -> Data?
  func save(_ data: Data) throws
  func delete() throws
}

private struct PersistedLicense: Codable, Sendable {
  let key: String
  let lastOnlineValidation: Int64
}

struct SystemLicenseKeychain: LicenseKeychainStoring {
  static let service = "com.vetcoders.codescribe.license"
  private static let account = "license"

  private let service: String
  private let account: String

  init(service: String = Self.service, account: String = Self.account) {
    self.service = service
    self.account = account
  }

  func load() throws -> Data? {
    let query: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: account,
      kSecReturnData as String: true,
      kSecMatchLimit as String: kSecMatchLimitOne,
    ]
    var result: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &result)
    if status == errSecItemNotFound { return nil }
    guard status == errSecSuccess else { throw LicenseKeychainError(status) }
    guard let data = result as? Data else { throw LicenseKeychainError(errSecDecode) }
    return data
  }

  func save(_ data: Data) throws {
    let key: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: account,
    ]
    let update = [kSecValueData as String: data]
    let updateStatus = SecItemUpdate(key as CFDictionary, update as CFDictionary)
    if updateStatus == errSecSuccess { return }
    guard updateStatus == errSecItemNotFound else {
      throw LicenseKeychainError(updateStatus)
    }
    var add = key
    add[kSecValueData as String] = data
    add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    // A signed license is an entitlement, not an authentication secret. It
    // must load unattended at app launch, so biometric user-presence access
    // control would break the local-first restore contract.
    // nosemgrep: swift.biometrics-and-auth.missing-user-auth.keychain-without-user-auth
    let addStatus = SecItemAdd(add as CFDictionary, nil)
    guard addStatus == errSecSuccess else { throw LicenseKeychainError(addStatus) }
  }

  func delete() throws {
    let query: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: account,
    ]
    let status = SecItemDelete(query as CFDictionary)
    guard status == errSecSuccess || status == errSecItemNotFound else {
      throw LicenseKeychainError(status)
    }
  }
}

struct LicenseKeychainError: LocalizedError {
  let status: OSStatus

  init(_ status: OSStatus) { self.status = status }

  var errorDescription: String? {
    SecCopyErrorMessageString(status, nil) as String?
      ?? String(
        localized: "Keychain error \(String(status))",
        comment: "Fallback when macOS has no message; the placeholder is the status code"
      )
  }
}

/// Keep the independent Swift license store on the same no-Keychain contract as
/// the Rust secret bundle. Xcode launches XCTest inside the app host before
/// `XCTestCase` is necessarily visible to lifecycle code, but it installs these
/// process markers before `App.body` evaluates `LicenseService.shared`.
/// A relocated data directory or generic CI flag is not proof of a test host
/// and must not discard persisted license truth.
func licenseKeychainDisabledByEnvironment(
  _ environment: [String: String] = ProcessInfo.processInfo.environment
) -> Bool {
  environment["CODESCRIBE_DISABLE_KEYCHAIN"] != nil
    || environment["XCTestConfigurationFilePath"] != nil
    || environment["XCTestSessionIdentifier"] != nil
    || environment["XCTestBundlePath"] != nil
}

@MainActor
final class LicenseService: ObservableObject {
  static let shared: LicenseService = {
    let keychain: LicenseKeychainStoring? =
      licenseKeychainDisabledByEnvironment()
      ? nil
      : SystemLicenseKeychain()
    return LicenseService(keychain: keychain, autoload: true)
  }()
  static let preview = LicenseService(keychain: nil, autoload: false)

  enum ReadState { case loading, available, unavailable }

  @Published private var persisted: PersistedLicense?
  @Published private(set) var readState: ReadState = .loading
  @Published private(set) var isBusy = false
  @Published private(set) var lastError: String?
  @Published private(set) var lastErrorDetails: String?

  // The signed payload is the authority; evaluate it at the current clock on
  // every gate read. A slow storage refresh must not freeze an active license
  // past updates_until or extend its original offline validation timestamp.
  var status: CsLicenseStatus {
    (try? statusBridge(
      persisted?.key, persisted?.lastOnlineValidation,
      Int64(now().timeIntervalSince1970)
    )) ?? .unlicensed
  }

  var canUseAgentic: Bool {
    let current = status
    guard current.agenticEntitled else { return false }
    switch current.state {
    case .active, .graceOffline: return true
    case .unlicensed, .expiredUpdates: return false
    }
  }

  var agenticBlockMessage: String {
    if persisted == nil, readState != .available {
      return readState == .loading
        ? String(localized: "Checking license…")
        : String(localized: "Couldn't read the saved license. Try again in Settings → License.")
    }
    return status.state == .expiredUpdates
      ? String(localized: "License access ended. Check Settings → License.")
      : String(localized: "Agent mode requires a license. Open Settings → License.")
  }

  private let keychain: (any LicenseKeychainStoring)?
  // Owned by this service, never by a view task. Cancellation does not release
  // isBusy: a synchronous SecItem call still owns the slot until it returns.
  private let storageQueue = DispatchQueue(label: "codescribe.license-storage", qos: .userInitiated)
  private let now: () -> Date
  private let activateBridge: (String, Int64) throws -> CsLicenseStatus
  private let statusBridge: (String?, Int64?, Int64) throws -> CsLicenseStatus

  init(
    keychain: LicenseKeychainStoring?,
    autoload: Bool,
    now: @escaping () -> Date = Date.init,
    activateBridge: @escaping (String, Int64) throws -> CsLicenseStatus = {
      try licenseActivate(key: $0, nowUnixSeconds: $1)
    },
    statusBridge: @escaping (String?, Int64?, Int64) throws -> CsLicenseStatus = {
      try licenseStatus(
        key: $0,
        lastOnlineValidationUnixSeconds: $1,
        nowUnixSeconds: $2
      )
    }
  ) {
    self.keychain = keychain
    self.now = now
    self.activateBridge = activateBridge
    self.statusBridge = statusBridge
    if autoload { refresh() } else { readState = .available }
  }

  private func storage<T: Sendable>(
    _ operation: @escaping @Sendable ((any LicenseKeychainStoring)?) throws -> T
  ) async throws -> T {
    let store = keychain
    return try await withCheckedThrowingContinuation { continuation in
      storageQueue.async {
        do { continuation.resume(returning: try operation(store)) }
        catch { continuation.resume(throwing: error) }
      }
    }
  }

  func refresh() {
    guard !isBusy else { return }
    isBusy = true
    readState = .loading
    lastError = nil
    lastErrorDetails = nil
    Task { @MainActor [self] in
      defer { isBusy = false }
      let data: Data?
      do {
        data = try await storage { try $0?.load() }
      } catch {
        // A storage failure is not evidence of absence. Retain the previously
        // verified payload and keep evaluating its time bounds normally.
        readState = .unavailable
        lastError = String(localized: "Couldn't read the saved license. Try again.")
        lastErrorDetails = error.localizedDescription
        return
      }
      do {
        let loaded = try data.map { try JSONDecoder().decode(PersistedLicense.self, from: $0) }
        _ = try statusBridge(
          loaded?.key, loaded?.lastOnlineValidation, Int64(now().timeIntervalSince1970)
        )
        persisted = loaded
        readState = .available
        lastError = nil
        lastErrorDetails = nil
      } catch {
        // Successfully read malformed or invalid signed data fails closed.
        persisted = nil
        readState = .unavailable
        lastError = String(localized: "Couldn't verify the saved license. Enter your key again.")
        lastErrorDetails = error.localizedDescription
      }
    }
  }

  @discardableResult
  func activate(_ rawKey: String) async -> Bool {
    guard !isBusy else { return false }
    let key = rawKey.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !key.isEmpty else {
      lastError = String(localized: "Enter a license key.")
      lastErrorDetails = nil
      return false
    }
    isBusy = true
    lastError = nil
    lastErrorDetails = nil
    defer { isBusy = false }
    do {
      let timestamp = Int64(now().timeIntervalSince1970)
      _ = try activateBridge(key, timestamp)
      let candidate = PersistedLicense(key: key, lastOnlineValidation: timestamp)
      let data = try JSONEncoder().encode(candidate)
      try await storage { try $0?.save(data) }
      // Publication follows durable success. A failed replacement never
      // discards an existing verified license or renews its grace timestamp.
      persisted = candidate
      readState = .available
      lastError = nil
      lastErrorDetails = nil
      return true
    } catch {
      lastError = String(localized: "Couldn't activate the key. Try again.")
      lastErrorDetails = error.localizedDescription
      return false
    }
  }

  func removeLicense() async {
    guard !isBusy else { return }
    isBusy = true
    lastError = nil
    lastErrorDetails = nil
    defer { isBusy = false }
    do {
      try await storage { try $0?.delete() }
      persisted = nil
      readState = .available
      lastError = nil
      lastErrorDetails = nil
    } catch {
      lastError = String(localized: "Couldn't remove the key. Try again.")
      lastErrorDetails = error.localizedDescription
    }
  }

}

extension CsLicenseStatus {
  static var unlicensed: Self {
    Self(
      state: .unlicensed,
      daysLeft: nil,
      sku: nil,
      emailHash: nil,
      issued: nil,
      updatesUntil: nil,
      seatLimit: nil,
      agenticEntitled: false
    )
  }
}
