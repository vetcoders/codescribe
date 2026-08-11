// codescribe-stt-bridge.swift
//
// Apple on-device STT bridge for Codescribe:
// - Reads one JSON request from stdin
// - Emits one JSON response to stdout
//
// Backend selection (per locale):
//   1. SpeechTranscriber (SpeechAnalyzer) when the locale is supported+installed
//   2. DictationTranscriber (SpeechAnalyzer, system-dictation model family) —
//      OPT-IN measurement lane, armed only by
//      CODESCRIBE_APPLE_DICTATION_TRANSCRIBER=1. Unlike ST it serves pl-PL.
//   3. SFSpeechRecognizer on-device when neither analyzer lane is armed/ready
//   4. Honest error when nothing can serve the locale
//
// Build example (pin target — do not inherit host triple):
//   swiftc -O -target arm64-apple-macos26.0 -o codescribe-stt-bridge \
//     core/stt/apple_stt/codescribe-stt-bridge.swift
//
// 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

import AVFoundation
import Dispatch
import Foundation
import Speech

struct BridgeRequest: Codable {
    let protocolVersion: Int
    let command: String
    let locale: String
    let audioPath: String?
    let allowDownload: Bool
}

struct BridgeSegment: Codable {
    let text: String
    let startTs: Double
    let endTs: Double
    /// `SFTranscriptionSegment.confidence` (0…1) — populated on finals in the
    /// streaming path (0 on partials, per Apple semantics). Absent elsewhere.
    let confidence: Double?

    init(text: String, startTs: Double, endTs: Double, confidence: Double? = nil) {
        self.text = text
        self.startTs = startTs
        self.endTs = endTs
        self.confidence = confidence
    }
}

struct BridgeResponse: Codable {
    let ok: Bool
    let status: String
    let text: String
    let segments: [BridgeSegment]
    let localeSupported: Bool?
    let localeInstalled: Bool?
    /// Selected Apple backend: `speech_transcriber` | `sf_speech_recognizer` | null
    let backend: String?
    let error: String?
    /// SFSpeechRecognizer authorization: not_determined | denied | restricted | authorized
    let speechAuth: String?
}

enum BridgeError: Error, CustomStringConvertible {
    case invalidInput(String)
    case unsupportedCommand(String)
    case missingAudioPath
    case runtime(String)

    var description: String {
        switch self {
        case let .invalidInput(message):
            return "invalid_input: \(message)"
        case let .unsupportedCommand(cmd):
            return "unsupported_command: \(cmd)"
        case .missingAudioPath:
            return "missing_audio_path"
        case let .runtime(message):
            return "runtime_error: \(message)"
        }
    }
}

enum AppleSttBackend: String {
    case speechTranscriber = "speech_transcriber"
    case dictationTranscriber = "dictation_transcriber"
    case sfSpeechRecognizer = "sf_speech_recognizer"
}

/// Opt-in gate for the `DictationTranscriber` lane (W4-A PoC).
///
/// DT is the model family the SYSTEM dictation uses and — unlike
/// `SpeechTranscriber` — its catalog includes `pl-PL` (measured on macOS 27.0,
/// SDK 26.5: ST supported-pl = [], DT supported-pl = ["pl-PL"]). That makes it
/// the first credible non-Whisper Polish upgrade, and exactly why it must not
/// arm itself: flipping pl-PL off SFSpeech is an operator decision backed by
/// measured bars, not a side effect of landing the code.
private func dictationTranscriberEnabled() -> Bool {
    switch ProcessInfo.processInfo.environment["CODESCRIBE_APPLE_DICTATION_TRANSCRIBER"]?
        .trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    {
    case "1", "true", "yes", "on": return true
    default: return false
    }
}

if CommandLine.arguments.contains("--phrase-restart-self-test") {
    exit(runPhraseRestartVectorSelfTest())
}

respawnSelfResponsibleIfRequested()

Task {
    do {
        let request = try readRequest()
        let response = try await handle(request: request)
        try writeResponse(response)
    } catch {
        let response = BridgeResponse(
            ok: false,
            status: "error",
            text: "",
            segments: [],
            localeSupported: nil,
            localeInstalled: nil,
            backend: nil,
            error: String(describing: error),
            speechAuth: speechAuthLabel(SFSpeechRecognizer.authorizationStatus())
        )
        do {
            try writeResponse(response)
        } catch {
            fputs("bridge_error: \(error)\n", stderr)
        }
    }
    exit(0)
}
dispatchMain()

// MARK: - Responsibility disclaim (test/CLI path)

/// TCC evaluates a privacy request against the *responsible process*, not the
/// process that asks. For a bridge spawned from a shell or a test runner the
/// responsible process is the hosting terminal, which has no
/// NSSpeechRecognitionUsageDescription — so the grant recorded for the
/// bridge's own identity is invisible (auth reads back `not_determined`) and
/// raising the dialog aborts the process
/// (`__TCC_CRASHING_DUE_TO_PRIVACY_VIOLATION__`). Measured 2026-07-25: even
/// `open -a` called from a terminal-descended process keeps the terminal
/// responsible (crash reports carry the terminal's `responsiblePid`), while a
/// launchd-spawned bridge reads `authorized` for the very same TCC record.
///
/// Fix, same as Chromium/Firefox helper processes: re-exec ourselves once via
/// `posix_spawn` with the responsibility-disclaim attribute, which makes the
/// child its own responsible process. stdio is inherited so the JSON protocol
/// is untouched; the parent just waits and mirrors the exit status.
///
/// Gated by `CODESCRIBE_BRIDGE_DISCLAIM=1` (set by `make engine-auth` /
/// `test-engine-apple` / the BlackHole harness). The production path — bridge
/// spawned by Codescribe.app — must NOT disclaim: there the bridge inherits
/// the app's Speech Recognition grant, which is the one users actually see.
private func respawnSelfResponsibleIfRequested() {
    let env = ProcessInfo.processInfo.environment
    guard env["CODESCRIBE_BRIDGE_DISCLAIM"] == "1",
        env["CODESCRIBE_BRIDGE_DISCLAIMED"] == nil
    else { return }

    // Private-but-stable libsystem symbol (used by major browsers):
    //   int responsibility_spawnattrs_setdisclaim(posix_spawnattr_t *attrs, int disclaim)
    typealias SetDisclaim = @convention(c) (UnsafeMutablePointer<posix_spawnattr_t?>?, Int32) -> Int32
    guard let sym = dlsym(dlopen(nil, RTLD_NOW), "responsibility_spawnattrs_setdisclaim") else {
        fputs("bridge_warning: setdisclaim symbol unavailable; staying terminal-responsible\n", stderr)
        return
    }
    let setDisclaim = unsafeBitCast(sym, to: SetDisclaim.self)

    var attr: posix_spawnattr_t?
    guard posix_spawnattr_init(&attr) == 0 else { return }
    defer { posix_spawnattr_destroy(&attr) }
    guard setDisclaim(&attr, 1) == 0 else { return }

    var exeBuf = [CChar](repeating: 0, count: 4096)
    var exeLen = UInt32(exeBuf.count)
    guard _NSGetExecutablePath(&exeBuf, &exeLen) == 0 else { return }
    let exePath = String(cString: exeBuf)

    var argv: [UnsafeMutablePointer<CChar>?] = CommandLine.arguments.map { strdup($0) }
    argv.append(nil)
    var envp: [UnsafeMutablePointer<CChar>?] = env.map { strdup("\($0.key)=\($0.value)") }
    envp.append(strdup("CODESCRIBE_BRIDGE_DISCLAIMED=1"))
    envp.append(nil)
    defer {
        argv.forEach { free($0) }
        envp.forEach { free($0) }
    }

    var pid: pid_t = 0
    let rc = posix_spawn(&pid, exePath, nil, &attr, argv, envp)
    guard rc == 0 else {
        fputs("bridge_warning: disclaim respawn failed (posix_spawn rc=\(rc)); staying terminal-responsible\n", stderr)
        return
    }
    var status: Int32 = 0
    while waitpid(pid, &status, 0) == -1 && errno == EINTR {}
    let signaled = status & 0x7f
    exit(signaled != 0 ? 128 + signaled : (status >> 8) & 0xff)
}

private func readRequest() throws -> BridgeRequest {
    // Argv form, for the ONE case where stdin cannot be used: raising the
    // Speech Recognition dialog. TCC attributes a privacy prompt to the
    // responsible process, and for a binary run from a shell that is the
    // terminal — which has no usage description, so the request aborts the
    // process instead of prompting. Launching the bundle through
    // LaunchServices (`open -a … --args request_auth`) makes the bundle its
    // own responsible process, and LaunchServices gives it no stdin.
    if CommandLine.arguments.count > 1 {
        let command = CommandLine.arguments[1]
        let locale =
            CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "pl-PL"
        return BridgeRequest(
            protocolVersion: 1,
            command: command,
            locale: locale,
            audioPath: nil,
            allowDownload: false
        )
    }

    // ONE newline-terminated JSON line — never drain the rest of stdin.
    // Streaming v2 (`command: "stream"`) needs the bytes after this line for
    // the PCM header + raw f32le frames. `readDataToEndOfFile` would swallow
    // them and leave the stream path with an empty pipe. Non-stream callers
    // already write a single compact JSON line and close stdin, so the
    // line-oriented contract is a pure win.
    guard let lineData = readRawStdinLine(), !lineData.isEmpty else {
        throw BridgeError.invalidInput("empty stdin")
    }
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    do {
        return try decoder.decode(BridgeRequest.self, from: lineData)
    } catch {
        let text = String(data: lineData, encoding: .utf8) ?? "<non-utf8>"
        throw BridgeError.invalidInput("decode failed: \(error). payload=\(text)")
    }
}

