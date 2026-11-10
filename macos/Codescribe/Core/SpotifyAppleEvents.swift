import Carbon
import Foundation
import os

protocol SpotifyVolumeAccess: Sendable {
  func isPlaying(pid: pid_t) throws -> Bool
  func volume(pid: pid_t) throws -> Int32
  func setVolume(_ value: Int32, pid: pid_t) throws
}

/// Spotify's shipped scripting dictionary defines pPlS/kPSP and pVol.
/// Events address a verified running PID, so they cannot launch the player.
/// Calls are serialized off the main actor by SpotifyVolumeController.
struct SpotifyAppleEvents: SpotifyVolumeAccess {
  private struct PassiveProbeState: Sendable {
    var inFlight = false
    var lastState: PermissionState = .notDetermined
  }

  private static let passiveProbeState = OSAllocatedUnfairLock(initialState: PassiveProbeState())
  private static let passiveProbeQueue = DispatchQueue(
    label: "com.vetcoders.codescribe.spotify-automation-probe",
    qos: .utility,
    attributes: .concurrent
  )

  /// Checking never sends an event or launches Spotify. Only an explicit
  /// permission action passes true; that call must run off the main actor.
  ///
  /// Passive checks (`askUserIfNeeded == false`) run off the caller's thread
  /// with a bounded timeout so a wedged `appleeventsd` / LaunchServices IPC
  /// never hangs `@MainActor` startup or Settings/Onboarding snapshots.
  static func automationPermission(
    askUserIfNeeded: Bool,
    probeTimeout: DispatchTimeInterval = .milliseconds(250),
    queryStatus: (@Sendable (Bool) -> OSStatus)? = nil
  ) -> PermissionState {
    if askUserIfNeeded {
      let status = (queryStatus ?? Self.determineAutomationStatus)(true)
      DiagnosticLog.installed?.append(
        category: "media", "action=spotify-automation-request status=\(status)")
      let resolved = automationPermissionState(status: status)
      if queryStatus == nil {
        passiveProbeState.withLock { $0.lastState = resolved }
      }
      return resolved
    }

    if queryStatus == nil {
      let cachedWhileInFlight: PermissionState? = passiveProbeState.withLock { state in
        if state.inFlight {
          return state.lastState
        }
        state.inFlight = true
        return nil
      }
      if let cachedWhileInFlight {
        return cachedWhileInFlight
      }
    }

    let query = queryStatus ?? Self.determineAutomationStatus
    let isProductionQuery = (queryStatus == nil)
    let semaphore = DispatchSemaphore(value: 0)
    let resultBox = OSAllocatedUnfairLock<PermissionState?>(initialState: nil)

    passiveProbeQueue.async {
      let status = query(false)
      let resolved = automationPermissionState(status: status)
      resultBox.withLock { $0 = resolved }
      if isProductionQuery {
        passiveProbeState.withLock {
          $0.lastState = resolved
          $0.inFlight = false
        }
      }
      semaphore.signal()
    }

    if semaphore.wait(timeout: .now() + probeTimeout) == .timedOut {
      DiagnosticLog.installed?.append(
        category: "media", "action=spotify-automation-probe-timeout")
      if isProductionQuery {
        return passiveProbeState.withLock { $0.lastState }
      }
      return .notDetermined
    }

    return resultBox.withLock { $0 ?? .notDetermined }
  }

  static func determineAutomationStatus(askUserIfNeeded: Bool) -> OSStatus {
    let target = NSAppleEventDescriptor(bundleIdentifier: "com.spotify.client")
    return withExtendedLifetime(target) {
      guard let descriptor = target.aeDesc else { return OSStatus(paramErr) }
      return AEDeterminePermissionToAutomateTarget(
        descriptor, AEEventClass(kAECoreSuite), AEEventID(kAESetData), askUserIfNeeded)
    }
  }

  static func automationPermissionState(status: OSStatus) -> PermissionState {
    switch status {
    case noErr: .granted
    case OSStatus(errAEEventNotPermitted): .denied
    // Spotify absent, consent not yet requested, or an unavailable check
    // cannot establish a grant. Never persist a guessed permission state.
    default: .notDetermined
    }
  }

  func isPlaying(pid: pid_t) throws -> Bool {
    try property(0x70506c53, pid: pid).enumCodeValue == 0x6b505350
  }

  func volume(pid: pid_t) throws -> Int32 {
    let descriptor = try property(0x70566f6c, pid: pid)
    guard let value = descriptor.coerce(toDescriptorType: DescType(typeSInt32)),
      (0...100).contains(value.int32Value)
    else { throw NSError(domain: NSOSStatusErrorDomain, code: Int(paramErr)) }
    return value.int32Value
  }

  func setVolume(_ value: Int32, pid: pid_t) throws {
    _ = try send(
      id: AEEventID(kAESetData), property: 0x70566f6c, pid: pid,
      value: NSAppleEventDescriptor(int32: value))
  }

  private func property(_ code: OSType, pid: pid_t) throws -> NSAppleEventDescriptor {
    let reply = try send(id: AEEventID(kAEGetData), property: code, pid: pid)
    guard let value = reply.paramDescriptor(forKeyword: AEKeyword(keyDirectObject)) else {
      throw NSError(domain: NSOSStatusErrorDomain, code: Int(errAEEventNotHandled))
    }
    return value
  }

  private func send(
    id: AEEventID, property: OSType, pid: pid_t, value: NSAppleEventDescriptor? = nil
  ) throws -> NSAppleEventDescriptor {
    let specifier = NSAppleEventDescriptor.record()
    specifier.setDescriptor(
      NSAppleEventDescriptor(typeCode: OSType(typeProperty)), forKeyword: AEKeyword(keyAEDesiredClass))
    specifier.setDescriptor(
      NSAppleEventDescriptor(enumCode: OSType(formPropertyID)), forKeyword: AEKeyword(keyAEKeyForm))
    specifier.setDescriptor(
      NSAppleEventDescriptor(typeCode: property), forKeyword: AEKeyword(keyAEKeyData))
    specifier.setDescriptor(NSAppleEventDescriptor.null(), forKeyword: AEKeyword(keyAEContainer))
    guard let target = specifier.coerce(toDescriptorType: DescType(typeObjectSpecifier)) else {
      throw NSError(domain: NSOSStatusErrorDomain, code: Int(paramErr))
    }
    let event = NSAppleEventDescriptor(
      eventClass: AEEventClass(kAECoreSuite), eventID: id,
      targetDescriptor: NSAppleEventDescriptor(processIdentifier: pid),
      returnID: AEReturnID(kAutoGenerateReturnID), transactionID: AETransactionID(kAnyTransactionID))
    event.setParam(target, forKeyword: AEKeyword(keyDirectObject))
    if let value { event.setParam(value, forKeyword: AEKeyword(keyAEData)) }
    // Recording must not raise a consent dialog. The permission checklist
    // owns deliberate requests; this AppleEvent returns -1744 when needed.
    let noPermissionPrompt = NSAppleEventDescriptor.SendOptions(
      rawValue: UInt(kAEDoNotPromptForUserConsent))
    let reply = try event.sendEvent(
      options: [.waitForReply, .neverInteract, noPermissionPrompt], timeout: 1)
    if let error = reply.paramDescriptor(forKeyword: AEKeyword(keyErrorNumber)), error.int32Value != 0 {
      throw NSError(domain: NSOSStatusErrorDomain, code: Int(error.int32Value))
    }
    return reply
  }
}
