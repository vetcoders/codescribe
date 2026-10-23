import Foundation

/// One durable desktop fiszka (Maciej, 2026-10-22): a view instance separate
/// from the conversation it presents. One agent keeps one open fiszka
/// (Maciej, 2026-10-23); it owns its geometry and window state, observes the
/// one shared conversation/draft state and never copies the inventory, the
/// document or a recorder.
struct OverlayIntercomInstance: Codable, Equatable, Identifiable, Sendable {
  let id: UUID
  /// The exact conversation this instance was detached from. A digit or a
  /// display name never rebinds an instance to another agent (Olga,
  /// 2026-10-22); when this owner stops being current the instance presents
  /// the saved conversation with its live actions withdrawn.
  var digit: String
  var conversationID: String
  var ownerID: String
  var name: String
  /// Window geometry in screen coordinates. Codable keeps plain fields so the
  /// stored JSON never depends on NSCoder.
  var x: Double
  var y: Double
  var width: Double
  var height: Double
  /// Mini keeps only the name, activity and one audio slot. Closed stays
  /// recorded: restore brings a closed instance back closed, and the agent's
  /// pill remains the explicit road to show it again.
  var minimized: Bool
  var closed: Bool

  var frame: CGRect {
    get { CGRect(x: x, y: y, width: width, height: height) }
    set {
      x = newValue.origin.x
      y = newValue.origin.y
      width = newValue.size.width
      height = newValue.size.height
    }
  }

  init(
    id: UUID = UUID(), identity: OverlayIntercomCardIdentity, frame: CGRect,
    minimized: Bool = false, closed: Bool = false
  ) {
    self.id = id
    digit = identity.digit
    conversationID = identity.conversationID ?? ""
    ownerID = identity.ownerID ?? ""
    name = identity.name
    x = frame.origin.x
    y = frame.origin.y
    width = frame.size.width
    height = frame.size.height
    self.minimized = minimized
    self.closed = closed
  }

  /// The card identity this instance presents, for the shared conversation
  /// lookups. Placeholder identities (no owner) are never persisted — except
  /// the broadcast channel "0", whose conversation legitimately has no owner
  /// and can never rebind to another agent (Maciej, 2026-10-23: fiszki must
  /// take channel 0 too).
  func matches(_ conversation: OverlayConversation) -> Bool {
    conversationID == conversation.id && ownerID == (conversation.owner?.id ?? "")
      && digit == conversation.channel
  }
}

/// The whole durable desktop: detached instances plus the group's
/// proportional scale. One presentation owner in the app domain; layout never
/// leaks into the engine's runtime configuration.
struct OverlayIntercomDesktopState: Codable, Equatable, Sendable {
  static let minimumGroupScale: Double = 1
  static let maximumGroupScale: Double = 2.5

  var groupScale: Double = 1
  var instances: [OverlayIntercomInstance] = []

  /// Resizing the group works like resizing a window: the ratio between the
  /// requested and the current width becomes the new proportional scale for
  /// every attached fiszka. Detached instances keep their own geometry.
  static func groupScale(current: Double, currentWidth: Double, requestedWidth: Double) -> Double {
    guard currentWidth > 0, requestedWidth > 0 else { return current }
    let scaled = current * (requestedWidth / currentWidth)
    return min(max(scaled, minimumGroupScale), maximumGroupScale)
  }

  /// The one open fiszka of this exact conversation, if any. A second detach
  /// never mints a copy: it shows this window, and the group row stays as the
  /// road back to it (Maciej, 2026-10-23). A closed record is a saved window,
  /// not an open one; mini still counts as open.
  func openInstance(for identity: OverlayIntercomCardIdentity) -> OverlayIntercomInstance? {
    instances.first {
      !$0.closed && $0.conversationID == identity.conversationID
        && $0.ownerID == (identity.ownerID ?? "")
    }
  }

  mutating func upsert(_ instance: OverlayIntercomInstance) {
    if let index = instances.firstIndex(where: { $0.id == instance.id }) {
      instances[index] = instance
    } else {
      instances.append(instance)
    }
  }

  mutating func remove(_ id: UUID) {
    instances.removeAll { $0.id == id }
  }
}

/// Persistence for the durable desktop, beside the other chrome preferences.
/// Restore is a UI operation: it brings windows back as they were and never
/// opens a microphone, re-sends a message or changes recipients.
@MainActor
final class OverlayIntercomDesktopStore {
  static let key = "codescribe.overlay.intercom.desktop"

  private let defaults: UserDefaults

  init(defaults: UserDefaults = .standard) {
    self.defaults = defaults
  }

  func load() -> OverlayIntercomDesktopState {
    guard let data = defaults.data(forKey: Self.key),
      var state = try? JSONDecoder().decode(OverlayIntercomDesktopState.self, from: data)
    else { return OverlayIntercomDesktopState() }
    state.groupScale = min(
      max(state.groupScale, OverlayIntercomDesktopState.minimumGroupScale),
      OverlayIntercomDesktopState.maximumGroupScale)
    // A persisted instance without its exact conversation identity cannot be
    // presented or safely rebound; drop it rather than guess an owner. The
    // broadcast channel "0" is the one identity that owns no agent: it stays.
    state.instances.removeAll {
      $0.conversationID.isEmpty || ($0.ownerID.isEmpty && $0.digit != "0")
    }
    return state
  }

  func save(_ state: OverlayIntercomDesktopState) {
    guard let data = try? JSONEncoder().encode(state) else { return }
    defaults.set(data, forKey: Self.key)
  }
}
