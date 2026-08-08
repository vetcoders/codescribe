// ============================================================================
// macos27_probe.swift — permissionless API-drift canaries for the host smoke
// ============================================================================
// One binary, several independent rows. Every row is designed to be safe to
// run while the operator is away:
//
//   * nothing here posts synthetic events, so the shipped app and whatever is
//     frontmost are never typed into;
//   * no TCC dialog can be raised — every authorization call is a pure status
//     read, and the event tap is only attempted when Accessibility trust is
//     ALREADY granted (checked with prompt suppressed);
//   * pasteboard CONTENT is only read behind an explicit opt-in flag, because
//     macOS 27 is expected to surface a user-visible read notice (S-6).
//
// Output protocol: TAB-separated `row<TAB>status<TAB>evidence` on stdout.
// Exit code is 0 unless a row that asserts a contract came back FAIL.
//
// Usage: macos27_probe [--clipboard-content]

import AVFoundation
import AppKit
import ApplicationServices
import CoreGraphics
import Foundation
import Speech

@main
enum MacOS27Probe {

    // MARK: - Row plumbing

    static var sawFailure = false

    static func emit(_ row: String, _ status: String, _ evidence: String) {
        if status == "FAIL" { sawFailure = true }
        let flat = evidence.replacingOccurrences(of: "\n", with: " ")
        print("\(row)\t\(status)\t\(flat)")
    }

    // MARK: - Host identity

    static func rowHost() {
        let v = ProcessInfo.processInfo.operatingSystemVersion
        emit(
            "host", "INFO",
            "macOS \(v.majorVersion).\(v.minorVersion).\(v.patchVersion) · arch \(archName()) · probe built against the SDK in PATH"
        )
    }

    static func archName() -> String {
        #if arch(arm64)
            return "arm64"
        #else
            return "x86_64"
        #endif
    }

    // MARK: - CoreGraphics constant table (no permissions required)
    //
    // `app/os/hotkeys/platform.rs` re-declares every CoreGraphics constant it
    // uses as a raw literal, because the hotkey layer talks to CoreGraphics
    // over raw FFI. If any of these drifts in a future SDK the hotkeys die
    // SILENTLY — no compile error, no exception, just a tap that never sees
    // Fn again. This row reads the live SDK values and compares them against
    // the pinned Rust table.

    struct ConstantPin {
        let name: String
        let pinned: UInt64
        let live: UInt64
    }

    static func rowEventTapConstants() {
        let pins: [ConstantPin] = [
            .init(
                name: "kCGEventKeyDown", pinned: 10,
                live: UInt64(CGEventType.keyDown.rawValue)),
            .init(name: "kCGEventKeyUp", pinned: 11, live: UInt64(CGEventType.keyUp.rawValue)),
            .init(
                name: "kCGEventFlagsChanged", pinned: 12,
                live: UInt64(CGEventType.flagsChanged.rawValue)),
            .init(
                name: "kCGEventTapDisabledByTimeout", pinned: 0xFFFF_FFFE,
                live: UInt64(CGEventType.tapDisabledByTimeout.rawValue)),
            .init(
                name: "kCGEventTapDisabledByUserInput", pinned: 0xFFFF_FFFF,
                live: UInt64(CGEventType.tapDisabledByUserInput.rawValue)),
            .init(
                name: "kCGEventFlagMaskControl", pinned: 0x0004_0000,
                live: CGEventFlags.maskControl.rawValue),
            .init(
                name: "kCGEventFlagMaskShift", pinned: 0x0002_0000,
                live: CGEventFlags.maskShift.rawValue),
            .init(
                name: "kCGEventFlagMaskAlternate", pinned: 0x0008_0000,
                live: CGEventFlags.maskAlternate.rawValue),
            .init(
                name: "kCGEventFlagMaskCommand", pinned: 0x0010_0000,
                live: CGEventFlags.maskCommand.rawValue),
            .init(
                name: "kCGEventFlagMaskSecondaryFn", pinned: 0x0080_0000,
                live: CGEventFlags.maskSecondaryFn.rawValue),
            .init(
                name: "kCGKeyboardEventKeycode", pinned: 9,
                live: UInt64(CGEventField.keyboardEventKeycode.rawValue)),
            .init(
                name: "kCGSessionEventTap", pinned: 1,
                live: UInt64(CGEventTapLocation.cgSessionEventTap.rawValue)),
            .init(
                name: "kCGHeadInsertEventTap", pinned: 0,
                live: UInt64(CGEventTapPlacement.headInsertEventTap.rawValue)),
            .init(
                name: "kCGEventTapOptionListenOnly", pinned: 1,
                live: UInt64(CGEventTapOptions.listenOnly.rawValue)),
        ]

        let drifted = pins.filter { $0.pinned != $0.live }
        if drifted.isEmpty {
            emit(
                "cgevent-constants", "PASS",
                "\(pins.count) CoreGraphics constants match the table pinned in app/os/hotkeys/platform.rs"
            )
        } else {
            let detail = drifted.map {
                "\($0.name): pinned 0x\(String($0.pinned, radix: 16)) != live 0x\(String($0.live, radix: 16))"
            }.joined(separator: " | ")
            emit("cgevent-constants", "FAIL", detail)
        }
    }

    // MARK: - TCC matrix (pure status reads, no dialogs)

