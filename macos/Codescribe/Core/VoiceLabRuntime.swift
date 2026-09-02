import AppKit
import Foundation
import OSLog

/// Loopback Voice Lab process owned by a CS-bake Codescribe.
///
/// Production DMGs never spawn. The XCTest host never spawns. A live
/// `:8765` is left alone. Missing `~/.codescribe/voice-lab/server.py`
/// is a no-op (install-voice-lab never ran).
actor VoiceLabRuntime {
  static let shared = VoiceLabRuntime()
  nonisolated static let consoleURL = URL(string: "http://127.0.0.1:8765/lab")!

  private static let logger = Logger(
    subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
    category: "voice-lab"
  )
  private var child: Process?

  nonisolated static var labRoot: URL {
    FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent(".codescribe/voice-lab", isDirectory: true)
  }

  nonisolated static var serverScript: URL {
    labRoot.appendingPathComponent("server.py")
  }

  nonisolated static func shouldSpawn(
    surfaceEnabled: Bool,
    runningTests: Bool,
    alreadyListening: Bool,
    serverExists: Bool
  ) -> Bool {
    surfaceEnabled && !runningTests && !alreadyListening && serverExists
  }

  /// Bring `:8765` up if this is a CS bake and Lab is down. The actor owns the
  /// child process; callers decide whether their UI task is fire-and-forget.
  func ensureListening(
    surfaceEnabled: Bool? = nil,
    runningTests: Bool = QualityCaptureHost.isRunningTests
  ) async {
    let cs = surfaceEnabled ?? DeveloperSurface.isEnabled()
    if cs, !runningTests { Self.writeLoopbackPointer() }
    let server = Self.serverScript
    let ownedRunning = child?.isRunning == true
    let alreadyListening = ownedRunning ? true : await Self.isListening()
    let spawn = Self.shouldSpawn(
      surfaceEnabled: surfaceEnabled ?? DeveloperSurface.isEnabled(),
      runningTests: runningTests,
      alreadyListening: alreadyListening,
      serverExists: FileManager.default.isReadableFile(atPath: server.path)
    )
    guard spawn else { return }
    startChild(server: server)
  }

  /// CS tray/settings: ensure the process, then open the console.
  func openConsole() async {
    await ensureListening()
    for _ in 0..<25 where !(await Self.isListening()) {
      try? await Task.sleep(nanoseconds: 80_000_000)
    }
    let opened = await MainActor.run {
      NSWorkspace.shared.open(Self.consoleURL)
    }
    if !opened {
      Self.logger.error("voice-lab console could not be opened")
    }
  }

  func stopOwnedProcess() {
    let process = child
    child = nil
    process?.terminate()
  }

  nonisolated static func isListening(timeout: TimeInterval = 0.2) async -> Bool {
    var request = URLRequest(url: Self.consoleURL)
    request.httpMethod = "GET"
    request.timeoutInterval = timeout
    do {
      let (_, response) = try await URLSession.shared.data(for: request)
      return (response as? HTTPURLResponse)?.statusCode == 200
    } catch {
      return false
    }
  }

  nonisolated static func writeLoopbackPointer() {
    let html = """
      <!doctype html>
      <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>codescribe loopback</title>
      </head>
      <body>
        <h1>codescribe loopback</h1>
        <p>Dev install pointers. Do not rewrite these URLs.</p>
        <ul>
          <li><a href="http://127.0.0.1:8765/lab">Voice Lab</a> — <code>http://127.0.0.1:8765/lab</code></li>
          <li>STT file HTTP — <code>http://127.0.0.1:8444/v1/audio/transcriptions</code></li>
          <li>STT live WebSocket — <code>ws://127.0.0.1:8446</code></li>
        </ul>
      </body>
      </html>
      """
    do {
      try FileManager.default.createDirectory(at: Self.labRoot, withIntermediateDirectories: true)
      try html.write(
        to: Self.labRoot.appendingPathComponent("loopback.html"),
        atomically: true,
        encoding: .utf8
      )
    } catch {
      Self.logger.error(
        "loopback.html write failed: \(error.localizedDescription, privacy: .public)"
      )
    }
  }

  private func startChild(server: URL) {
    if let child, child.isRunning { return }
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/python3")
    process.arguments = [server.path]
    process.currentDirectoryURL = Self.labRoot
    var environment = ProcessInfo.processInfo.environment
    environment["VOICE_LAB_REMOTE_HOST"] = "off"
    process.environment = environment
    process.standardInput = FileHandle.nullDevice
    process.standardOutput = FileHandle.nullDevice
    process.standardError = FileHandle.nullDevice
    do {
      try process.run()
      child = process
      Self.logger.info("voice-lab spawned pid=\(process.processIdentifier, privacy: .public)")
    } catch {
      Self.logger.error(
        "voice-lab spawn failed: \(error.localizedDescription, privacy: .public)"
      )
    }
  }
}
