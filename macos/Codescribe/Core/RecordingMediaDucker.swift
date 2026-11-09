import AppKit
import Observation

/// One app-owned output-volume policy, driven by the existing capture projection.
/// Dictation and open agent channels share it; ending one cannot restore while
/// the other still owns microphone capture. No speech or transcript is filtered.
@MainActor
@Observable
final class RecordingMediaDucker {
  private(set) var activeOutputs: [SystemAudioProcess] = []
  private(set) var playbackObserved = false
  private(set) var volumeState: RecordingMediaVolumeState = .unchanged
  private(set) var recording = false

  @ObservationIgnored private let volume: SpotifyVolumeController
  @ObservationIgnored private var monitor: SystemPlaybackMonitor?
  @ObservationIgnored private weak var overlay: OverlayState?
  @ObservationIgnored private var started = false
  @ObservationIgnored private var revision: UInt64 = 0
  @ObservationIgnored private var observationGeneration: UInt64 = 0
  @ObservationIgnored private var updateTask: Task<Void, Never>?
  @ObservationIgnored private var playbackToken: NSObjectProtocol?

  init(recoveryURL: URL? = nil) {
    let root = URL.applicationSupportDirectory
    volume = SpotifyVolumeController(
      recoveryURL: recoveryURL ?? root.appendingPathComponent("Codescribe/runtime/spotify-volume.json"))
  }

  func start(observing overlay: OverlayState) {
    guard !started else { return }
    started = true
    observationGeneration &+= 1
    self.overlay = overlay
    monitor = SystemPlaybackMonitor { [weak self] outputs in
      guard let self else { return }
      self.playbackObserved = outputs != nil
      self.activeOutputs = outputs ?? []
      self.reconcile()
    }
    monitor?.start()
    // Spotify's own playback edge also covers resuming on an IO stream that
    // stayed open while paused. The actor still queries actual player state.
    playbackToken = DistributedNotificationCenter.default().addObserver(
      forName: Notification.Name("com.spotify.client.PlaybackStateChanged"), object: nil, queue: .main
    ) { [weak self] _ in
      MainActor.assumeIsolated { self?.reconcile() }
    }
    observeCapture(generation: observationGeneration)
  }

  func stop() async {
    started = false
    observationGeneration &+= 1
    monitor?.stop()
    monitor = nil
    if let playbackToken { DistributedNotificationCenter.default().removeObserver(playbackToken) }
    playbackToken = nil
    updateTask?.cancel()
    recording = false
    revision &+= 1
    volumeState = await volume.update(
      recording: false, application: spotifyApplication, outputActive: false, revision: revision)
  }

  func retry() { reconcile() }

  private var spotifyApplication: SpotifyApplicationIdentity? {
    guard let app = NSRunningApplication.runningApplications(withBundleIdentifier: "com.spotify.client")
      .first(where: { !$0.isTerminated }), let launchDate = app.launchDate
    else { return nil }
    return SpotifyApplicationIdentity(pid: app.processIdentifier, launchDate: launchDate)
  }

  private func observeCapture(generation: UInt64) {
    guard started, observationGeneration == generation, let overlay else { return }
    let capturing = withObservationTracking {
      overlay.audioCaptureActive
    } onChange: { [weak self] in
      Task { @MainActor [weak self] in self?.observeCapture(generation: generation) }
    }
    recording = capturing
    reconcile()
  }

  private func reconcile() {
    guard started else { return }
    updateTask?.cancel()
    revision &+= 1
    let request = revision
    let capturing = recording
    let application = spotifyApplication
    let localOutput = playbackObserved && activeOutputs.contains {
      $0.pid == application?.pid || $0.bundleIdentifier?.hasPrefix("com.spotify.client") == true
    }
    updateTask = Task { [weak self, volume] in
      let result = await volume.update(
        recording: capturing, application: application, outputActive: localOutput, revision: request)
      guard !Task.isCancelled, let self, self.revision == request else { return }
      self.volumeState = result
    }
  }
}
