import XCTest

@testable import Codescribe

/// The viewer of a Frame session is read from one kernel snapshot.
///
/// The fixtures copy two snapshots recorded on 2026-11-08 on the machine where
/// the founder reported that a click switched the agent's tab but never raised
/// the terminal: one at 06:11Z, when the only frontend showed
/// `vc-host-48720aa5`, and one at 06:5xZ, after the same frontend visited
/// `vc-host`. The follower of the agent in `vc-host` still named host 29313,
/// which had exited days before.
final class FrameViewerHostResolverTests: XCTestCase {
  private typealias Row = FrameViewerHostResolver.ProcessRow
  private typealias Socket = FrameViewerHostResolver.SocketRow

  private static let socketDir = "/tmp/vc-frame-501/contract_version_5"
  private static let vcTerminal = "/Applications/Vibecrafted.app/Contents/MacOS/vc-terminal"
  private static let frameBinary =
    "/Users/polyversai/.local/share/vibecrafted/releases/4.5.1+g47b12911/libexec/vc-frame"

  private let processes: [Row] = [
    Row(pid: 3531, ppid: 1, tty: "??", executable: "/usr/libexec/launcher"),
    Row(pid: 65097, ppid: 3531, tty: "??", executable: vcTerminal),
    Row(pid: 65224, ppid: 65097, tty: "ttys066", executable: frameBinary),
    Row(pid: 29612, ppid: 1, tty: "??", executable: frameBinary),
    Row(pid: 65364, ppid: 1, tty: "??", executable: frameBinary),
  ]
  private let applications: Set<Int32> = [65097]

  /// 06:5xZ: the frontend is attached to `vc-host`. Its socket pair 38u/39u
  /// reuses the control block 65364 had accepted at 06:11Z.
  private var visitedOldSession: [Socket] {
    [
      Socket(pid: 29612, pcb: "0x7a8257f6702f64c2", name: "\(Self.socketDir)/vc-host"),
      Socket(pid: 29612, pcb: "0xca025c4abd83655f", name: "\(Self.socketDir)/vc-host"),
      Socket(pid: 29612, pcb: "0xca025c4abd83655f", name: "\(Self.socketDir)/vc-host"),
      Socket(pid: 65364, pcb: "0xb84f934a39ea050b", name: "\(Self.socketDir)/vc-host-48720aa5"),
      Socket(pid: 65224, pcb: "0x554dd7109a85f9c5", name: "->0xa12d415a6e439769"),
      Socket(pid: 65224, pcb: "0xa12d415a6e439769", name: "->0x554dd7109a85f9c5"),
      Socket(pid: 65224, pcb: "0xb34494c62a93d8af", name: "->0xca025c4abd83655f"),
      Socket(pid: 65224, pcb: "0xb34494c62a93d8af", name: "->0xca025c4abd83655f"),
    ]
  }

  /// 06:11Z: the same frontend is attached to `vc-host-48720aa5`.
  private var showingNewSession: [Socket] {
    [
      Socket(pid: 29612, pcb: "0x7a8257f6702f64c2", name: "\(Self.socketDir)/vc-host"),
      Socket(pid: 65364, pcb: "0xb84f934a39ea050b", name: "\(Self.socketDir)/vc-host-48720aa5"),
      Socket(pid: 65364, pcb: "0xa12d415a6e439769", name: "\(Self.socketDir)/vc-host-48720aa5"),
      Socket(pid: 65224, pcb: "0x554dd7109a85f9c5", name: "->0xa12d415a6e439769"),
    ]
  }

  private func resolve(
    _ session: String,
    processes: [Row]? = nil,
    sockets: [Socket],
    applications: Set<Int32>? = nil
  ) -> FrameViewerHost {
    let registered = applications ?? self.applications
    return FrameViewerHostResolver.resolve(
      session: session, processes: processes ?? self.processes, sockets: sockets,
      isApplication: { registered.contains($0) })
  }