private func writeResponse(_ response: BridgeResponse) throws {
    let encoder = JSONEncoder()
    encoder.keyEncodingStrategy = .convertToSnakeCase
    let data = try encoder.encode(response)
    FileHandle.standardOutput.write(data)
    if let newline = "\n".data(using: .utf8) {
        FileHandle.standardOutput.write(newline)
    }
}

private func handle(request: BridgeRequest) async throws -> BridgeResponse {
    let locale = Locale(identifier: request.locale)
    switch request.command {
    case "probe":
        return try await probe(locale: locale, allowDownload: request.allowDownload)
    case "request_auth":
        // Operator/CLI: force the system Speech Recognition dialog (if notDetermined).
        return try await requestSpeechAuthorizationResponse()
    case "transcribe":
        // Offline / final-pass: may select SpeechTranscriber OR SFSpeech.
        // Speech Recognition TCC is enforced only inside SFSpeech entry points
        // (W4-B) — ST must not force ensureSpeechAuthorizedForSfSpeech.
        guard let audioPath = request.audioPath, !audioPath.isEmpty else {
            throw BridgeError.missingAudioPath
        }
        let transcription = try await transcribe(audioPath: audioPath, locale: locale)
        return BridgeResponse(
            ok: true,
            status: "ok",
            text: transcription.text,
            segments: transcription.segments,
            localeSupported: true,
            localeInstalled: true,
            backend: transcription.backend.rawValue,
            error: nil,
            speechAuth: speechAuthLabel(SFSpeechRecognizer.authorizationStatus())
        )
    case "stream":
        // Streaming v2: ONE long-lived SFSpeechAudioBufferRecognitionRequest
        // fed raw PCM from stdin — the same shape the SYSTEM dictation uses,
        // and the reason it beats our per-request WAV path (which replays each
        // window at 1× realtime through a muted AVAudioEngine and loses
        // cross-window context). Emits NDJSON partial/final events on stdout
        // as recognition progresses; the closing summary line is a normal
        // BridgeResponse. Protocol: first stdin line = JSON header
        // {"rate":48000,"channels":1}, then raw interleaved f32le frames, EOF
        // ends the stream.
        // Speech TCC: requested inside transcribeStreaming (SF-only path).
        let payload = try await transcribeStreaming(locale: locale)
        return BridgeResponse(
            ok: true,
            status: "ok",
            text: payload.text,
            segments: payload.segments,
            localeSupported: true,
            localeInstalled: true,
            backend: payload.backend.rawValue,
            error: nil,
            speechAuth: speechAuthLabel(SFSpeechRecognizer.authorizationStatus())
        )
    case "transcribe_live":
        // Live / virtual-mic: may select SpeechTranscriber (buffer/file) or
        // SFSpeechAudioBufferRecognitionRequest. Speech TCC only on SF entry.
        guard let audioPath = request.audioPath, !audioPath.isEmpty else {
            throw BridgeError.missingAudioPath
        }
        let transcription = try await transcribeLiveBuffered(audioPath: audioPath, locale: locale)
        return BridgeResponse(
            ok: true,
            status: "ok",
            text: transcription.text,
            segments: transcription.segments,
            localeSupported: true,
            localeInstalled: true,
            backend: transcription.backend.rawValue,
            error: nil,
            speechAuth: speechAuthLabel(SFSpeechRecognizer.authorizationStatus())
        )
    default:
        throw BridgeError.unsupportedCommand(request.command)
    }
}

// MARK: - Probe (ST → DT (opt-in) → SFSpeech on-device)

private func probe(locale: Locale, allowDownload: Bool) async throws -> BridgeResponse {
    // Prefer SpeechTranscriber only when the locale is supported AND installed.
    // Catalog support alone must not abandon SFSpeech on-device (e.g. ST listed
    // for a locale whose model assets are missing).
    let stSupported = await SpeechTranscriber.supportedLocales
    if let effectiveLocale = bestAvailableLocale(requested: locale, available: stSupported) {
        let st = try await probeSpeechTranscriber(
            effectiveLocale: effectiveLocale,
            allowDownload: allowDownload
        )
        if st.localeInstalled == true {
            return st
        }
        // ST supported but not installed (or install failed): try the armed DT
        // lane, then SF.
        if let dt = try await probeDictationTranscriberIfArmed(
            locale: locale,
            allowDownload: allowDownload
        ), dt.localeInstalled == true {
            return dt
        }
        let sf = probeSfSpeech(locale: locale)
        if sf.localeSupported == true {
            return sf
        }
        // Neither ready: return the ST probe (honest not-installed / install error).
        return st
    }

    // ST does not serve this locale (pl-PL today). The armed DT lane gets the
    // next look; unarmed, this whole branch is a no-op and the shipped SFSpeech
    // default is byte-for-byte unchanged.
    let dt = try await probeDictationTranscriberIfArmed(
        locale: locale,
        allowDownload: allowDownload
    )
    if let dt, dt.localeInstalled == true {
        return dt
    }

    // Fallback: SFSpeechRecognizer on-device for locales the analyzer lanes
    // cannot serve.
    let sf = probeSfSpeech(locale: locale)
    if sf.localeSupported == true {
        return sf
    }
    // Nothing ready: prefer the honest DT not-installed report over SF's
    // "unsupported" when DT at least listed the locale.
    return dt ?? sf
}

/// `probeSpeechTranscriber`'s sibling for the DT lane. Returns `nil` when the
/// operator has not armed the lane — the caller then behaves exactly as before.
private func probeDictationTranscriberIfArmed(
    locale: Locale,
    allowDownload: Bool
) async throws -> BridgeResponse? {
    guard dictationTranscriberEnabled() else { return nil }
    let supported = await DictationTranscriber.supportedLocales
    guard let effectiveLocale = bestAvailableLocale(requested: locale, available: supported) else {
        return nil
    }
    return try await probeDictationTranscriber(
        effectiveLocale: effectiveLocale,
        allowDownload: allowDownload
    )
}

private func probeDictationTranscriber(
    effectiveLocale: Locale,
    allowDownload: Bool
) async throws -> BridgeResponse {
    // Reserving is not bookkeeping: measured 2026-08-08, an unreserved locale
    // reports AssetInventory.status == .supported and the analyzer yields ZERO
    // results with no error. After `reserve` the same locale reports .installed
    // and transcribes. Reservation persists across processes.
    let reserved = (try? await AssetInventory.reserve(locale: effectiveLocale)) ?? false
    var installed = await DictationTranscriber.installedLocales
    var isInstalled = containsLocale(installed, locale: effectiveLocale)
    let transcriber = makeDictationTranscriber(locale: effectiveLocale)
    var status = await AssetInventory.status(forModules: [transcriber])

    if status != .installed, allowDownload {
        do {
            guard let downloader = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) else {
                throw BridgeError.runtime("asset installation request unavailable")
            }
            try await downloader.downloadAndInstall()
            installed = await DictationTranscriber.installedLocales
            isInstalled = containsLocale(installed, locale: effectiveLocale)
            status = await AssetInventory.status(forModules: [transcriber])
        } catch {
            return BridgeResponse(
                ok: false,
                status: "error",
                text: "",
                segments: [],
                localeSupported: true,
                localeInstalled: false,
                backend: AppleSttBackend.dictationTranscriber.rawValue,
                error: "asset_install_failed: \(error)",
                speechAuth: speechAuthLabel(SFSpeechRecognizer.authorizationStatus())
            )
        }
    }

    // Both signals must agree. `installedLocales` alone listed pl-PL on a host
    // where the analyzer produced nothing — the asset status is what predicts
    // whether recognition actually runs.
    let ready = isInstalled && status == .installed
    fputs(
        "bridge_dt_probe locale=\(effectiveLocale.identifier) reserved=\(reserved) "
            + "listed=\(isInstalled) asset_status=\(status) ready=\(ready)\n",
        stderr
    )
    return BridgeResponse(
        ok: ready,
        status: ready ? "ok" : "error",
        text: "",
        segments: [],
        localeSupported: true,
        localeInstalled: ready,
        backend: AppleSttBackend.dictationTranscriber.rawValue,
        error: ready ? nil : "dictation_assets_\(status)",
        speechAuth: speechAuthLabel(SFSpeechRecognizer.authorizationStatus())
    )
}