    static func micLabel() -> String {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized: return "authorized"
        case .denied: return "denied"
        case .restricted: return "restricted"
        case .notDetermined: return "not_determined"
        @unknown default: return "unknown"
        }
    }

    static func speechLabel() -> String {
        switch SFSpeechRecognizer.authorizationStatus() {
        case .authorized: return "authorized"
        case .denied: return "denied"
        case .restricted: return "restricted"
        case .notDetermined: return "not_determined"
        @unknown default: return "unknown"
        }
    }

    /// Accessibility trust WITHOUT the prompt — a smoke run must never raise a
    /// system dialog at the operator.
    static func accessibilityTrusted() -> Bool {
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: false]
        return AXIsProcessTrustedWithOptions(options as CFDictionary)
    }

    static func rowTccMatrix() {
        let screen = CGPreflightScreenCaptureAccess()
        // TCC evaluates the RESPONSIBLE process, not the asking one: a probe
        // launched from a terminal reads the terminal's slot, which is why the
        // same binary reports different answers from a worker and from the
        // app. Record who is asking so the row stays interpretable later.
        let responsible = ProcessInfo.processInfo.processName
        emit(
            "tcc-matrix", "INFO",
            "mic=\(micLabel()) speech=\(speechLabel()) accessibility=\(accessibilityTrusted() ? "trusted" : "untrusted") screen-capture=\(screen ? "granted" : "not-granted, unused by product") · asking-process=\(responsible) (TCC scores the responsible process, not this one)"
        )
    }

    // MARK: - CGEventTap create → force-disable → re-arm

    static func rowEventTapRearm() {
        guard accessibilityTrusted() else {
            emit(
                "eventtap-rearm", "SKIP",
                "responsible process has no Accessibility grant; creating a session tap here would raise a TCC dialog at an absent operator (AGENT_BUS OPERATOR_AWAY). Run from a granted context to exercise this row."
            )
            return
        }

        let mask: CGEventMask =
            (1 << CGEventType.keyDown.rawValue) | (1 << CGEventType.flagsChanged.rawValue)
        guard
            let tap = CGEvent.tapCreate(
                tap: .cgSessionEventTap,
                place: .headInsertEventTap,
                options: .listenOnly,
                eventsOfInterest: mask,
                callback: { _, _, event, _ in Unmanaged.passUnretained(event) },
                userInfo: nil
            )
        else {
            emit(
                "eventtap-rearm", "FAIL",
                "CGEvent.tapCreate returned nil despite Accessibility trust — the hotkey layer cannot arm on this host"
            )
            return
        }
        defer { CFMachPortInvalidate(tap) }

        let armed = CGEvent.tapIsEnabled(tap: tap)
        // Force the exact state macOS puts the tap in when it disables it
        // (timeout / user input). The production re-arm in
        // `platform.rs:event_callback` answers that state with the same call
        // this row makes, so a green row proves the recovery API still works.
        CGEvent.tapEnable(tap: tap, enable: false)
        let disabled = CGEvent.tapIsEnabled(tap: tap)
        CGEvent.tapEnable(tap: tap, enable: true)
        let rearmed = CGEvent.tapIsEnabled(tap: tap)

        if armed && !disabled && rearmed {
            emit(
                "eventtap-rearm", "PASS",
                "tapCreate armed=true → tapEnable(false) disabled=true → tapEnable(true) re-armed=true, no restart; matches platform.rs re-arm path"
            )
        } else {
            emit(
                "eventtap-rearm", "FAIL",
                "armed=\(armed) disabledAfterDisable=\(!disabled) rearmed=\(rearmed) — re-arm contract broken on this host"
            )
        }
    }

    // MARK: - Responsibility-disclaim private symbol (S-5)

    static func rowDisclaimSymbol() {
        let handle = dlopen(nil, RTLD_NOW)
        if dlsym(handle, "responsibility_spawnattrs_setdisclaim") != nil {
            emit(
                "disclaim-symbol", "PASS",
                "responsibility_spawnattrs_setdisclaim resolves via dlsym; the CODESCRIBE_BRIDGE_DISCLAIM re-exec path in codescribe-stt-bridge.swift stays available"
            )
        } else {
            emit(
                "disclaim-symbol", "FAIL",
                "responsibility_spawnattrs_setdisclaim no longer resolves — the bridge falls back to staying terminal-responsible, so harness/CI Speech grants read not_determined (documented fallback, not a crash)"
            )
        }
    }

    // MARK: - Clipboard fallback canary (S-6)

    static func rowClipboardFallback(readContent: Bool) {
        let pb = NSPasteboard.general
        let before = pb.changeCount
        // `app/os/selection.rs` decides "did the synthetic Cmd+C land?" purely
        // from changeCount — deliberately content-agnostic. Reading the COUNT
        // is metadata and raises no user-facing notice; reading the STRING is
        // the operation macOS 27 is expected to start surfacing.
        guard readContent else {
            emit(
                "clipboard-fallback", "SKIP",
                "changeCount readable (=\(before)) with no content read; the pasteboard-read notice canary needs --clipboard-content and an operator watching the screen"
            )
            return
        }
        let text = pb.string(forType: .string)
        let after = pb.changeCount
        emit(
            "clipboard-fallback", "INFO",
            "content read performed: changeCount \(before)→\(after), text \(text == nil ? "absent" : "present (\(text!.count) chars, not logged)"). Operator must state whether macOS showed a pasteboard-read notice."
        )
    }

    // MARK: - main

    static func main() {
        let readClipboardContent = CommandLine.arguments.contains("--clipboard-content")

        rowHost()
        rowEventTapConstants()
        rowTccMatrix()
        rowEventTapRearm()
        rowDisclaimSymbol()
        rowClipboardFallback(readContent: readClipboardContent)

        exit(sawFailure ? 1 : 0)
    }
}
