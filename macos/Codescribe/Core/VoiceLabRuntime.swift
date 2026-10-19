import AppKit
import Foundation
import OSLog

/// Loopback Voice Lab process owned by a CS-bake Codescribe.
///
/// Production DMGs never spawn. The XCTest host never spawns. A live Lab is
/// left alone. `:8765` is preferred, but another loopback tool (screenscribe)
/// often holds it: the runtime then takes the next free candidate port and
/// records it in `~/.codescribe/voice-lab/port`, which the Rust level feed
/// reads. Missing `~/.codescribe/voice-lab/server.py` is a no-op
/// (install-voice-lab never ran).
actor VoiceLabRuntime {
  static let shared = VoiceLabRuntime()
  nonisolated static let preferredPort = 8765
  nonisolated static let candidatePorts = Array(8765...8774)

  /// What answers on one loopback port.
  enum PortState: Equatable, Sendable {
    case voiceLab
    case free
    case foreign
  }

  private static let logger = Logger(
    subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
    category: "voice-lab"
  )
  private var child: Process?
  private var port: Int?

  nonisolated static var labRoot: URL {
    FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent(".codescribe/voice-lab", isDirectory: true)
  }

  nonisolated static var serverScript: URL {
    labRoot.appendingPathComponent("server.py")
  }

  nonisolated static var portFile: URL {
    labRoot.appendingPathComponent("port")
  }

  nonisolated static func consoleURL(port: Int) -> URL {
    URL(string: "http://127.0.0.1:\(port)/lab")!
  }

  nonisolated static func shouldSpawn(
    surfaceEnabled: Bool,
    runningTests: Bool,
    alreadyListening: Bool,
    serverExists: Bool
  ) -> Bool {
    surfaceEnabled && !runningTests && !alreadyListening && serverExists
  }

  /// The remembered port first, then the candidates in order. A port that
  /// already serves Voice Lab is reused; the first free one is taken for a
  /// spawn; a port held by anything else is skipped, never contested.
  nonisolated static func choosePort(
    remembered: Int?,
    candidates: [Int],
    probe: (Int) async -> PortState
  ) async -> (port: Int, running: Bool)? {
    var order = candidates
    if let remembered {
      order.removeAll { $0 == remembered }
      order.insert(remembered, at: 0)
    }
    for port in order {
      switch await probe(port) {
      case .voiceLab: return (port, true)
      case .free: return (port, false)
      case .foreign: continue
      }
    }
    return nil
  }

  /// Bring the Lab up if this is a CS bake and it is down. The actor owns the
  /// child process; callers decide whether their UI task is fire-and-forget.
  /// Returns the port the Lab answers (or is starting) on.
  @discardableResult
  func ensureListening(
    surfaceEnabled: Bool? = nil,
    runningTests: Bool = QualityCaptureHost.isRunningTests
  ) async -> Int? {
    if child?.isRunning == true, let port { return port }
    let cs = surfaceEnabled ?? DeveloperSurface.isEnabled()
    guard cs, !runningTests else { return nil }
    guard
      let choice = await Self.choosePort(
        remembered: Self.rememberedPort(),
        candidates: Self.candidatePorts,
        probe: { await Self.probe($0) }
      )
    else {
      Self.logger.error("voice-lab: every candidate port is held by another process")
      return nil
    }
    let server = Self.serverScript
    let spawn = Self.shouldSpawn(
      surfaceEnabled: cs,
      runningTests: runningTests,
      alreadyListening: choice.running,
      serverExists: FileManager.default.isReadableFile(atPath: server.path)
    )
    if choice.running || spawn {
      port = choice.port
      Self.remember(port: choice.port)
    }
    Self.writeLoopbackPointer(port: choice.port)
    if spawn { startChild(server: server, port: choice.port) }
    return choice.port
  }

  /// CS tray/settings: ensure the process, then open the console.
  func openConsole() async {
    let port = await ensureListening() ?? Self.preferredPort
    for _ in 0..<25 where await Self.probe(port) != .voiceLab {
      try? await Task.sleep(nanoseconds: 80_000_000)
    }
    let url = Self.consoleURL(port: port)
    let opened = await MainActor.run {
      NSWorkspace.shared.open(url)
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

  /// Identify the Lab by its own health answer, not by any HTTP 200:
  /// screenscribe on the same port must read as foreign.
  nonisolated static func probe(_ port: Int, timeout: TimeInterval = 0.2) async -> PortState {
    var request = URLRequest(url: URL(string: "http://127.0.0.1:\(port)/healthz")!)
    request.httpMethod = "GET"
    request.timeoutInterval = timeout
    do {
      let (data, response) = try await URLSession.shared.data(for: request)
      let healthy = (response as? HTTPURLResponse)?.statusCode == 200
      let body = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
      return healthy && body?["service"] as? String == "voice-lab" ? .voiceLab : .foreign
    } catch {
      return portIsBindable(port) ? .free : .foreign
    }
  }

  /// No `SO_REUSEADDR`: on BSD it would let 127.0.0.1 bind over a wildcard
  /// listener and the Lab would steal another tool's loopback traffic.
  nonisolated static func portIsBindable(_ port: Int) -> Bool {
    let fd = socket(AF_INET, SOCK_STREAM, 0)
    guard fd >= 0 else { return false }
    defer { close(fd) }
    var address = sockaddr_in()
    address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
    address.sin_family = sa_family_t(AF_INET)
    address.sin_port = in_port_t(UInt16(port).bigEndian)
    address.sin_addr.s_addr = inet_addr("127.0.0.1")
    return withUnsafePointer(to: &address) {
      $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
        bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size)) == 0
      }
    }
  }

  nonisolated static func rememberedPort() -> Int? {
    guard let raw = try? String(contentsOf: portFile, encoding: .utf8),
      let port = Int(raw.trimmingCharacters(in: .whitespacesAndNewlines)),
      (1024...65535).contains(port)
    else { return nil }
    return port
  }

  nonisolated static func remember(port: Int) {
    do {
      try FileManager.default.createDirectory(at: labRoot, withIntermediateDirectories: true)
      try "\(port)\n".write(to: portFile, atomically: true, encoding: .utf8)
    } catch {
      logger.error("voice-lab port write failed: \(error.localizedDescription, privacy: .public)")
    }
  }

  nonisolated static func writeLoopbackPointer(port: Int) {
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
          <li><a href="http://127.0.0.1:\(port)/lab">Voice Lab</a> — <code>http://127.0.0.1:\(port)/lab</code></li>
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

  private func startChild(server: URL, port: Int) {
    if let child, child.isRunning { return }
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/python3")
    process.arguments = [server.path, "--port", String(port)]
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
