import AppKit
import Foundation
import OSLog

/// Loopback Voice Lab process owned by a CS-bake Codescribe.
///
/// Production DMGs never spawn. The XCTest host never spawns. A live
/// `:8765` is left alone. Missing `~/.codescribe/voice-lab/server.py`
/// is a no-op (install-voice-lab never ran).
enum VoiceLabRuntime {
  static let consoleURL = URL(string: "http://127.0.0.1:8765/lab")!

  private static let logger = Logger(
    subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
    category: "voice-lab"
  )
  private static let lock = NSLock()
  private static var child: Process?

  static var labRoot: URL {
    FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent(".codescribe/voice-lab", isDirectory: true)
  }

  static var serverScript: URL {
    labRoot.appendingPathComponent("server.py")
  }

  static func shouldSpawn(
    surfaceEnabled: Bool,
    runningTests: Bool,
    alreadyListening: Bool,
    serverExists: Bool
  ) -> Bool {
    surfaceEnabled && !runningTests && !alreadyListening && serverExists
  }

  /// Fire-and-forget: bring `:8765` up if this is a CS bake and Lab is down.
  static func ensureListening(
    surfaceEnabled: Bool? = nil,
    runningTests: Bool = QualityCaptureHost.isRunningTests
  ) {
    let cs = surfaceEnabled ?? DeveloperSurface.isEnabled()
    if cs, !runningTests { writeLoopbackPointer() }
    let server = serverScript
    lock.lock()
    let ownedRunning = child?.isRunning == true
    lock.unlock()
    let spawn = shouldSpawn(
      surfaceEnabled: surfaceEnabled ?? DeveloperSurface.isEnabled(),
      runningTests: runningTests,
      alreadyListening: ownedRunning || isListening(),
      serverExists: FileManager.default.isReadableFile(atPath: server.path)
    )
    guard spawn else { return }
    startChild(server: server)
  }

  /// CS tray/settings: ensure the process, then open the console.
  static func openConsole() {
    Task.detached(priority: .utility) {
      ensureListening()
      for _ in 0..<25 where !isListening() {
        try? await Task.sleep(nanoseconds: 80_000_000)
      }
      await MainActor.run {
        NSWorkspace.shared.open(consoleURL)
      }
    }
  }

  static func stopOwnedProcess() {
    lock.lock()
    let process = child
    child = nil
    lock.unlock()
    process?.terminate()
  }

  static func isListening(timeout: TimeInterval = 0.2) -> Bool {
    var request = URLRequest(url: consoleURL)
    request.httpMethod = "GET"
    request.timeoutInterval = timeout
    let semaphore = DispatchSemaphore(value: 0)
    var ok = false
    URLSession.shared.dataTask(with: request) { _, response, _ in
      ok = (response as? HTTPURLResponse)?.statusCode == 200
      semaphore.signal()
    }.resume()
    _ = semaphore.wait(timeout: .now() + timeout + 0.05)
    return ok
  }

  static func writeLoopbackPointer() {
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
      try FileManager.default.createDirectory(at: labRoot, withIntermediateDirectories: true)
      try html.write(
        to: labRoot.appendingPathComponent("loopback.html"),
        atomically: true,
        encoding: .utf8
      )
    } catch {
      logger.error(
        "loopback.html write failed: \(error.localizedDescription, privacy: .public)"
      )
    }
  }

  private static func startChild(server: URL) {
    lock.lock()
    defer { lock.unlock() }
    if let child, child.isRunning { return }
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/python3")
    process.arguments = [server.path]
    process.currentDirectoryURL = labRoot
    var environment = ProcessInfo.processInfo.environment
    environment["VOICE_LAB_REMOTE_HOST"] = "off"
    process.environment = environment
    process.standardInput = FileHandle.nullDevice
    process.standardOutput = FileHandle.nullDevice
    process.standardError = FileHandle.nullDevice
    process.terminationHandler = { _ in
      lock.lock()
      child = nil
      lock.unlock()
    }
    do {
      try process.run()
      child = process
      logger.info("voice-lab spawned pid=\(process.processIdentifier, privacy: .public)")
    } catch {
      logger.error(
        "voice-lab spawn failed: \(error.localizedDescription, privacy: .public)"
      )
    }
  }
}
