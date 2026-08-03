#!/usr/bin/env swift
//
// audio-device-unmute — clear device-level mute and raise volume on ONE named
// CoreAudio device, both scopes.
//
// Why this exists: pressing the system mute key while a virtual loopback
// device (BlackHole) is the default output writes a PERSISTENT device-level
// mute into the driver — on the output scope, and (measured 2026-07-25) the
// input scope as well. From then on the loopback carries digital silence
// (-91 dB) no matter what the player renders, and AppleScript's
// `set volume without output muted` cannot clear it: it targets the current
// default device's master control and reads back muted:true forever. Only a
// direct kAudioDevicePropertyMute write clears the state.
//
// The BlackHole harness runs this as a preflight so a stuck mute is repaired,
// not just reported. Exit codes: 0 device is unmuted (possibly after fixing)
// · 1 device not found · 2 a mute control exists that could not be cleared.
//
//     scripts/audio-device-unmute.swift "BlackHole 2ch"

import CoreAudio
import Foundation

func fail(_ message: String, code: Int32) -> Never {
    FileHandle.standardError.write(Data("audio-device-unmute: \(message)\n".utf8))
    exit(code)
}

let name = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "BlackHole 2ch"

var listAddr = AudioObjectPropertyAddress(
    mSelector: kAudioHardwarePropertyDevices,
    mScope: kAudioObjectPropertyScopeGlobal,
    mElement: kAudioObjectPropertyElementMain
)
var listSize: UInt32 = 0
AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &listAddr, 0, nil, &listSize)
var devices = [AudioObjectID](repeating: 0, count: Int(listSize) / MemoryLayout<AudioObjectID>.size)
AudioObjectGetPropertyData(
    AudioObjectID(kAudioObjectSystemObject), &listAddr, 0, nil, &listSize, &devices)

var target: AudioObjectID = 0
for device in devices {
    var nameAddr = AudioObjectPropertyAddress(
        mSelector: kAudioObjectPropertyName,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    var cfName: CFString = "" as CFString
    var nameSize = UInt32(MemoryLayout<CFString>.size)
    let status = withUnsafeMutablePointer(to: &cfName) { pointer in
        AudioObjectGetPropertyData(device, &nameAddr, 0, nil, &nameSize, pointer)
    }
    if status == noErr && (cfName as String).lowercased() == name.lowercased() {
        target = device
        break
    }
}
if target == 0 { fail("device '\(name)' not found", code: 1) }

var stuck = 0
for scope in [kAudioDevicePropertyScopeOutput, kAudioDevicePropertyScopeInput] {
    let scopeLabel = scope == kAudioDevicePropertyScopeOutput ? "output" : "input"
    for element in [UInt32(0), 1, 2] {
        var muteAddr = AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyMute, mScope: scope, mElement: element)
        if AudioObjectHasProperty(target, &muteAddr) {
            var current: UInt32 = 0
            var size = UInt32(MemoryLayout<UInt32>.size)
            AudioObjectGetPropertyData(target, &muteAddr, 0, nil, &size, &current)
            if current != 0 {
                var cleared: UInt32 = 0
                let status = AudioObjectSetPropertyData(target, &muteAddr, 0, nil, size, &cleared)
                if status == noErr {
                    print("cleared stuck mute: \(scopeLabel) element \(element)")
                } else {
                    print("FAILED to clear mute: \(scopeLabel) element \(element) (OSStatus \(status))")
                    stuck += 1
                }
            }
        }
        var volAddr = AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyVolumeScalar, mScope: scope, mElement: element)
        if AudioObjectHasProperty(target, &volAddr) {
            var volume: Float32 = 1.0
            _ = AudioObjectSetPropertyData(
                target, &volAddr, 0, nil, UInt32(MemoryLayout<Float32>.size), &volume)
        }
    }
}
if stuck > 0 { fail("\(stuck) mute control(s) would not clear on '\(name)'", code: 2) }
print("'\(name)' is unmuted, volume up")