private func probeSpeechTranscriber(
    effectiveLocale: Locale,
    allowDownload: Bool
) async throws -> BridgeResponse {
    var installed = await SpeechTranscriber.installedLocales
    let transcriber = makeTranscriber(locale: effectiveLocale)
    var isInstalled = containsLocale(installed, locale: effectiveLocale)

    if !isInstalled && allowDownload {
        do {
            guard let downloader = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) else {
                throw BridgeError.runtime("asset installation request unavailable")
            }
            try await downloader.downloadAndInstall()
            installed = await SpeechTranscriber.installedLocales
            isInstalled = containsLocale(installed, locale: effectiveLocale)
        } catch {
            return BridgeResponse(
                ok: false,
                status: "error",
                text: "",
                segments: [],
                localeSupported: true,
                localeInstalled: isInstalled,
                backend: AppleSttBackend.speechTranscriber.rawValue,
                error: "asset_install_failed: \(error)",
                speechAuth: speechAuthLabel(SFSpeechRecognizer.authorizationStatus())
            )
        }
    }

    return BridgeResponse(
        ok: true,
        status: "ok",
        text: "",
        segments: [],
        localeSupported: true,
        localeInstalled: isInstalled,
        backend: AppleSttBackend.speechTranscriber.rawValue,
        error: nil,
        speechAuth: speechAuthLabel(SFSpeechRecognizer.authorizationStatus())
    )
}

private func probeSfSpeech(locale: Locale) -> BridgeResponse {
    let auth = SFSpeechRecognizer.authorizationStatus()
    let authLabel = speechAuthLabel(auth)
    guard let recognizer = SFSpeechRecognizer(locale: locale) else {
        return BridgeResponse(
            ok: true,
            status: "ok",
            text: "",
            segments: [],
            localeSupported: false,
            localeInstalled: false,
            backend: nil,
            error: nil,
            speechAuth: authLabel
        )
    }

    let onDevice = recognizer.supportsOnDeviceRecognition
    // Serving path requires on-device recognition (product doctrine for offline Polish).
    // Auth must also be authorized — otherwise probe used to lie "supported" while
    // every transcribe returned empty text (notDetermined / denied).
    let authOk = auth == .authorized
    let supported = recognizer.isAvailable && onDevice && authOk
    var err: String? = nil
    if onDevice && !authOk {
        err = "speech_auth_\(authLabel)"
    }
    return BridgeResponse(
        ok: authOk && supported,
        status: authOk && supported ? "ok" : "error",
        text: "",
        segments: [],
        localeSupported: recognizer.isAvailable && onDevice,
        localeInstalled: onDevice,
        backend: (authOk && supported) ? AppleSttBackend.sfSpeechRecognizer.rawValue : nil,
        error: err,
        speechAuth: authLabel
    )
}

// MARK: - Speech recognition authorization (TCC)

private func speechAuthLabel(_ status: SFSpeechRecognizerAuthorizationStatus) -> String {
    switch status {
    case .notDetermined: return "not_determined"
    case .denied: return "denied"
    case .restricted: return "restricted"
    case .authorized: return "authorized"
    @unknown default: return "unknown"
    }
}

/// Prompt system dialog when status is notDetermined; fail loudly if denied.
///
/// Call **only** from SFSpeechRecognizer entry points. SpeechTranscriber /
/// DictationTranscriber (SpeechAnalyzer family) must not invoke this — Apple
/// docs do not require Speech Recognition TCC for those APIs. Mic grant for
/// live capture is independent (`AVCaptureDevice` / app permissions).
private func ensureSpeechAuthorizedForSfSpeech() async throws {
    let status = SFSpeechRecognizer.authorizationStatus()
    switch status {
    case .authorized:
        return
    case .denied:
        throw BridgeError.runtime(
            "speech_auth_denied: enable Speech Recognition for this process in System Settings › Privacy & Security › Speech Recognition"
        )
    case .restricted:
        throw BridgeError.runtime("speech_auth_restricted: Speech Recognition restricted by policy")
    case .notDetermined:
        let granted = await requestSpeechAuthorization()
        if !granted {
            throw BridgeError.runtime(
                "speech_auth_denied: user declined Speech Recognition (or dialog could not be shown outside an .app with NSSpeechRecognitionUsageDescription)"
            )
        }
    @unknown default:
        throw BridgeError.runtime("speech_auth_unknown")
    }
}

private func requestSpeechAuthorization() async -> Bool {
    await withCheckedContinuation { (continuation: CheckedContinuation<Bool, Never>) in
        SFSpeechRecognizer.requestAuthorization { status in
            continuation.resume(returning: status == .authorized)
        }
    }
}

private func requestSpeechAuthorizationResponse() async throws -> BridgeResponse {
    let before = speechAuthLabel(SFSpeechRecognizer.authorizationStatus())
    if SFSpeechRecognizer.authorizationStatus() == .notDetermined {
        _ = await requestSpeechAuthorization()
    }
    let after = SFSpeechRecognizer.authorizationStatus()
    let label = speechAuthLabel(after)
    let ok = after == .authorized
    return BridgeResponse(
        ok: ok,
        status: ok ? "ok" : "error",
        text: "",
        segments: [],
        localeSupported: nil,
        localeInstalled: nil,
        backend: nil,
        error: ok ? nil : "speech_auth_\(label) (was \(before))",
        speechAuth: label
    )
}

// MARK: - Transcribe

private struct TranscriptionPayload {
    let text: String
    let segments: [BridgeSegment]
    let backend: AppleSttBackend
}

private func makeTranscriber(locale: Locale) -> SpeechTranscriber {
    SpeechTranscriber(locale: locale, preset: .timeIndexedTranscriptionWithAlternatives)
}

/// `.progressiveLongDictation` plus the time index.
///
/// The stock preset carries punctuation + volatile results but NO
/// `audioTimeRange`, and `BridgeSegment.startTs/endTs` is a hard bridge
/// contract (Silero tail-drop and the overlay both read it). Composing the
/// preset by hand is the DT analogue of ST's
/// `.timeIndexedTranscriptionWithAlternatives`, not a deviation for its own sake.
private func makeDictationTranscriber(locale: Locale) -> DictationTranscriber {
    DictationTranscriber(
        locale: locale,
        contentHints: [],
        transcriptionOptions: [.punctuation],
        reportingOptions: [.volatileResults],
        attributeOptions: [.audioTimeRange]
    )
}

private func transcribe(audioPath: String, locale: Locale) async throws -> TranscriptionPayload {
    // Same ready-backend decision as probe: ST only when supported+installed.
    let supportedLocales = await SpeechTranscriber.supportedLocales
    if let effectiveLocale = bestAvailableLocale(requested: locale, available: supportedLocales) {
        let installed = await SpeechTranscriber.installedLocales
        if containsLocale(installed, locale: effectiveLocale) {
            let payload = try await transcribeWithSpeechTranscriber(
                audioPath: audioPath,
                locale: effectiveLocale
            )
            return TranscriptionPayload(
                text: payload.text,
                segments: payload.segments,
                backend: .speechTranscriber
            )
        }
        // ST in catalog but assets missing → armed DT lane, then SFSpeech.
        if let dt = try await transcribeWithDictationTranscriberIfArmed(
            audioPath: audioPath,
            locale: locale
        ) {
            return dt
        }
        if sfSpeechOnDeviceReady(locale: locale) {
            return try await transcribeWithSfSpeech(audioPath: audioPath, locale: locale)
        }
        // Last resort: attempt ST (may fail with a clear runtime error).
        let payload = try await transcribeWithSpeechTranscriber(
            audioPath: audioPath,
            locale: effectiveLocale
        )
        return TranscriptionPayload(
            text: payload.text,
            segments: payload.segments,
            backend: .speechTranscriber
        )
    }

    if let dt = try await transcribeWithDictationTranscriberIfArmed(
        audioPath: audioPath,
        locale: locale
    ) {
        return dt
    }
    return try await transcribeWithSfSpeech(audioPath: audioPath, locale: locale)
}

/// Returns `nil` when the DT lane is unarmed or cannot serve the locale, so the
/// caller falls through to the shipped default untouched.
private func transcribeWithDictationTranscriberIfArmed(
    audioPath: String,
    locale: Locale
) async throws -> TranscriptionPayload? {
    guard dictationTranscriberEnabled() else { return nil }
    let supported = await DictationTranscriber.supportedLocales
    guard let effectiveLocale = bestAvailableLocale(requested: locale, available: supported) else {
        return nil
    }
    _ = try? await AssetInventory.reserve(locale: effectiveLocale)
    let transcriber = makeDictationTranscriber(locale: effectiveLocale)
    guard await AssetInventory.status(forModules: [transcriber]) == .installed else {
        return nil
    }
    let (text, segments) = try await transcribeWithDictationTranscriber(
        audioPath: audioPath,
        transcriber: transcriber
    )
    return TranscriptionPayload(text: text, segments: segments, backend: .dictationTranscriber)
}

