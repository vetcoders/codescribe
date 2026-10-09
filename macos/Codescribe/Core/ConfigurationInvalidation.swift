import Combine
import Foundation

/// Payload-free "re-read canonical truth" edges for already-open projections.
///
/// `settings.json` → loader → immutable runtime snapshot stays the only
/// configuration authority. An event carries no setting value, so no observer
/// can apply one: each re-reads through its own engine. That is what separates
/// this from the retired `NotificationCenter` bus, which the acoustic
/// authority canary classifies as "parallel mutable runtime settings
/// propagation".
///
/// Scoped, not global: `AppModel` owns the single instance and hands it to the
/// Settings window model and the tray. View models built without one (tests,
/// previews, the Max-permission model) neither publish nor observe, so a mock
/// engine test can never wake a live core listener in the XCTest host
/// (`CodescribeTests/README.md`, bus fan-out).
@MainActor
final class ConfigurationInvalidation {
  enum Edge: Equatable {
    /// A projection persisted a key through its engine. Peers re-read
    /// `settings.json` truth; the writer already did.
    case settingsWritten
    /// The shared recorder crossed a start/stop edge. Resident Whisper weights
    /// load at a take start and a deferred model switch applies at the
    /// recording-idle boundary, so this is when residency can move.
    case recordingLifecycle
  }

  private struct Event {
    let edge: Edge
    let origin: ObjectIdentifier?
  }

  private let subject = PassthroughSubject<Event, Never>()

  func settingsWritten(by origin: AnyObject) {
    subject.send(Event(edge: .settingsWritten, origin: ObjectIdentifier(origin)))
  }

  func recordingLifecycleChanged() {
    subject.send(Event(edge: .recordingLifecycle, origin: nil))
  }

  /// Edges from every publisher except `observer` itself. A writer has
  /// already re-read its own snapshot, and observers only read, so no edge can
  /// cause another publish: there is no feedback or write loop.
  func edges(excluding observer: AnyObject) -> AnyPublisher<Edge, Never> {
    let own = ObjectIdentifier(observer)
    return
      subject
      .filter { $0.origin != own }
      .map(\.edge)
      .eraseToAnyPublisher()
  }
}
