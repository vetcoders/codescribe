#!/usr/bin/env swift
//
// audio-play-to-device — play a WAV into ONE named output device.
//
// This is the source half of the BlackHole loopback harness. `afplay` can only
// use the system default output, so driving audio into a virtual device without
// touching the operator's speakers needs an explicit device binding:
//
//     scripts/audio-play-to-device.swift "BlackHole 2ch" fixture.wav
//
// BlackHole loops its output back to its input, so a recorder listening on
// "BlackHole 2ch" hears exactly this file — the same path a microphone takes,
// through CoreAudio, at real time. That is the difference between "the decoder
// parses a WAV" and "the app hears speech".
//
// Exit codes: 0 played to completion · 2 device not found · 3 file unreadable
// · 4 audio engine failed. Device names are matched case-insensitively, exact
// match first so "BlackHole 2ch" never picks "BlackHole 16ch".

import AVFoundation
import CoreAudio
import Foundation

func fail(_ message: String, code: Int32) -> Never {
    FileHandle.standardError.write(Data("audio-play-to-device: \(message)\n".utf8))
    exit(code)
}

/// Every output device's (id, name). Input-only devices are skipped so a name
/// collision between an input and an output cannot bind the wrong one.
func outputDevices() -> [(id: AudioDeviceID, name: String)] {
    var size: UInt32 = 0
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioHardwarePropertyDevices,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    guard
        AudioObjectGetPropertyDataSize(
            AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size) == noErr
    else { return [] }

    let count = Int(size) / MemoryLayout<AudioDeviceID>.size
    var ids = [AudioDeviceID](repeating: 0, count: count)
    guard
        AudioObjectGetPropertyData(
            AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &ids) == noErr
    else { return [] }

    return ids.compactMap { id in
        guard hasOutputStreams(id), let name = deviceName(id) else { return nil }
        return (id, name)
    }
}

func hasOutputStreams(_ id: AudioDeviceID) -> Bool {
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioDevicePropertyStreamConfiguration,
        mScope: kAudioDevicePropertyScopeOutput,
        mElement: kAudioObjectPropertyElementMain
    )
    var size: UInt32 = 0
    guard AudioObjectGetPropertyDataSize(id, &address, 0, nil, &size) == noErr, size > 0 else {
        return false
    }
    let buffer = UnsafeMutableRawPointer.allocate(
        byteCount: Int(size), alignment: MemoryLayout<AudioBufferList>.alignment)
    defer { buffer.deallocate() }
    guard AudioObjectGetPropertyData(id, &address, 0, nil, &size, buffer) == noErr else {
        return false
    }
    let list = buffer.assumingMemoryBound(to: AudioBufferList.self)
    return UnsafeMutableAudioBufferListPointer(list).contains { $0.mNumberChannels > 0 }
}

func deviceName(_ id: AudioDeviceID) -> String? {
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioObjectPropertyName,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    var name: CFString = "" as CFString
    var size = UInt32(MemoryLayout<CFString>.size)
    guard AudioObjectGetPropertyData(id, &address, 0, nil, &size, &name) == noErr else {
        return nil
    }
    return name as String
}

// MARK: - Arguments

let args = CommandLine.arguments
guard args.count >= 3 else {
    fail("usage: audio-play-to-device.swift <device-name> <file.wav> [--list]", code: 64)
}
let wanted = args[1]
let path = args[2]

let devices = outputDevices()
if wanted == "--list" {
    for device in devices { print(device.name) }
    exit(0)
}

// Exact (case-insensitive) match wins; a prefix match is the fallback so a
// trailing-space typo still works, but never silently across distinct devices.
let matches =
    devices.filter { $0.name.lowercased() == wanted.lowercased() }.isEmpty
    ? devices.filter { $0.name.lowercased().hasPrefix(wanted.lowercased()) }
    : devices.filter { $0.name.lowercased() == wanted.lowercased() }

guard let device = matches.first else {
    let known = devices.map(\.name).joined(separator: ", ")
    fail("output device '\(wanted)' not found. Available: [\(known)]", code: 2)
}
guard matches.count == 1 else {
    fail("device name '\(wanted)' is ambiguous: \(matches.map(\.name))", code: 2)
}

let url = URL(fileURLWithPath: path)
guard FileManager.default.fileExists(atPath: url.path) else {
    fail("file not found: \(url.path)", code: 3)
}
guard let file = try? AVAudioFile(forReading: url) else {
    fail("cannot read as audio: \(url.path)", code: 3)
}

// MARK: - Play

let engine = AVAudioEngine()
let player = AVAudioPlayerNode()
engine.attach(player)

// Bind the engine's output to the requested device BEFORE starting it —
// AVAudioEngine caches the device on start, so setting it afterwards is a no-op
// that silently plays to the default output instead (i.e. the operator's
// speakers, and no audio into the loopback at all).
var deviceID = device.id
let status = AudioUnitSetProperty(
    engine.outputNode.audioUnit!,
    kAudioOutputUnitProperty_CurrentDevice,
    kAudioUnitScope_Global,
    0,
    &deviceID,
    UInt32(MemoryLayout<AudioDeviceID>.size)
)
guard status == noErr else {
    fail("cannot bind output to '\(device.name)' (OSStatus \(status))", code: 4)
}

engine.connect(player, to: engine.mainMixerNode, format: file.processingFormat)

let finished = DispatchSemaphore(value: 0)
do {
    try engine.start()
} catch {
    fail("engine start failed: \(error.localizedDescription)", code: 4)
}

let seconds = Double(file.length) / file.processingFormat.sampleRate
player.scheduleFile(file, at: nil) { finished.signal() }
player.play()

FileHandle.standardError.write(
    Data("playing \(String(format: "%.1f", seconds))s into '\(device.name)'\n".utf8))

// The completion handler fires when the last buffer is HANDED to the device, not
// when it has been rendered, so add the file's own duration as the ceiling plus
// a small tail for the device to drain.
if finished.wait(timeout: .now() + seconds + 5) == .timedOut {
    fail("playback did not finish within \(String(format: "%.1f", seconds + 5))s", code: 4)
}
// Let the device drain its last buffers before tearing the engine down.
Thread.sleep(forTimeInterval: 0.4)
player.stop()
engine.stop()