/// File path drives the analyzer with `start(inputAudioFile:finishAfterFile:)`
/// rather than the shared `streamAudio` feeder: Apple owns the read/convert
/// loop, so there is no second place for a format-conversion bug to truncate
/// the corpus.
private func transcribeWithDictationTranscriber(
    audioPath: String,
    transcriber: DictationTranscriber
) async throws -> (text: String, segments: [BridgeSegment]) {
    let analyzer = SpeechAnalyzer(modules: [transcriber])
    let file = try AVAudioFile(forReading: URL(fileURLWithPath: audioPath))

    let collector = Task { () -> (String, [BridgeSegment]) in
        var finalTextParts: [String] = []
        var finalSegments: [BridgeSegment] = []
        var volatileText = ""
        var volatileSegments: [BridgeSegment] = []
        for try await result in transcriber.results {
            let text = String(result.text.characters)
            let segments = segmentsFromAttributedText(result.text)
            if result.isFinal {
                finalTextParts.append(text)
                finalSegments.append(contentsOf: segments)
                volatileText = ""
                volatileSegments = []
            } else {
                volatileText = text
                volatileSegments = segments
            }
        }
        let combined = finalTextParts.joined(separator: " ")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let fallback = volatileText.trimmingCharacters(in: .whitespacesAndNewlines)
        return (
            combined.isEmpty ? fallback : combined,
            normalizeSegments(finalSegments.isEmpty ? volatileSegments : finalSegments)
        )
    }

    try await analyzer.start(inputAudioFile: file, finishAfterFile: true)
    return try await collector.value
}

private func sfSpeechOnDeviceReady(locale: Locale) -> Bool {
    guard let recognizer = SFSpeechRecognizer(locale: locale) else { return false }
    return recognizer.isAvailable && recognizer.supportsOnDeviceRecognition
}

private func transcribeWithSpeechTranscriber(
    audioPath: String,
    locale: Locale
) async throws -> (text: String, segments: [BridgeSegment]) {
    let transcriber = makeTranscriber(locale: locale)
    let analyzer = SpeechAnalyzer(modules: [transcriber])
    guard let analyzerFormat = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [transcriber]) else {
        throw BridgeError.runtime("no compatible analyzer audio format available")
    }
    let (inputSequence, inputBuilder) = AsyncStream<AnalyzerInput>.makeStream()

    var volatileText = ""
    var volatileSegments: [BridgeSegment] = []
    var finalTextParts: [String] = []
    var finalSegments: [BridgeSegment] = []

    let collector = Task {
        for try await result in transcriber.results {
            let text = String(result.text.characters)
            let segments = segmentsFromAttributedText(result.text)
            if result.isFinal {
                finalTextParts.append(text)
                finalSegments.append(contentsOf: segments)
                volatileText = ""
                volatileSegments = []
            } else {
                volatileText = text
                volatileSegments = segments
            }
        }
    }

    try await analyzer.start(inputSequence: inputSequence)
    try streamAudio(
        fromPath: audioPath,
        destinationFormat: analyzerFormat,
        into: inputBuilder
    )
    inputBuilder.finish()
    try await analyzer.finalizeAndFinishThroughEndOfInput()
    _ = await collector.result

    let combined = finalTextParts.joined(separator: " ").trimmingCharacters(in: .whitespacesAndNewlines)
    let fallback = volatileText.trimmingCharacters(in: .whitespacesAndNewlines)
    let text = combined.isEmpty ? fallback : combined
    let segments = normalizeSegments(finalSegments.isEmpty ? volatileSegments : finalSegments)
    return (text, segments)
}

/// Virtual-mic live path: feed fixture/hardware PCM into
/// `SFSpeechAudioBufferRecognitionRequest` (dictation engine), not the file URL engine.
///
/// WAV on disk is only a *source of samples* — same as a virtual CoreAudio device
/// would deliver. Recognition uses the buffer API Apple designs for live mic.
private func transcribeLiveBuffered(audioPath: String, locale: Locale) async throws -> TranscriptionPayload {
    // Prefer SpeechTranscriber streaming when locale is installed; else SF buffer.
    let stSupported = await SpeechTranscriber.supportedLocales
    if let effectiveLocale = bestAvailableLocale(requested: locale, available: stSupported) {
        let installed = await SpeechTranscriber.installedLocales
        if containsLocale(installed, locale: effectiveLocale) {
            // SpeechAnalyzer already streams PCM from file into the analyzer —
            // that is the live/buffer path for ST locales (not SF URL).
            let (text, segments) = try await transcribeWithSpeechTranscriber(
                audioPath: audioPath,
                locale: effectiveLocale
            )
            return TranscriptionPayload(
                text: text,
                segments: segments,
                backend: .speechTranscriber
            )
        }
    }
    return try await transcribeWithSfSpeechAudioBuffer(audioPath: audioPath, locale: locale)
}

private func transcribeWithSfSpeechAudioBuffer(
    audioPath: String,
    locale: Locale
) async throws -> TranscriptionPayload {
    // SFSpeech path only — Speech Recognition TCC required here (not on ST).
    try await ensureSpeechAuthorizedForSfSpeech()
    guard let recognizer = SFSpeechRecognizer(locale: locale) else {
        throw BridgeError.runtime(
            "neither SpeechTranscriber nor SFSpeechRecognizer supports locale \(locale.identifier)"
        )
    }
    guard recognizer.isAvailable else {
        throw BridgeError.runtime("SFSpeechRecognizer is not available for locale \(locale.identifier)")
    }
    guard recognizer.supportsOnDeviceRecognition else {
        throw BridgeError.runtime(
            "SFSpeechRecognizer on-device recognition not supported for locale \(locale.identifier)"
        )
    }

    let url = URL(fileURLWithPath: audioPath)
    let audioFile: AVAudioFile
    do {
        audioFile = try AVAudioFile(forReading: url)
    } catch {
        throw BridgeError.runtime("sf_speech_buffer: open audio failed: \(error.localizedDescription)")
    }

    // Live engine = SFSpeechAudioBufferRecognitionRequest (dictation buffer API).
    // Feed PCM by direct file dump — same request type as a CoreAudio mic tap,
    // without AVAudioEngine real-time playback (which settled on the first
    // isFinal, raced the mixer under concurrent Whisper load, and wall-clocked
    // 1× realtime so multi-utterance e2e often hit timeout/empty).
    let fileFormat = audioFile.processingFormat
    let audioSeconds = Double(audioFile.length) / max(fileFormat.sampleRate, 1.0)
    // On-device SFSpeechAudioBuffer keeps a short active hypothesis window.
    // Continuous dumps longer than ~12–15s routinely collapse to a tail
    // fragment (product VAD already seals shorter utterances; this chunking
    // is the safety net for long temp WAVs / Teacher full-file probes).
    let maxWindowSeconds: Double = 12.0

    if audioSeconds <= maxWindowSeconds {
        return try await recognizeSfSpeechBufferWindow(
            audioFile: audioFile,
            locale: locale,
            recognizer: recognizer,
            startFrame: 0,
            frameCount: AVAudioFramePosition(audioFile.length),
            timeOffset: 0
        )
    }

    var textParts: [String] = []
    var allSegments: [BridgeSegment] = []
    var frame: AVAudioFramePosition = 0
    let total = AVAudioFramePosition(audioFile.length)
    let rate = fileFormat.sampleRate
    let windowFramesMax = AVAudioFramePosition(maxWindowSeconds * rate)
    // Small overlap so phrase boundaries on the cut don't drop words.
    let overlapFrames = AVAudioFramePosition(0.4 * rate)
    while frame < total {
        let windowFrames = min(windowFramesMax, total - frame)
        if windowFrames <= 0 { break }
        let timeOffset = Double(frame) / rate
        let part = try await recognizeSfSpeechBufferWindow(
            audioFile: audioFile,
            locale: locale,
            recognizer: recognizer,
            startFrame: frame,
            frameCount: windowFrames,
            timeOffset: timeOffset
        )
        let trimmed = part.text.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty {
            textParts.append(trimmed)
        }
        allSegments.append(contentsOf: part.segments)
        if windowFrames < windowFramesMax {
            break
        }
        let advance = max(windowFrames - overlapFrames, windowFrames / 2)
        frame += advance
    }
    return TranscriptionPayload(
        text: textParts.joined(separator: " "),
        segments: normalizeSegments(allSegments),
        backend: .sfSpeechRecognizer
    )
}

