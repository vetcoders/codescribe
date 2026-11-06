import Foundation
import XCTest
import os

@testable import Codescribe

/// The volume lease of `SpotifyVolumeController`, driven through an injected
/// player. Written by the integrator rather than by the author of the
/// controller; no Apple Event leaves these tests.
final class SpotifyVolumeControllerTests: XCTestCase {
  /// A player that answers like Spotify's scripting suite and can stand in
  /// for a listener who moves the volume between two of the ramp's steps.
  private final class FakePlayer: SpotifyVolumeAccess, @unchecked Sendable {
    private let lock = NSLock()
    private var current: Int32
    private var isActive: Bool
    private var written: [Int32] = []
    private var failure: NSError?
    private var listenerMoves: [Int: Int32] = [:]
    private var afterWrite: [Int: @Sendable () -> Void] = [:]

    init(volume: Int32, playing: Bool = true) {
      current = volume
      isActive = playing
    }

    var volume: Int32 { lock.withLock { current } }
    var sets: [Int32] { lock.withLock { written } }
    func listenerSets(_ value: Int32) { lock.withLock { current = value } }
    func fail(with error: NSError?) { lock.withLock { failure = error } }
    /// After the controller's `count`-th write, the listener moves the volume.
    func listenerMoves(afterWrite count: Int, to value: Int32) {
      lock.withLock { listenerMoves[count] = value }
    }

    /// Runs `action` right after the controller's `count`-th write, while the
    /// ramp that made it is still between two steps.
    func whenWritten(_ count: Int, _ action: @escaping @Sendable () -> Void) {
      lock.withLock { afterWrite[count] = action }
    }

    func isPlaying(pid: pid_t) throws -> Bool {
      try lock.withLock {
        if let failure { throw failure }
        return isActive
      }
    }

    func volume(pid: pid_t) throws -> Int32 {
      try lock.withLock {
        if let failure { throw failure }
        return current
      }
    }

    func setVolume(_ value: Int32, pid: pid_t) throws {
      let action = try lock.withLock { () throws -> (@Sendable () -> Void)? in
        if let failure { throw failure }
        written.append(value)
        current = listenerMoves[written.count] ?? value
        return afterWrite[written.count]
      }
      action?()
    }
  }

  private var directory: URL!
  private var recoveryURL: URL { directory.appendingPathComponent("spotify-volume.json") }
  private let spotify = SpotifyApplicationIdentity(
    pid: 4242, launchDate: Date(timeIntervalSinceReferenceDate: 800_000_000))
  private let relaunched = SpotifyApplicationIdentity(
    pid: 4242, launchDate: Date(timeIntervalSinceReferenceDate: 800_000_600))

