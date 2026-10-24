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