/// One SFSpeechAudioBuffer window via virtual-mic (AVAudioEngine player → tap).
///
/// Accumulates multi-phrase `isFinal` results and settles only after playback
/// ends — never on the first intermediate final (that was the live under-gen bug).
private func recognizeSfSpeechBufferWindow(
    audioFile: AVAudioFile,
    locale: Locale,
    recognizer: SFSpeechRecognizer,
    startFrame: AVAudioFramePosition,
    frameCount: AVAudioFramePosition,
    timeOffset: Double
) async throws -> TranscriptionPayload {
    let fileFormat = audioFile.processingFormat
    let rate = max(fileFormat.sampleRate, 1.0)
    let audioSeconds = Double(frameCount) / rate
    let deadlineSeconds = max(20.0, audioSeconds + 25.0)
    let retained = recognizer

    // Slice window into a temp buffer file when not the full file — player
    // schedules whole files; for multi-window we read PCM and feed via engine.
    // Single-window full-file path uses scheduleFile for fidelity.
    let isFullFile = startFrame == 0 && frameCount == AVAudioFramePosition(audioFile.length)

    let request = SFSpeechAudioBufferRecognitionRequest()
    request.requiresOnDeviceRecognition = true
    request.shouldReportPartialResults = true
    request.taskHint = .dictation
    // Isolate non-Sendable Speech request behind a serial-queue handle so
    // Dispatch/AVAudio @Sendable completions never capture it bare (S-4).
    let requestHandle = SfSpeechRequestHandle(request, label: "com.vetcoders.codescribe.stt.sf-buffer")

    let engine = AVAudioEngine()
    let player = AVAudioPlayerNode()
    engine.attach(player)
    engine.connect(player, to: engine.mainMixerNode, format: fileFormat)
    engine.mainMixerNode.outputVolume = 0

    let bus: AVAudioNodeBus = 0
    let bufferSize: AVAudioFrameCount = 1024
    // Tap the player (proven path for short Polish fixtures). Mixer-bus tap
    // returned silent buffers on this machine.
    player.installTap(onBus: bus, bufferSize: bufferSize, format: fileFormat) { buffer, _ in
        requestHandle.append(buffer)
    }

    let session = SfSpeechBufferSession(
        player: player, engine: engine, bus: bus, request: requestHandle)

    return try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<TranscriptionPayload, Error>) in
        let gate = SfSpeechSettleGate()
        let acc = SfSpeechPhraseAccumulator(timeOffset: timeOffset)
        let timeout = SfSpeechTimeoutCancel()
        let settle: @Sendable (Result<TranscriptionPayload, Error>) -> Void = { outcome in
            guard gate.trySettle() else { return }
            session.tearDown()
            switch outcome {
            case .success(let payload):
                continuation.resume(returning: payload)
            case .failure(let error):
                continuation.resume(throwing: error)
            }
        }
        let localeId = locale.identifier
        timeout.schedule(after: deadlineSeconds) {
            gate.cancelTask()
            if let payload = acc.snapshotPayload(), !payload.text.isEmpty {
                settle(.success(payload))
            } else {
                settle(.failure(BridgeError.runtime(
                    "sf_speech_buffer: recognition_timeout after \(String(format: "%.1f", deadlineSeconds))s (locale=\(localeId))"
                )))
            }
        }

        let task = retained.recognitionTask(with: requestHandle.bareRequest) { result, error in
            if gate.isSettled() { return }
            if let error {
                if let payload = acc.snapshotPayload(), !payload.text.isEmpty {
                    timeout.cancel()
                    settle(.success(payload))
                    return
                }
                let ns = error as NSError
                // Cancel / no-speech after endAudio — treat as empty success when no text.
                if ns.domain == "kAFAssistantErrorDomain" || ns.code == 216 || ns.code == 1 {
                    timeout.cancel()
                    settle(.success(
                        TranscriptionPayload(text: "", segments: [], backend: .sfSpeechRecognizer)
                    ))
                    return
                }
                timeout.cancel()
                settle(.failure(BridgeError.runtime(
                    "sf_speech_buffer: \(error.localizedDescription) (domain=\(ns.domain) code=\(ns.code))"
                )))
                return
            }
            guard let result else { return }
            let text = result.bestTranscription.formattedString
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let segments = result.bestTranscription.segments.compactMap { segment -> BridgeSegment? in
                let segText = segment.substring.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !segText.isEmpty else { return nil }
                let start = segment.timestamp + timeOffset
                let end = segment.timestamp + segment.duration + timeOffset
                guard start.isFinite, end.isFinite, end >= start else { return nil }
                return BridgeSegment(text: segText, startTs: start, endTs: end)
            }
            if result.isFinal {
                // Multi-phrase: freeze and keep listening — do NOT settle here.
                acc.commitFinal(text: text, segments: segments)
            } else {
                acc.storePartial(text: text, segments: segments)
            }
        }
        gate.storeTask(task)

        do {
            try engine.start()
            let onPlaybackDone: @Sendable () -> Void = {
                // File finished through the engine — end recognition input.
                DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + 0.35) {
                    if gate.isSettled() { return }
                    requestHandle.endAudio()
                    let settleGrace = max(1.5, min(4.0, audioSeconds * 0.05 + 1.2))
                    DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + settleGrace) {
                        if gate.isSettled() { return }
                        timeout.cancel()
                        if let payload = acc.snapshotPayload() {
                            settle(.success(payload))
                        } else {
                            settle(.success(
                                TranscriptionPayload(text: "", segments: [], backend: .sfSpeechRecognizer)
                            ))
                        }
                    }
                }
            }

            if isFullFile {
                player.scheduleFile(audioFile, at: nil, completionHandler: onPlaybackDone)
            } else {
                // Windowed: schedule a single PCM buffer for [startFrame, frameCount).
                audioFile.framePosition = startFrame
                guard let buffer = AVAudioPCMBuffer(
                    pcmFormat: fileFormat,
                    frameCapacity: AVAudioFrameCount(frameCount)
                ) else {
                    throw BridgeError.runtime("sf_speech_buffer: window buffer alloc failed")
                }
                try audioFile.read(into: buffer, frameCount: AVAudioFrameCount(frameCount))
                player.scheduleBuffer(buffer, completionHandler: onPlaybackDone)
            }
            player.play()
        } catch {
            timeout.cancel()
            settle(.failure(BridgeError.runtime(
                "sf_speech_buffer: engine start failed: \(error.localizedDescription)"
            )))
        }
    }
}

// MARK: - Streaming v2 (one long-lived request, PCM from stdin)

/// One NDJSON progress event on stdout. Every line the Rust client reads has
/// either an `event` key (progress) or a `status` key (the closing summary
/// BridgeResponse) — that disjunction IS the framing.
private struct StreamEventOut: Codable {
    let event: String
    var text: String? = nil
    var segments: [BridgeSegment]? = nil
    var message: String? = nil
}

/// Static holder: globals in a main file initialize in top-level source order
/// (this one sits after `dispatchMain()` and would NEVER run) — a `static let`
/// is lazily initialized on first use instead.
private enum StreamOut {
    static let lock = NSLock()
}

private func emitStreamEvent(_ event: StreamEventOut) {
    let encoder = JSONEncoder()
    encoder.keyEncodingStrategy = .convertToSnakeCase
    guard let data = try? encoder.encode(event), let newline = "\n".data(using: .utf8) else {
        return
    }
    StreamOut.lock.lock()
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(newline)
    StreamOut.lock.unlock()
}

/// Read one `\n`-terminated line from raw stdin without libc stdio buffering —
/// the bytes after this line are binary PCM and must not be swallowed by a
/// lookahead buffer.
private func readRawStdinLine(maxBytes: Int = 4096) -> Data? {
    var line = Data()
    var byte: UInt8 = 0
    while line.count < maxBytes {
        let n = read(0, &byte, 1)
        if n <= 0 { return line.isEmpty ? nil : line }
        if byte == UInt8(ascii: "\n") { return line }
        line.append(byte)
    }
    return line
}

private struct StreamHeader: Codable {
    let rate: Double
    let channels: Int
}

