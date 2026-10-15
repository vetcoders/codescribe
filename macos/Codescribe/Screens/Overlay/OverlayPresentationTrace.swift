import AppKit
import os

/// Ledger of the overlay's presentation morphs: every mode request, every
/// window-frame write that belongs to one, every frame the window actually
/// painted on the way, and the hands that can write the frame around a morph
/// (placement, content growth, a user resize or drag).
///
/// Forensics first (Maciej, 2026-10-14: mini ⇄ midi ⇄ expanded sometimes
/// jumps). A jump is attributed from this ledger — which writer, which frame,
/// how many milliseconds after the animation landed — instead of guessed from
/// a feeling. Mirrored to the unified log under `overlay-presentation`:
///
///     log stream --predicate 'subsystem == "com.vetcoders.codescribe" && category == "overlay-presentation"'
///
/// The ring keeps the last 512 entries; a morph paints 15–20 frames.
@MainActor
final class OverlayPresentationTrace {
  enum Kind: String {
    /// `OverlayState` was asked for a mode (user or automatic cause).
    case modeRequested = "mode.requested"
    /// The panel chose its one animation target.
    case frameTransitionBegan = "frame.began"
    /// The animator landed; the panel settled its size floors.
    case frameTransitionCompleted = "frame.completed"
    /// A caller forced the pending target without waiting for the animator.
    case frameTransitionSettled = "frame.settled"
    /// A live edge gesture owns the frame; the mode waits for its end.
    case frameTransitionDeferredByResize = "frame.deferred_resize"
    /// Every frame AppKit painted, animator steps included.
    case windowResized = "window.resized"
    /// The window moved (a drag, or a programmatic placement write).
    case windowMoved = "window.moved"
    /// `OverlayController.applyPlacement` wrote the frame from preferences.
    case placementApplied = "placement.applied"
    /// Placement waited for a transition or a gesture to end.
    case placementDeferred = "placement.deferred"
    /// `resizeForProjectedContent` grew the expanded panel for its text.
    case contentResize = "content.resize"
    case userResizeBegan = "user.resize.began"
    case userResizeEnded = "user.resize.ended"
    /// The panel rebuilt its cursor rectangles after a morph.
    case cursorRectsInvalidated = "cursor.invalidated"
  }

  struct Entry: Equatable {
    /// Seconds on the trace clock (process uptime in production).
    let at: TimeInterval
    let kind: Kind
    let from: OverlayPresentationMode?
    let mode: OverlayPresentationMode?
    let animated: Bool?
    let frame: NSRect?
    let target: NSRect?
    let generation: UInt64?
    let note: String?
  }

  static let shared = OverlayPresentationTrace()

  static let capacity = 512
  private(set) var entries: [Entry] = []
  /// Count of entries ever recorded; the ring drops the oldest past capacity.
  private(set) var recorded = 0
  /// Tests inject a clock; production reads process uptime.
  var clock: () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }
  /// Mirrors every entry to the unified log; tests keep it quiet.
  var mirrorsToLog = true

  private let log = Logger(subsystem: "com.vetcoders.codescribe", category: "overlay-presentation")

  func record(
    _ kind: Kind,
    from: OverlayPresentationMode? = nil,
    mode: OverlayPresentationMode? = nil,
    animated: Bool? = nil,
    frame: NSRect? = nil,
    target: NSRect? = nil,
    generation: UInt64? = nil,
    note: String? = nil
  ) {
    let entry = Entry(
      at: clock(), kind: kind, from: from, mode: mode, animated: animated, frame: frame,
      target: target, generation: generation, note: note)
    entries.append(entry)
    recorded += 1
    if entries.count > Self.capacity { entries.removeFirst(entries.count - Self.capacity) }
    if mirrorsToLog {
      let line = Self.line(entry)
      log.debug("\(line, privacy: .public)")
    }
  }

  func reset() {
    entries.removeAll(keepingCapacity: true)
    recorded = 0
  }

  /// Entries after the first `count` ever recorded, oldest first; a caller
  /// that remembers `recorded` before an action reads only what it caused.
  func entries(after count: Int) -> [Entry] {
    let dropped = recorded - entries.count
    let skip = max(0, count - dropped)
    return skip < entries.count ? Array(entries[skip...]) : []
  }

  /// One line per entry, relative milliseconds, for a report or a bus reply.
  func dump() -> String {
    guard let first = entries.first else { return "(empty)" }
    return entries.map { entry in
      let ms = Int(((entry.at - first.at) * 1000).rounded())
      return String(format: "%6d ms  ", ms) + Self.line(entry)
    }.joined(separator: "\n")
  }

  static func line(_ entry: Entry) -> String {
    var parts = [entry.kind.rawValue]
    if let from = entry.from, let mode = entry.mode {
      parts.append("\(from)→\(mode)")
    } else if let mode = entry.mode {
      parts.append("\(mode)")
    }
    if let animated = entry.animated { parts.append(animated ? "animated" : "instant") }
    if let generation = entry.generation { parts.append("gen=\(generation)") }
    if let frame = entry.frame { parts.append("frame=" + rect(frame)) }
    if let target = entry.target { parts.append("target=" + rect(target)) }
    if let note = entry.note, !note.isEmpty { parts.append(note) }
    return parts.joined(separator: " ")
  }

  private static func rect(_ rect: NSRect) -> String {
    String(
      format: "%.0f,%.0f %.0fx%.0f", rect.minX, rect.minY, rect.width, rect.height)
  }
}
