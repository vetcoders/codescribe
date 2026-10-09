import AppKit

/// Who asked the overlay for a presentation. Only `OverlayState` reads it, to
/// decide whether an automatic expansion may later be undone.
enum OverlayPresentationCause {
  /// A control, a resize gesture, a menu choice or an explicit window entry.
  case user
  /// Take start under "Show transcript by default", or a followed channel reply.
  case automatic
}

/// Finite idle deadline for an automatically expanded overlay.
///
/// `OverlayState.presentationMode` stays the one presentation owner. This type
/// only remembers which compact form (mini or midi) an automatic expansion
/// replaced, and when that expansion may return to it. It holds no transcript,
/// conversation or document state.
///
/// At most one wake is outstanding. Activity moves the deadline without
/// rescheduling; an early wake sleeps the remainder. Every cancellation bumps
/// one token, so a wake armed before a hide, a manual choice or a successor
/// expansion cannot act on the presentation that replaced it.
@MainActor
final class OverlayAutoCollapse {
  /// Schedules `wake` after `delay` seconds and returns its cancellation.
  typealias Scheduler =
    @MainActor (_ delay: TimeInterval, _ wake: @escaping @MainActor () -> Void) -> () -> Void

  static let idleSeconds: TimeInterval = 10

  static let liveScheduler: Scheduler = { delay, wake in
    let task = Task { @MainActor in
      try? await Task.sleep(nanoseconds: UInt64(max(0, delay) * 1_000_000_000))
      guard !Task.isCancelled else { return }
      wake()
    }
    return { task.cancel() }
  }

  /// The compact form an automatic expansion replaced. Nil when the expanded
  /// form was chosen by the user or by no one.
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

  /// Remember the replaced compact form once. Later automatic expansions keep
  /// the first memo; an expansion from an already expanded form records none.
  func noteAutomaticExpansion(from mode: OverlayPresentationMode) {
    if restoreMode == nil, mode != .expanded { restoreMode = mode }
  }

  /// A manual choice, an automatic collapse or any compact form: nothing to
  /// return to any more.
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

  /// A full idle interval from now. Reuses an outstanding wake.
  func restartDeadline() {
    guard restoreMode != nil else { return }
    deadline = now() + Self.idleSeconds
    if cancelWake == nil { scheduleWake(after: Self.idleSeconds) }
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
/// Any open menu or popover holds the overlay's automatic collapse; closing one
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