/// Streaming v2 session: header line → raw f32le PCM until EOF, ONE
/// long-lived `SFSpeechAudioBufferRecognitionRequest` for the whole stream.
///
/// This is the system-dictation shape: phrase-level `isFinal` results arrive
/// mid-stream while audio keeps flowing into the SAME request, so the engine
/// keeps full context — no window seams to drop heads at, no 1× realtime
/// replay. Partials become `partial` events, phrase finals become `final`
/// events (with `SFTranscriptionSegment.confidence`), EOF ends audio and the
/// accumulated text is returned as the summary payload.
private func transcribeStreaming(locale: Locale) async throws -> TranscriptionPayload {
    // Stream command is SFSpeechAudioBuffer only today — gate Speech TCC here.
    try await ensureSpeechAuthorizedForSfSpeech()
    guard let headerData = readRawStdinLine(),
        let header = try? {
            let decoder = JSONDecoder()
            decoder.keyDecodingStrategy = .convertFromSnakeCase
            return try decoder.decode(StreamHeader.self, from: headerData)
        }()
    else {
        throw BridgeError.invalidInput("stream: missing/invalid header line {rate, channels}")
    }
    guard header.rate > 0, (1...16).contains(header.channels) else {
        throw BridgeError.invalidInput(
            "stream: unsupported format rate=\(header.rate) channels=\(header.channels)")
    }
    guard
        let format = AVAudioFormat(
            commonFormat: .pcmFormatFloat32,
            sampleRate: header.rate,
            channels: AVAudioChannelCount(header.channels),
            interleaved: true
        )
    else {
        throw BridgeError.runtime("stream: AVAudioFormat init failed")
    }
    guard let recognizer = SFSpeechRecognizer(locale: locale) else {
        throw BridgeError.runtime("stream: SFSpeechRecognizer init failed for \(locale.identifier)")
    }
    recognizer.defaultTaskHint = .dictation

    let request = SFSpeechAudioBufferRecognitionRequest()
    request.requiresOnDeviceRecognition = true
    request.shouldReportPartialResults = true
    request.taskHint = .dictation
    // Stream PCM pump runs on a Dispatch queue (@Sendable); isolate the bare
    // request so append/endAudio never cross the Sendable boundary (L1068).
    let requestHandle = SfSpeechRequestHandle(request, label: "com.vetcoders.codescribe.stt.sf-stream")

    emitStreamEvent(StreamEventOut(event: "ready"))

    return try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<TranscriptionPayload, Error>) in
        let gate = SfSpeechSettleGate()
        let acc = SfSpeechPhraseAccumulator(timeOffset: 0)
        let settle: @Sendable (Result<TranscriptionPayload, Error>) -> Void = { outcome in
            guard gate.trySettle() else { return }
            requestHandle.endAudio()
            switch outcome {
            case .success(let payload):
                emitStreamEvent(StreamEventOut(event: "end"))
                continuation.resume(returning: payload)
            case .failure(let error):
                emitStreamEvent(
                    StreamEventOut(event: "error", message: String(describing: error)))
                continuation.resume(throwing: error)
            }
        }

        let task = recognizer.recognitionTask(with: requestHandle.bareRequest) { result, error in
            if gate.isSettled() { return }
            if let error {
                if let payload = acc.snapshotPayload(), !payload.text.isEmpty {
                    settle(.success(payload))
                    return
                }
                let ns = error as NSError
                // Cancel / no-speech after endAudio — empty success, not a failure.
                if ns.domain == "kAFAssistantErrorDomain" || ns.code == 216 || ns.code == 1 {
                    settle(.success(
                        TranscriptionPayload(text: "", segments: [], backend: .sfSpeechRecognizer)
                    ))
                    return
                }
                settle(.failure(BridgeError.runtime(
                    "stream: \(error.localizedDescription) (domain=\(ns.domain) code=\(ns.code))"
                )))
                return
            }
            guard let result else { return }
            let text = result.bestTranscription.formattedString
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let segments = result.bestTranscription.segments.compactMap { segment -> BridgeSegment? in
                let segText = segment.substring.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !segText.isEmpty else { return nil }
                let start = segment.timestamp
                let end = segment.timestamp + segment.duration
                guard start.isFinite, end.isFinite, end >= start else { return nil }
                return BridgeSegment(
                    text: segText,
                    startTs: start,
                    endTs: end,
                    confidence: Double(segment.confidence)
                )
            }
            if result.isFinal {
                acc.commitFinal(text: text, segments: segments)
                emitStreamEvent(StreamEventOut(event: "final", text: text, segments: segments))
            } else {
                // A partial that collapses to an unrelated hypothesis means
                // SFSpeech restarted its phrase (measured: 13 restarts vs ONE
                // isFinal over 150s of Polish). The accumulator already folds
                // the frozen phrase into its snapshot; emitting it here is what
                // makes the EVENT stream complete and disjoint. Without it the
                // consumer sees only overlapping partials and re-derives phrase
                // boundaries wrongly — the duplicated span + dropped span in
                // the 0.777 parity run.
                if let frozen = acc.storePartial(text: text, segments: segments) {
                    emitStreamEvent(
                        StreamEventOut(
                            event: "final", text: frozen.text, segments: frozen.segments))
                }
                emitStreamEvent(StreamEventOut(event: "partial", text: text, segments: segments))
            }
        }
        gate.storeTask(task)

        // Blocking PCM pump on a background queue: read stdin → append buffers.
        // The recognizer's callbacks land on its own queue; nothing here blocks
        // them. EOF → endAudio → settle grace → snapshot.
        let streamRate = header.rate
        let streamChannels = header.channels
        DispatchQueue.global(qos: .userInitiated).async {
            let bytesPerFrame = 4 * streamChannels
            let chunkBytes = 32768
            var carry = Data()
            var raw = [UInt8](repeating: 0, count: chunkBytes)
            var fedFrames: Int64 = 0
            while true {
                let n = read(0, &raw, chunkBytes)
                if n < 0 {
                    if errno == EINTR { continue }
                    break
                }
                if n == 0 { break }  // EOF — sender closed the stream.
                carry.append(contentsOf: raw[0..<n])
                let frames = carry.count / bytesPerFrame
                if frames == 0 { continue }
                let usable = frames * bytesPerFrame
                guard
                    let buffer = AVAudioPCMBuffer(
                        pcmFormat: format, frameCapacity: AVAudioFrameCount(frames))
                else { continue }
                buffer.frameLength = AVAudioFrameCount(frames)
                carry.withUnsafeBytes { (src: UnsafeRawBufferPointer) in
                    if let dst = buffer.floatChannelData?[0], let base = src.baseAddress {
                        memcpy(dst, base, usable)
                    }
                }
                carry.removeSubrange(0..<usable)
                fedFrames += Int64(frames)
                requestHandle.append(buffer)
            }

            if gate.isSettled() { return }
            requestHandle.endAudio()
            // Settle grace scaled to how much audio flowed (same shape as the
            // window path) — SFSpeech needs a beat to flush its last final.
            let fedSeconds = Double(fedFrames) / streamRate
            let settleGrace = max(1.5, min(6.0, fedSeconds * 0.05 + 1.5))
            DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + settleGrace) {
                if gate.isSettled() { return }
                gate.cancelTask()
                // The stream ends mid-phrase: freeze and EMIT the open
                // hypothesis, or the last phrase reaches only the summary and
                // the consumer's transcript loses its tail.
                if let frozen = acc.freezeOpenPhrase() {
                    emitStreamEvent(
                        StreamEventOut(
                            event: "final", text: frozen.text, segments: frozen.segments))
                }
                if let payload = acc.snapshotPayload() {
                    settle(.success(payload))
                } else {
                    settle(.success(
                        TranscriptionPayload(text: "", segments: [], backend: .sfSpeechRecognizer)
                    ))
                }
            }
        }
    }
}

/// Accumulates multi-phrase SFSpeech finals + current partial for one buffer request.
///
/// Critical: intermediate `isFinal` must NOT settle the request — continuous
/// Polish dictation emits phrase-level finals while more audio still arrives.
final class SfSpeechPhraseAccumulator: @unchecked Sendable {
    private let lock = NSLock()
    private let timeOffset: Double
    private var finals: [String] = []
    private var finalSegments: [BridgeSegment] = []
    private var partialText: String = ""
    private var partialSegments: [BridgeSegment] = []

    init(timeOffset: Double) {
        self.timeOffset = timeOffset
    }

    fileprivate func commitFinal(text: String, segments: [BridgeSegment]) {
        lock.lock()
        defer { lock.unlock() }
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if !t.isEmpty {
            finals.append(t)
            finalSegments.append(contentsOf: segments)
        }
        partialText = ""
        partialSegments = []
    }

    /// A hypothesis the accumulator just froze — the streaming path emits it as
    /// a `final` event so the Rust side receives DISJOINT phrases instead of
    /// having to re-derive phrase boundaries from raw partials.
    struct FrozenPhrase {
        let text: String
        let segments: [BridgeSegment]
    }

    /// Store the current hypothesis; returns the previous one when SFSpeech
    /// restarted its phrase (measured on a 150s Polish fixture: 13 restarts,
    /// ONE `isFinal`). Callers that ignore the return value keep the old
    /// behaviour — the phrase is still folded into `snapshotPayload`.
    @discardableResult
    fileprivate func storePartial(text: String, segments: [BridgeSegment]) -> FrozenPhrase? {
        lock.lock()
        defer { lock.unlock() }
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        var frozen: FrozenPhrase? = nil
        // Detect SFSpeech phrase restart without isFinal → freeze prior.
        //
        // A RESTART collapses the hypothesis to a few words; a REVISION keeps
        // most of it and only rewords the middle. Measured on the parity
        // fixture: all 13 restarts landed at ≤12 chars (from 47…191), while a
        // revision shrank 95 → 79 — and the old "shorter by 12+" rule froze
        // that revision as a phrase, sealing the same span twice (the residual
        // duplication at similarity 0.872).
        //
        // NAMED DROP MECHANISM (w1-b, 2026-08-10 three-way live):
        // `shared_opener_restart_suppresses_freeze`. Consecutive Polish
        // sentences share openers ("Zdanie" / "Zadanie"). The previous rule
        // treated `prev.hasPrefix(t)` / substring containment as "extends",
        // so a post-stressor collapse to the next sentence's short opener
        // OVERWROTE the open hypothesis without freezing it — s6/s8/s10
        // vanished from committed raw while native dictation kept them.
        // Same-phrase rewind only suppresses freeze when the short text is a
        // TRUE substantial prefix (>15 chars) of the prior hypothesis.
        if !partialText.isEmpty && !t.isEmpty {
            let prev = partialText
            if Self.phraseRestartShouldFreezePrior(prev: prev, next: t) {
                finals.append(prev)
                finalSegments.append(contentsOf: partialSegments)
                frozen = FrozenPhrase(text: prev, segments: partialSegments)
                let ts = ISO8601DateFormatter().string(from: Date())
                fputs(
                    "apple_lifecycle: freeze reason=shared_opener_restart_suppresses_freeze "
                        + "ts=\(ts) prev_chars=\(prev.count) next_chars=\(t.count) "
                        + "prev_head=\(String(prev.prefix(40))) next_head=\(String(t.prefix(40)))\n",
                    stderr
                )
            }
        }
        partialText = t
        partialSegments = segments
        return frozen
    }

