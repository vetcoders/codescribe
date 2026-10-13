import Combine
import Foundation

extension CsWhisperModelStatus {
  /// Placeholder for canvas / engine-less previews (no network, no disk probe).
  static let sampleUnavailable = CsWhisperModelStatus(
    available: false,
    embedded: false,
    path: nil,
    modelsDir: "~/.codescribe/models",
    modelId: "whisper-large-v3-turbo",
    repo: "mlx-community/whisper-large-v3-turbo",
    sizeHint: "~1.6 GB"
  )
}

/// How the last transfer of this process ended without installing the model.
enum WhisperDownloadInterruption: Equatable, Sendable {
  /// The user stopped it; Rust removed the partial file.
  case cancelled
  /// The transfer failed, or returned without usable weights.
  case failed
}

private enum WhisperDownloadEvent: Sendable {
  /// `at` is stamped where the callback arrives from the transfer thread, not
  /// where the main actor finally paints it, so a rate computed from two events
  /// measures the network instead of how far behind the UI has fallen.
  case progress(file: String, bytesDone: UInt64, bytesTotal: Int64, at: Date)
  case complete(path: String)
}

/// One-way flag the store raises on the main actor and the Rust transfer thread
/// polls between body chunks.
private final class CancellationFlag: @unchecked Sendable {
  private let lock = NSLock()
  private var cancelled = false

  var isCancelled: Bool { lock.withLock { cancelled } }

  func cancel() { lock.withLock { cancelled = true } }
}

/// Value-only UniFFI callbacks; the store owns one ordered MainActor consumer.
/// The same object also answers the transfer's cancellation question, so a
/// stopped generation can never stop the one started after it.
final class WhisperDownloadProgressSink: CsWhisperDownloadListener, Sendable {
  private let continuation: AsyncStream<WhisperDownloadEvent>.Continuation
  private let cancellation = CancellationFlag()
  private let clock: @Sendable () -> Date

  fileprivate init(
    continuation: AsyncStream<WhisperDownloadEvent>.Continuation,
    clock: @escaping @Sendable () -> Date
  ) {
    self.continuation = continuation
    self.clock = clock
  }

  func onProgress(file: String, bytesDone: UInt64, bytesTotal: Int64) {
    continuation.yield(
      .progress(file: file, bytesDone: bytesDone, bytesTotal: bytesTotal, at: clock()))
  }

  func onComplete(path: String) {
    continuation.yield(.complete(path: path))
  }

  func isCancelled() -> Bool { cancellation.isCancelled }

  /// Ask the running transfer to stop. Rust clears the partial file and fails
  /// the call; the store turns that failure into the cancelled state.
  func requestCancel() { cancellation.cancel() }

  func finish() {
    continuation.finish()
  }
}

/// Process-lifetime owner of the opt-in Whisper download. Windows only observe
/// it; navigating or closing onboarding never cancels the download generation.
@MainActor
final class WhisperDownloadStore: ObservableObject {
  typealias StatusProvider = @MainActor () -> CsWhisperModelStatus
  typealias Download =
    @Sendable (any CsWhisperDownloadListener) async throws -> CsWhisperModelStatus

  static let shared = WhisperDownloadStore()

  @Published private(set) var status: CsWhisperModelStatus = .sampleUnavailable
  @Published private(set) var inFlight = false
  /// A stop was requested and the transfer has not ended yet.
  @Published private(set) var cancelling = false
  @Published private(set) var fraction: Double?
  @Published private(set) var detail: String?
  @Published private(set) var error: String?
  /// Set when the last transfer stopped without installing the model, so a
  /// stopped download never reads as one that was never started. Cleared by
  /// the next start and by weights that are present.
  @Published private(set) var interruption: WhisperDownloadInterruption?

  /// The one reading setup and Settings take of the local model.
  var readiness: LocalModelReadiness {
    if status.available { return .installed }
    if inFlight { return .downloading }
    switch interruption {
    case .cancelled: return .cancelled
    case .failed: return .failed
    case nil: return .notInstalled
    }
  }

