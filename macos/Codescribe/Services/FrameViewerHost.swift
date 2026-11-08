import AppKit
import Foundation
import OSLog

/// The application whose Frame client shows a session at this moment.
///
/// A follower learns its pane's terminal host once, from the environment the
/// agent inherited when it started. The window that shows the session changes
/// whenever a client detaches, reconnects or visits another peer, so that
/// inherited process id outlives the window it named. A click reads the
/// viewer from one kernel snapshot instead: the session server's listening
/// socket, the sockets the server accepted on it, the client processes on the
/// other ends, and the registered application each client runs under.
enum FrameViewerHost: Equatable, Sendable {
  /// Exactly one application shows the session.
  case host(pid: Int32)
  /// No server answers the session, or no terminal client under an
  /// application is attached to it.
  case noViewer
  /// Two servers answer the session name, or two applications show it.
  case ambiguous(pids: [Int32])
  /// The process or socket snapshot could not be read; nothing is proven.
  case unavailable
}

enum FrameViewerHostResolver {
  /// One `ps -axo pid=,ppid=,tty=,comm=` row.
  struct ProcessRow: Equatable, Sendable {
    let pid: Int32
    let ppid: Int32
    /// Controlling terminal as `ps` prints it, `??` when there is none.
    let tty: String
    let executable: String
  }

  /// One Unix-domain socket row from `lsof -F pfdn`.
  struct SocketRow: Equatable, Sendable {
    let pid: Int32
    /// The socket's own protocol control block (the `d` field).
    let pcb: String
    /// A bound path, or `->0x…` naming the peer's control block.
    let name: String
  }

  static let logger = Logger(subsystem: "com.vetcoders.codescribe", category: "agent-panel-focus")
  private static let ancestorLimit = 32
  private static let commandTimeout: TimeInterval = 2

  /// Decides from one snapshot. Control block addresses are reused by the
  /// kernel, so a client counts only when its peer is a socket the session's
  /// server holds in this same snapshot under the session's listening path.
  static func resolve(
    session: String,
    processes: [ProcessRow],
    sockets: [SocketRow],
    isApplication: (Int32) -> Bool
  ) -> FrameViewerHost {
    guard isSessionName(session) else { return .noViewer }
    let listenerRows = sockets.filter { isListenerPath($0.name, session: session) }
    let servers = Set(listenerRows.map(\.pid))
    guard servers.count <= 1 else { return .ambiguous(pids: servers.sorted()) }
    guard let server = servers.first else { return .noViewer }
    let accepted = Set(listenerRows.map(\.pcb))
    let clients = Set(
      sockets
        .filter { row in
          row.pid != server && row.name.hasPrefix("->")
            && accepted.contains(String(row.name.dropFirst(2)))
        }
        .map(\.pid))
    let table = Dictionary(processes.map { ($0.pid, $0) }, uniquingKeysWith: { first, _ in first })
    // A frontend renders into a terminal. A connection without one (a
    // subscriber, a scripted action) shows no window to raise.
    let hosts = Set(
      clients.compactMap { client -> Int32? in
        guard let row = table[client], hasTerminal(row) else { return nil }
        return applicationAncestor(of: client, in: table, isApplication: isApplication)
      })
    if hosts.count > 1 { return .ambiguous(pids: hosts.sorted()) }
    guard let host = hosts.first else { return .noViewer }
    return .host(pid: host)
  }

  /// Reads `ps` and `lsof` off the main actor and resolves the session.
  static func resolveLive(session: String) async -> FrameViewerHost {
    await Task.detached(priority: .userInitiated) { () -> FrameViewerHost in
      guard let table = run("/bin/ps", ["-axo", "pid=,ppid=,tty=,comm="]) else {
        return .unavailable
      }
      let processes = parseProcessTable(table)
      guard !processes.isEmpty else { return .unavailable }
      let framePIDs = processes.filter {
        ($0.executable as NSString).lastPathComponent == "vc-frame"
      }.map(\.pid)
      guard !framePIDs.isEmpty else { return .noViewer }
      // lsof exits 1 when a listed process ended after `ps`; its rows for
      // the others still describe this moment.
      guard
        let output = run(
          "/usr/sbin/lsof",
          [
            "-nP", "-a", "-U", "-F", "pfdn", "-p",
            framePIDs.map(String.init).joined(separator: ","),
          ],
          partialStatus: 1)
      else { return .unavailable }
      return resolve(
        session: session, processes: processes, sockets: parseSocketFields(output),
        isApplication: { NSRunningApplication(processIdentifier: $0) != nil })
    }.value
  }

