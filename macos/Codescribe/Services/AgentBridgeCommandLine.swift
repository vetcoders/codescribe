import Foundation

/// Parses the agent-bridge installation arguments of the app executable.
///
/// Client selection is an explicit act. Settings and Setup express it by
/// calling `install(selectedClients:)`; a headless host has no window, so the
/// same act is expressed on the command line. This type only reads arguments
/// and renders the result line — the app shell owns the installer call and the
/// exit codes, and no decision here touches the disk.
enum AgentBridgeCommandLine {
  static let installFlag = "--install-agent-bridge"
  static let clientsFlag = "--agent-clients"

  /// What the process was asked to do before any scene exists.
  enum Request: Equatable, Sendable {
    /// No installation argument: the app launches normally.
    case notRequested
    /// `--install-agent-bridge` alone: runtime plus the already selected clients.
    case runtime
    /// `--install-agent-bridge --agent-clients …`: replace the selection.
    case clients(Set<AgentBridgeClient>)
    /// The arguments do not name a runnable request; the message is one line.
    case usage(String)
  }

  /// Raw client tokens, in one stable order, for every diagnostic that lists them.
  static var clientNames: String {
    AgentBridgeClient.allCases.map(\.rawValue).sorted().joined(separator: ", ")
  }

  private static var usageLine: String {
    "usage: Codescribe \(installFlag) [\(clientsFlag) <\(clientNames)>]"
  }

  static func request(arguments: [String]) -> Request {
    let wantsInstall = arguments.contains(installFlag)
    switch selection(arguments: arguments) {
    case .absent:
      return wantsInstall ? .runtime : .notRequested
    case .malformed(let message):
      return .usage(message)
    case .present(let value):
      guard wantsInstall else {
        return .usage("\(clientsFlag) requires \(installFlag); \(usageLine)")
      }
      var selected: Set<AgentBridgeClient> = []
      for token in value.split(separator: ",", omittingEmptySubsequences: false) {
        let name = token.trimmingCharacters(in: .whitespaces)
        guard let client = AgentBridgeClient(rawValue: name) else {
          return .usage("\(clientsFlag): unknown client \"\(name)\"; valid names: \(clientNames)")
        }
        selected.insert(client)
      }
      return .clients(selected)
    }
  }

  /// One line naming what the explicit selection installed and where.
  static func summary(_ status: AgentBridgeInstallationStatus) -> String {
    let installed = zip(status.installedClients, status.installedPaths)
      .map { "\($0.displayName) → \($1)" }
      .joined(separator: ", ")
    guard !installed.isEmpty else {
      return "Agent bridge runtime installed; no client skill is selected."
    }
    return "Agent bridge runtime and client skills installed: \(installed)."
  }

  private enum Selection {
    case absent
    case present(String)
    case malformed(String)
  }

  /// The last occurrence wins, the way a repeated flag does elsewhere; an
  /// empty value is malformed rather than a silent teardown of the selection.
  private static func selection(arguments: [String]) -> Selection {
    var found: Selection = .absent
    var index = arguments.startIndex
    while index < arguments.endIndex {
      let argument = arguments[index]
      index += 1
      let value: String?
      if argument == clientsFlag {
        value = index < arguments.endIndex ? arguments[index] : nil
        if value != nil { index += 1 }
      } else if argument.hasPrefix(clientsFlag + "=") {
        value = String(argument.dropFirst(clientsFlag.count + 1))
      } else {
        continue
      }
      guard let value, !value.trimmingCharacters(in: .whitespaces).isEmpty else {
        return .malformed(
          "\(clientsFlag) requires a comma-separated client list: \(clientNames)")
      }
      found = .present(value)
    }
    return found
  }
}
