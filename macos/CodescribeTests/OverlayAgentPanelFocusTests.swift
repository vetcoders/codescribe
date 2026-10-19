import AppKit
import Foundation
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayAgentPanelFocusTests: XCTestCase {
  private static let olderSupportedHelp = """
    vc-frame-action-focus-pane-id
    Focus a pane by its id (e.g. terminal_1 or plugin_2), switching to its tab if needed

    USAGE:
        vc-frame action focus-pane-id [OPTIONS] <PANE_ID> [VERSION: 4.4.0+g959bf184]
    """

  private static let currentSupportedHelp = """
    vc-frame-action-focus-pane-id
    Focus a pane by its id (e.g. terminal_1 or plugin_2), switching to its tab if needed

    USAGE:
        vc-frame action focus-pane-id [OPTIONS] <PANE_ID> [VERSION: 4.4.0+ga3f8214c]
    """

  private static let unsupportedStderr = """
    error: Found argument 'focus-pane-id' which wasn't expected, or isn't valid in this context
    """

  private func makeOwner(channel: String = "5", name: String = "stanislaw")
    -> OverlayConversationOwner
  {
    let leaseID = AgentPlaybackIdentity.leaseIdentifier(
      provider: "claude-code", session: "session-\(channel)")
    return OverlayConversationOwner(row: [
      "provider": "claude-code",
      "provider_session_id": "session-\(channel)",
      "channel": channel,
      "name": name,
      "lease_id": leaseID,
    ])!
  }

  private func writeExecutable(at directory: URL, name: String = "vc-frame", body: String) throws
    -> URL
  {
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    let fileURL = directory.appendingPathComponent(name)
    let script = "#!/bin/sh\n" + body
    try script.write(to: fileURL, atomically: true, encoding: .utf8)
    try FileManager.default.setAttributes(
      [.posixPermissions: 0o755],
      ofItemAtPath: fileURL.path)
    return fileURL
  }

  private func renderedRowSize(
    in view: OverlayChannelStatusView,
    for channel: OverlayChannelDelivery
  ) -> NSSize {
    let host = NSHostingView(rootView: view.channelRow(channel).fixedSize())
    let window = NSWindow(
      contentRect: NSRect(x: -10000, y: -10000, width: 440, height: 120),
      styleMask: [.titled],
      backing: .buffered,
      defer: false)
    window.contentView = host
    window.orderFrontRegardless()
    defer { window.orderOut(nil) }
    host.layoutSubtreeIfNeeded()
    window.displayIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    host.layoutSubtreeIfNeeded()
    return host.fittingSize
  }

  private func armRoster(
    on state: OverlayState,
    owner: OverlayConversationOwner,
    panelFocusAvailable: Bool = true,
    frameDetected: Bool? = nil
  ) {
    state.applyChannelRoster([
      CsChannelRosterState(
        channel: "0", audience: "All", provider: nil, providerSessionId: nil, open: false,
        loud: false, autosealDeadlineUnixMs: nil, followerAlive: nil, unreadSinceUnixMs: nil),
      CsChannelRosterState(
        channel: owner.channel, audience: owner.name, provider: owner.provider,
        providerSessionId: owner.providerSessionID, open: false, loud: false,
        autosealDeadlineUnixMs: nil, followerAlive: true, frameDetected: frameDetected,
        unreadSinceUnixMs: nil),
    ])
    if panelFocusAvailable {
      state.setAgentPanelFocusAvailable(true)
    }
  }

  func testFocusAgentPanelImmediatelyEntersRequestingStateBeforeAsyncCommand() throws {
    let state = OverlayState.previewFormatted()
    let owner = makeOwner()
    armRoster(on: state, owner: owner)

    var issuedRequests: [OverlayAgentPanelFocusRequest] = []
    state.requestAgentPanelFocusCommand = { request in
      issuedRequests.append(request)
      try await Task.sleep(for: .seconds(10))
    }

    state.focusAgentPanel(owner)

    // Synchronous <1 frame check before the Task body even runs:
    let focusState = try XCTUnwrap(state.agentPanelFocusStates[owner.channel])
    XCTAssertEqual(focusState.phase, .requesting)
    XCTAssertTrue(focusState.isRequesting)
    XCTAssertEqual(focusState.statusText, String(localized: "Opening panel in Frame…"))
    XCTAssertEqual(focusState.owner, owner)
    XCTAssertEqual(focusState.requestID.count, 24)

    var statusView = OverlayChannelStatusView(
      channels: state.visibleChannelRows.filter { $0.channel != "0" },
      unavailable: false, palette: .dark, animates: false,
      hudStates: state.channelHudStates)
    statusView.panelFocusStates = state.agentPanelFocusStates
    let row = try XCTUnwrap(state.visibleChannelRows.first { $0.channel == owner.channel })
    XCTAssertEqual(
      statusView.rowStateText(for: row, receiptText: "Read"),
      String(localized: "Opening panel in Frame…"))
  }

  func testAgentWithoutFramePaneRestsGreyedAndNeverRequestsFocus() throws {
    let state = OverlayState.previewFormatted()
    let owner = makeOwner()
    armRoster(on: state, owner: owner, frameDetected: false)

    var issuedRequests: [OverlayAgentPanelFocusRequest] = []
    state.requestAgentPanelFocusCommand = { issuedRequests.append($0) }
    state.focusAgentPanel(owner)
    XCTAssertTrue(issuedRequests.isEmpty, "no pane, no request")
    XCTAssertNil(state.agentPanelFocusStates[owner.channel])

    var statusView = OverlayChannelStatusView(
      channels: state.visibleChannelRows.filter { $0.channel != "0" },
      unavailable: false, palette: .dark, animates: false,
      hudStates: state.channelHudStates)
    statusView.panelFocusAvailable = true
    let row = try XCTUnwrap(state.visibleChannelRows.first { $0.channel == owner.channel })
    XCTAssertTrue(statusView.hasNoFramePane(row))
    XCTAssertEqual(
      statusView.panelFocusHelp(for: row, state: nil),
      String(localized: "Agent is not in a Frame pane"))
    XCTAssertEqual(statusView.rowStateText(for: row, receiptText: "Read"), "Read")
  }

  func testFailedFocusRidesTheButtonGlyphNotTheRowLine() throws {
    let state = OverlayState.previewFormatted()
    let owner = makeOwner()
    armRoster(on: state, owner: owner, frameDetected: true)
    let failed = OverlayAgentPanelFocusState(
      requestID: "r", owner: owner, phase: .failed,
      statusText: String(localized: "Panel not found in Frame"), updatedAt: Date())

    var statusView = OverlayChannelStatusView(
      channels: state.visibleChannelRows.filter { $0.channel != "0" },
      unavailable: false, palette: .dark, animates: false,
      hudStates: state.channelHudStates)
    statusView.panelFocusStates = [owner.channel: failed]
    let row = try XCTUnwrap(state.visibleChannelRows.first { $0.channel == owner.channel })
    XCTAssertFalse(statusView.hasNoFramePane(row))
    XCTAssertEqual(statusView.rowStateText(for: row, receiptText: "Read"), "Read")
    XCTAssertEqual(statusView.panelFocusSymbol(failed), "exclamationmark.triangle")
    XCTAssertEqual(
      statusView.panelFocusHelp(for: row, state: failed),
      String(localized: "Panel not found in Frame"))
  }

  func testFocusResultReceiptFoldsExpandedOverlayAndActivatesTerminalHostOnce() throws {
    let state = OverlayState.previewFormatted()
    let owner = makeOwner()
    armRoster(on: state, owner: owner)
    state.setPresentationMode(.midi)
    state.setPresentationMode(.expanded)
    XCTAssertEqual(state.presentationMode, .expanded)

    state.panelFocusSuccessHoldDuration = .seconds(30)
    state.requestAgentPanelFocusCommand = { _ in }
    var activations: [(Int32?, String?)] = []
    state.activateTerminalHost = { pid, bundleID in
      activations.append((pid, bundleID))
      return true
    }

    state.focusAgentPanel(owner)
    let requestID = try XCTUnwrap(state.agentPanelFocusStates[owner.channel]?.requestID)

    let result = OverlayPanelFocusResult(
      requestID: requestID,
      provider: owner.provider,
      providerSessionID: owner.providerSessionID,
      leaseID: owner.leaseID,
      status: "focused",
      exitCode: 0,
      elapsedMs: 12,
      emittedAt: "2026-10-18T08:30:00Z",
      frame: AgentFrameBinding(
        session: "vc-frame-main",
        paneID: 42,
        hostBundleID: "io.vetcoders.vibecrafted",
        hostPID: 54321
      )
    )
    let snapshot = OverlayChannelDeliverySnapshot(
      deliveries: [],
      conversations: [],
      panelFocusResults: [requestID: result]
    )
    state.applyConversationSnapshot(snapshot)

    let focusState = try XCTUnwrap(state.agentPanelFocusStates[owner.channel])
    XCTAssertEqual(focusState.phase, .succeeded)
    XCTAssertEqual(focusState.statusText, String(localized: "Focused in Frame"))
    XCTAssertEqual(state.presentationMode, .midi, "expanded overlay folds back after panel focus")
    XCTAssertEqual(activations.count, 1)
    XCTAssertEqual(activations.first?.0, 54321)
    XCTAssertEqual(activations.first?.1, "io.vetcoders.vibecrafted")

    // Re-applying the same snapshot must not re-trigger host activation or fold again.
    state.setPresentationMode(.expanded)
    state.applyConversationSnapshot(snapshot)
    XCTAssertEqual(activations.count, 1)
    XCTAssertEqual(state.presentationMode, .expanded)
  }

  func testReaderConsumesPanelFocusResultFromChannelBusAndReportsFailureReasons() async throws {
    let tempRoot = FileManager.default.temporaryDirectory
      .appendingPathComponent("codescribe-panel-focus-\(UUID().uuidString)", isDirectory: true)
    let busesDir = tempRoot.appendingPathComponent("buses", isDirectory: true)
    try FileManager.default.createDirectory(at: busesDir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: tempRoot) }

    let owner = makeOwner(channel: "5", name: "stanislaw")
    let state = OverlayState.previewFormatted()
    armRoster(on: state, owner: owner)
    state.setPresentationMode(.expanded)
    state.requestAgentPanelFocusCommand = { _ in }
    var activated = false
    state.activateTerminalHost = { _, _ in
      activated = true
      return true
    }

    state.focusAgentPanel(owner)
    let requestID = try XCTUnwrap(state.agentPanelFocusStates[owner.channel]?.requestID)

    let channelBus = busesDir.appendingPathComponent("channel-5.jsonl")
    let leasesDir = tempRoot.appendingPathComponent("leases", isDirectory: true)
    try FileManager.default.createDirectory(at: leasesDir, withIntermediateDirectories: true)
    let leaseReceipt: [String: Any] = [
      "schema": "codescribe.agent-bridge.lease.v1",
      "lease_id": owner.leaseID,
      "provider": owner.provider,
      "provider_session_id": owner.providerSessionID,
      "channel": owner.channel,
      "name": owner.name,
      "bus": channelBus.path,
    ]
    let leaseJSON = try String(
      data: JSONSerialization.data(withJSONObject: leaseReceipt, options: [.sortedKeys]),
      encoding: .utf8)!
    try leaseJSON.write(
      to: leasesDir.appendingPathComponent("\(owner.leaseID).json"),
      atomically: true,
      encoding: .utf8)
    let resultRow: [String: Any] = [
      "schema": "codescribe.agent-panel-focus-result.v1",
      "kind": "agent_panel_focus_result",
      "request_id": requestID,
      "provider": owner.provider,
      "provider_session_id": owner.providerSessionID,
      "lease_id": owner.leaseID,
      "status": "pane_missing",
      "exit_code": 1,
      "elapsed_ms": 8,
      "emitted_at": "2026-10-18T08:31:00Z",
      "frame": [
        "session": "vc-frame-main",
        "pane_id": 19,
        "host_bundle_id": "io.vetcoders.vibecrafted",
        "host_pid": 9999,
      ],
    ]
    let line =
      try String(
        data: JSONSerialization.data(withJSONObject: resultRow, options: [.sortedKeys]),
        encoding: .utf8)! + "\n"
    try line.write(to: channelBus, atomically: true, encoding: .utf8)

    let sharedBus = tempRoot.appendingPathComponent("transcript-bus.jsonl")
    let reader = OverlayChannelDeliveryReader(root: tempRoot, sharedBus: sharedBus)
    let snapshot = try await reader.readSnapshot()
    let parsed = try XCTUnwrap(snapshot.panelFocusResults[requestID])
    XCTAssertEqual(parsed.status, "pane_missing")
    XCTAssertFalse(parsed.isSuccess)
    XCTAssertEqual(parsed.frame?.paneID, 19)
    XCTAssertEqual(parsed.frame?.hostPID, 9999)

    state.applyConversationSnapshot(snapshot)
    let focusState = try XCTUnwrap(state.agentPanelFocusStates[owner.channel])
    XCTAssertEqual(focusState.phase, .failed)
    XCTAssertEqual(focusState.statusText, String(localized: "Panel not found in Frame"))
    XCTAssertEqual(state.presentationMode, .expanded, "failed focus leaves the overlay open")
    XCTAssertFalse(activated)
  }

  func testClickingAlreadySelectedAgentRowRequestsPanelFocus() throws {
    let owner = makeOwner(channel: "5", name: "stanislaw")
    let conversation = OverlayConversation(
      id: "conv-5", channel: "5", name: "stanislaw", owner: owner, messages: [])
    let row = OverlayChannelDelivery(
      channel: "5", agent: "stanislaw", deliveryID: nil, stage: nil, isOpen: false)
    var selectedIDs: [String?] = []
    var focusedOwners: [OverlayConversationOwner] = []

    var view = OverlayChannelStatusView(
      channels: [row], unavailable: false, palette: .dark, animates: false,
      hudStates: [
        "5": OverlayChannelHudProjection(
          open: false, loud: false, autosealDeadline: nil, followerAlive: true,
          provider: owner.provider, providerSessionID: owner.providerSessionID)
      ],
      conversations: [conversation],
      selectedConversationID: conversation.id,
      unreadCounts: [:],
      onSelectConversation: { selectedIDs.append($0) })
    view.archiveCandidates = ["5": owner]
    view.panelFocusAvailable = true
    view.onFocusAgentPanel = { focusedOwners.append($0) }

    view.viewConversation(row)
    XCTAssertEqual(focusedOwners, [owner])
    XCTAssertTrue(selectedIDs.isEmpty)
  }

  func testRequestWireObjectMatchesAgentPanelFocusSchema() {
    let owner = makeOwner(channel: "5", name: "stanislaw")
    let request = OverlayAgentPanelFocusRequest(
      requestID: "0123456789abcdef01234567",
      owner: owner,
      emittedAt: "2026-10-18T08:32:00Z"
    )
    let event = request.eventObject(busPath: "/tmp/channel-5.jsonl")
    XCTAssertEqual(event["schema"] as? String, "codescribe.agent-panel-focus.v1")
    XCTAssertEqual(event["kind"] as? String, "focus_agent_panel")
    XCTAssertEqual(event["request_id"] as? String, "0123456789abcdef01234567")
    XCTAssertEqual(event["provider"] as? String, owner.provider)
    XCTAssertEqual(event["provider_session_id"] as? String, owner.providerSessionID)
    XCTAssertEqual(event["lease_id"] as? String, owner.leaseID)
    XCTAssertEqual(event["channel"] as? String, "5")
    XCTAssertEqual(event["bus"] as? String, "/tmp/channel-5.jsonl")
  }

  func testPanelFocusButtonUsesOverlayHeaderTooltipChrome() throws {
    let sourceURL = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .appendingPathComponent("Codescribe/Screens/Overlay/OverlayChannelStatusView.swift")
    let source = try String(contentsOf: sourceURL, encoding: .utf8)
    XCTAssertTrue(
      source.contains("OverlayHeaderControlChrome(title: focusHelp, accent: nil, palette: palette)")
    )
    XCTAssertFalse(source.contains(".help(focusHelp)"))
  }

  func testVcFrameCapabilityDetectionRejectsAbsentAndIncompatibleBinariesAndIgnoresEnginePin()
    async throws
  {
    let tempRoot = FileManager.default.temporaryDirectory
      .appendingPathComponent("codescribe-vc-frame-cap-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: tempRoot, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: tempRoot) }

    let emptyBin = tempRoot.appendingPathComponent("empty-bin", isDirectory: true)
    try FileManager.default.createDirectory(at: emptyBin, withIntermediateDirectories: true)
    let pinnedLog = tempRoot.appendingPathComponent("pinned.log")
    let pinnedExecutable = try writeExecutable(
      at: tempRoot.appendingPathComponent("pinned", isDirectory: true),
      body: """
        printf 'pinned %s\\n' "$*" >> '\(pinnedLog.path)'
        cat <<'EOF'
        \(Self.currentSupportedHelp)
        EOF
        exit 0
        """)

    // 1. No vc-frame on PATH, even when HOME/.local/bin/vc-frame and VIBECRAFTED_VC_FRAME_BIN exist:
    let unconfiguredHome = tempRoot.appendingPathComponent("home-absent", isDirectory: true)
    let unconfiguredLocalLog = tempRoot.appendingPathComponent("unconfigured-local.log")
    _ = try writeExecutable(
      at: unconfiguredHome.appendingPathComponent(".local/bin", isDirectory: true),
      body: """
        printf 'local %s\\n' "$*" >> '\(unconfiguredLocalLog.path)'
        cat <<'EOF'
        \(Self.currentSupportedHelp)
        EOF
        exit 0
        """)
    let absentEnv = [
      "PATH": emptyBin.path,
      "VIBECRAFTED_VC_FRAME_BIN": pinnedExecutable.path,
    ]
    XCTAssertNil(
      RealAgentBridgeInstaller.vcFrameExecutableURL(
        homeDirectory: unconfiguredHome, environment: absentEnv),
      "vcFrameExecutableURL must agree with PATH-only resolution and never append a literal HOME/.local/bin fallback"
    )
    let absentInstaller = RealAgentBridgeInstaller(
      resourceRoot: nil,
      homeDirectory: unconfiguredHome,
      environment: absentEnv)
    let absentSupported = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: absentInstaller)
    XCTAssertFalse(absentSupported)
    XCTAssertFalse(
      FileManager.default.fileExists(atPath: pinnedLog.path),
      "VIBECRAFTED_VC_FRAME_BIN must never be probed as a fallback")
    XCTAssertFalse(
      FileManager.default.fileExists(atPath: unconfiguredLocalLog.path),
      "HOME/.local/bin must never be probed when absent from PATH")

    // 2. Installed vc-frame on PATH that lacks `focus-pane-id` (exits 2):
    let unsupportedBin = tempRoot.appendingPathComponent("unsupported-bin", isDirectory: true)
    let unsupportedLog = tempRoot.appendingPathComponent("unsupported.log")
    _ = try writeExecutable(
      at: unsupportedBin,
      body: """
        printf '%s\\n' "$*" >> '\(unsupportedLog.path)'
        cat >&2 <<'EOF'
        \(Self.unsupportedStderr)
        EOF
        exit 2
        """)
    let unsupportedInstaller = RealAgentBridgeInstaller(
      resourceRoot: nil,
      homeDirectory: tempRoot.appendingPathComponent("home-unsupported", isDirectory: true),
      environment: ["PATH": unsupportedBin.path])
    let unsupportedResult = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: unsupportedInstaller)
    XCTAssertFalse(unsupportedResult)
    let unsupportedInvocations = try String(contentsOf: unsupportedLog, encoding: .utf8)
      .split(separator: "\n")
    XCTAssertEqual(unsupportedInvocations, ["action focus-pane-id --help"])
  }

  func testVcFrameCapabilityDetectionAcceptsOlderAndCurrentSupportedReleasesWithoutSideEffects()
    async throws
  {
    let tempRoot = FileManager.default.temporaryDirectory
      .appendingPathComponent("codescribe-vc-frame-ok-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: tempRoot, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: tempRoot) }

    // 1. Older supported release (`4.4.0+g959bf184` in `4.4.0+g0a387b2e`) on PATH:
    let olderBin = tempRoot.appendingPathComponent("older-bin", isDirectory: true)
    let olderLog = tempRoot.appendingPathComponent("older.log")
    _ = try writeExecutable(
      at: olderBin,
      body: """
        printf '%s|%s|%s\\n' "$*" "$VC_FRAME_SESSION_NAME" "$VC_FRAME_SOCKET_DIR" >> '\(olderLog.path)'
        if [ "$1" = "action" ] && [ "$2" = "focus-pane-id" ] && [ "$3" = "--help" ]; then
          cat <<'EOF'
        \(Self.olderSupportedHelp)
        EOF
          exit 0
        fi
        exit 99
        """)
    let olderInstaller = RealAgentBridgeInstaller(
      resourceRoot: nil,
      homeDirectory: tempRoot.appendingPathComponent("home-older", isDirectory: true),
      environment: [
        "PATH": olderBin.path,
        "VC_FRAME_SESSION_NAME": "vibecrafted",
        "ZELLIJ_SESSION_NAME": "vibecrafted",
        "VC_FRAME_SOCKET_DIR": "/tmp/vc-frame-501",
        "ZELLIJ_SOCKET_DIR": "/tmp/vc-frame-501",
      ])
    let olderFirst = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: olderInstaller)
    let olderSecond = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: olderInstaller)
    XCTAssertTrue(olderFirst)
    XCTAssertTrue(olderSecond)
    let olderLines = try String(contentsOf: olderLog, encoding: .utf8).split(separator: "\n")
    XCTAssertEqual(
      olderLines,
      ["action focus-pane-id --help||"],
      "probe must strip session/socket env vars and memoize unchanged binary by stat stamp")

    // 2. Current supported release (`4.4.0+ga3f8214c`) on the user's configured `.zshenv` PATH
    //    while the GUI process runs under launchd's minimal PATH (`/usr/bin:/bin:/usr/sbin:/sbin`):
    let launchdHome = tempRoot.appendingPathComponent("home-launchd", isDirectory: true)
    let localBin = launchdHome.appendingPathComponent(".local/bin", isDirectory: true)
    let currentLog = tempRoot.appendingPathComponent("current.log")
    _ = try writeExecutable(
      at: localBin,
      body: """
        printf '%s|%s|%s\\n' "$*" "$ZELLIJ_SESSION_NAME" "$ZELLIJ_SOCKET_DIR" >> '\(currentLog.path)'
        if [ "$1" = "action" ] && [ "$2" = "focus-pane-id" ] && [ "$3" = "--help" ]; then
          cat <<'EOF'
        \(Self.currentSupportedHelp)
        EOF
          exit 0
        fi
        exit 99
        """)
    try "export PATH=\"$HOME/.local/bin:$PATH\"\n".write(
      to: launchdHome.appendingPathComponent(".zshenv"),
      atomically: true,
      encoding: .utf8)
    let launchdInstaller = RealAgentBridgeInstaller(
      resourceRoot: nil,
      homeDirectory: launchdHome,
      environment: [
        "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
        "ZELLIJ_SESSION_NAME": "vibecrafted",
        "ZELLIJ_SOCKET_DIR": "/tmp/vc-frame-501",
      ])
    let currentSupported = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: launchdInstaller)
    XCTAssertTrue(currentSupported)
    let currentLines = try String(contentsOf: currentLog, encoding: .utf8).split(separator: "\n")
    XCTAssertEqual(currentLines, ["action focus-pane-id --help||"])

    // 3. Public wrapper whose internal engine executable is initially missing and then restored
    //    without modifying the outer wrapper file:
    let wrapperRoot = tempRoot.appendingPathComponent("wrapper-repair", isDirectory: true)
    let wrapperBin = wrapperRoot.appendingPathComponent("bin", isDirectory: true)
    let engineURL = wrapperRoot.appendingPathComponent("engine")
    _ = try writeExecutable(
      at: wrapperBin,
      body: "exec '\(engineURL.path)' \"$@\"\n")
    let wrapperInstaller = RealAgentBridgeInstaller(
      resourceRoot: nil,
      homeDirectory: wrapperRoot,
      environment: ["PATH": wrapperBin.path])
    let beforeRepair = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: wrapperInstaller)
    XCTAssertFalse(beforeRepair)

    try "#!/bin/sh\nprintf 'vc-frame action focus-pane-id <PANE_ID>\\n'\n".write(
      to: engineURL, atomically: true, encoding: .utf8)
    try FileManager.default.setAttributes(
      [.posixPermissions: 0o755], ofItemAtPath: engineURL.path)
    let afterRepair = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: wrapperInstaller)
    XCTAssertTrue(
      afterRepair,
      "restoring a missing wrapper dependency must turn capability true without touching the outer wrapper"
    )

    try FileManager.default.setAttributes(
      [.posixPermissions: 0o644], ofItemAtPath: engineURL.path)
    let afterChmodNonExec = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: wrapperInstaller)
    XCTAssertFalse(
      afterChmodNonExec,
      "losing execute permission on a wrapper dependency must invalidate the positive capability cache"
    )

    try FileManager.default.setAttributes(
      [.posixPermissions: 0o755], ofItemAtPath: engineURL.path)
    let afterChmodRestored = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: wrapperInstaller)
    XCTAssertTrue(afterChmodRestored)

    try FileManager.default.removeItem(at: engineURL)
    let afterRemoval = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: wrapperInstaller)
    XCTAssertFalse(
      afterRemoval,
      "removing a referenced wrapper dependency must invalidate the cached capability")
  }

  func testPanelFocusButtonExcludedWhenUnavailableAndRenderedWhenAvailableIncludingDeadFollower()
    throws
  {
    let owner = makeOwner(channel: "5", name: "stanislaw")
    let conversation = OverlayConversation(
      id: "conv-5", channel: "5", name: "stanislaw", owner: owner, messages: [])
    let row = OverlayChannelDelivery(
      channel: "5", agent: "stanislaw", deliveryID: nil, stage: nil, isOpen: false)

    // Initial unknown state before any async probe completes must fail-closed:
    let initialState = OverlayState.previewFormatted()
    armRoster(on: initialState, owner: owner, panelFocusAvailable: false)
    XCTAssertFalse(
      initialState.agentPanelFocusAvailable,
      "OverlayState must start fail-closed (false) before capability detection completes")
    var initialRequests: [OverlayAgentPanelFocusRequest] = []
    initialState.requestAgentPanelFocusCommand = { initialRequests.append($0) }
    initialState.focusAgentPanel(owner)
    XCTAssertNil(initialState.agentPanelFocusStates[owner.channel])
    XCTAssertTrue(initialRequests.isEmpty)

    var defaultStatusView = OverlayChannelStatusView(
      channels: [row],
      unavailable: false,
      palette: .dark,
      animates: false,
      hudStates: [
        "5": OverlayChannelHudProjection(
          open: false, loud: false, autosealDeadline: nil, followerAlive: true,
          provider: owner.provider, providerSessionID: owner.providerSessionID)
      ],
      conversations: [conversation],
      selectedConversationID: conversation.id,
      unreadCounts: [:])
    defaultStatusView.archiveCandidates = ["5": owner]
    defaultStatusView.onArchiveAgent = { _ in }
    defaultStatusView.onFocusAgentPanel = { _ in }
    XCTAssertFalse(
      defaultStatusView.panelFocusAvailable,
      "OverlayChannelStatusView must default panelFocusAvailable to false")
    XCTAssertFalse(
      defaultStatusView.showsPanelFocusButton(for: row),
      "first render before capability evidence must exclude the Focus in Frame button")

    func makeStatusView(
      panelFocusAvailable: Bool,
      followerAlive: Bool,
      onSelectConversation: @escaping (String?) -> Void = { _ in },
      onFocusAgentPanel: @escaping (OverlayConversationOwner) -> Void = { _ in }
    ) -> OverlayChannelStatusView {
      var view = OverlayChannelStatusView(
        channels: [row],
        unavailable: false,
        palette: .dark,
        animates: false,
        hudStates: [
          "5": OverlayChannelHudProjection(
            open: false, loud: false, autosealDeadline: nil, followerAlive: followerAlive,
            provider: owner.provider, providerSessionID: owner.providerSessionID)
        ],
        conversations: [conversation],
        selectedConversationID: conversation.id,
        unreadCounts: [:],
        onSelectConversation: onSelectConversation)
      view.archiveCandidates = ["5": owner]
      view.onArchiveAgent = { _ in }
      view.panelFocusAvailable = panelFocusAvailable
      view.onFocusAgentPanel = onFocusAgentPanel
      return view
    }

    let unavailableView = makeStatusView(panelFocusAvailable: false, followerAlive: true)
    let availableLiveView = makeStatusView(panelFocusAvailable: true, followerAlive: true)
    let availableDeadFollowerView = makeStatusView(panelFocusAvailable: true, followerAlive: false)

    XCTAssertFalse(unavailableView.showsPanelFocusButton(for: row))
    XCTAssertTrue(availableLiveView.showsPanelFocusButton(for: row))
    XCTAssertTrue(
      availableDeadFollowerView.showsPanelFocusButton(for: row),
      "a disconnected follower must not hide the Focus in Frame button when vc-frame is installed")
    XCTAssertEqual(
      availableDeadFollowerView.panelFocusHelp(for: row, state: nil),
      String(localized: "Agent is disconnected"))

    let defaultInitialSize = renderedRowSize(in: defaultStatusView, for: row)
    let unavailableSize = renderedRowSize(in: unavailableView, for: row)
    let availableLiveSize = renderedRowSize(in: availableLiveView, for: row)
    let availableDeadSize = renderedRowSize(in: availableDeadFollowerView, for: row)

    XCTAssertEqual(
      defaultInitialSize.width,
      unavailableSize.width,
      accuracy: 1.0,
      "initial unprobed render excludes the Focus in Frame button")
    XCTAssertGreaterThan(
      availableLiveSize.width,
      unavailableSize.width + 20,
      "the Focus in Frame button is excluded from the rendered row when vc-frame is unavailable")
    XCTAssertEqual(
      availableDeadSize.width,
      availableLiveSize.width,
      accuracy: 1.0,
      "a dead follower keeps the Focus in Frame button rendered (disabled) when vc-frame is installed"
    )

    // Clicking an already-selected row when panelFocusAvailable is false must not request panel focus:
    var focusedWhenUnavailable: [OverlayConversationOwner] = []
    var selectedWhenUnavailable: [String?] = []
    let checkView = makeStatusView(
      panelFocusAvailable: false,
      followerAlive: true,
      onSelectConversation: { selectedWhenUnavailable.append($0) },
      onFocusAgentPanel: { focusedWhenUnavailable.append($0) })
    checkView.viewConversation(row)
    XCTAssertTrue(focusedWhenUnavailable.isEmpty)
    XCTAssertEqual(selectedWhenUnavailable, [conversation.id])

    // OverlayState.focusAgentPanel is also a no-op when agentPanelFocusAvailable is false:
    let state = OverlayState.previewFormatted()
    armRoster(on: state, owner: owner)
    state.setAgentPanelFocusAvailable(false)
    var issuedRequests: [OverlayAgentPanelFocusRequest] = []
    state.requestAgentPanelFocusCommand = { request in
      issuedRequests.append(request)
    }
    state.focusAgentPanel(owner)
    XCTAssertNil(state.agentPanelFocusStates[owner.channel])
    XCTAssertTrue(issuedRequests.isEmpty)
  }
}