  func testTheFrontendFollowsTheSessionItIsAttachedToNow() {
    XCTAssertEqual(resolve("vc-host", sockets: visitedOldSession), .host(pid: 65097))
    XCTAssertEqual(resolve("vc-host-48720aa5", sockets: visitedOldSession), .noViewer)

    XCTAssertEqual(resolve("vc-host-48720aa5", sockets: showingNewSession), .host(pid: 65097))
    XCTAssertEqual(resolve("vc-host", sockets: showingNewSession), .noViewer)
  }

  func testAReusedControlBlockIsNotAViewer() {
    // 0xa12d… belonged to 65364 at 06:11Z. At 06:5xZ it is one end of the
    // frontend's own socket pair; only this snapshot's server sockets count.
    var sockets = visitedOldSession
    sockets.removeAll { $0.pid == 65224 && $0.name == "->0xca025c4abd83655f" }
    XCTAssertEqual(resolve("vc-host-48720aa5", sockets: sockets), .noViewer)
    XCTAssertEqual(resolve("vc-host", sockets: sockets), .noViewer)
  }

  func testDuplicateDescriptorsAndTwoClientsInOneApplicationAreOneHost() {
    let processes =
      self.processes + [Row(pid: 70001, ppid: 65097, tty: "ttys070", executable: Self.frameBinary)]
    let sockets =
      visitedOldSession + [
        Socket(pid: 29612, pcb: "0x1111", name: "\(Self.socketDir)/vc-host"),
        Socket(pid: 70001, pcb: "0x2222", name: "->0x1111"),
      ]
    XCTAssertEqual(resolve("vc-host", processes: processes, sockets: sockets), .host(pid: 65097))
  }

  func testTwoApplicationsShowingTheSessionAreAmbiguous() {
    let processes =
      self.processes + iTermChain(client: 72000, tty: "ttys080")
    let sockets =
      visitedOldSession + [
        Socket(pid: 29612, pcb: "0x3333", name: "\(Self.socketDir)/vc-host"),
        Socket(pid: 72000, pcb: "0x4444", name: "->0x3333"),
      ]
    XCTAssertEqual(
      resolve("vc-host", processes: processes, sockets: sockets, applications: [65097, 71519]),
      .ambiguous(pids: [65097, 71519]))
  }

  func testAFrontendInsideITermResolvesToITermThroughItsHelpers() {
    let processes = self.processes + iTermChain(client: 72000, tty: "ttys080")
    let sockets: [Socket] = [
      Socket(pid: 29612, pcb: "0x3333", name: "\(Self.socketDir)/vc-host"),
      Socket(pid: 72000, pcb: "0x4444", name: "->0x3333"),
    ]
    XCTAssertEqual(
      resolve("vc-host", processes: processes, sockets: sockets, applications: [65097, 71519]),
      .host(pid: 71519))
  }

  func testTheSameSessionNameInTwoSocketDirectoriesIsAmbiguous() {
    let processes =
      self.processes + [Row(pid: 80000, ppid: 1, tty: "??", executable: Self.frameBinary)]
    let sockets =
      visitedOldSession + [
        Socket(pid: 80000, pcb: "0x5555", name: "/tmp/vc-frame-other/contract_version_5/vc-host")
      ]
    XCTAssertEqual(
      resolve("vc-host", processes: processes, sockets: sockets),
      .ambiguous(pids: [29612, 80000]))
  }

  func testConnectionsWithoutATerminalOrAnApplicationShowNothing() {
    // A subscriber started by an application has no terminal; an action run
    // from an agent pane has a terminal but descends from the Frame server.
    let processes =
      self.processes + [
        Row(pid: 81000, ppid: 65097, tty: "??", executable: Self.frameBinary),
        Row(pid: 81100, ppid: 29612, tty: "ttys005", executable: "/bin/zsh"),
        Row(pid: 81101, ppid: 81100, tty: "ttys005", executable: Self.frameBinary),
      ]
    let sockets: [Socket] = [
      Socket(pid: 29612, pcb: "0x6666", name: "\(Self.socketDir)/vc-host"),
      Socket(pid: 81000, pcb: "0x7777", name: "->0x6666"),
      Socket(pid: 81101, pcb: "0x8888", name: "->0x6666"),
    ]
    XCTAssertEqual(resolve("vc-host", processes: processes, sockets: sockets), .noViewer)
  }

