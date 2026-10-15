import AppKit
import Foundation

/// A restart request is admitted by the live capture owner, never by a stale bus read.
@MainActor
final class IdleRestartCoordinator {
  struct Ready: Encodable {
    let schema = "codescribe.restart-if-idle.ready.v1"
    let pid: Int32
    let nonce: String
    let bundle: String
    let commit: String
  }

  struct Request: Codable {
    let schema: String
    let request: String
    let pid: Int32
    let nonce: String
    let created: Double
  }

  private let root: URL
  private let ready: Ready
  private let acquireLease: @MainActor () async throws -> FileHandle
  private let reserveCapture: @MainActor () async throws -> Bool
  private let terminate: @MainActor () -> Void
  private var lease: FileHandle?
  private var handling = false

  init(
    root: URL, pid: Int32 = ProcessInfo.processInfo.processIdentifier,
    nonce: String = UUID().uuidString, bundle: URL = Bundle.main.bundleURL,
    commit: String = Bundle.main.object(forInfoDictionaryKey: "CSBuildCommit") as? String ?? "",
    acquireLease: @escaping @MainActor () async throws -> FileHandle,
    reserveCapture: @escaping @MainActor () async throws -> Bool,
    terminate: @escaping @MainActor () -> Void
  ) throws {
    self.root = root
    self.ready = Ready(pid: pid, nonce: nonce, bundle: bundle.path, commit: commit)
    self.acquireLease = acquireLease
    self.reserveCapture = reserveCapture
    self.terminate = terminate
    try FileManager.default.createDirectory(
      at: root, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
    try write(ready, to: root.appendingPathComponent("\(pid).ready.json"))
  }

  func handle(_ url: URL) async {
    guard url.scheme == "codescribe", url.host == "restart-if-idle",
      let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
      let id = components.queryItems?.first(where: { $0.name == "request" })?.value,
      UUID(uuidString: id) != nil,
      let bytes = try? Data(contentsOf: root.appendingPathComponent("\(id).request.json")),
      let request = try? JSONDecoder().decode(Request.self, from: bytes),
      request.schema == "codescribe.restart-if-idle.request.v1",
      request.request == id, request.pid == ready.pid, request.nonce == ready.nonce
    else { return }
    let age = Date().timeIntervalSince1970 - request.created
    guard age >= -1, age <= 30 else {
      try? receipt(id, status: "refused", reason: "expired_request")
      return
    }
    guard !handling else {
      try? receipt(id, status: "refused", reason: "restart_in_progress")
      return
    }
    handling = true
    do {
      // Prove receipt storage works before closing capture admission.
      try receipt(id, status: "checking", reason: nil)
      let heldLease = try await acquireLease()
      guard try await reserveCapture() else {
        try? heldLease.close()
        handling = false
        try receipt(id, status: "refused", reason: "capture_busy_or_not_ready")
        return
      }
      lease = heldLease
      // Admission is now closed under the controller's start lock. Complete
      // normal termination even if writing the final receipt fails.
      try? receipt(id, status: "accepted", reason: nil)
      terminate()
    } catch {
      handling = false
      try? receipt(id, status: "refused", reason: "idle_lease_unavailable")
    }
  }

  func invalidate() {
    try? FileManager.default.removeItem(at: root.appendingPathComponent("\(ready.pid).ready.json"))
  }

  private func receipt(_ id: String, status: String, reason: String?) throws {
    struct Receipt: Encodable {
      let schema = "codescribe.restart-if-idle.receipt.v1"
      let request: String
      let pid: Int32
      let nonce: String
      let status: String
      let reason: String?
    }
    try write(
      Receipt(request: id, pid: ready.pid, nonce: ready.nonce, status: status, reason: reason),
      to: root.appendingPathComponent("\(id).receipt.json"))
  }

  private func write<T: Encodable>(_ value: T, to path: URL) throws {
    try JSONEncoder().encode(value).write(to: path, options: .atomic)
    try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: path.path)
  }
}
