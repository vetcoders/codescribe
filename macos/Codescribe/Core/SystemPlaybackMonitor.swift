import AppKit
import CoreAudio

struct SystemAudioProcess: Equatable, Identifiable, Sendable {
  let id: AudioObjectID
  let pid: pid_t
  let name: String
  let bundleIdentifier: String?
}

/// Observes HAL output activity, without tapping audio or opening a microphone.
/// Active IO is not a claim that the output contains music or reached the input.
@MainActor
final class SystemPlaybackMonitor {
  private let onChange: ([SystemAudioProcess]?) -> Void
  private var processListener: AudioObjectPropertyListenerBlock?
  private var outputListener: AudioObjectPropertyListenerBlock?
  private var observedProcesses: Set<AudioObjectID> = []
  private var pending: Task<Void, Never>?

  init(onChange: @escaping ([SystemAudioProcess]?) -> Void) {
    self.onChange = onChange
  }

  func start() {
    guard processListener == nil else { return }
    let block: AudioObjectPropertyListenerBlock = { [weak self] _, _ in
      MainActor.assumeIsolated { self?.scheduleRefresh() }
    }
    var address = Self.address(kAudioHardwarePropertyProcessObjectList)
    let result = AudioObjectAddPropertyListenerBlock(
      AudioObjectID(kAudioObjectSystemObject), &address, .main, block)
    guard result == noErr else { onChange(nil); return }
    processListener = block
    outputListener = block
    refresh()
  }

  func stop() {
    pending?.cancel()
    pending = nil
    if let processListener {
      var address = Self.address(kAudioHardwarePropertyProcessObjectList)
      AudioObjectRemovePropertyListenerBlock(
        AudioObjectID(kAudioObjectSystemObject), &address, .main, processListener)
    }
    if let outputListener {
      for process in observedProcesses {
        var address = Self.address(kAudioProcessPropertyIsRunningOutput)
        AudioObjectRemovePropertyListenerBlock(process, &address, .main, outputListener)
      }
    }
    observedProcesses.removeAll()
    processListener = nil
    outputListener = nil
  }

  private func scheduleRefresh() {
    pending?.cancel()
    pending = Task { [weak self] in
      try? await Task.sleep(for: .milliseconds(100))
      guard !Task.isCancelled else { return }
      self?.refresh()
    }
  }

  private func refresh() {
    guard processListener != nil, let outputListener else { return }
    var address = Self.address(kAudioHardwarePropertyProcessObjectList)
    var size: UInt32 = 0
    let system = AudioObjectID(kAudioObjectSystemObject)
    guard AudioObjectGetPropertyDataSize(system, &address, 0, nil, &size) == noErr else {
      onChange(nil)
      return
    }
    var objects = [AudioObjectID](
      repeating: 0, count: Int(size) / MemoryLayout<AudioObjectID>.size)
    let result: OSStatus = objects.isEmpty ? OSStatus(noErr) : objects.withUnsafeMutableBytes { bytes in
      guard let base = bytes.baseAddress else { return OSStatus(paramErr) }
      return AudioObjectGetPropertyData(system, &address, 0, nil, &size, base)
    }
    guard result == noErr else { onChange(nil); return }
    objects = Array(objects.prefix(Int(size) / MemoryLayout<AudioObjectID>.size))
    let current = Set(objects)
    for process in observedProcesses.subtracting(current) {
      var property = Self.address(kAudioProcessPropertyIsRunningOutput)
      AudioObjectRemovePropertyListenerBlock(process, &property, .main, outputListener)
    }
    observedProcesses.formIntersection(current)
    for process in current.subtracting(observedProcesses) {
      var property = Self.address(kAudioProcessPropertyIsRunningOutput)
      if AudioObjectAddPropertyListenerBlock(process, &property, .main, outputListener) == noErr {
        observedProcesses.insert(process)
      }
    }
    let outputs = objects.compactMap { object -> SystemAudioProcess? in
      guard Self.number(object, kAudioProcessPropertyIsRunningOutput) == 1,
        let number = Self.number(object, kAudioProcessPropertyPID),
        let pid = pid_t(exactly: number), pid != ProcessInfo.processInfo.processIdentifier
      else { return nil }
      let app = NSRunningApplication(processIdentifier: pid)
      let bundle = Self.bundleIdentifier(object) ?? app?.bundleIdentifier
      let name = app?.localizedName
        ?? (bundle?.hasPrefix("com.spotify.client") == true ? "Spotify" : String(pid))
      return SystemAudioProcess(
        id: object, pid: pid, name: name, bundleIdentifier: bundle)
    }.sorted { ($0.name, $0.id) < ($1.name, $1.id) }
    onChange(outputs)
  }

  private static func number(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector)
    -> UInt32?
  {
    var address = address(selector)
    var value: UInt32 = 0
    var size = UInt32(MemoryLayout<UInt32>.size)
    return AudioObjectGetPropertyData(object, &address, 0, nil, &size, &value) == noErr
      ? value : nil
  }

  private static func bundleIdentifier(_ object: AudioObjectID) -> String? {
    var property = address(kAudioProcessPropertyBundleID)
    var value: Unmanaged<CFString>?
    var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
    guard AudioObjectGetPropertyData(object, &property, 0, nil, &size, &value) == noErr,
      let value
    else { return nil }
    // AudioHardware.h explicitly gives ownership of this returned CFObject
    // to the caller, including for background audio helper processes.
    return value.takeRetainedValue() as String
  }

  private static func address(_ selector: AudioObjectPropertySelector)
    -> AudioObjectPropertyAddress
  {
    AudioObjectPropertyAddress(
      mSelector: selector, mScope: kAudioObjectPropertyScopeGlobal,
      mElement: kAudioObjectPropertyElementMain)
  }
}
