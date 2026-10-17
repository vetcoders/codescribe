import AppKit

/// Who asked the overlay for a presentation. `OverlayState` records it on the
/// presentation trace. Entering the overlay during an automatic fold brings
/// the expanded form back; a fold the user chose stays folded.
enum OverlayPresentationCause {
  /// A control, a resize gesture, a menu choice or an explicit window entry.
  case user
  /// Take start under "Show transcript by default", or a followed channel reply.
  case automatic
}

/// Finite idle deadline for an expanded overlay.
///
/// `OverlayState.presentationMode` stays the one presentation owner. This type
/// only remembers which compact form (mini or midi) an expansion replaced, and
/// when that expansion may return to it. It holds no transcript, conversation
/// or document state.
///
/// The chosen base replaces the 10 s default. `0` arms nothing: an expanded
/// overlay stays expanded. Reading time may extend an interval and never
/// shortens it below that base.
///
/// At most one wake is outstanding. Activity moves the deadline without
/// rescheduling; an early wake sleeps the remainder. Every cancellation bumps
/// one token, so a wake armed before a hide, a compact choice or a successor
/// expansion cannot act on the presentation that replaced it.
@MainActor
final class OverlayAutoCollapse {
  /// Schedules `wake` after `delay` seconds and returns its cancellation.
  typealias Scheduler =
    @MainActor (_ delay: TimeInterval, _ wake: @escaping @MainActor () -> Void) -> () -> Void

  /// Default idle interval when the user has not chosen one. Tests advance the
  /// clock by this value. A chosen base replaces it as the floor of every arm.
  static let idleSeconds: TimeInterval = 10
  /// The user's base, in seconds. `0` means never fold.
  var baseIdleSeconds: TimeInterval = OverlayAutoCollapse.idleSeconds

  static let liveScheduler: Scheduler = { delay, wake in
    let task = Task { @MainActor in
      try? await Task.sleep(nanoseconds: UInt64(max(0, delay) * 1_000_000_000))
      guard !Task.isCancelled else { return }
      wake()
    }
    return { task.cancel() }
  }

  /// The compact form an expansion replaced. Nil when nothing is owed a return.
  private(set) var restoreMode: OverlayPresentationMode?
  private(set) var deadline: TimeInterval?
  var isWakeScheduled: Bool { cancelWake != nil }
  /// Runs when a wake finds the deadline elapsed. The owner re-checks its holds.
  var onDeadline: (@MainActor () -> Void)?

  private let now: () -> TimeInterval
  private let schedule: Scheduler
  private var cancelWake: (() -> Void)?
  private var wakeToken: UInt64 = 0

  init(now: @escaping () -> TimeInterval, schedule: @escaping Scheduler) {
    self.now = now
    self.schedule = schedule
  }

  /// Remember the compact form an expansion replaced, once. A later expansion
  /// keeps the first memo. An expansion that is already expanded, and has no
  /// memo, folds back to midi.
  func noteExpansion(from mode: OverlayPresentationMode) {
    guard restoreMode == nil else { return }
    restoreMode = mode == .expanded ? .midi : mode
  }

  /// A compact form, including the fold itself: nothing to return to any more.
  func forget() {
    restoreMode = nil
    suspend()
  }

  /// Drop the deadline but keep the memo: the panel is hidden or the user is
  /// interacting. The next activity arms a fresh full interval.
  func suspend() {
    deadline = nil
    cancelScheduledWake()
  }

  /// A full idle/read interval from now. The floor is the chosen base, and a
  /// base of zero arms nothing. Reuses an outstanding wake.
  func restartDeadline(after interval: TimeInterval) {
    guard baseIdleSeconds > 0 else {
      suspend()
      return
    }
    guard restoreMode != nil else { return }
    let delay = max(baseIdleSeconds, interval)
    deadline = now() + delay
    if cancelWake == nil { scheduleWake(after: delay) }
  }

  private func scheduleWake(after delay: TimeInterval) {
    wakeToken &+= 1
    let token = wakeToken
    cancelWake = schedule(delay) { [weak self] in
      self?.wake(token: token)
    }
  }

  private func cancelScheduledWake() {
    wakeToken &+= 1
    cancelWake?()
    cancelWake = nil
  }

  private func wake(token: UInt64) {
    guard token == wakeToken else { return }
    cancelWake = nil
    guard restoreMode != nil, let deadline else { return }
    let remaining = deadline - now()
    if remaining > 0 {
      scheduleWake(after: remaining)
      return
    }
    self.deadline = nil
    onDeadline?()
  }
}

/// App-level menu and popover tracking. SwiftUI does not expose whether a
/// `Menu` or `.popover` in the overlay is open, and the uncertain-word explainer
/// is an `NSPopover`; all of them post these AppKit notifications on main.
/// Any open menu or popover holds the overlay's auto-fold; closing one
/// reports the end of that interaction.
///
/// Selector observers are removed by NotificationCenter when this object goes.
@MainActor
final class OverlayTransientInteractionMonitor: NSObject {
  private var open: Set<ObjectIdentifier> = []
  var isActive: Bool { !open.isEmpty }
  var onInteractionEnded: (() -> Void)?

  override init() {
    super.init()
    let center = NotificationCenter.default
    center.addObserver(
      self, selector: #selector(began(_:)), name: NSMenu.didBeginTrackingNotification, object: nil)
    center.addObserver(
      self, selector: #selector(ended(_:)), name: NSMenu.didEndTrackingNotification, object: nil)
    center.addObserver(
      self, selector: #selector(began(_:)), name: NSPopover.willShowNotification, object: nil)
    center.addObserver(
      self, selector: #selector(ended(_:)), name: NSPopover.didCloseNotification, object: nil)
  }

  @objc nonisolated private func began(_ note: Notification) {
    guard let object = note.object as AnyObject? else { return }
    let id = ObjectIdentifier(object)
    MainActor.assumeIsolated { begin(id) }
  }

  @objc nonisolated private func ended(_ note: Notification) {
    guard let object = note.object as AnyObject? else { return }
    let id = ObjectIdentifier(object)
    MainActor.assumeIsolated { end(id) }
  }

  func begin(_ id: ObjectIdentifier) {
    open.insert(id)
  }

  func end(_ id: ObjectIdentifier) {
    guard open.remove(id) != nil else { return }
    if open.isEmpty { onInteractionEnded?() }
  }
}
