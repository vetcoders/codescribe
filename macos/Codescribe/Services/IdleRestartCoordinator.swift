import AppKit
import Foundation

/// A restart request is admitted by the live capture owner, never by a stale bus read.
@MainActor
final class IdleRestartCoordinator {
  typealias Finalization = @MainActor @Sendable () -> Void
  typealias FinalizationScheduler = @MainActor (@escaping Finalization) -> Void

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
  private let reserveCapture: @MainActor () throws -> Bool
  private let scheduleFinalization: FinalizationScheduler
  /// The Founder talking to an agent includes typing to one: a focused
  /// composer, keys within the typing horizon or an unsent draft refuse the
  /// restart the same way a live take does (Maciej, 2026-10-23, after a
  /// restart-if-idle quit the app under a long message he was writing).
  private let founderComposing: @MainActor () -> Bool
  private let terminate: @MainActor () -> Void
  private var lease: FileHandle?
  private var handling = false

  init(
    root: URL, pid: Int32 = ProcessInfo.processInfo.processIdentifier,
    nonce: String = UUID().uuidString, bundle: URL = Bundle.main.bundleURL,
    commit: String = Bundle.main.object(forInfoDictionaryKey: "CSBuildCommit") as? String ?? "",
    acquireLease: @escaping @MainActor () async throws -> FileHandle,
    reserveCapture: @escaping @MainActor () throws -> Bool,
    founderComposing: @escaping @MainActor () -> Bool = { false },
    scheduleFinalization: @escaping FinalizationScheduler = { action in
      RunLoop.main.perform { MainActor.assumeIsolated { action() } }
    },
    terminate: @escaping @MainActor () -> Void
  ) throws {
    self.root = root
    self.ready = Ready(pid: pid, nonce: nonce, bundle: bundle.path, commit: commit)
    self.acquireLease = acquireLease
    self.reserveCapture = reserveCapture
    self.scheduleFinalization = scheduleFinalization
    self.founderComposing = founderComposing
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
      guard !founderComposing() else {
        handling = false
        try receipt(id, status: "refused", reason: "founder_composing")
        return
      }
      let heldLease = try await acquireLease()
      // The lease wait is a window too: words typed during it count. Check
      // them before the reservation, which closes capture admission for the
      // rest of the process and has no release: every refusal comes first.
      guard !founderComposing() else {
        try? heldLease.close()
        handling = false
        try receipt(id, status: "refused", reason: "founder_composing")
        return
      }
      // Leave the async MainActor job before terminate: enters its nested
      // event loop. The callback owns the lease and keeps handling true; its
      // final composing check and synchronous reservation cannot interleave
      // with typing or another queued MainActor job.
      scheduleFinalization { [self, heldLease] in
        finalize(id, heldLease: heldLease)
      }
    } catch {
      handling = false
      try? receipt(id, status: "refused", reason: "idle_lease_unavailable")
    }
  }

  private func finalize(_ id: String, heldLease: FileHandle) {
    guard !founderComposing() else {
      try? heldLease.close()
      handling = false
      try? receipt(id, status: "refused", reason: "founder_composing")
      return
    }
    do {
      guard try reserveCapture() else {
        try? heldLease.close()
        handling = false
        try? receipt(id, status: "refused", reason: "capture_busy_or_not_ready")
        return
      }
    } catch {
      try? heldLease.close()
      handling = false
      try? receipt(id, status: "refused", reason: "idle_lease_unavailable")
      return
    }
    lease = heldLease
    // Admission is now permanently closed. No refusal, await or dispatch
    // follows a successful reservation; receipt failure cannot cancel Quit.
    try? receipt(id, status: "accepted", reason: nil)
    terminate()
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