    /// Pure freeze decision — kept in lockstep with
    /// `phrase_restart_should_freeze_prior` in `apple_live_session.rs`.
    fileprivate static func phraseRestartShouldFreezePrior(prev: String, next: String) -> Bool {
        let prev = prev.trimmingCharacters(in: .whitespacesAndNewlines)
        let next = next.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !prev.isEmpty, !next.isEmpty else { return false }
        let restarted = (next.count * 3 < prev.count) || (next.count <= 15 && prev.count >= 25)
        guard restarted else { return false }
        // Substantial true-prefix rewind of the SAME phrase — not a 1–2 word
        // opener every "Zdanie N" sentence shares.
        let samePhraseRewind = prev.hasPrefix(next) && next.count > 15
        return !samePhraseRewind
    }

    /// Freeze whatever hypothesis is still open (stream ended mid-phrase).
    /// Without this the LAST phrase reaches only `snapshotPayload`, never the
    /// event stream — the tail-drop seen in the parity run.
    fileprivate func freezeOpenPhrase() -> FrozenPhrase? {
        lock.lock()
        defer { lock.unlock() }
        guard !partialText.isEmpty else { return nil }
        let frozen = FrozenPhrase(text: partialText, segments: partialSegments)
        finals.append(partialText)
        finalSegments.append(contentsOf: partialSegments)
        partialText = ""
        partialSegments = []
        return frozen
    }

    fileprivate func snapshotPayload() -> TranscriptionPayload? {
        lock.lock()
        defer { lock.unlock() }
        var parts = finals
        if !partialText.isEmpty {
            parts.append(partialText)
        }
        let text = parts.joined(separator: " ").trimmingCharacters(in: .whitespacesAndNewlines)
        var segs = finalSegments
        if !partialText.isEmpty {
            segs.append(contentsOf: partialSegments)
        }
        // Return payload even when empty so callers can treat silence honestly.
        return TranscriptionPayload(
            text: text,
            segments: normalizeSegments(segs),
            backend: .sfSpeechRecognizer
        )
    }
}

/// Test-only command path over the same accumulator decision used in live
/// streaming. It intentionally reads the exact TSV compiled into the Rust
/// mirror test, so fixture drift cannot make the two languages look aligned.
private func runPhraseRestartVectorSelfTest() -> Int32 {
    let source = URL(fileURLWithPath: #filePath)
    let root = source
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .deletingLastPathComponent()
    let fixture = root.appendingPathComponent("tests/fixtures/phrase_restart_vectors.tsv")

    do {
        let contents = try String(contentsOf: fixture, encoding: .utf8)
        var sawNamedCase = false
        for line in contents.split(separator: "\n").map(String.init)
            where !line.hasPrefix("#")
        {
            let fields = line.split(separator: "\t", omittingEmptySubsequences: false).map(String.init)
            guard fields.count == 4, let expected = Bool(fields[1]) else {
                fputs("phrase_restart_self_test malformed_vector=\(line)\n", stderr)
                return 2
            }
            if fields[0] == "missed_collapse_40_to_20" {
                sawNamedCase = true
                guard fields[2].count == 40, fields[3].count == 20 else {
                    fputs("phrase_restart_self_test invalid_40_to_20_lengths\n", stderr)
                    return 2
                }
            }
            let actual = SfSpeechPhraseAccumulator.phraseRestartShouldFreezePrior(
                prev: fields[2],
                next: fields[3]
            )
            guard actual == expected else {
                fputs(
                    "phrase_restart_self_test FAIL id=\(fields[0]) expected=\(expected) "
                        + "actual=\(actual) prev_chars=\(fields[2].count) "
                        + "next_chars=\(fields[3].count)\n",
                    stderr
                )
                return 1
            }
        }
        guard sawNamedCase else {
            fputs("phrase_restart_self_test missing=missed_collapse_40_to_20\n", stderr)
            return 2
        }
        fputs("phrase_restart_self_test PASS\n", stderr)
        return 0
    } catch {
        fputs("phrase_restart_self_test fixture_error=\(error)\n", stderr)
        return 2
    }
}

private func transcribeWithSfSpeech(audioPath: String, locale: Locale) async throws -> TranscriptionPayload {
    // SFSpeech URL path — Speech Recognition TCC required here (not on ST).
    try await ensureSpeechAuthorizedForSfSpeech()
    guard let recognizer = SFSpeechRecognizer(locale: locale) else {
        throw BridgeError.runtime(
            "neither SpeechTranscriber nor SFSpeechRecognizer supports locale \(locale.identifier)"
        )
    }
    guard recognizer.isAvailable else {
        throw BridgeError.runtime("SFSpeechRecognizer is not available for locale \(locale.identifier)")
    }
    guard recognizer.supportsOnDeviceRecognition else {
        throw BridgeError.runtime(
            "SFSpeechRecognizer on-device recognition not supported for locale \(locale.identifier)"
        )
    }

    let url = URL(fileURLWithPath: audioPath)
    let request = SFSpeechURLRecognitionRequest(url: url)
    request.requiresOnDeviceRecognition = true
    request.shouldReportPartialResults = false

    // Keep the recognizer alive for the task lifetime.
    let retained = recognizer
    // In-bridge deadline sits under the Apple stop-path budget (≤3 s final pass)
    // and well below the Rust bridge subprocess timeout (30 s) so a stalled
    // callback can still fall back to Whisper promptly.
    let deadlineSeconds = sfSpeechRecognitionDeadlineSeconds()
    return try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<TranscriptionPayload, Error>) in
        // settled + recognitionTask are shared between the timeout queue and the
        // recognition callback queue — serialize all access under one lock so
        // resume happens exactly once under race.
        let gate = SfSpeechSettleGate()
        let settle: (Result<TranscriptionPayload, Error>) -> Void = { outcome in
            guard gate.trySettle() else { return }
            switch outcome {
            case .success(let payload):
                continuation.resume(returning: payload)
            case .failure(let error):
                continuation.resume(throwing: error)
            }
        }
        let timeoutWork = DispatchWorkItem {
            gate.cancelTask()
            settle(.failure(BridgeError.runtime("sf_speech: recognition_timeout")))
        }
        DispatchQueue.global(qos: .userInitiated).asyncAfter(
            deadline: .now() + deadlineSeconds,
            execute: timeoutWork
        )
        let task = retained.recognitionTask(with: request) { result, error in
            if gate.isSettled() { return }
            if let error {
                timeoutWork.cancel()
                settle(.failure(BridgeError.runtime("sf_speech: \(error.localizedDescription)")))
                return
            }
            guard let result, result.isFinal else { return }
            timeoutWork.cancel()
            let text = result.bestTranscription.formattedString
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let segments = result.bestTranscription.segments.compactMap { segment -> BridgeSegment? in
                let segText = segment.substring.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !segText.isEmpty else { return nil }
                let start = segment.timestamp
                let end = segment.timestamp + segment.duration
                guard start.isFinite, end.isFinite, end >= start else { return nil }
                return BridgeSegment(text: segText, startTs: start, endTs: end)
            }
            settle(.success(
                TranscriptionPayload(
                    text: text,
                    segments: normalizeSegments(segments),
                    backend: .sfSpeechRecognizer
                )
            ))
        }
        gate.storeTask(task)
    }
}

/// Exactly-once settle gate for SFSpeech timeout vs callback race.
///
/// Timeout work and recognition callbacks run on different queues; all state
/// transitions go through this lock so resume is exactly-once and task cancel
/// cannot race the assignment of `recognitionTask`.
final class SfSpeechSettleGate: @unchecked Sendable {
    private let lock = NSLock()
    private var settled = false
    private var recognitionTask: SFSpeechRecognitionTask?

    func storeTask(_ task: SFSpeechRecognitionTask) {
        lock.lock()
        defer { lock.unlock() }
        recognitionTask = task
        if settled {
            // Timeout already won before the task handle was stored.
            task.cancel()
        }
    }

    func cancelTask() {
        lock.lock()
        defer { lock.unlock() }
        recognitionTask?.cancel()
    }

    func trySettle() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if settled { return false }
        settled = true
        return true
    }

    func isSettled() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return settled
    }
}

// MARK: - Sendable isolation (actor/serial-queue; never suppress-attribute)

/// Serial-queue isolation for non-Sendable `SFSpeechAudioBufferRecognitionRequest`.
///
/// Dispatch / AVAudio completion handlers are `@Sendable`; capturing the bare
/// request crosses that boundary and surfaces Sendable warnings (and hard
/// failures under `-warnings-as-errors`). Callers capture this handle instead
/// and hop through the serial queue for append/endAudio.
final class SfSpeechRequestHandle: @unchecked Sendable {
    private let queue: DispatchQueue
    private let request: SFSpeechAudioBufferRecognitionRequest

    init(
        _ request: SFSpeechAudioBufferRecognitionRequest,
        label: String = "com.vetcoders.codescribe.stt.sf-request"
    ) {
        self.request = request
        self.queue = DispatchQueue(label: label)
    }

    /// Bare request for APIs that require it on the creating thread
    /// (`recognitionTask(with:)`, installTap setup before concurrent access).
    var bareRequest: SFSpeechAudioBufferRecognitionRequest { request }

    func append(_ buffer: AVAudioPCMBuffer) {
        queue.sync { request.append(buffer) }
    }

    func endAudio() {
        queue.sync { request.endAudio() }
    }
}

