// codescribe-stt-bridge.swift
//
// Apple on-device STT bridge for Codescribe:
// - Reads one JSON request from stdin
// - Emits one JSON response to stdout
//
// Backend selection (per locale):
//   1. SpeechTranscriber (SpeechAnalyzer) when the locale is supported+installed
//   2. SFSpeechRecognizer on-device when ST lacks the locale but SF supports it
//   3. Honest error when neither can serve the locale
//
// Build example:
//   swiftc -O -o codescribe-stt-bridge core/stt/apple_stt/codescribe-stt-bridge.swift
//
// 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders (c)2024-2026 LibraxisAI

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
    case sfSpeechRecognizer = "sf_speech_recognizer"
}

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

private func readRequest() throws -> BridgeRequest {
    let input = FileHandle.standardInput.readDataToEndOfFile()
    guard !input.isEmpty else {
        throw BridgeError.invalidInput("empty stdin")
    }
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    do {
        return try decoder.decode(BridgeRequest.self, from: input)
    } catch {
        let text = String(data: input, encoding: .utf8) ?? "<non-utf8>"
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
        // Offline / final-pass: SFSpeechURLRecognitionRequest (file engine).
        // Known to under-generate / collapse on long Polish dictation — product
        // floor-guards against this path winning over live assembly.
        guard let audioPath = request.audioPath, !audioPath.isEmpty else {
            throw BridgeError.missingAudioPath
        }
        try await ensureSpeechAuthorizedForSfSpeech()
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
    case "transcribe_live":
        // Live / virtual-mic: SFSpeechAudioBufferRecognitionRequest.
        // Feed PCM buffers as if from a microphone (fixture WAV or hardware
        // mic path both land here). This is the Apple engine that product
        // live dictation must use — NOT the file URL path.
        guard let audioPath = request.audioPath, !audioPath.isEmpty else {
            throw BridgeError.missingAudioPath
        }
        try await ensureSpeechAuthorizedForSfSpeech()
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

// MARK: - Probe (ST → SFSpeech on-device)

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
        // ST supported but not installed (or install failed): fall through to SF.
        let sf = probeSfSpeech(locale: locale)
        if sf.localeSupported == true {
            return sf
        }
        // Neither ready: return the ST probe (honest not-installed / install error).
        return st
    }

    // Fallback: SFSpeechRecognizer on-device for locales ST does not serve (e.g. pl-PL).
    return probeSfSpeech(locale: locale)
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
        // ST in catalog but assets missing → SFSpeech on-device when available.
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

    return try await transcribeWithSfSpeech(audioPath: audioPath, locale: locale)
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
        request.append(buffer)
    }

    return try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<TranscriptionPayload, Error>) in
        let gate = SfSpeechSettleGate()
        let acc = SfSpeechPhraseAccumulator(timeOffset: timeOffset)
        let settle: (Result<TranscriptionPayload, Error>) -> Void = { outcome in
            guard gate.trySettle() else { return }
            player.removeTap(onBus: bus)
            player.stop()
            engine.stop()
            request.endAudio()
            switch outcome {
            case .success(let payload):
                continuation.resume(returning: payload)
            case .failure(let error):
                continuation.resume(throwing: error)
            }
        }
        let timeoutWork = DispatchWorkItem {
            gate.cancelTask()
            if let payload = acc.snapshotPayload(), !payload.text.isEmpty {
                settle(.success(payload))
            } else {
                settle(.failure(BridgeError.runtime(
                    "sf_speech_buffer: recognition_timeout after \(String(format: "%.1f", deadlineSeconds))s (locale=\(locale.identifier))"
                )))
            }
        }
        DispatchQueue.global(qos: .userInitiated).asyncAfter(
            deadline: .now() + deadlineSeconds,
            execute: timeoutWork
        )

        let task = retained.recognitionTask(with: request) { result, error in
            if gate.isSettled() { return }
            if let error {
                if let payload = acc.snapshotPayload(), !payload.text.isEmpty {
                    timeoutWork.cancel()
                    settle(.success(payload))
                    return
                }
                let ns = error as NSError
                // Cancel / no-speech after endAudio — treat as empty success when no text.
                if ns.domain == "kAFAssistantErrorDomain" || ns.code == 216 || ns.code == 1 {
                    timeoutWork.cancel()
                    settle(.success(
                        TranscriptionPayload(text: "", segments: [], backend: .sfSpeechRecognizer)
                    ))
                    return
                }
                timeoutWork.cancel()
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
            let onPlaybackDone: () -> Void = {
                // File finished through the engine — end recognition input.
                DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + 0.35) {
                    if gate.isSettled() { return }
                    request.endAudio()
                    let settleGrace = max(1.5, min(4.0, audioSeconds * 0.05 + 1.2))
                    DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + settleGrace) {
                        if gate.isSettled() { return }
                        timeoutWork.cancel()
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
            timeoutWork.cancel()
            settle(.failure(BridgeError.runtime(
                "sf_speech_buffer: engine start failed: \(error.localizedDescription)"
            )))
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

    fileprivate func storePartial(text: String, segments: [BridgeSegment]) {
        lock.lock()
        defer { lock.unlock() }
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        // Detect SFSpeech phrase restart without isFinal: new partial neither
        // extends nor is a prefix of the previous hypothesis → freeze prior.
        if !partialText.isEmpty && !t.isEmpty {
            let prev = partialText
            let extends = t.hasPrefix(prev) || prev.hasPrefix(t) || t.contains(prev) || prev.contains(t)
            let collapsed = t.count + 12 < prev.count
            if collapsed && !extends {
                finals.append(prev)
                finalSegments.append(contentsOf: partialSegments)
            }
        }
        partialText = t
        partialSegments = segments
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

private func transcribeWithSfSpeech(audioPath: String, locale: Locale) async throws -> TranscriptionPayload {
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

private func streamAudio(
    fromPath path: String,
    destinationFormat: AVAudioFormat,
    into builder: AsyncStream<AnalyzerInput>.Continuation
) throws {
    let url = URL(fileURLWithPath: path)
    let inputFile = try AVAudioFile(forReading: url)
    let sourceFormat = inputFile.processingFormat
    let bufferFrameCapacity: AVAudioFrameCount = 4096

    guard let converter = AVAudioConverter(from: sourceFormat, to: destinationFormat) else {
        throw BridgeError.runtime("unable to create AVAudioConverter")
    }

    while true {
        guard let sourceBuffer = AVAudioPCMBuffer(
            pcmFormat: sourceFormat,
            frameCapacity: bufferFrameCapacity
        ) else {
            throw BridgeError.runtime("unable to allocate source buffer")
        }
        try inputFile.read(into: sourceBuffer)
        if sourceBuffer.frameLength == 0 {
            break
        }

        guard let destBuffer = AVAudioPCMBuffer(
            pcmFormat: destinationFormat,
            frameCapacity: AVAudioFrameCount(Double(sourceBuffer.frameLength) * 2.0 + 64.0)
        ) else {
            throw BridgeError.runtime("unable to allocate destination buffer")
        }

        var convertError: NSError?
        var sourceConsumed = false
        let status = converter.convert(to: destBuffer, error: &convertError) { _, outStatus in
            if sourceConsumed {
                outStatus.pointee = .endOfStream
                return nil
            }
            sourceConsumed = true
            outStatus.pointee = .haveData
            return sourceBuffer
        }

        if let convertError {
            throw BridgeError.runtime("audio conversion failed: \(convertError)")
        }

        switch status {
        case .haveData, .inputRanDry:
            if destBuffer.frameLength > 0 {
                builder.yield(AnalyzerInput(buffer: destBuffer))
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
