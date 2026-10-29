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

  private func makeOwner(
    channel: String = "5", name: String = "stanislaw", provider: String = "claude-code"
  )
    -> OverlayConversationOwner
  {
    let leaseID = AgentPlaybackIdentity.leaseIdentifier(
      provider: provider, session: "session-\(channel)")
    return OverlayConversationOwner(row: [
      "provider": provider,
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

  private func armRoster(
    on state: OverlayState,
    owner: OverlayConversationOwner,
    panelFocusAvailable: Bool = true,
    frameDetected: Bool? = true,
    terminalDetected: Bool? = nil,
    followerAlive: Bool? = true,
    codexThreadUrl: String? = nil,
    codexThreadLocal: Bool? = nil
  ) {
    state.applyChannelRoster([
      CsChannelRosterState(
        channel: "0", audience: "All", provider: nil, providerSessionId: nil, open: false,
        loud: false, autosealDeadlineUnixMs: nil, followerAlive: nil, unreadSinceUnixMs: nil),
      CsChannelRosterState(
        channel: owner.channel, audience: owner.name, provider: owner.provider,
        providerSessionId: owner.providerSessionID, open: false, loud: false,
        autosealDeadlineUnixMs: nil, followerAlive: followerAlive, frameDetected: frameDetected,
        terminalDetected: terminalDetected, codexThreadUrl: codexThreadUrl,
        codexThreadLocal: codexThreadLocal, unreadSinceUnixMs: nil),
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

  func testAgentWithoutFramePaneNeverRequestsFocus() throws {
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
    XCTAssertTrue(statusView.hasNoTerminalTarget(row))
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
    XCTAssertFalse(statusView.hasNoTerminalTarget(row))
    XCTAssertEqual(statusView.rowStateText(for: row, receiptText: "Read"), "Read")
    XCTAssertEqual(statusView.panelFocusSymbol(failed), "exclamationmark.triangle")
    XCTAssertEqual(
      statusView.panelFocusHelp(for: row, state: failed),
      String(localized: "Panel not found in Frame"))
  }

  func testFocusResultReceiptFoldsExpandedOverlayAndActivatesTerminalHostOnce() async throws {
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

    XCTAssertEqual(state.agentPanelFocusStates[owner.channel]?.phase, .activating)
    for _ in 0..<30 { await Task.yield() }
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

  func testPaneReceiptWaitsForHostConfirmationAndDeduplicatesWhileWaiting() async throws {
    let state = OverlayState.previewFormatted()
    let owner = makeOwner()
    armRoster(on: state, owner: owner)
    state.setPresentationMode(.expanded)
    state.requestAgentPanelFocusCommand = { _ in }
    var confirmation: CheckedContinuation<Bool, Never>?
    var calls = 0
    state.activateTerminalHost = { _, _ in
      calls += 1
      return await withCheckedContinuation { confirmation = $0 }
    }
    state.focusAgentPanel(owner)
    let requestID = try XCTUnwrap(state.agentPanelFocusStates[owner.channel]?.requestID)
    let snapshot = focusSnapshot(owner: owner, requestID: requestID)
    state.applyConversationSnapshot(snapshot)
    for _ in 0..<30 { await Task.yield() }
    XCTAssertEqual(state.agentPanelFocusStates[owner.channel]?.phase, .activating)
    XCTAssertEqual(state.presentationMode, .expanded)
    state.applyConversationSnapshot(snapshot)
    for _ in 0..<30 { await Task.yield() }
    XCTAssertEqual(calls, 1)
    let pending = try XCTUnwrap(confirmation)
    pending.resume(returning: true)
    for _ in 0..<30 { await Task.yield() }
    XCTAssertEqual(state.agentPanelFocusStates[owner.channel]?.phase, .succeeded)
    XCTAssertNotEqual(state.presentationMode, .expanded)
  }

  func testHostActivationFailureKeepsConversationOpen() async throws {
    let state = OverlayState.previewFormatted()
    let owner = makeOwner()
    armRoster(on: state, owner: owner)
    state.setPresentationMode(.expanded)
    state.requestAgentPanelFocusCommand = { _ in }
    state.activateTerminalHost = { _, _ in false }
    state.focusAgentPanel(owner)
    let requestID = try XCTUnwrap(state.agentPanelFocusStates[owner.channel]?.requestID)
    state.applyConversationSnapshot(focusSnapshot(owner: owner, requestID: requestID))
    for _ in 0..<30 { await Task.yield() }
    XCTAssertEqual(state.agentPanelFocusStates[owner.channel]?.phase, .failed)
    XCTAssertEqual(state.presentationMode, .expanded)
  }

  func testLateHostConfirmationCannotOverwriteTimeout() async throws {
    let state = OverlayState.previewFormatted()
    let owner = makeOwner()
    armRoster(on: state, owner: owner)
    state.setPresentationMode(.expanded)
    state.panelFocusTimeoutDuration = .milliseconds(20)
    state.requestAgentPanelFocusCommand = { _ in }
    var confirmation: CheckedContinuation<Bool, Never>?
    state.activateTerminalHost = { _, _ in
      await withCheckedContinuation { confirmation = $0 }
    }
    state.focusAgentPanel(owner)
    let requestID = try XCTUnwrap(state.agentPanelFocusStates[owner.channel]?.requestID)
    state.applyConversationSnapshot(focusSnapshot(owner: owner, requestID: requestID))
    for _ in 0..<30 { await Task.yield() }
    let pending = try XCTUnwrap(confirmation)
    try await Task.sleep(for: .milliseconds(50))
    XCTAssertEqual(state.agentPanelFocusStates[owner.channel]?.phase, .failed)
    pending.resume(returning: true)
    for _ in 0..<30 { await Task.yield() }
    XCTAssertEqual(state.agentPanelFocusStates[owner.channel]?.phase, .failed)
    XCTAssertEqual(state.presentationMode, .expanded)
  }

  func testReceiptFromDifferentOwnerCannotActivateHost() throws {
    let state = OverlayState.previewFormatted()
    let owner = makeOwner()
    armRoster(on: state, owner: owner)
    state.requestAgentPanelFocusCommand = { _ in }
    state.activateTerminalHost = { _, _ in
      XCTFail("Wrong owner cannot activate")
      return false
    }
    state.focusAgentPanel(owner)
    let requestID = try XCTUnwrap(state.agentPanelFocusStates[owner.channel]?.requestID)
    state.applyConversationSnapshot(
      focusSnapshot(
        owner: makeOwner(provider: "codex"), requestID: requestID))
    XCTAssertEqual(state.agentPanelFocusStates[owner.channel]?.phase, .requesting)
  }

  private func focusSnapshot(owner: OverlayConversationOwner, requestID: String)
    -> OverlayChannelDeliverySnapshot
  {
    let result = OverlayPanelFocusResult(
      requestID: requestID, provider: owner.provider, providerSessionID: owner.providerSessionID,
      leaseID: owner.leaseID, status: "focused", exitCode: 0, elapsedMs: 1,
      emittedAt: "2026-10-25T20:00:00Z",
      frame: AgentFrameBinding(
        session: "fixture", paneID: 42,
        hostBundleID: "io.vetcoders.vibecrafted", hostPID: 54321))
    return .init(deliveries: [], conversations: [], panelFocusResults: [requestID: result])
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
    let reader = OverlayChannelDeliveryReader(
      root: tempRoot, cacheRoot: tempRoot.appendingPathComponent("data.noindex", isDirectory: true),
      sharedBus: sharedBus)
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
          provider: owner.provider, providerSessionID: owner.providerSessionID, frameDetected: true)
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

  func testCompactActionMenuKeepsFocusResultInProductTooltip() throws {
    let sourceURL = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .appendingPathComponent("Codescribe/Screens/Overlay/OverlayChannelStatusView.swift")
    let source = try String(contentsOf: sourceURL, encoding: .utf8)
    XCTAssertTrue(source.contains(".overlayFastHint("))
    XCTAssertTrue(source.contains("focusState?.statusText ?? String(localized: \"Agent actions\")"))
    XCTAssertTrue(
      source.contains(".accessibilityValue(Text(verbatim: focusState?.statusText ?? \"\"))"))
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

    // 2. Current supported release (`4.4.0+ga3f8214c`) on the user's configured `.zshenv`/`.zprofile`
    //    PATH while the GUI process runs under launchd's minimal PATH (`/usr/bin:/bin:/usr/sbin:/sbin`):
    let launchdHome = tempRoot.appendingPathComponent("home-launchd", isDirectory: true)
    let localBin = launchdHome.appendingPathComponent(".local/bin", isDirectory: true)
    let currentLog = tempRoot.appendingPathComponent("current.log")
    let zprofileEvalLog = tempRoot.appendingPathComponent("zprofile-eval.log")
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
    let zprofileURL = launchdHome.appendingPathComponent(".zprofile")
    try """
    printf 'eval-1\\n' >> '\(zprofileEvalLog.path)'
    export PATH="$HOME/.local/bin:$PATH"
    """.write(to: zprofileURL, atomically: true, encoding: .utf8)
    let launchdInstaller = RealAgentBridgeInstaller(
      resourceRoot: nil,
      homeDirectory: launchdHome,
      environment: [
        "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
        "ZELLIJ_SESSION_NAME": "vibecrafted",
        "ZELLIJ_SOCKET_DIR": "/tmp/vc-frame-501",
      ])
    let currentSupportedFirst = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: launchdInstaller)
    let currentSupportedSecond = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: launchdInstaller)
    let currentSupportedThird = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: launchdInstaller)
    XCTAssertTrue(currentSupportedFirst)
    XCTAssertTrue(currentSupportedSecond)
    XCTAssertTrue(currentSupportedThird)
    let currentLines = try String(contentsOf: currentLog, encoding: .utf8).split(separator: "\n")
    XCTAssertEqual(currentLines, ["action focus-pane-id --help||"])
    let evalLinesBeforeEdit = try String(contentsOf: zprofileEvalLog, encoding: .utf8)
      .split(separator: "\n")
    XCTAssertEqual(
      evalLinesBeforeEdit,
      ["eval-1"],
      "launchd PATH resolution must memoize .zprofile/.zshenv by file stats instead of spawning /bin/zsh on every poll"
    )

    try """
    printf 'eval-2\\n' >> '\(zprofileEvalLog.path)'
    export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"
    """.write(to: zprofileURL, atomically: true, encoding: .utf8)
    let currentSupportedAfterProfileEdit = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: launchdInstaller)
    let currentSupportedAfterProfileEditCached =
      await RealAgentBridgeInstaller.supportsAgentPanelFocus(installer: launchdInstaller)
    XCTAssertTrue(currentSupportedAfterProfileEdit)
    XCTAssertTrue(currentSupportedAfterProfileEditCached)
    let evalLinesAfterEdit = try String(contentsOf: zprofileEvalLog, encoding: .utf8)
      .split(separator: "\n")
    XCTAssertEqual(
      evalLinesAfterEdit,
      ["eval-1", "eval-2"],
      "modifying .zprofile must invalidate VcFrameShellPathCache once and memoize the updated stamp"
    )

    // 2b. Mid-flight profile mutation during /bin/zsh execution (`postStamp != profileStamp`)
    //     must NOT cache the stale pre-mutation PATH under the new file stamp:
    let midflightHome = tempRoot.appendingPathComponent("home-midflight", isDirectory: true)
    let midflightBin = midflightHome.appendingPathComponent(".local/bin", isDirectory: true)
    let midflightEvalLog = tempRoot.appendingPathComponent("midflight-eval.log")
    _ = try writeExecutable(
      at: midflightBin,
      body: """
        if [ "$1" = "action" ] && [ "$2" = "focus-pane-id" ] && [ "$3" = "--help" ]; then
          cat <<'EOF'
        \(Self.currentSupportedHelp)
        EOF
          exit 0
        fi
        exit 99
        """)
    let midflightZprofileURL = midflightHome.appendingPathComponent(".zprofile")
    try """
    printf 'midflight-1\\n' >> '\(midflightEvalLog.path)'
    export PATH="/usr/bin:/bin"
    cat > "$HOME/.zprofile" <<'EOF'
    printf 'midflight-2\\n' >> '\(midflightEvalLog.path)'
    export PATH="$HOME/.local/bin:/usr/bin:/bin"
    EOF
    """.write(to: midflightZprofileURL, atomically: true, encoding: .utf8)
    let midflightInstaller = RealAgentBridgeInstaller(
      resourceRoot: nil,
      homeDirectory: midflightHome,
      environment: ["PATH": "/usr/bin:/bin:/usr/sbin:/sbin"])
    let midflightFirst = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: midflightInstaller)
    let midflightSecond = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: midflightInstaller)
    let midflightThird = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: midflightInstaller)
    XCTAssertFalse(
      midflightFirst,
      "first call evaluates the pre-mutation .zprofile which did not include ~/.local/bin")
    XCTAssertTrue(
      midflightSecond,
      "second call must re-evaluate the newly written .zprofile instead of serving stale PATH under postStamp"
    )
    XCTAssertTrue(midflightThird)
    let midflightEvalLines = try String(contentsOf: midflightEvalLog, encoding: .utf8)
      .split(separator: "\n")
    XCTAssertEqual(
      midflightEvalLines,
      ["midflight-1", "midflight-2"],
      "mid-flight .zprofile change must force one re-evaluation on the next poll and then memoize the stable stamp"
    )

    // 2c. Broken/empty .zprofile under launchd PATH must be throttled by bounded failure cooldown
    //     (preventing /bin/zsh spawn on every 500ms poll), retry after cooldown expiry, and
    //     immediately invalidate when .zprofile is edited on disk:
    let failureHome = tempRoot.appendingPathComponent("home-failure", isDirectory: true)
    let failureBin = failureHome.appendingPathComponent(".local/bin", isDirectory: true)
    let failureEvalLog = tempRoot.appendingPathComponent("failure-eval.log")
    _ = try writeExecutable(
      at: failureBin,
      body: """
        if [ "$1" = "action" ] && [ "$2" = "focus-pane-id" ] && [ "$3" = "--help" ]; then
          cat <<'EOF'
        \(Self.currentSupportedHelp)
        EOF
          exit 0
        fi
        exit 99
        """)
    let failureZprofileURL = failureHome.appendingPathComponent(".zprofile")
    try """
    printf 'fail-nonzero\\n' >> '\(failureEvalLog.path)'
    exit 1
    """.write(to: failureZprofileURL, atomically: true, encoding: .utf8)
    let failureInstaller = RealAgentBridgeInstaller(
      resourceRoot: nil,
      homeDirectory: failureHome,
      environment: ["PATH": "/usr/bin:/bin:/usr/sbin:/sbin"])
    let t0 = Date(timeIntervalSince1970: 1_700_000_000)
    let failInitial = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: failureInstaller, now: t0)
    let failThrottled = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: failureInstaller, now: t0.addingTimeInterval(0.5))
    XCTAssertFalse(failInitial)
    XCTAssertFalse(failThrottled)
    let evalAfterThrottledPoll = try String(contentsOf: failureEvalLog, encoding: .utf8)
      .split(separator: "\n")
    XCTAssertEqual(
      evalAfterThrottledPoll,
      ["fail-nonzero"],
      "non-zero .zprofile exit must be throttled within failureCooldown instead of spawning /bin/zsh on the next 500ms poll"
    )

    let tRetry = t0.addingTimeInterval(RealAgentBridgeInstaller.vcFrameShellFailureCooldown + 0.1)
    let failAfterCooldown = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: failureInstaller, now: tRetry)
    let failAfterCooldownThrottled = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: failureInstaller, now: tRetry.addingTimeInterval(0.5))
    XCTAssertFalse(failAfterCooldown)
    XCTAssertFalse(failAfterCooldownThrottled)
    let evalAfterCooldownRetry = try String(contentsOf: failureEvalLog, encoding: .utf8)
      .split(separator: "\n")
    XCTAssertEqual(
      evalAfterCooldownRetry,
      ["fail-nonzero", "fail-nonzero"],
      "once failureCooldown expires, the next poll must retry /bin/zsh once and re-arm the cooldown"
    )

    // Editing .zprofile during an active failure cooldown (here to an empty-PATH failure, then to a
    // valid PATH) must immediately invalidate the previous stamp's cooldown without waiting for TTL:
    try """
    printf 'fail-empty\\n' >> '\(failureEvalLog.path)'
    export PATH=""
    """.write(to: failureZprofileURL, atomically: true, encoding: .utf8)
    let tEmpty = tRetry.addingTimeInterval(1.0)
    let failEmptyInitial = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: failureInstaller, now: tEmpty)
    let failEmptyThrottled = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: failureInstaller, now: tEmpty.addingTimeInterval(0.5))
    XCTAssertFalse(failEmptyInitial)
    XCTAssertFalse(failEmptyThrottled)
    let evalAfterEmpty = try String(contentsOf: failureEvalLog, encoding: .utf8)
      .split(separator: "\n")
    XCTAssertEqual(
      evalAfterEmpty,
      ["fail-nonzero", "fail-nonzero", "fail-empty"],
      "empty PATH output must also be throttled after immediate invalidation on profile edit"
    )

    try """
    printf 'repaired\\n' >> '\(failureEvalLog.path)'
    export PATH="$HOME/.local/bin:/usr/bin:/bin"
    """.write(to: failureZprofileURL, atomically: true, encoding: .utf8)
    let repairedDuringCooldown = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: failureInstaller, now: tEmpty.addingTimeInterval(1.0))
    let repairedCached = await RealAgentBridgeInstaller.supportsAgentPanelFocus(
      installer: failureInstaller, now: tEmpty.addingTimeInterval(1.5))
    XCTAssertTrue(
      repairedDuringCooldown,
      "repairing .zprofile on disk must immediately bypass the previous stamp's failure cooldown"
    )
    XCTAssertTrue(repairedCached)
    let evalAfterRepair = try String(contentsOf: failureEvalLog, encoding: .utf8)
      .split(separator: "\n")
    XCTAssertEqual(
      evalAfterRepair,
      ["fail-nonzero", "fail-nonzero", "fail-empty", "repaired"]
    )

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

  func testPanelFocusButtonRequiresCapabilityDetectedPaneAndLiveFollower()
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
          provider: owner.provider, providerSessionID: owner.providerSessionID, frameDetected: true)
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
            provider: owner.provider, providerSessionID: owner.providerSessionID,
            frameDetected: true)
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
    XCTAssertFalse(
      availableDeadFollowerView.showsPanelFocusButton(for: row),
      "a disconnected follower is not an available Frame destination")
    XCTAssertEqual(
      availableDeadFollowerView.panelFocusHelp(for: row, state: nil),
      String(localized: "Agent is disconnected"))

    var unknownPaneView = OverlayChannelStatusView(
      channels: [row], unavailable: false, palette: .dark, animates: false,
      hudStates: [
        "5": OverlayChannelHudProjection(
          open: false, loud: false, autosealDeadline: nil, followerAlive: true,
          provider: owner.provider, providerSessionID: owner.providerSessionID)
      ])
    unknownPaneView.archiveCandidates = ["5": owner]
    unknownPaneView.panelFocusAvailable = true
    unknownPaneView.onFocusAgentPanel = { _ in }
    XCTAssertFalse(unknownPaneView.showsPanelFocusButton(for: row), "unknown is not detected")

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

  // MARK: Terminal tab focus (UI-69)

  private func terminalStatusView(
    owner: OverlayConversationOwner, row: OverlayChannelDelivery,
    frameDetected: Bool?, terminalDetected: Bool?, panelFocusAvailable: Bool
  ) -> OverlayChannelStatusView {
    var view = OverlayChannelStatusView(
      channels: [row], unavailable: false, palette: .dark, animates: false,
      hudStates: [
        row.channel: OverlayChannelHudProjection(
          open: false, loud: false, autosealDeadline: nil, followerAlive: true,
          provider: owner.provider, providerSessionID: owner.providerSessionID,
          frameDetected: frameDetected, terminalDetected: terminalDetected)
      ])
    view.archiveCandidates = [row.channel: owner]
    view.panelFocusAvailable = panelFocusAvailable
    view.onFocusAgentPanel = { _ in }
    return view
  }

  func testTerminalHostedAgentShowsTheButtonWithoutVcFrame() throws {
    let owner = makeOwner()
    let row = OverlayChannelDelivery(
      channel: owner.channel, agent: owner.name, deliveryID: nil, stage: nil, isOpen: false)
    let view = terminalStatusView(
      owner: owner, row: row, frameDetected: false, terminalDetected: true,
      panelFocusAvailable: false)
    XCTAssertEqual(view.panelFocusTarget(for: row), .terminal)
    XCTAssertTrue(view.showsPanelFocusButton(for: row))
    XCTAssertFalse(view.hasNoTerminalTarget(row))
    XCTAssertEqual(
      view.panelFocusHelp(for: row, state: nil),
      String(localized: "Show the agent's terminal tab"))
    XCTAssertEqual(view.panelFocusSymbol(nil, target: .terminal), "terminal")
    XCTAssertEqual(view.panelFocusSymbol(nil, target: .frame), "macwindow.on.rectangle")

    let state = OverlayState.previewFormatted()
    armRoster(
      on: state, owner: owner, panelFocusAvailable: false, frameDetected: false,
      terminalDetected: true)
    XCTAssertTrue(state.canFocusAgentPanel(owner))
    XCTAssertEqual(
      state.agentPanelFocusHelp(owner), String(localized: "Show the agent's terminal tab"))
    var issuedRequests: [OverlayAgentPanelFocusRequest] = []
    state.requestAgentPanelFocusCommand = { request in
      issuedRequests.append(request)
      try await Task.sleep(for: .seconds(10))
    }
    state.focusAgentPanel(owner)
    let focusState = try XCTUnwrap(state.agentPanelFocusStates[owner.channel])
    XCTAssertEqual(focusState.phase, .requesting)
    XCTAssertEqual(focusState.target, .terminal)
    XCTAssertEqual(focusState.statusText, String(localized: "Opening the agent's terminal…"))
    // `vc-frame` disappearing does not cancel a terminal request.
    state.setAgentPanelFocusAvailable(true)
    state.setAgentPanelFocusAvailable(false)
    XCTAssertEqual(state.agentPanelFocusStates[owner.channel]?.target, .terminal)
  }

  func testAgentWithNeitherPaneNorTerminalHasNoButton() throws {
    let owner = makeOwner()
    let row = OverlayChannelDelivery(
      channel: owner.channel, agent: owner.name, deliveryID: nil, stage: nil, isOpen: false)
    let view = terminalStatusView(
      owner: owner, row: row, frameDetected: false, terminalDetected: false,
      panelFocusAvailable: true)
    XCTAssertNil(view.panelFocusTarget(for: row))
    XCTAssertFalse(view.showsPanelFocusButton(for: row))
    XCTAssertTrue(view.hasNoTerminalTarget(row))
    XCTAssertEqual(
      view.panelFocusHelp(for: row, state: nil),
      String(localized: "No terminal known for this agent"))

    let state = OverlayState.previewFormatted()
    armRoster(on: state, owner: owner, frameDetected: false, terminalDetected: false)
    var issuedRequests: [OverlayAgentPanelFocusRequest] = []
    state.requestAgentPanelFocusCommand = { issuedRequests.append($0) }
    state.focusAgentPanel(owner)
    XCTAssertFalse(state.canFocusAgentPanel(owner))
    XCTAssertNil(state.agentPanelFocusHelp(owner))
    XCTAssertTrue(issuedRequests.isEmpty)
  }

  func testAKnownPaneWinsOnlyWhileVcFrameCanReachIt() {
    let owner = makeOwner()
    let row = OverlayChannelDelivery(
      channel: owner.channel, agent: owner.name, deliveryID: nil, stage: nil, isOpen: false)
    let both = terminalStatusView(
      owner: owner, row: row, frameDetected: true, terminalDetected: true,
      panelFocusAvailable: true)
    XCTAssertEqual(both.panelFocusTarget(for: row), .frame)
    XCTAssertEqual(
      both.panelFocusHelp(for: row, state: nil), String(localized: "Focus agent panel in Frame"))
    let withoutVcFrame = terminalStatusView(
      owner: owner, row: row, frameDetected: true, terminalDetected: true,
      panelFocusAvailable: false)
    XCTAssertEqual(withoutVcFrame.panelFocusTarget(for: row), .terminal)
    XCTAssertTrue(withoutVcFrame.showsPanelFocusButton(for: row))
  }

  func testTerminalReceiptActivatesTheTerminalAppNotFrame() async throws {
    let state = OverlayState.previewFormatted()
    let owner = makeOwner()
    armRoster(
      on: state, owner: owner, panelFocusAvailable: false, frameDetected: false,
      terminalDetected: true)
    state.panelFocusSuccessHoldDuration = .seconds(30)
    state.requestAgentPanelFocusCommand = { _ in }
    var activations: [(Int32?, String?)] = []
    state.activateTerminalHost = { pid, bundleID in
      activations.append((pid, bundleID))
      return true
    }
    state.focusAgentPanel(owner)
    let requestID = try XCTUnwrap(state.agentPanelFocusStates[owner.channel]?.requestID)

    let row: [String: Any] = [
      "schema": "codescribe.agent-panel-focus-result.v1",
      "kind": "agent_panel_focus_result",
      "request_id": requestID,
      "provider": owner.provider,
      "provider_session_id": owner.providerSessionID,
      "lease_id": owner.leaseID,
      "status": "focused",
      "exit_code": 0,
      "elapsed_ms": 140,
      "emitted_at": "2026-10-21T08:30:00Z",
      "target": "iterm2",
      "frame": NSNull(),
      "terminal": [
        "host": "iterm2", "host_bundle_id": "com.googlecode.iterm2", "tty": "/dev/ttys005",
      ],
    ]
    let result = try XCTUnwrap(OverlayPanelFocusResult(row: row))
    XCTAssertEqual(result.target, "iterm2")
    XCTAssertNil(result.frame)
    XCTAssertEqual(
      result.terminal,
      AgentTerminalBinding(
        host: "iterm2", hostBundleID: "com.googlecode.iterm2", tty: "/dev/ttys005"))

    state.applyConversationSnapshot(
      OverlayChannelDeliverySnapshot(
        deliveries: [], conversations: [], panelFocusResults: [requestID: result]))
    for _ in 0..<30 { await Task.yield() }
    let focusState = try XCTUnwrap(state.agentPanelFocusStates[owner.channel])
    XCTAssertEqual(focusState.phase, .succeeded)
    XCTAssertEqual(focusState.statusText, String(localized: "Terminal tab shown"))
    XCTAssertEqual(activations.count, 1)
    XCTAssertNil(activations.first?.0)
    XCTAssertEqual(activations.first?.1, "com.googlecode.iterm2")
  }

  func testTerminalFailureStatusesDecodeAndReadAsTerminalReasons() throws {
    func result(_ status: String, target: String? = nil) throws -> OverlayPanelFocusResult {
      var row: [String: Any] = [
        "schema": "codescribe.agent-panel-focus-result.v1",
        "kind": "agent_panel_focus_result",
        "request_id": String(repeating: "a", count: 24),
        "provider": "codex", "provider_session_id": "session-5",
        "lease_id": String(repeating: "b", count: 32),
        "status": status, "emitted_at": "2026-10-21T08:31:00Z",
      ]
      if let target { row["target"] = target }
      return try XCTUnwrap(OverlayPanelFocusResult(row: row), status)
    }
    XCTAssertEqual(
      OverlayState.panelFocusStatusReason(
        for: try result("terminal_missing", target: "terminal"), requested: .terminal),
      String(localized: "Agent's terminal tab not found"))
    XCTAssertEqual(
      OverlayState.panelFocusStatusReason(
        for: try result("automation_denied", target: "iterm2"), requested: .terminal),
      String(
        localized:
          "Terminal control not allowed. Allow it in System Settings › Privacy & Security › Automation."
      ))
    XCTAssertEqual(
      OverlayState.panelFocusStatusReason(for: try result("no_frame"), requested: .terminal),
      String(localized: "No terminal known for this agent"))
    XCTAssertEqual(
      OverlayState.panelFocusStatusReason(for: try result("no_frame"), requested: .frame),
      String(localized: "Agent is not running in Frame"))
    XCTAssertEqual(
      OverlayState.panelFocusStatusReason(
        for: try result("error", target: "terminal"), requested: .terminal),
      String(localized: "Could not show the agent's terminal tab"))
    XCTAssertEqual(
      OverlayState.panelFocusStatusReason(for: try result("error"), requested: .frame),
      String(localized: "Could not focus panel in Frame"))
    XCTAssertNil(try result("focused", target: "kitty").target, "an unknown target is dropped")
  }

  // MARK: Codex thread in the Codex app (UI-70)

  private static let codexThreadLink = "codex://threads/c0ffee01-0000-7000-8000-000000000001"

  private func codexStatusView(
    owner: OverlayConversationOwner, row: OverlayChannelDelivery,
    followerAlive: Bool?, terminalDetected: Bool?, codexThreadLocal: Bool?
  ) -> OverlayChannelStatusView {
    var view = OverlayChannelStatusView(
      channels: [row], unavailable: false, palette: .dark, animates: false,
      hudStates: [
        row.channel: OverlayChannelHudProjection(
          open: false, loud: false, autosealDeadline: nil, followerAlive: followerAlive,
          provider: owner.provider, providerSessionID: owner.providerSessionID,
          frameDetected: nil, terminalDetected: terminalDetected,
          codexThreadUrl: Self.codexThreadLink, codexThreadLocal: codexThreadLocal)
      ])
    view.archiveCandidates = [row.channel: owner]
    view.panelFocusAvailable = true
    view.onFocusAgentPanel = { _ in }
    return view
  }

  func testACodexThreadComesAfterAPaneAndATerminalTab() {
    let local = OverlayChannelHudProjection(
      open: false, loud: false, autosealDeadline: nil, followerAlive: true,
      frameDetected: true, terminalDetected: true,
      codexThreadUrl: Self.codexThreadLink, codexThreadLocal: true)
    XCTAssertEqual(OverlayAgentPanelFocusTarget.resolve(local, frameFocusAvailable: true), .frame)
    var tabOnly = local
    tabOnly.frameDetected = false
    XCTAssertEqual(
      OverlayAgentPanelFocusTarget.resolve(tabOnly, frameFocusAvailable: true), .terminal)
    var threadOnly = tabOnly
    threadOnly.terminalDetected = nil
    XCTAssertEqual(
      OverlayAgentPanelFocusTarget.resolve(threadOnly, frameFocusAvailable: true), .codexThread)
    XCTAssertFalse(
      OverlayAgentPanelFocusTarget.isCodexCloudThread(threadOnly, frameFocusAvailable: true))

    var cloud = threadOnly
    cloud.codexThreadLocal = false
    XCTAssertNil(OverlayAgentPanelFocusTarget.resolve(cloud, frameFocusAvailable: true))
    XCTAssertTrue(
      OverlayAgentPanelFocusTarget.isCodexCloudThread(cloud, frameFocusAvailable: true))
    var cloudInTerminal = cloud
    cloudInTerminal.terminalDetected = true
    XCTAssertEqual(
      OverlayAgentPanelFocusTarget.resolve(cloudInTerminal, frameFocusAvailable: true), .terminal)
    XCTAssertFalse(
      OverlayAgentPanelFocusTarget.isCodexCloudThread(cloudInTerminal, frameFocusAvailable: true))
    XCTAssertFalse(OverlayAgentPanelFocusTarget.codexThread.needsLiveFollower)
    XCTAssertTrue(OverlayAgentPanelFocusTarget.terminal.needsLiveFollower)
  }

  func testADisconnectedCodexFollowerStillOpensItsLocalThread() throws {
    let owner = makeOwner(provider: "codex")
    let row = OverlayChannelDelivery(
      channel: owner.channel, agent: owner.name, deliveryID: nil, stage: nil, isOpen: false)
    let view = codexStatusView(
      owner: owner, row: row, followerAlive: false, terminalDetected: nil,
      codexThreadLocal: true)
    XCTAssertTrue(view.hasDeadFollower(row))
    XCTAssertEqual(view.panelFocusTarget(for: row), .codexThread)
    XCTAssertTrue(view.showsPanelFocusButton(for: row))
    XCTAssertFalse(view.panelFocusUnreachable(row))
    XCTAssertEqual(
      view.panelFocusHelp(for: row, state: nil),
      String(localized: "Show the agent's Codex thread"))
    XCTAssertEqual(
      view.panelFocusSymbol(nil, target: .codexThread), "bubble.left.and.text.bubble.right")

    let state = OverlayState.previewFormatted()
    armRoster(
      on: state, owner: owner, panelFocusAvailable: false, frameDetected: nil,
      followerAlive: false, codexThreadUrl: Self.codexThreadLink, codexThreadLocal: true)
    state.panelFocusSuccessHoldDuration = .seconds(30)
    XCTAssertTrue(state.canFocusAgentPanel(owner))
    XCTAssertEqual(
      state.agentPanelFocusHelp(owner), String(localized: "Show the agent's Codex thread"))
    var issuedRequests: [OverlayAgentPanelFocusRequest] = []
    state.requestAgentPanelFocusCommand = { issuedRequests.append($0) }
    var openedLinks: [URL] = []
    state.openCodexThreadLink = { url in
      openedLinks.append(url)
      return true
    }
    state.focusAgentPanel(owner)
    XCTAssertEqual(openedLinks, [URL(string: Self.codexThreadLink)!])
    XCTAssertTrue(issuedRequests.isEmpty, "the app opens the link; no follower is asked")
    let focusState = try XCTUnwrap(state.agentPanelFocusStates[owner.channel])
    XCTAssertEqual(focusState.phase, .succeeded)
    XCTAssertEqual(focusState.target, .codexThread)
    XCTAssertEqual(focusState.statusText, String(localized: "Codex thread opened"))
  }

  func testACodexThreadTheSystemCannotOpenReportsAFailure() throws {
    let owner = makeOwner(provider: "codex")
    let state = OverlayState.previewFormatted()
    armRoster(
      on: state, owner: owner, panelFocusAvailable: false, frameDetected: nil,
      followerAlive: true, codexThreadUrl: Self.codexThreadLink, codexThreadLocal: true)
    state.openCodexThreadLink = { _ in false }
    state.focusAgentPanel(owner)
    let focusState = try XCTUnwrap(state.agentPanelFocusStates[owner.channel])
    XCTAssertEqual(focusState.phase, .failed)
    XCTAssertEqual(
      focusState.statusText, String(localized: "Could not open the agent's Codex thread"))
  }

  func testACodexCloudThreadIsGreyedAndNamesWhereItLives() throws {
    let owner = makeOwner(provider: "codex")
    let row = OverlayChannelDelivery(
      channel: owner.channel, agent: owner.name, deliveryID: nil, stage: nil, isOpen: false)
    for followerAlive in [true, false] {
      let view = codexStatusView(
        owner: owner, row: row, followerAlive: followerAlive, terminalDetected: false,
        codexThreadLocal: false)
      XCTAssertNil(view.panelFocusTarget(for: row))
      XCTAssertTrue(view.hasCodexCloudThread(row))
      XCTAssertTrue(view.showsPanelFocusButton(for: row), "the button rests greyed")
      XCTAssertTrue(view.panelFocusUnreachable(row))
      XCTAssertEqual(
        view.panelFocusHelp(for: row, state: nil),
        String(localized: "Codex Cloud thread: open it in ChatGPT"))
    }

    let state = OverlayState.previewFormatted()
    armRoster(
      on: state, owner: owner, panelFocusAvailable: false, frameDetected: nil,
      followerAlive: true, codexThreadUrl: Self.codexThreadLink, codexThreadLocal: false)
    var openedLinks: [URL] = []
    state.openCodexThreadLink = { url in
      openedLinks.append(url)
      return true
    }
    var issuedRequests: [OverlayAgentPanelFocusRequest] = []
    state.requestAgentPanelFocusCommand = { issuedRequests.append($0) }
    XCTAssertFalse(state.canFocusAgentPanel(owner))
    state.focusAgentPanel(owner)
    XCTAssertTrue(openedLinks.isEmpty, "a cloud link would end in the app's error")
    XCTAssertTrue(issuedRequests.isEmpty)
    XCTAssertNil(state.agentPanelFocusStates[owner.channel])
  }
}
