import CoreAudio
import Foundation

/// Edge for Settings when the audio hardware changes under it: a device
/// appears or disappears, or the system default input moves (AirPods put on,
/// a switch in System Settings). Settings reads the device list and the
/// controller's readiness verdict on demand, so without this edge an open
/// window kept naming the previous input as current and "ready".
///
/// CoreAudio delivers on the main queue; bursts (a Bluetooth headset adds
/// two devices and moves both defaults) collapse into one `onChange` after a
/// short quiet period. Stop explicitly: the listener outlives nothing.
@MainActor
final class AudioInputDeviceMonitor {
  static let quietPeriod: Duration = .milliseconds(300)

  private static let selectors: [AudioObjectPropertySelector] = [
    kAudioHardwarePropertyDevices,
    kAudioHardwarePropertyDefaultInputDevice,
  ]

  private let onChange: @MainActor () -> Void
  private var listener: AudioObjectPropertyListenerBlock?
  private var pending: Task<Void, Never>?

  init(onChange: @escaping @MainActor () -> Void) {
    self.onChange = onChange
  }

  var isObserving: Bool { listener != nil }

  func start() {
    guard listener == nil else { return }
    let block: AudioObjectPropertyListenerBlock = { [weak self] _, _ in
      MainActor.assumeIsolated { self?.hardwareChanged() }
    }
    for selector in Self.selectors {
      var address = Self.address(selector)
      AudioObjectAddPropertyListenerBlock(
        AudioObjectID(kAudioObjectSystemObject), &address, .main, block)
    }
    listener = block
  }

  func stop() {
    pending?.cancel()
    pending = nil
    guard let listener else { return }
    // The stored value is already a block, so removal sees the same object
    // that was registered.
    for selector in Self.selectors {
      var address = Self.address(selector)
      AudioObjectRemovePropertyListenerBlock(
        AudioObjectID(kAudioObjectSystemObject), &address, .main, listener)
    }
    self.listener = nil
  }

  /// One CoreAudio notification; the coalesced `onChange` follows the burst.
  func hardwareChanged() {
    pending?.cancel()
    pending = Task { [weak self] in
      try? await Task.sleep(for: Self.quietPeriod)
      guard !Task.isCancelled, let self else { return }
      self.pending = nil
      self.onChange()
    }
  }

  private static func address(_ selector: AudioObjectPropertySelector)
    -> AudioObjectPropertyAddress
  {
    AudioObjectPropertyAddress(
      mSelector: selector,
      mScope: kAudioObjectPropertyScopeGlobal,
      mElement: kAudioObjectPropertyElementMain)
  }
}
