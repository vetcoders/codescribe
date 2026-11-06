import Foundation

struct SpotifyApplicationIdentity: Codable, Equatable, Sendable {
  let pid: pid_t
  let launchDate: Date
}

enum RecordingMediaVolumeState: Equatable, Sendable {
  case unchanged
  case lowered
  case manualOverride
  case permissionRequired
  case unavailable
}

/// Only this actor sends volume events. It never sends play/pause, changes
/// system volume, launches Spotify, or touches the recorder's PCM.
actor SpotifyVolumeController {
  private struct Lease: Codable {
    let application: SpotifyApplicationIdentity
    let original: Int32
    let target: Int32
    var lastApplied: Int32
  }

  private let recoveryURL: URL
  private let access: any SpotifyVolumeAccess
  private let stepDelay: Duration
  private var lease: Lease?
  private var recoveryRead = false
  private var revision: UInt64 = 0
  private var overridden = false
  private var wasRecording = false

  init(
    recoveryURL: URL, access: any SpotifyVolumeAccess = SpotifyAppleEvents(),
    stepDelay: Duration = .milliseconds(60)
  ) {
    self.recoveryURL = recoveryURL
    self.access = access
    self.stepDelay = stepDelay
  }

  func update(
    recording: Bool, application: SpotifyApplicationIdentity?, outputActive: Bool,
    revision request: UInt64
  ) async
    -> RecordingMediaVolumeState
  {
    guard request >= revision else { return .unchanged }
    revision = request
    if recording && !wasRecording { overridden = false }
    wasRecording = recording
    if !recoveryRead {
      recoveryRead = true
      if let data = try? Data(contentsOf: recoveryURL),
        let saved = try? JSONDecoder().decode(Lease.self, from: data),
        saved.application.pid > 0,
        [saved.original, saved.target, saved.lastApplied].allSatisfy({ (0...100).contains($0) })
      {
        lease = saved
      }
    }
    // A new Spotify process has its own volume. Never apply a predecessor's
    // restoration to a relaunched application, even if the recording stayed open.
    if let held = lease, held.application != application { clearLease() }
    do {
      if !recording {
        overridden = false
        guard let held = lease else { return .unchanged }
        return try await ramp(to: held.original, restoring: true, revision: request)
      }
      guard !overridden else { return .manualOverride }
      guard let application else { return .unchanged }
      if lease == nil {
        // A playing Spotify Connect session need not output on this Mac.
        // HAL activity is a prerequisite only for taking volume ownership;
        // a pause does not release an existing recording's volume lease.
        guard outputActive else { return .unchanged }
        guard try access.isPlaying(pid: application.pid) else { return .unchanged }
        let original = try access.volume(pid: application.pid)
        lease = Lease(
          application: application, original: original,
          target: Int32((Double(original) * 0.3).rounded()), lastApplied: original)
        do { try saveLease() }
        catch { clearLease(); throw error }
      }
      guard let held = lease else { return .unchanged }
      return try await ramp(to: held.target, restoring: false, revision: request)
    } catch {
      return (error as NSError).code == -1743 ? .permissionRequired : .unavailable
    }
  }

  private func ramp(to target: Int32, restoring: Bool, revision request: UInt64) async throws
    -> RecordingMediaVolumeState
  {
    guard let initial = lease else { return .unchanged }
    for step in 1...10 {
      guard request == revision, !Task.isCancelled, let held = lease else { return .unchanged }
      guard try access.volume(pid: held.application.pid) == held.lastApplied else {
        // The user or another volume owner moved it. Their value wins, both
        // during the ramp and when Stop or normal application shutdown arrives.
        clearLease()
        overridden = true
        return .manualOverride
      }
      if held.lastApplied == target {
        if restoring { clearLease() }
        return restoring ? .unchanged : .lowered
      }
      let fraction = Double(step) / 10
      let eased = fraction * fraction * (3 - 2 * fraction)
      let next = Int32(
        (Double(initial.lastApplied) + Double(target - initial.lastApplied) * eased).rounded())
      if next != held.lastApplied {
        try access.setVolume(next, pid: held.application.pid)
        lease?.lastApplied = next
        do { try saveLease() }
        catch {
          // A full disk may stop lowering, but must not prevent an already
          // owned in-memory volume from being restored on Stop.
          if !restoring { throw error }
        }
      }
      if step < 10 {
        do { try await Task.sleep(for: stepDelay) }
        catch { return .unchanged }
      }
    }
    guard request == revision, !Task.isCancelled else { return .unchanged }
    if restoring { clearLease() }
    return restoring ? .unchanged : .lowered
  }

  private func saveLease() throws {
    guard let lease else { return }
    try FileManager.default.createDirectory(
      at: recoveryURL.deletingLastPathComponent(), withIntermediateDirectories: true)
    try JSONEncoder().encode(lease).write(to: recoveryURL, options: .atomic)
  }

  private func clearLease() {
    lease = nil
    try? FileManager.default.removeItem(at: recoveryURL)
  }
}
