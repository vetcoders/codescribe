import Foundation

/// The diagnostic log: a second file beside `codescribe.log` in
/// `~/.codescribe/data.noindex/logs/`
/// for disturbances worth reading after the fact — presentation morphs, policy
/// decisions, refusals (Maciej, 2026-10-17: a diagnostic log next to the main
/// one, for "zaburzenia"). The unified log stays the live stream; this file
/// outlives it, because debug-level unified entries are never persisted and a
/// morph that jumped an hour ago is otherwise gone.
///
/// One line per entry: `<ISO-8601 UTC, milliseconds> <category> <text>`. The
/// file is opened with `O_APPEND`, so another writer (the Rust core, a helper)
/// can append whole lines to the same file without clobbering. When the file
/// passes `rollBytes` at open it is rolled once to `diagnostic.log.1`; the
/// previous generation is overwritten.
final class DiagnosticLog: @unchecked Sendable {
  static let filename = "diagnostic.log"
  static let defaultRollBytes: UInt64 = 8 << 20

  /// The installed log for the running app, wired at launch; nil in tests and
  /// before launch, so no writer has to know whether a file exists.
  nonisolated(unsafe) private(set) static var installed: DiagnosticLog?

  /// `<churnDirectory>/logs/diagnostic.log`, the folder the main log lives in.
  /// `churnDirectory` is the core's high-churn folder
  /// (`CodescribeConfig.churnDir()`), which Spotlight does not index.
  static func url(churnDirectory: String) -> URL {
    URL(fileURLWithPath: churnDirectory, isDirectory: true)
      .appendingPathComponent("logs", isDirectory: true)
      .appendingPathComponent(filename, isDirectory: false)
  }

  @discardableResult
  static func install(churnDirectory: String) -> DiagnosticLog {
    let log = DiagnosticLog(url: url(churnDirectory: churnDirectory))
    installed = log
    return log
  }

  let url: URL
  let rollBytes: UInt64
  private let runID = UUID()
  private let lock = NSLock()
  private var handle: FileHandle?
  private var opened = false
  private let timestamp: ISO8601DateFormatter = {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    formatter.timeZone = TimeZone(identifier: "UTC")
    return formatter
  }()

  init(url: URL, rollBytes: UInt64 = DiagnosticLog.defaultRollBytes) {
    self.url = url
    self.rollBytes = rollBytes
  }

  /// Labels are source literals, never user text, errors, paths or payloads.
  /// A caller finishes at its actual result callback, not when it launches work.
  func beginInteraction(action: StaticString, surface: StaticString) -> Interaction {
    Interaction(log: self, action: action, surface: surface)
  }

  /// A missing sink must not change an action's execution or its result.
  static func beginInteraction(action: StaticString, surface: StaticString) -> Interaction {
    Interaction(log: installed, action: action, surface: surface)
  }

  enum InteractionOutcome: String, Encodable, Sendable {
    case succeeded, failed, cancelled, refused, unverified
  }

  enum InteractionEffect: String, Encodable, Sendable {
    case stateChanged, noChange, callbackCompleted, requestAccepted, unverified
  }

  enum InteractionCleanup: String, Encodable, Sendable {
    case complete, notRequired, pending, unverified
  }

  enum InteractionReason: String, Encodable, Sendable {
    case none, busy, notFound, ownerMismatch, permissionDenied, operationFailed
    case superseded, userCancelled, lifetimeEnded
  }

  /// This is diagnostic evidence, not a second state machine for the product.
  /// Racing completion/cancel callbacks share one handle and one terminal row.
  final class Interaction: @unchecked Sendable {
    let id = UUID()
    private let log: DiagnosticLog?
    private let action: String
    private let surface: String
    private let started = ProcessInfo.processInfo.systemUptime
    private let terminalLock = NSLock()
    private var finished = false

    fileprivate init(log: DiagnosticLog?, action: StaticString, surface: StaticString) {
      self.log = log
      self.action = action.description
      self.surface = surface.description
      record(phase: "started")
    }

    deinit {
      // Dropped work is not success or inferred user cancellation.
      finish(.unverified, effect: .unverified, cleanup: .unverified, reason: .lifetimeEnded)
    }

    @discardableResult
    /// True means this callback won the terminal race; it does not certify I/O.
    /// Missing diagnostic rows must remain missing evidence in an audit.
    func finish(
      _ outcome: InteractionOutcome,
      effect: InteractionEffect,
      cleanup: InteractionCleanup,
      reason: InteractionReason = .none
    ) -> Bool {
      terminalLock.lock()
      defer { terminalLock.unlock() }
      guard !finished else { return false }
      finished = true
      record(
        phase: "completed", outcome: outcome, effect: effect, cleanup: cleanup, reason: reason)
      return true
    }

    private func record(
      phase: String,
      outcome: InteractionOutcome? = nil,
      effect: InteractionEffect? = nil,
      cleanup: InteractionCleanup? = nil,
      reason: InteractionReason? = nil
    ) {
      guard let log else { return }
      struct Event: Encodable {
        let schema = "codescribe.interaction.v1"
        let runID: UUID
        let actionID: UUID
        let action: String
        let surface: String
        let phase: String
        let durationMilliseconds: Double?
        let outcome: InteractionOutcome?
        let effect: InteractionEffect?
        let cleanup: InteractionCleanup?
        let reason: InteractionReason?
      }
      let event = Event(
        runID: log.runID, actionID: id, action: action, surface: surface, phase: phase,
        durationMilliseconds: phase == "completed"
          ? max(0, (ProcessInfo.processInfo.systemUptime - started) * 1000) : nil,
        outcome: outcome, effect: effect, cleanup: cleanup, reason: reason)
      let encoder = JSONEncoder()
      encoder.outputFormatting = [.sortedKeys]
      guard let data = try? encoder.encode(event), let text = String(data: data, encoding: .utf8)
      else { return }
      log.append(category: "interaction", text)
    }
  }

  deinit { try? handle?.close() }

  /// Appends one line. A log that cannot be opened stays silent after its
  /// first failed open: diagnostics never take the product down with them.
  func append(category: String, _ text: String, at date: Date = Date()) {
    lock.lock()
    defer { lock.unlock() }
    let line = timestamp.string(from: date) + " " + category + " " + text + "\n"
    if !opened { open() }
    guard let handle else { return }
    try? handle.write(contentsOf: Data(line.utf8))
  }

  /// The file's current lines, for a report or a test.
  func lines() throws -> [String] {
    let text = try String(contentsOf: url, encoding: .utf8)
    return text.split(separator: "\n", omittingEmptySubsequences: true).map(String.init)
  }

  private func open() {
    opened = true
    let manager = FileManager.default
    let directory = url.deletingLastPathComponent()
    try? manager.createDirectory(at: directory, withIntermediateDirectories: true)
    if let size = (try? manager.attributesOfItem(atPath: url.path)[.size]) as? NSNumber,
      size.uint64Value > rollBytes
    {
      let previous = url.appendingPathExtension("1")
      try? manager.removeItem(at: previous)
      try? manager.moveItem(at: url, to: previous)
    }
    let descriptor = Darwin.open(url.path, O_WRONLY | O_APPEND | O_CREAT | O_CLOEXEC, 0o600)
    guard descriptor >= 0 else { return }
    handle = FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
  }
}
