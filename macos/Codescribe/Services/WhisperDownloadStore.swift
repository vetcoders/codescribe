import Combine
import Foundation

extension CsWhisperModelStatus {
  /// Placeholder for canvas / engine-less previews (no network, no disk probe).
  static let sampleUnavailable = CsWhisperModelStatus(
    available: false,
    embedded: false,
    path: nil,
    modelId: "whisper-large-v3-turbo",
    repo: "mlx-community/whisper-large-v3-turbo",
    sizeHint: "~1.6 GB"
  )
}

private enum WhisperDownloadEvent: Sendable {
  case progress(detail: String, fraction: Double?)
  case complete(path: String)
}

/// Value-only UniFFI callbacks; the store owns one ordered MainActor consumer.
final class WhisperDownloadProgressSink: CsWhisperDownloadListener, Sendable {
  private let continuation: AsyncStream<WhisperDownloadEvent>.Continuation

  fileprivate init(continuation: AsyncStream<WhisperDownloadEvent>.Continuation) {
    self.continuation = continuation
  }

  func onProgress(file: String, bytesDone: UInt64, bytesTotal: Int64) {
    let fraction: Double?
    if bytesTotal > 0 {
      fraction = min(1.0, Double(bytesDone) / Double(bytesTotal))
    } else {
      fraction = nil
    }
    let mbDone = Double(bytesDone) / 1_048_576.0
    let detail: String
    if bytesTotal > 0 {
      let mbTotal = Double(bytesTotal) / 1_048_576.0
      detail = String(format: "%@ · %.0f / %.0f MB", file, mbDone, mbTotal)
    } else {
      detail = String(format: "%@ · %.0f MB", file, mbDone)
    }
    continuation.yield(.progress(detail: detail, fraction: fraction))
  }

  func onComplete(path: String) {
    continuation.yield(.complete(path: path))
  }

  func finish() {
    continuation.finish()
  }
}

/// Process-lifetime owner of the opt-in Whisper download. Windows only observe
/// it; navigating or closing onboarding never cancels the download generation.
@MainActor
final class WhisperDownloadStore: ObservableObject {
  typealias StatusProvider = @MainActor () -> CsWhisperModelStatus
  typealias Download = @Sendable (any CsWhisperDownloadListener) async throws -> CsWhisperModelStatus

  static let shared = WhisperDownloadStore()

  @Published private(set) var status: CsWhisperModelStatus = .sampleUnavailable
  @Published private(set) var inFlight = false
  @Published private(set) var fraction: Double?
  @Published private(set) var detail: String?
  @Published private(set) var error: String?

  private let statusProvider: StatusProvider
  private let download: Download
  private var downloadTask: Task<Void, Never>?

  /// Injection keeps the integrator's fixtures offline. Construction itself
  /// does no disk or network work; refresh is an explicit surface lifecycle read.
  init(
    statusProvider: @escaping StatusProvider = { whisperModelStatus() },
    download: @escaping Download = { listener in
      try await downloadWhisperModel(listener: listener)
    }
  ) {
    self.statusProvider = statusProvider
    self.download = download
  }

  func refresh() {
    guard !inFlight else { return }
    status = statusProvider()
  }

  func start() {
    guard !inFlight else { return }
    refresh()
    guard !status.available else { return }
    // Claim the generation before enqueuing either task, so simultaneous
    // Download clicks from Settings and onboarding cannot start two transfers.
    inFlight = true
    fraction = nil
    detail = String(localized: "Starting download…", comment: "Whisper download starting status")
    error = nil
    let channel = AsyncStream<WhisperDownloadEvent>.makeStream()
    let sink = WhisperDownloadProgressSink(continuation: channel.continuation)
    let events = Task { @MainActor [self] in
      for await event in channel.stream {
        switch event {
        case .progress(let progressDetail, let progressFraction):
          detail = progressDetail
          fraction = progressFraction
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
        detail = installed.available
          ? String(localized: "Ready · \(location)", comment: "Whisper is ready; model path or id")
          : String(localized: "Download finished but model still unavailable",
                   comment: "Whisper download returned without usable weights")
        fraction = installed.available ? 1 : nil
        if !installed.available { error = detail }
      case .failure(let failure):
        error = failure.userFacingMessage
        detail = String(localized: "Download failed", comment: "Whisper download failed status")
        fraction = nil
      }
      inFlight = false
      downloadTask = nil
    }
  }
}