  private let statusProvider: StatusProvider
  private let download: Download
  /// Stamps progress events on the transfer thread; injected so a fixture can
  /// hand-wind it.
  private let now: @Sendable () -> Date
  private var downloadTask: Task<Void, Never>?
  private var sink: WhisperDownloadProgressSink?
  private var rateSamples: [(at: Date, bytes: UInt64)] = []
  private var sampledFile: String?

  private static let bytesPerMegabyte = 1_048_576.0
  /// Rate is averaged over the recent past, so a stalled or bursty connection
  /// moves the number instead of freezing the first reading on the screen.
  private static let rateWindow: TimeInterval = 5
  private static let shortestRateInterval: TimeInterval = 0.75

  /// Injection keeps the integrator's fixtures offline. Construction itself
  /// does no disk or network work; refresh is an explicit surface lifecycle read.
  init(
    statusProvider: @escaping StatusProvider = { whisperModelStatus() },
    download: @escaping Download = { listener in
      try await downloadWhisperModel(listener: listener)
    },
    now: @escaping @Sendable () -> Date = { Date() }
  ) {
    self.statusProvider = statusProvider
    self.download = download
    self.now = now
  }

  func refresh() {
    guard !inFlight else { return }
    status = statusProvider()
    if status.available { interruption = nil }
  }

  func start() {
    guard !inFlight else { return }
    refresh()
    guard !status.available else { return }
    // Claim the generation before enqueuing either task, so simultaneous
    // Download clicks from Settings and onboarding cannot start two transfers.
    inFlight = true
    cancelling = false
    fraction = nil
    detail = String(localized: "Starting download…", comment: "Whisper download starting status")
    error = nil
    interruption = nil
    rateSamples.removeAll()
    sampledFile = nil
    // Keep only the newest progress: the main actor repaints the whole window
    // per event and falls behind a fast transfer, and an unbounded backlog is
    // then painted long after the bytes arrived. A `.complete` dropped by the
    // same policy costs nothing — the terminal outcome comes from the awaited
    // `download(sink)` result below, which sets detail and fraction itself.
    let channel = AsyncStream<WhisperDownloadEvent>.makeStream(
      bufferingPolicy: .bufferingNewest(1))
    let sink = WhisperDownloadProgressSink(continuation: channel.continuation, clock: now)
    self.sink = sink
    let events = Task { @MainActor [self] in
      for await event in channel.stream {
        switch event {
        case .progress(let file, let bytesDone, let bytesTotal, let at):
          // Once a stop is asked for, the screen belongs to "Stopping the
          // download…" until the terminal outcome. Events still in flight say
          // nothing about a transfer the user has already ended.
          guard !cancelling else { continue }
          detail = Self.progressLine(
            file: file, bytesDone: bytesDone, bytesTotal: bytesTotal,
            bytesPerSecond: observedRate(file: file, bytesDone: bytesDone, at: at))
          if bytesTotal > 0 {
            fraction = min(1.0, Double(bytesDone) / Double(bytesTotal))
          } else {
            fraction = nil
          }
        case .complete(let path):
          detail = String(
            localized: "Saved · \(path)",
            comment: "Download callback completed; the placeholder is a file path"
          )
          fraction = 1
        }
      }
    }
    downloadTask = Task { @MainActor [self] in
      let result: Result<CsWhisperModelStatus, Error>
      do {
        result = .success(try await download(sink))
      } catch {
        result = .failure(error)
      }
      sink.finish()
      // Drain callbacks before publishing the terminal outcome. An onComplete
      // callback must never overwrite an error or the final availability verdict.
      await events.value
      switch result {
      case .success(let installed):
        status = installed
        let location = installed.path ?? installed.modelId
        detail =
          installed.available
          ? String(localized: "Ready · \(location)", comment: "Whisper is ready; model path or id")
          : String(
            localized: "Download finished but model still unavailable",
            comment: "Whisper download returned without usable weights")
        fraction = installed.available ? 1 : nil
        if !installed.available { error = detail }
        interruption = installed.available ? nil : .failed
      case .failure where cancelling:
        // A stop is not a failure: Rust removed the partial file, so the step
        // goes back to offering the download instead of reporting an error.
        // It is still not an install: `interruption` keeps that visible.
        status = statusProvider()
        detail = nil
        fraction = nil
        error = nil
        interruption = status.available ? nil : .cancelled
      case .failure(let failure):
        error = failure.userFacingMessage
        detail = String(localized: "Download failed", comment: "Whisper download failed status")
        fraction = nil
        interruption = .failed
      }
      inFlight = false
      cancelling = false
      self.sink = nil
      rateSamples.removeAll()
      sampledFile = nil
      downloadTask = nil
    }
  }

