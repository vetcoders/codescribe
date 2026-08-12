import Foundation
import Security

protocol LicenseKeychainStoring {
    func load() throws -> Data?
    func save(_ data: Data) throws
    func delete() throws
}

private struct PersistedLicense: Codable {
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
        return result as? Data
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
        let addStatus = SecItemAdd(add as CFDictionary, nil) // nosemgrep: swift.biometrics-and-auth.missing-user-auth.keychain-without-user-auth
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
            ?? "Keychain error \(status)"
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
        let keychain: LicenseKeychainStoring? = licenseKeychainDisabledByEnvironment()
            ? nil
            : SystemLicenseKeychain()
        return LicenseService(keychain: keychain, autoload: true)
    }()
    static let preview = LicenseService(keychain: nil, autoload: false)

    @Published private(set) var status: CsLicenseStatus = .unlicensed
    @Published private(set) var lastError: String?

    var canUseAgentic: Bool { status.agenticEntitled }
    var agenticBlockMessage: String {
        "Agentic requires a license. Basic dictation remains free."
    }

    private let keychain: LicenseKeychainStoring?
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
        if autoload { refresh() }
    }

    func refresh() {
        do {
            let stored = try keychain?.load().map { try JSONDecoder().decode(PersistedLicense.self, from: $0) }
            status = try statusBridge(
                stored?.key,
                stored?.lastOnlineValidation,
                Int64(now().timeIntervalSince1970)
            )
            lastError = nil
        } catch {
            status = .unlicensed
            lastError = String(describing: error)
        }
    }

    @discardableResult
    func activate(_ rawKey: String) -> Bool {
        let key = rawKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty else {
            lastError = "Enter a CSK1 license key."
            return false
        }
        do {
            let timestamp = Int64(now().timeIntervalSince1970)
            let validated = try activateBridge(key, timestamp)
            let persisted = try JSONEncoder().encode(PersistedLicense(
                key: key,
                lastOnlineValidation: timestamp
            ))
            try keychain?.save(persisted)
            status = validated
            lastError = nil
            return true
        } catch {
            status = .unlicensed
            lastError = String(describing: error)
            return false
        }
    }

    func removeLicense() {
        do {
            try keychain?.delete()
            status = .unlicensed
            lastError = nil
        } catch {
            lastError = String(describing: error)
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
