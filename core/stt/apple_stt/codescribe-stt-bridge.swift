// codescribe-stt-bridge.swift
//
// SpeechAnalyzer bridge for Codescribe:
// - Reads one JSON request from stdin
// - Emits one JSON response to stdout
//
// Build example:
//   swiftc -O -o codescribe-stt-bridge core/stt/apple_stt/codescribe-stt-bridge.swift
//
// Created by Vetcoders (c)2026

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
    let error: String?
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
            error: String(describing: error)
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
    case "transcribe":
        guard let audioPath = request.audioPath, !audioPath.isEmpty else {
            throw BridgeError.missingAudioPath
        }
        let transcription = try await transcribe(audioPath: audioPath, locale: locale)
        return BridgeResponse(
            ok: true,
            status: "ok",
            text: transcription.text,
            segments: transcription.segments,
            localeSupported: nil,
            localeInstalled: nil,
            error: nil
        )
    default:
        throw BridgeError.unsupportedCommand(request.command)
    }
}

private func probe(locale: Locale, allowDownload: Bool) async throws -> BridgeResponse {
    let supported = await SpeechTranscriber.supportedLocales
    var installed = await SpeechTranscriber.installedLocales

    guard let effectiveLocale = bestAvailableLocale(requested: locale, available: supported) else {
        return BridgeResponse(
            ok: true,
            status: "ok",
            text: "",
            segments: [],
            localeSupported: false,
            localeInstalled: false,
            error: nil
        )
    }
    let transcriber = SpeechTranscriber(locale: effectiveLocale, preset: .transcription)
    let isSupported = true
    var isInstalled = containsLocale(installed, locale: effectiveLocale)

    if isSupported && !isInstalled && allowDownload {
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
                localeSupported: isSupported,
                localeInstalled: isInstalled,
                error: "asset_install_failed: \(error)"
            )
        }
    }

    return BridgeResponse(
        ok: true,
        status: "ok",
        text: "",
        segments: [],
        localeSupported: isSupported,
        localeInstalled: isInstalled,
        error: nil
    )
}

private struct TranscriptionPayload {
    let text: String
    let segments: [BridgeSegment]
}

private func transcribe(audioPath: String, locale: Locale) async throws -> TranscriptionPayload {
    let supportedLocales = await SpeechTranscriber.supportedLocales
    guard let effectiveLocale = bestAvailableLocale(requested: locale, available: supportedLocales) else {
        throw BridgeError.runtime("locale \(locale.identifier) is not supported")
    }
    let transcriber = SpeechTranscriber(locale: effectiveLocale, preset: .transcription)
    let analyzer = SpeechAnalyzer(modules: [transcriber])
    guard let analyzerFormat = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [transcriber]) else {
        throw BridgeError.runtime("no compatible analyzer audio format available")
    }
    let (inputSequence, inputBuilder) = AsyncStream<AnalyzerInput>.makeStream()

    var volatileText = ""
    var finalTextParts: [String] = []

    let collector = Task {
        for try await result in transcriber.results {
            let text = String(result.text.characters)
            if result.isFinal {
                finalTextParts.append(text)
            } else {
                volatileText = text
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
    return TranscriptionPayload(text: text, segments: [])
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
