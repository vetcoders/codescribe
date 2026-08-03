#!/usr/bin/env swift
//
// audio-default-devices — read / write macOS default input & output devices.
//
// Product rule: daily Codescribe uses real mic + speakers. The BlackHole
// harness must NEVER leave BlackHole as the system default after a run.
// Play/capture scripts bind BH by name; system defaults stay on the daily path.
//
// Usage:
//   audio-default-devices.swift show
//   audio-default-devices.swift get-input
//   audio-default-devices.swift get-output
//   audio-default-devices.swift set-input  "MacBook Pro Microphone"
//   audio-default-devices.swift set-output "MacBook Pro Speakers"
//   audio-default-devices.swift save  /tmp/audio-defaults.env
//   audio-default-devices.swift restore /tmp/audio-defaults.env
//
// Exit: 0 ok · 1 device not found · 2 CoreAudio write failed · 64 usage

import CoreAudio
import Foundation

func die(_ message: String, code: Int32) -> Never {
    FileHandle.standardError.write(Data("audio-default-devices: \(message)\n".utf8))
    exit(code)
}

func deviceName(_ id: AudioDeviceID) -> String? {
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioObjectPropertyName,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    var name: CFString = "" as CFString
    var size = UInt32(MemoryLayout<CFString>.size)
    let status = withUnsafeMutablePointer(to: &name) { ptr -> OSStatus in
        AudioObjectGetPropertyData(id, &address, 0, nil, &size, ptr)
    }
    guard status == noErr else { return nil }
    return name as String
}

func allDevices() -> [AudioDeviceID] {
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioHardwarePropertyDevices,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    var size: UInt32 = 0
    guard
        AudioObjectGetPropertyDataSize(
            AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size
        ) == noErr
    else { return [] }
    let count = Int(size) / MemoryLayout<AudioDeviceID>.size
    var ids = [AudioDeviceID](repeating: 0, count: count)
    guard
        AudioObjectGetPropertyData(
            AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &ids
        ) == noErr
    else { return [] }
    return ids
}

func hasScope(_ id: AudioDeviceID, scope: AudioObjectPropertyScope) -> Bool {
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioDevicePropertyStreamConfiguration,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain
    )
    var size: UInt32 = 0
    guard AudioObjectGetPropertyDataSize(id, &address, 0, nil, &size) == noErr, size > 0 else {
        return false
    }
    let raw = UnsafeMutableRawPointer.allocate(
        byteCount: Int(size), alignment: MemoryLayout<AudioBufferList>.alignment
    )
    defer { raw.deallocate() }
    guard AudioObjectGetPropertyData(id, &address, 0, nil, &size, raw) == noErr else {
        return false
    }
    let list = raw.assumingMemoryBound(to: AudioBufferList.self)
    return UnsafeMutableAudioBufferListPointer(list).contains { $0.mNumberChannels > 0 }
}

