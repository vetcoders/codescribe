import Foundation
import XCTest

@testable import Codescribe

@MainActor
final class OverlayAgentPanelFocusTests: XCTestCase {
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

  private func armRoster(on state: OverlayState, owner: OverlayConversationOwner) {
    state.applyChannelRoster([
      CsChannelRosterState(
        channel: "0", audience: "All", open: false, loud: false, autosealDeadlineUnixMs: nil,
        followerAlive: nil, provider: nil, providerSessionId: nil, unreadSinceUnixMs: nil),
      CsChannelRosterState(
        channel: owner.channel, audience: owner.name, open: false, loud: false,
        autosealDeadlineUnixMs: nil, followerAlive: true, provider: owner.provider,
        providerSessionId: owner.providerSessionID, unreadSinceUnixMs: nil),
    ])
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
      id: "conv-5", channel: "5", name: "stanislaw", owner: owner, messages: [], replyIDs: [])
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
}