  /// Stop the running transfer. Rust deletes the partial file it was writing,
  /// so the model directory never holds a half-downloaded bundle.
  func cancel() {
    guard inFlight, !cancelling else { return }
    cancelling = true
    sink?.requestCancel()
    detail = String(
      localized: "Stopping the download…", comment: "Whisper download is being cancelled")
    fraction = nil
  }

  /// Transfer rate in bytes per second over the recent window, or `nil` while
  /// too little has been seen of the current file to say anything honest.
  private func observedRate(file: String, bytesDone: UInt64, at: Date) -> Double? {
    // Progress is reported per file, so a new file starts a new measurement.
    if file != sampledFile {
      sampledFile = file
      rateSamples.removeAll()
    }
    rateSamples.append((at: at, bytes: bytesDone))
    rateSamples.removeAll { at.timeIntervalSince($0.at) > Self.rateWindow }
    guard let first = rateSamples.first, let last = rateSamples.last else { return nil }
    let seconds = last.at.timeIntervalSince(first.at)
    guard seconds >= Self.shortestRateInterval, last.bytes > first.bytes else { return nil }
    return Double(last.bytes - first.bytes) / seconds
  }

  /// One line under the progress bar: what is being fetched, how far it is, and
  /// — once the rate is known — how fast and how much longer.
  static func progressLine(
    file: String, bytesDone: UInt64, bytesTotal: Int64, bytesPerSecond: Double?
  ) -> String {
    let done = megabytes(Double(bytesDone))
    guard bytesTotal > 0 else {
      return String(
        localized: "\(file) · \(done) MB",
        comment:
          "Download progress without a known total: file name, then megabytes received")
    }
    let total = megabytes(Double(bytesTotal))
    guard let bytesPerSecond, bytesPerSecond > 0 else {
      return String(
        localized: "\(file) · \(done) / \(total) MB",
        comment:
          "Download progress: file name, megabytes received, megabytes expected")
    }
    let rate = (bytesPerSecond / bytesPerMegabyte).formatted(
      .number.precision(.fractionLength(1)).grouping(.never))
    let remaining = max(0, Double(bytesTotal) - Double(bytesDone)) / bytesPerSecond
    let left = remainingTime(seconds: remaining)
    return String(
      localized: "\(file) · \(done) / \(total) MB · \(rate) MB/s · about \(left)",
      comment:
        "Download progress: file name, megabytes received, megabytes expected, megabytes per second, time left"
    )
  }

  private static func megabytes(_ bytes: Double) -> String {
    (bytes / bytesPerMegabyte).formatted(.number.precision(.fractionLength(0)).grouping(.never))
  }

  /// Time left in one unit, rounded: seconds below a minute, whole minutes above.
  private static func remainingTime(seconds: Double) -> String {
    guard seconds >= 45 else {
      return Duration.seconds(max(1, seconds.rounded()))
        .formatted(.units(allowed: [.seconds], width: .abbreviated))
    }
    let minutes = max(1, (seconds / 60).rounded())
    return Duration.seconds(minutes * 60)
      .formatted(.units(allowed: [.minutes], width: .abbreviated))
  }
}