  func testACycleOrAMissingParentEndsWithoutAHost() {
    let processes: [Row] = [
      Row(pid: 29612, ppid: 1, tty: "??", executable: Self.frameBinary),
      Row(pid: 90001, ppid: 90002, tty: "ttys001", executable: Self.frameBinary),
      Row(pid: 90002, ppid: 90001, tty: "ttys001", executable: "/bin/zsh"),
      Row(pid: 90003, ppid: 99999, tty: "ttys002", executable: Self.frameBinary),
    ]
    let sockets: [Socket] = [
      Socket(pid: 29612, pcb: "0x9999", name: "\(Self.socketDir)/vc-host"),
      Socket(pid: 90001, pcb: "0xaaaa", name: "->0x9999"),
      Socket(pid: 90003, pcb: "0xbbbb", name: "->0x9999"),
    ]
    XCTAssertEqual(resolve("vc-host", processes: processes, sockets: sockets), .noViewer)
  }

  func testTheListeningPathMustNameTheWholeSessionUnderAContractDirectory() {
    XCTAssertEqual(resolve("vc-host-4872", sockets: showingNewSession), .noViewer)
    let bare: [Socket] = [
      Socket(pid: 29612, pcb: "0xcccc", name: "/tmp/vc-host"),
      Socket(pid: 65224, pcb: "0xdddd", name: "->0xcccc"),
    ]
    XCTAssertEqual(resolve("vc-host", sockets: bare), .noViewer)
    XCTAssertEqual(resolve("contract_version_5/vc-host", sockets: visitedOldSession), .noViewer)
    XCTAssertEqual(resolve("", sockets: visitedOldSession), .noViewer)
  }

  func testProcessTableKeepsExecutablePathsWithSpaces() {
    let table = """
        3531     1 ??       /usr/libexec/launcher
      71830 71519 ??       /Users/polyversai/Library/Application Support/iTerm2/iTermServer-3.7.7beta1
      65224 65097 ttys066  \(Self.frameBinary)
      not a row
      """
    XCTAssertEqual(
      FrameViewerHostResolver.parseProcessTable(table),
      [
        Row(pid: 3531, ppid: 1, tty: "??", executable: "/usr/libexec/launcher"),
        Row(
          pid: 71830, ppid: 71519, tty: "??",
          executable: "/Users/polyversai/Library/Application Support/iTerm2/iTermServer-3.7.7beta1"),
        Row(pid: 65224, ppid: 65097, tty: "ttys066", executable: Self.frameBinary),
      ])
  }

  func testSocketFieldsKeepEachDescriptorWithItsOwnProcess() {
    let output = """
      p29612
      f5
      d0x7a8257f6702f64c2
      n\(Self.socketDir)/vc-host
      f30
      d0xca025c4abd83655f
      n\(Self.socketDir)/vc-host
      p65224
      f7
      n->0xffff
      f52
      d0xb34494c62a93d8af
      n->0xca025c4abd83655f
      """
    XCTAssertEqual(
      FrameViewerHostResolver.parseSocketFields(output),
      [
        Socket(pid: 29612, pcb: "0x7a8257f6702f64c2", name: "\(Self.socketDir)/vc-host"),
        Socket(pid: 29612, pcb: "0xca025c4abd83655f", name: "\(Self.socketDir)/vc-host"),
        Socket(pid: 65224, pcb: "0xb34494c62a93d8af", name: "->0xca025c4abd83655f"),
      ])
  }

  /// iTerm2 → iTermServer → login → zsh → Frame client.
  private func iTermChain(client: Int32, tty: String) -> [Row] {
    [
      Row(
        pid: 71519, ppid: 1, tty: "??", executable: "/Applications/iTerm.app/Contents/MacOS/iTerm2"),
      Row(
        pid: 71830, ppid: 71519, tty: "??",
        executable: "/Users/polyversai/Library/Application Support/iTerm2/iTermServer-3.7.7beta1"),
      Row(pid: client - 2, ppid: 71830, tty: tty, executable: "/usr/bin/login"),
      Row(pid: client - 1, ppid: client - 2, tty: tty, executable: "/bin/zsh"),
      Row(pid: client, ppid: client - 1, tty: tty, executable: Self.frameBinary),
    ]
  }
}