func defaultDevice(selector: AudioObjectPropertySelector) -> (id: AudioDeviceID, name: String)? {
    var address = AudioObjectPropertyAddress(
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    var id: AudioDeviceID = 0
    var size = UInt32(MemoryLayout<AudioDeviceID>.size)
    guard
        AudioObjectGetPropertyData(
            AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &id
        ) == noErr,
        let name = deviceName(id)
    else { return nil }
    return (id, name)
}

func findDevice(named want: String, input: Bool) -> AudioDeviceID? {
    let scope: AudioObjectPropertyScope =
        input ? kAudioDevicePropertyScopeInput : kAudioDevicePropertyScopeOutput
    let wantLower = want.lowercased()
    var exact: AudioDeviceID?
    var fuzzy: AudioDeviceID?
    for id in allDevices() {
        guard hasScope(id, scope: scope), let name = deviceName(id) else { continue }
        if name == want { exact = id }
        else if name.lowercased() == wantLower { fuzzy = id }
    }
    return exact ?? fuzzy
}

func setDefault(selector: AudioObjectPropertySelector, device: AudioDeviceID) -> Bool {
    var address = AudioObjectPropertyAddress(
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    var id = device
    let size = UInt32(MemoryLayout<AudioDeviceID>.size)
    return AudioObjectSetPropertyData(
        AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, size, &id
    ) == noErr
}

let args = Array(CommandLine.arguments.dropFirst())
guard let cmd = args.first else {
    die(
        "usage: show | get-input | get-output | set-input <name> | set-output <name> | save <file> | restore <file>",
        code: 64
    )
}

switch cmd {
case "show":
    let inn = defaultDevice(selector: kAudioHardwarePropertyDefaultInputDevice)
    let out = defaultDevice(selector: kAudioHardwarePropertyDefaultOutputDevice)
    print("input=\(inn?.name ?? "?")")
    print("output=\(out?.name ?? "?")")

case "get-input":
    guard let inn = defaultDevice(selector: kAudioHardwarePropertyDefaultInputDevice) else {
        die("no default input", code: 2)
    }
    print(inn.name)

case "get-output":
    guard let out = defaultDevice(selector: kAudioHardwarePropertyDefaultOutputDevice) else {
        die("no default output", code: 2)
    }
    print(out.name)

case "set-input":
    guard args.count >= 2 else { die("set-input needs a device name", code: 64) }
    let name = args[1]
    guard let id = findDevice(named: name, input: true) else {
        die("input device not found: \(name)", code: 1)
    }
    guard setDefault(selector: kAudioHardwarePropertyDefaultInputDevice, device: id) else {
        die("failed to set default input to \(name)", code: 2)
    }
    print("default input → \(name)")

case "set-output":
    guard args.count >= 2 else { die("set-output needs a device name", code: 64) }
    let name = args[1]
    guard let id = findDevice(named: name, input: false) else {
        die("output device not found: \(name)", code: 1)
    }
    guard setDefault(selector: kAudioHardwarePropertyDefaultOutputDevice, device: id) else {
        die("failed to set default output to \(name)", code: 2)
    }
    print("default output → \(name)")

case "save":
    guard args.count >= 2 else { die("save needs a file path", code: 64) }
    let path = args[1]
    let inn = defaultDevice(selector: kAudioHardwarePropertyDefaultInputDevice)?.name ?? ""
    let out = defaultDevice(selector: kAudioHardwarePropertyDefaultOutputDevice)?.name ?? ""
    let body = """
    CODESCRIBE_SAVED_INPUT=\(inn)
    CODESCRIBE_SAVED_OUTPUT=\(out)
    """
    do {
        try body.write(toFile: path, atomically: true, encoding: .utf8)
    } catch {
        die("write \(path): \(error)", code: 2)
    }
    print("saved input=\(inn) output=\(out) → \(path)")

case "restore":
    guard args.count >= 2 else { die("restore needs a file path", code: 64) }
    let path = args[1]
    guard let text = try? String(contentsOfFile: path, encoding: .utf8) else {
        die("cannot read \(path)", code: 2)
    }
    var savedIn = ""
    var savedOut = ""
    for line in text.split(separator: "\n") {
        let parts = line.split(separator: "=", maxSplits: 1).map(String.init)
        guard parts.count == 2 else { continue }
        if parts[0] == "CODESCRIBE_SAVED_INPUT" { savedIn = parts[1] }
        if parts[0] == "CODESCRIBE_SAVED_OUTPUT" { savedOut = parts[1] }
    }
    if !savedIn.isEmpty, let id = findDevice(named: savedIn, input: true) {
        _ = setDefault(selector: kAudioHardwarePropertyDefaultInputDevice, device: id)
        print("restored input → \(savedIn)")
    } else if !savedIn.isEmpty {
        FileHandle.standardError.write(
            Data("audio-default-devices: skip input restore, not found: \(savedIn)\n".utf8)
        )
    }
    if !savedOut.isEmpty, let id = findDevice(named: savedOut, input: false) {
        _ = setDefault(selector: kAudioHardwarePropertyDefaultOutputDevice, device: id)
        print("restored output → \(savedOut)")
    } else if !savedOut.isEmpty {
        FileHandle.standardError.write(
            Data("audio-default-devices: skip output restore, not found: \(savedOut)\n".utf8)
        )
    }

default:
    die("unknown command: \(cmd)", code: 64)
}