  static func parseProcessTable(_ text: String) -> [ProcessRow] {
    text.split(separator: "\n").compactMap { line in
      var rest = Substring(line)
      var fields: [Substring] = []
      while fields.count < 3 {
        rest = rest.drop(while: \.isWhitespace)
        guard let end = rest.firstIndex(where: \.isWhitespace) else { return nil }
        fields.append(rest[..<end])
        rest = rest[end...]
      }
      let executable = rest.trimmingCharacters(in: .whitespaces)
      guard let pid = Int32(fields[0]), let ppid = Int32(fields[1]), !executable.isEmpty else {
        return nil
      }
      return ProcessRow(pid: pid, ppid: ppid, tty: String(fields[2]), executable: executable)
    }
  }

  static func parseSocketFields(_ text: String) -> [SocketRow] {
    var rows: [SocketRow] = []
    var pid: Int32?
    var pcb: String?
    for line in text.split(separator: "\n") {
      guard let tag = line.first else { continue }
      let value = String(line.dropFirst())
      switch tag {
      case "p":
        pid = Int32(value)
        pcb = nil
      case "f":
        pcb = nil
      case "d":
        pcb = value
      case "n":
        if let pid, let pcb, !value.isEmpty {
          rows.append(SocketRow(pid: pid, pcb: pcb, name: value))
        }
      default:
        continue
      }
    }
    return rows
  }

  private static func isSessionName(_ session: String) -> Bool {
    !session.isEmpty && session.count <= 160 && !session.contains("/")
  }

  /// `<socket dir>/contract_version_<n>/<session>`, the path a server listens on.
  private static func isListenerPath(_ name: String, session: String) -> Bool {
    guard name.hasPrefix("/") else { return false }
    let components = name.split(separator: "/")
    guard components.count >= 2 else { return false }
    return components[components.count - 1] == session
      && components[components.count - 2].hasPrefix("contract_version_")
  }

  private static func hasTerminal(_ row: ProcessRow) -> Bool {
    !row.tty.isEmpty && row.tty != "??" && row.tty != "-"
  }

  private static func applicationAncestor(
    of pid: Int32,
    in table: [Int32: ProcessRow],
    isApplication: (Int32) -> Bool
  ) -> Int32? {
    var current = pid
    var visited: Set<Int32> = []
    for _ in 0..<ancestorLimit {
      guard current > 1, visited.insert(current).inserted, let row = table[current] else {
        return nil
      }
      if isApplication(current) { return current }
      current = row.ppid
    }
    return nil
  }

  private static func run(_ path: String, _ arguments: [String], partialStatus: Int32? = nil)
    -> String?
  {
    let process = Process()
    let output = Pipe()
    process.executableURL = URL(fileURLWithPath: path)
    process.arguments = arguments
    process.standardInput = FileHandle.nullDevice
    process.standardOutput = output
    process.standardError = FileHandle.nullDevice
    process.environment = ["LC_ALL": "C", "PATH": "/usr/bin:/bin:/usr/sbin:/sbin"]
    do {
      try process.run()
    } catch {
      logger.error("viewer snapshot could not start \(path, privacy: .public)")
      return nil
    }
    let watchdog = DispatchWorkItem { [process] in
      if process.isRunning { process.terminate() }
    }
    DispatchQueue.global(qos: .userInitiated).asyncAfter(
      deadline: .now() + commandTimeout, execute: watchdog)
    // Reading to EOF first keeps a long process table from filling the pipe
    // and deadlocking against waitUntilExit.
    let data = output.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    watchdog.cancel()
    let status = process.terminationStatus
    guard process.terminationReason == .exit,
      status == 0 || (status == partialStatus && !data.isEmpty)
    else {
      logger.error(
        "viewer snapshot \(path, privacy: .public) ended with status \(status, privacy: .public)")
      return nil
    }
    return String(decoding: data, as: UTF8.self)
  }
}