  override func setUpWithError() throws {
    directory = FileManager.default.temporaryDirectory
      .appendingPathComponent("codescribe-spotify-volume-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
  }

  override func tearDownWithError() throws {
    try? FileManager.default.removeItem(at: directory)
  }

  private func controller(_ player: FakePlayer) -> SpotifyVolumeController {
    SpotifyVolumeController(recoveryURL: recoveryURL, access: player, stepDelay: .zero)
  }

  private var leaseOnDisk: Bool { FileManager.default.fileExists(atPath: recoveryURL.path) }

  func testRecordingLowersToThirtyPercentAndStopRestores() async {
    let player = FakePlayer(volume: 80)
    let volume = controller(player)
    let lowered = await volume.update(
      recording: true, application: spotify, outputActive: true, revision: 1)
    XCTAssertEqual(lowered, .lowered)
    XCTAssertEqual(player.volume, 24)
    XCTAssertEqual(player.sets, player.sets.sorted(by: >), "the fade only goes down")
    XCTAssertTrue(leaseOnDisk, "an owned volume survives a crash of the application")

    let writesWhileLowering = player.sets.count
    let held = await volume.update(
      recording: true, application: spotify, outputActive: true, revision: 2)
    XCTAssertEqual(held, .lowered)
    XCTAssertEqual(player.sets.count, writesWhileLowering, "a held volume is not written again")

    let restored = await volume.update(
      recording: false, application: spotify, outputActive: true, revision: 3)
    XCTAssertEqual(restored, .unchanged)
    XCTAssertEqual(player.volume, 80)
    XCTAssertFalse(leaseOnDisk)
  }

  func testAListenersMoveWhileHeldWinsAndStopRestoresNothing() async {
    let player = FakePlayer(volume: 80)
    let volume = controller(player)
    _ = await volume.update(recording: true, application: spotify, outputActive: true, revision: 1)
    player.listenerSets(55)
    let writes = player.sets.count

    for revision in UInt64(2)...3 {
      let state = await volume.update(
        recording: true, application: spotify, outputActive: true, revision: revision)
      XCTAssertEqual(state, .manualOverride)
    }
    XCTAssertFalse(leaseOnDisk, "the listener's value is not the controller's to restore")

    let stopped = await volume.update(
      recording: false, application: spotify, outputActive: true, revision: 4)
    XCTAssertEqual(stopped, .unchanged)
    XCTAssertEqual(player.volume, 55)
    XCTAssertEqual(player.sets.count, writes, "nothing was written after the listener moved it")
  }

  func testAListenersMoveDuringTheFadeStopsTheFade() async {
    let player = FakePlayer(volume: 80)
    player.listenerMoves(afterWrite: 3, to: 90)
    let state = await controller(player).update(
      recording: true, application: spotify, outputActive: true, revision: 1)
    XCTAssertEqual(state, .manualOverride)
    XCTAssertEqual(player.volume, 90)
    XCTAssertEqual(player.sets.count, 3)
    XCTAssertFalse(leaseOnDisk)
  }

  func testAListenersMoveDuringRestoreWinsAndTheNextRecordingStartsFromIt() async {
    let player = FakePlayer(volume: 80)
    let volume = controller(player)
    _ = await volume.update(recording: true, application: spotify, outputActive: true, revision: 1)
    player.listenerMoves(afterWrite: player.sets.count + 2, to: 10)

    let stopped = await volume.update(
      recording: false, application: spotify, outputActive: true, revision: 2)
    XCTAssertEqual(stopped, .manualOverride)
    XCTAssertEqual(player.volume, 10)
    XCTAssertFalse(leaseOnDisk)

    // An override ends with its recording: the next one lowers again, from
    // the volume the listener chose.
    let next = await volume.update(
      recording: true, application: spotify, outputActive: true, revision: 3)
    XCTAssertEqual(next, .lowered)
    XCTAssertEqual(player.volume, 3)
    _ = await volume.update(recording: false, application: spotify, outputActive: true, revision: 4)
    XCTAssertEqual(player.volume, 10)
  }

  func testARequestOlderThanTheLastOneChangesNothing() async {
    let player = FakePlayer(volume: 80)
    let volume = controller(player)
    _ = await volume.update(recording: true, application: spotify, outputActive: true, revision: 5)
    let writes = player.sets.count

    let stale = await volume.update(
      recording: false, application: spotify, outputActive: true, revision: 3)
    XCTAssertEqual(stale, .unchanged)
    XCTAssertEqual(player.volume, 24, "a late Stop from before the current Start restores nothing")
    XCTAssertEqual(player.sets.count, writes)
    XCTAssertTrue(leaseOnDisk)

    _ = await volume.update(recording: false, application: spotify, outputActive: true, revision: 6)
    XCTAssertEqual(player.volume, 80)
  }

  private typealias Pending = OSAllocatedUnfairLock<Task<RecordingMediaVolumeState, Never>?>

  /// A controller whose ramp really waits between steps, so a request sent
  /// from the player's write hook arrives in the middle of it.
  private func pacedController(_ player: FakePlayer) -> SpotifyVolumeController {
    SpotifyVolumeController(recoveryURL: recoveryURL, access: player, stepDelay: .milliseconds(30))
  }

  func testAStopDuringTheFadeTakesOverAndTheSupersededFadeWritesNothingMore() async {
    let player = FakePlayer(volume: 80)
    let volume = pacedController(player)
    let spotify = self.spotify
    let stop = Pending(initialState: nil)
    player.whenWritten(2) {
      stop.withLock {
        $0 = Task {
          await volume.update(
            recording: false, application: spotify, outputActive: true, revision: 2)
        }
      }
    }

    let started = await volume.update(
      recording: true, application: spotify, outputActive: true, revision: 1)
    let stopped = await stop.withLock { $0 }?.value
    XCTAssertEqual(started, .unchanged, "a superseded fade reports nothing of its own")
    XCTAssertEqual(stopped, .unchanged)
    XCTAssertEqual(player.volume, 80)
    XCTAssertFalse(leaseOnDisk)

    // Down until Stop arrived, then only up: the two ramps never alternate.
    let sets = player.sets
    let turn = sets.firstIndex(of: sets.min() ?? 0) ?? 0
    XCTAssertEqual(Array(sets[...turn]), sets[...turn].sorted(by: >), "\(sets)")
    XCTAssertEqual(Array(sets[turn...]), sets[turn...].sorted(), "\(sets)")
    XCTAssertEqual(sets.last, 80)
  }

  func testAStartDuringTheRestoreLowersAgainAndKeepsTheFirstVolume() async {
    let player = FakePlayer(volume: 80)
    let volume = pacedController(player)
    let spotify = self.spotify
    let stop = Pending(initialState: nil)
    let again = Pending(initialState: nil)
    player.whenWritten(2) {
      stop.withLock {
        $0 = Task {
          await volume.update(
            recording: false, application: spotify, outputActive: true, revision: 2)
        }
      }
    }
    player.whenWritten(4) {
      again.withLock {
        $0 = Task {
          await volume.update(
            recording: true, application: spotify, outputActive: true, revision: 3)
        }
      }
    }

    let started = await volume.update(
      recording: true, application: spotify, outputActive: true, revision: 1)
    let stopped = await stop.withLock { $0 }?.value
    let restarted = await again.withLock { $0 }?.value
    XCTAssertEqual(started, .unchanged)
    XCTAssertEqual(stopped, .unchanged, "a restore overtaken by a new recording clears nothing")
    XCTAssertEqual(restarted, .lowered)
    XCTAssertEqual(player.volume, 24, "\(player.sets)")
    XCTAssertTrue(leaseOnDisk)

    let restored = await volume.update(
      recording: false, application: spotify, outputActive: true, revision: 4)
    XCTAssertEqual(restored, .unchanged)
    XCTAssertEqual(player.volume, 80, "the lease still holds the volume from before the first Start")
    XCTAssertFalse(leaseOnDisk)
  }

  func testQuickStopAndStartEndsLoweredAndRestoresTheFirstVolume() async {
    for round in 0..<25 {
      let player = FakePlayer(volume: 80)
      let volume = controller(player)
      let base = UInt64(round * 10)
      let spotify = self.spotify
      async let start = volume.update(
        recording: true, application: spotify, outputActive: true, revision: base + 1)
      async let stop = volume.update(
        recording: false, application: spotify, outputActive: true, revision: base + 2)
      async let again = volume.update(
        recording: true, application: spotify, outputActive: true, revision: base + 3)
      _ = await (start, stop, again)

      let settled = await volume.update(
        recording: true, application: spotify, outputActive: true, revision: base + 4)
      XCTAssertEqual(settled, .lowered, "round \(round)")
      XCTAssertEqual(player.volume, 24, "round \(round): the lease still knows the first volume")
      XCTAssertTrue(
        player.sets.allSatisfy { (24...80).contains($0) },
        "round \(round): a write left the range of this lease, \(player.sets)")

      let restored = await volume.update(
        recording: false, application: spotify, outputActive: true, revision: base + 5)
      XCTAssertEqual(restored, .unchanged, "round \(round)")
      XCTAssertEqual(player.volume, 80, "round \(round)")
      XCTAssertFalse(leaseOnDisk, "round \(round)")
    }
  }

  func testPlaybackThatIsNotOnThisMacOrIsPausedIsNotTouched() async {
    let connect = FakePlayer(volume: 80)
    let elsewhere = await controller(connect).update(
      recording: true, application: spotify, outputActive: false, revision: 1)
    XCTAssertEqual(elsewhere, .unchanged)
    XCTAssertTrue(connect.sets.isEmpty, "Spotify Connect plays on another device")

    let paused = FakePlayer(volume: 80, playing: false)
    let silent = await controller(paused).update(
      recording: true, application: spotify, outputActive: true, revision: 1)
    XCTAssertEqual(silent, .unchanged)
    XCTAssertTrue(paused.sets.isEmpty)

    let absent = await controller(FakePlayer(volume: 80)).update(
      recording: true, application: nil, outputActive: true, revision: 1)
    XCTAssertEqual(absent, .unchanged)
    XCTAssertFalse(leaseOnDisk)
  }

  func testRecoveryRestoresTheSameProcessAfterACrash() async {
    let player = FakePlayer(volume: 80)
    _ = await controller(player).update(
      recording: true, application: spotify, outputActive: true, revision: 1)
    XCTAssertEqual(player.volume, 24)

    // A new controller is a new application launch reading the lease file.
    let recovered = await controller(player).update(
      recording: false, application: spotify, outputActive: false, revision: 1)
    XCTAssertEqual(recovered, .unchanged)
    XCTAssertEqual(player.volume, 80)
    XCTAssertFalse(leaseOnDisk)
  }

  func testRecoveryNeverWritesAPredecessorsVolumeIntoARelaunchedSpotify() async {
    let player = FakePlayer(volume: 80)
    _ = await controller(player).update(
      recording: true, application: spotify, outputActive: true, revision: 1)
    let writes = player.sets.count
    // Spotify quit and came back with its own volume.
    player.listenerSets(60)

    let recovered = await controller(player).update(
      recording: false, application: relaunched, outputActive: true, revision: 1)
    XCTAssertEqual(recovered, .unchanged)
    XCTAssertEqual(player.volume, 60)
    XCTAssertEqual(player.sets.count, writes)
    XCTAssertFalse(leaseOnDisk)
  }

  func testARelaunchDuringTheRecordingStartsANewLeaseFromTheNewVolume() async {
    let player = FakePlayer(volume: 80)
    let volume = controller(player)
    _ = await volume.update(recording: true, application: spotify, outputActive: true, revision: 1)
    player.listenerSets(70)

    let lowered = await volume.update(
      recording: true, application: relaunched, outputActive: true, revision: 2)
    XCTAssertEqual(lowered, .lowered)
    XCTAssertEqual(player.volume, 21)

    _ = await volume.update(
      recording: false, application: relaunched, outputActive: true, revision: 3)
    XCTAssertEqual(player.volume, 70, "the new process gets its own volume back, not 80")
  }

  func testARefusedAutomationPermissionIsToldApartFromAnUnavailablePlayer() async {
    let player = FakePlayer(volume: 80)
    player.fail(with: NSError(domain: NSOSStatusErrorDomain, code: -1743))
    let refused = await controller(player).update(
      recording: true, application: spotify, outputActive: true, revision: 1)
    XCTAssertEqual(refused, .permissionRequired)

    player.fail(with: NSError(domain: NSOSStatusErrorDomain, code: -600))
    let gone = await controller(player).update(
      recording: true, application: spotify, outputActive: true, revision: 1)
    XCTAssertEqual(gone, .unavailable)
    XCTAssertTrue(player.sets.isEmpty)
    XCTAssertFalse(leaseOnDisk)
  }
}