/// Cancelable timeout without capturing non-Sendable `DispatchWorkItem` in
/// nested `@Sendable` closures (the L829 warning surface).
final class SfSpeechTimeoutCancel: @unchecked Sendable {
    private let lock = NSLock()
    private var cancelled = false
    private var workItem: DispatchWorkItem?

    func schedule(
        after seconds: TimeInterval,
        qos: DispatchQoS.QoSClass = .userInitiated,
        _ body: @escaping @Sendable () -> Void
    ) {
        let item = DispatchWorkItem { [weak self] in
            guard let self, !self.isCancelled else { return }
            body()
        }
        lock.lock()
        workItem = item
        lock.unlock()
        DispatchQueue.global(qos: qos).asyncAfter(deadline: .now() + seconds, execute: item)
    }

    func cancel() {
        lock.lock()
        cancelled = true
        workItem?.cancel()
        workItem = nil
        lock.unlock()
    }

    var isCancelled: Bool {
        lock.lock()
        defer { lock.unlock() }
        return cancelled
    }
}

/// Holds non-Sendable AVAudioEngine / player for buffer-path teardown so
/// `@Sendable` completion handlers never capture them directly.
final class SfSpeechBufferSession: @unchecked Sendable {
    private let lock = NSLock()
    private var player: AVAudioPlayerNode?
    private var engine: AVAudioEngine?
    private let bus: AVAudioNodeBus
    private let request: SfSpeechRequestHandle
    private var tornDown = false

    init(
        player: AVAudioPlayerNode,
        engine: AVAudioEngine,
        bus: AVAudioNodeBus,
        request: SfSpeechRequestHandle
    ) {
        self.player = player
        self.engine = engine
        self.bus = bus
        self.request = request
    }

    func tearDown() {
        lock.lock()
        defer { lock.unlock() }
        guard !tornDown else { return }
        tornDown = true
        player?.removeTap(onBus: bus)
        player?.stop()
        engine?.stop()
        request.endAudio()
        player = nil
        engine = nil
    }
}

/// Seconds before a stalled SFSpeech callback is cancelled. Overridable for tests.
func sfSpeechRecognitionDeadlineSeconds() -> TimeInterval {
    if let raw = ProcessInfo.processInfo.environment["CODESCRIBE_SFSPEECH_DEADLINE_SECS"],
       let value = TimeInterval(raw), value > 0, value <= 30
    {
        return value
    }
    // Comfortably below the 3 s Apple final-pass product budget.
    return 2.5
}

// MARK: - Segment helpers

private func segmentsFromAttributedText(_ attributedText: AttributedString) -> [BridgeSegment] {
    attributedText.runs.compactMap { run in
        guard let timeRange = run.audioTimeRange else {
            return nil
        }
        let text = String(attributedText[run.range].characters)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else {
            return nil
        }
        let start = timeRange.start.seconds
        let end = timeRange.end.seconds
        guard start.isFinite, end.isFinite, end >= start else {
            return nil
        }
        return BridgeSegment(text: text, startTs: start, endTs: end)
    }
}

private func normalizeSegments(_ segments: [BridgeSegment]) -> [BridgeSegment] {
    let sorted = segments.sorted {
        if $0.startTs == $1.startTs {
            return $0.endTs < $1.endTs
        }
        return $0.startTs < $1.startTs
    }
    var normalized: [BridgeSegment] = []
    var previousEnd = -Double.infinity
    for segment in sorted {
        let text = segment.text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty,
              segment.startTs.isFinite,
              segment.endTs.isFinite,
              segment.endTs >= segment.startTs
        else {
            continue
        }
        if segment.startTs < previousEnd, segment.endTs <= previousEnd {
            continue
        }
        let start = max(segment.startTs, previousEnd)
        guard segment.endTs >= start else {
            continue
        }
        normalized.append(BridgeSegment(text: text, startTs: start, endTs: segment.endTs))
        previousEnd = segment.endTs
    }
    return normalized
}

/// Feed a file into the analyzer, resampling to the analyzer's format.
///
/// The input block is a PULL callback: `AVAudioConverter` calls it as often as
/// it needs while filling one destination buffer, and a sample-rate converter
/// needs it more than once. The previous shape handed the converter a single
/// source buffer per `convert` call and answered `.endOfStream` for every
/// further request — so the converter reported `.endOfStream` on the second
/// outer iteration and the loop returned. Measured 2026-08-08 on the 140.85 s
/// pl-PL parity fixture (44.1 kHz → 16 kHz): **1 buffer / 1486 frames** reached
/// the analyzer instead of ~2.25 M, i.e. the file path was truncated to ~0.09 s
/// of audio. Only the ST lane consumes this feeder, and ST serves no Polish
/// locale, which is why the truncation never showed up in the pl-PL bars.
private func streamAudio(
    fromPath path: String,
    destinationFormat: AVAudioFormat,
    into builder: AsyncStream<AnalyzerInput>.Continuation
) throws {
    let url = URL(fileURLWithPath: path)
    let inputFile = try AVAudioFile(forReading: url)
    let sourceFormat = inputFile.processingFormat
    let sourceFrameCapacity: AVAudioFrameCount = 4096

    guard let converter = AVAudioConverter(from: sourceFormat, to: destinationFormat) else {
        throw BridgeError.runtime("unable to create AVAudioConverter")
    }

    // Destination capacity must cover the resampled span of one source read
    // plus converter slack; upsampling makes the ratio > 1.
    let rateRatio = destinationFormat.sampleRate / sourceFormat.sampleRate
    let destFrameCapacity = AVAudioFrameCount(
        (Double(sourceFrameCapacity) * max(rateRatio, 1.0)).rounded(.up) + 1024.0
    )

    var readError: Error?
    var reachedEndOfFile = false

    while true {
        guard let destBuffer = AVAudioPCMBuffer(
            pcmFormat: destinationFormat,
            frameCapacity: destFrameCapacity
        ) else {
            throw BridgeError.runtime("unable to allocate destination buffer")
        }

        var convertError: NSError?
        let status = converter.convert(to: destBuffer, error: &convertError) { _, outStatus in
            // `framePosition >= length` is the reliable end-of-file signal.
            // Reading past it does not return an empty buffer — it throws a bare
            // ObjC failure with no NSError attached
            // (`Foundation._GenericObjCError.nilError`, measured 2026-08-08), so
            // a feeder that only checks `frameLength == 0` reports EOF as a hard
            // transcription error.
            if reachedEndOfFile || inputFile.framePosition >= inputFile.length {
                reachedEndOfFile = true
                outStatus.pointee = .endOfStream
                return nil
            }
            guard let sourceBuffer = AVAudioPCMBuffer(
                pcmFormat: sourceFormat,
                frameCapacity: sourceFrameCapacity
            ) else {
                readError = BridgeError.runtime("unable to allocate source buffer")
                outStatus.pointee = .endOfStream
                return nil
            }
            do {
                try inputFile.read(into: sourceBuffer)
            } catch {
                // Only fatal while frames remain; at the boundary it is EOF.
                if inputFile.framePosition < inputFile.length {
                    readError = error
                }
                reachedEndOfFile = true
                outStatus.pointee = .endOfStream
                return nil
            }
            if sourceBuffer.frameLength == 0 {
                reachedEndOfFile = true
                outStatus.pointee = .endOfStream
                return nil
            }
            outStatus.pointee = .haveData
            return sourceBuffer
        }

        if let readError {
            throw readError
        }
        if let convertError {
            throw BridgeError.runtime("audio conversion failed: \(convertError)")
        }

        if destBuffer.frameLength > 0 {
            builder.yield(AnalyzerInput(buffer: destBuffer))
        }

        switch status {
        case .haveData, .inputRanDry:
            // `.inputRanDry` with the file exhausted means the converter has
            // nothing left to pull — keep going only while audio remains.
            if reachedEndOfFile, destBuffer.frameLength == 0 {
                return
            }
        case .endOfStream:
            return
        case .error:
            throw BridgeError.runtime("audio conversion failed with .error")
        @unknown default:
            throw BridgeError.runtime("audio conversion failed with unknown status")
        }
    }
}

// MARK: - Locale helpers

private func bestAvailableLocale(requested: Locale, available: [Locale]) -> Locale? {
    let requestedNormalized = normalizedLocaleIdentifier(requested.identifier)
    if let exact = available.first(where: { normalizedLocaleIdentifier($0.identifier) == requestedNormalized }) {
        return exact
    }
    guard let requestedLanguage = localeLanguageCode(from: requestedNormalized) else {
        return nil
    }
    return available.first {
        localeLanguageCode(from: normalizedLocaleIdentifier($0.identifier)) == requestedLanguage
    }
}

private func containsLocale(_ locales: [Locale], locale: Locale) -> Bool {
    bestAvailableLocale(requested: locale, available: locales) != nil
}

private func localeLanguageCode(from identifier: String) -> String? {
    let normalized = normalizedLocaleIdentifier(identifier)
    return normalized.split(separator: "-").first.map(String.init)
}

private func normalizedLocaleIdentifier(_ identifier: String) -> String {
    identifier
        .trimmingCharacters(in: .whitespacesAndNewlines)
        .replacingOccurrences(of: "_", with: "-")
        .lowercased()
}
