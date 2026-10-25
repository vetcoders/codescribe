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

  var isHumanInbox: Bool {
    digit == "10" && conversationID == "10" && ownerID.isEmpty
  }
  var displayDigit: String { isHumanInbox ? "" : digit }
  var displayTitle: String { isHumanInbox ? name : "\(digit) · \(name)" }

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
  /// the broadcast channel "0" and the human inbox, which have no agent owner.
  /// Neither can rebind to an agent by name or channel reuse.
  func matches(_ conversation: OverlayConversation) -> Bool {
    conversationID == conversation.id && ownerID == (conversation.owner?.id ?? "")
      && digit == conversation.channel
  }
}

/// The whole durable desktop: detached instances plus the group's content
/// dimensions and tier. One presentation owner in the app domain; layout never
/// leaks into the engine's runtime configuration.
struct OverlayIntercomDesktopState: Codable, Equatable, Sendable {
  var groupWidth: Double? = nil
  var groupRowHeight: Double? = nil
  var groupTier: IntercomContentTier = .actions
  var instances: [OverlayIntercomInstance] = []

  /// The one open fiszka of this exact conversation, if any. A second detach
  /// never mints a copy: it shows this window (Maciej, 2026-10-23). A closed
  /// record is a saved window, not an open one; mini still counts as open.
  func openInstance(for identity: OverlayIntercomCardIdentity) -> OverlayIntercomInstance? {
    instances.first {
      !$0.closed && $0.conversationID == identity.conversationID
        && $0.ownerID == (identity.ownerID ?? "")
    }
  }

  /// Detaching changes the conversation's window, not roster membership.
  /// Keep every current agent in order, including an open or minimized fiszka
  /// (Founder decision, 2026-10-25).
  func groupRows(of identities: [OverlayIntercomCardIdentity]) -> [OverlayIntercomCardIdentity] {
    identities
  }

  mutating func upsert(_ instance: OverlayIntercomInstance) {
    if let index = instances.firstIndex(where: { $0.id == instance.id }) {
      instances[index] = instance
    } else {
      instances.append(instance)
    }
  }

  /// Archiving retires open windows of the exact lease, without deleting
  /// geometry, messages or drafts. A later explicit history reopen is separate.
  @discardableResult
  mutating func closeArchivedInstances(owners: Set<OverlayConversationOwner>) -> Bool {
    var changed = false
    for index in instances.indices where !instances[index].closed {
      if owners.contains(where: {
        $0.id == instances[index].ownerID && $0.channel == instances[index].digit
      }) {
        instances[index].closed = true
        changed = true
      }
    }
    return changed
  }

  mutating func remove(_ id: UUID) {
    instances.removeAll { $0.id == id }
  }
}

extension OverlayIntercomDesktopState {
  private enum CodingKeys: String, CodingKey {
    case groupWidth, groupRowHeight, groupTier, instances
  }

  /// A desktop saved before content tiers still owns its detached instances.
  /// Missing dimensions let the current content establish their resting size;
  /// an obsolete scale never changes text or glyph metrics on restore.
  init(from decoder: Decoder) throws {
    let values = try decoder.container(keyedBy: CodingKeys.self)
    let width = try? values.decode(Double.self, forKey: .groupWidth)
    let rowHeight = try? values.decode(Double.self, forKey: .groupRowHeight)
    groupWidth = width.flatMap { $0.isFinite && $0 > 0 ? $0 : nil }
    groupRowHeight = rowHeight.flatMap { $0.isFinite && $0 > 0 ? $0 : nil }
    groupTier = (try? values.decode(IntercomContentTier.self, forKey: .groupTier)) ?? .actions
    instances = try values.decodeIfPresent([OverlayIntercomInstance].self, forKey: .instances) ?? []
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
    // A persisted instance without its exact conversation identity cannot be
    // presented or safely rebound; drop it rather than guess an owner. The
    // broadcast channel and the exact human inbox own no agent: both stay.
    state.instances.removeAll {
      $0.conversationID.isEmpty
        || ($0.ownerID.isEmpty && $0.digit != "0" && !$0.isHumanInbox)
    }
    return state
  }

  func save(_ state: OverlayIntercomDesktopState) {
    guard let data = try? JSONEncoder().encode(state) else { return }
    defaults.set(data, forKey: Self.key)
  }
}
