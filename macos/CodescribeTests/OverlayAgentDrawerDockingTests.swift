import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Falsifiers for the agents-drawer cut (Founder, 2026-10-16; mockup
/// `szuflada-agentow.html`): the roster docks as a right-hand column on wide
/// windows and covers the chat on narrow ones (Founder, 2026-10-30).
/// Visibility is explicit; the bubble cap follows ⌘+ text scale.
@MainActor
final class OverlayAgentDrawerDockingTests: XCTestCase {

  // MARK: - Fixtures

  private func overlaySource(_ name: String) throws -> String {
    let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .deletingLastPathComponent()
    return try String(
      contentsOf: root.appendingPathComponent("Codescribe/Screens/Overlay/\(name)"),
      encoding: .utf8)
  }

  private func section(of source: String, from: String, to: String) throws -> String {
    let start = try XCTUnwrap(source.range(of: from), "missing section start: \(from)")
    let end = try XCTUnwrap(
      source.range(of: to, range: start.upperBound..<source.endIndex),
      "missing section end: \(to)")
    return String(source[start.lowerBound..<end.lowerBound])
  }

  private func ownerConversation(
    _ name: String, channel: String, session: String, lease: String, lastReply: String? = nil
  ) throws -> OverlayConversation {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": session,
        "lease_id": String(repeating: lease, count: 32), "channel": channel, "name": name,
      ]))
    var messages: [OverlayConversationMessage] = []
    if let lastReply {
      messages.append(
        OverlayConversationMessage(
          id: "reply:\(name)-1", kind: .reply, text: lastReply, order: 1,
          emittedAt: "2026-10-16T14:53:00Z", owner: owner, recipients: [],
          deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: ""))
    }
    return OverlayConversation(
      id: owner.id, channel: channel, name: name, owner: owner, messages: messages)
  }

  private func hud(session: String, frameDetected: Bool? = nil) -> OverlayChannelHudProjection {
    OverlayChannelHudProjection(
      open: false, loud: false, autosealDeadline: nil, followerAlive: true,
      provider: "codex", providerSessionID: session, frameDetected: frameDetected)
  }

  private func rosterFixture() throws -> (
    broadcast: OverlayConversation, lena: OverlayConversation, astra: OverlayConversation
  ) {
    let broadcast = OverlayConversation(
      id: "dock-all", channel: "0", name: "All", owner: nil, messages: [])
    let lena = try ownerConversation(
      "lena", channel: "2", session: "lena-session", lease: "a", lastReply: "PR is green")
    let astra = try ownerConversation(
      "astra", channel: "3", session: "astra-session", lease: "b", lastReply: "on it")
    return (broadcast, lena, astra)
  }

  // MARK: - Threshold (deliverables 1 and 3)

  func testBubbleCapScalesWithTextAndYieldsToTheWindow() {
    XCTAssertEqual(
      OverlayConversationView.bubbleCap(contentWidth: 2000, textScale: 1.0), 660,
      accuracy: 0.001)
    XCTAssertEqual(
      OverlayConversationView.bubbleCap(contentWidth: 2000, textScale: 1.4), 924,
      accuracy: 0.001, "⌘+ grows the cap with the letters")
    XCTAssertEqual(
      OverlayConversationView.bubbleCap(contentWidth: 700, textScale: 1.0), (700 - 40) * 0.92,
      accuracy: 0.001)
    XCTAssertEqual(
      OverlayConversationView.bubbleCap(contentWidth: 700, textScale: 1.4), (700 - 40) * 0.92,
      accuracy: 0.001, "the window bound wins regardless of scale")
  }

  // MARK: - Narrow list (below the threshold, 2026-10-19)

  func testNarrowListCoversTheWindowAndSelectionReturnsToTheConversation() throws {
    let (broadcast, lena, astra) = try rosterFixture()
    let state = OverlayState.previewFormatted()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena, astra]))
    state.selectConversation(lena.id)
    let recorder = OverlayDrawerFrameRecorder()
    let host = NSHostingView(
      rootView: DictationOverlayView(state: state)
        .onPreferenceChange(OverlayDrawerFramePreferenceKey.self) { recorder.frame = $0 }
        .frame(width: 380, height: 500))
    host.sizingOptions = []
    let panel = NSPanel(
      contentRect: CGRect(x: -10000, y: -10000, width: 380, height: 500),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.contentView = host
    BackgroundTestWindow.present(panel)
    defer { panel.orderOut(nil) }
    settle(host)
    XCTAssertFalse(state.agentDrawerDocked)
    XCTAssertEqual(recorder.frame, .zero)
    state.showAgentMonitor()
    settle(host)
    XCTAssertEqual(recorder.frame.minX, 0, accuracy: 1)
    XCTAssertEqual(recorder.frame.width, 380, accuracy: 1)
    XCTAssertEqual(panel.frame.width, 380, accuracy: 1, "opening the panel never widens the window")
    state.chooseConversation(astra.id)
    settle(host)
    XCTAssertEqual(state.selectedConversationID, astra.id)
    XCTAssertFalse(state.showsAgentMonitor)
    XCTAssertEqual(recorder.frame, .zero)
  }

  func testPanelVisibilitySurvivesGeometryAndRevealKeepsRouteAndDraft() throws {
    let (_, lena, _) = try rosterFixture()
    let state = OverlayState()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [lena]))
    state.selectConversation(lena.id)
    state.conversationDrafts[lena.id] = "draft survives panel changes"
    let generation = state.captureGeneration
    state.hideAgentSidebar()
    for docked in [true, false, true] {
      state.setAgentDrawerDocked(docked)
      XCTAssertFalse(state.showsAgentMonitor)
    }
    state.showAgentMonitor()
    XCTAssertTrue(state.showsAgentMonitor)
    XCTAssertEqual(state.selectedConversationID, lena.id)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertEqual(state.conversationDrafts[lena.id], "draft survives panel changes")
  }

  // MARK: - Docked column (deliverables 1 and 4)

  func testGeometryReportingDoesNotChangeExplicitPaneStateOrDraft() throws {
    let (_, lena, _) = try rosterFixture()
    let state = OverlayState()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [lena]))
    state.selectConversation(lena.id)
    state.conversationDrafts[lena.id] = "half-typed draft"
    state.showAgentMonitor()
    let shown = state.showsAgentMonitor
    let focus = state.conversationFocusRevision
    for docked in [true, false, true] {
      state.setAgentDrawerDocked(docked)
      XCTAssertEqual(state.showsAgentMonitor, shown)
      XCTAssertEqual(state.selectedConversationID, lena.id)
      XCTAssertEqual(state.conversationDrafts[lena.id], "half-typed draft")
      XCTAssertEqual(state.conversationFocusRevision, focus)
    }
  }

  func testDockedNamePillFocusesTheSelectedRowInsteadOfOpening() throws {
    let (broadcast, lena, _) = try rosterFixture()
    let state = OverlayState()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
    state.selectConversation(lena.id)
    state.setAgentDrawerDocked(true)
    let focus = state.agentDrawerFocusRevision

    state.showAgentMonitor()

    XCTAssertTrue(state.showsAgentMonitor, "the list has one explicit visible state")
    XCTAssertEqual(
      state.agentDrawerFocusRevision, focus + 1, "docked: the click focuses the current row")
  }

  /// Maciej, 2026-10-19 on div0 (keyboard navigation on): the system focus
  /// ring sat on "0 · All" while the selection bar and the composer were on
  /// another agent. Keyboard focus in the roster is a SwiftUI `FocusState`
  /// that an XCTest mount does not drive, so the wiring is pinned in source.
  func testRosterKeyboardFocusFollowsTheSelectedRow() throws {
    let source = try overlaySource("OverlayChannelStatusView.swift")
    let body = try section(
      of: source, from: "private struct OverlayAgentMonitorBody", to: "\n}\n")
    XCTAssertTrue(
      body.contains(".defaultFocus($rowFocus, focusedRowID)"),
      "focus entering the roster lands on the open conversation, not the first row")
    XCTAssertTrue(
      body.contains(".onChange(of: focusedRowID)"),
      "a focused row follows a selection made with the pointer")
    // `monitorBody` reads the installed text scale, then builds the column here.
    let wiring = try section(of: source, from: "private func scaledMonitorBody(", to: "\n  }\n")
    XCTAssertTrue(
      wiring.contains("focusedRowID: selectedConversationID"),
      "the followed row is the selected conversation")
  }

  func testDockedDrawerReservesTheScaledColumnBesideTheConversation() throws {
    for (width, scale, drawerWidth) in [(1200.0, 1.0, 300.0), (1400.0, 1.4, 420.0)]
      as [(CGFloat, CGFloat, CGFloat)]
    {
      let (broadcast, lena, _) = try rosterFixture()
      let state = OverlayState.previewFormatted()
      state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
      state.selectConversation(lena.id)
      state.showAgentMonitor()
      let recorder = OverlayDrawerFrameRecorder()
      let host = NSHostingView(
        rootView:
          DictationOverlayView(state: state)
          .environment(\.csTextScale, scale)
          .onPreferenceChange(OverlayDrawerFramePreferenceKey.self) { recorder.frame = $0 }
          .frame(width: width, height: 500))
      host.sizingOptions = []
      let panel = NSPanel(
        contentRect: CGRect(x: 0, y: 0, width: width, height: 500),
        styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
      panel.contentView = host
      BackgroundTestWindow.present(panel)
      host.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.1))
      host.layoutSubtreeIfNeeded()

      let frame = recorder.frame
      XCTAssertEqual(
        frame.width, drawerWidth, accuracy: 1,
        "width \(width) scale \(scale): the column is 300 × scale")
      XCTAssertEqual(
        frame.minX, width - drawerWidth, accuracy: 1,
        "the column owns the right edge; the conversation keeps width − 300 × scale")
      XCTAssertEqual(
        frame.maxY, 500, accuracy: 1, "the column runs the full height under the header")
      XCTAssertEqual(
        state.selectedConversationID, lena.id, "layout never disturbs the selection")
      panel.orderOut(nil)
    }

    // Narrow: no docked column at all; the list appears only when asked for.
    let (broadcast, lena, _) = try rosterFixture()
    let state = OverlayState.previewFormatted()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
    state.selectConversation(lena.id)
    let recorder = OverlayDrawerFrameRecorder()
    let host = NSHostingView(
      rootView:
        DictationOverlayView(state: state)
        .onPreferenceChange(OverlayDrawerFramePreferenceKey.self) { recorder.frame = $0 }
        .frame(width: 700, height: 500))
    host.sizingOptions = []
    let panel = NSPanel(
      contentRect: CGRect(x: 0, y: 0, width: 700, height: 500),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.contentView = host
    BackgroundTestWindow.present(panel)
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    host.layoutSubtreeIfNeeded()
    XCTAssertEqual(recorder.frame, .zero, "a closed panel reserves no column")
    panel.orderOut(nil)
  }

  func testWideTranscriptionKeepsItsWidthUntilTheRosterIsAskedFor() throws {
    let (broadcast, lena, _) = try rosterFixture()
    let state = OverlayState.previewFormatted()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
    XCTAssertNil(state.selectedConversationID, "the transcription surface has no conversation")
    let recorder = OverlayDrawerFrameRecorder()
    let host = NSHostingView(
      rootView:
        DictationOverlayView(state: state)
        .onPreferenceChange(OverlayDrawerFramePreferenceKey.self) { recorder.frame = $0 }
        .frame(width: 1200, height: 500))
    host.sizingOptions = []
    let panel = NSPanel(
      contentRect: CGRect(x: 0, y: 0, width: 1200, height: 500),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.contentView = host
    BackgroundTestWindow.present(panel)
    defer { panel.orderOut(nil) }
    func settle() {
      host.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.1))
      host.layoutSubtreeIfNeeded()
    }
    settle()
    XCTAssertTrue(state.agentDrawerDocked, "1200 wide at 1.0 is docked")
    XCTAssertEqual(recorder.frame, .zero, "no column eats the transcription's width unasked")

    state.showAgentMonitor()
    settle()
    XCTAssertTrue(state.showsAgentMonitor, "docked transcription: the button opens the column")
    XCTAssertEqual(
      recorder.frame.width, 1200, accuracy: 1, "without a conversation the list fills the window")

    state.hideAgentSidebar()
    settle()
    XCTAssertEqual(recorder.frame, .zero, "Esc gives the transcription its width back")

    state.selectConversation(lena.id)
    settle()
    XCTAssertFalse(state.showsAgentMonitor)
    XCTAssertEqual(recorder.frame, .zero, "selecting a conversation preserves the explicit fold")
  }

  func testWideningUnderTheOpenListDocksItAsTheColumn() throws {
    let (broadcast, lena, _) = try rosterFixture()
    let state = OverlayState.previewFormatted()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
    state.selectConversation(lena.id)
    let recorder = OverlayDrawerFrameRecorder()
    // No fixed frame: the panel's content size is the window the layout reads.
    let host = NSHostingView(
      rootView:
        DictationOverlayView(state: state)
        .onPreferenceChange(OverlayDrawerFramePreferenceKey.self) { recorder.frame = $0 })
    host.sizingOptions = []
    let panel = NSPanel(
      contentRect: CGRect(x: 0, y: 0, width: 380, height: 500),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.contentView = host
    BackgroundTestWindow.present(panel)
    defer { panel.orderOut(nil) }
    func settle() {
      host.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.1))
      host.layoutSubtreeIfNeeded()
    }
    settle()
    state.showAgentMonitor()
    settle()
    XCTAssertEqual(recorder.frame.width, 380, accuracy: 1, "the narrow panel covers the chat")

    // Docking engages while the list is open on the transcription: the list
    // becomes the column instead of vanishing.
    panel.setContentSize(NSSize(width: 1200, height: 500))
    settle()
    XCTAssertTrue(state.agentDrawerDocked)
    XCTAssertTrue(state.showsAgentMonitor)
    XCTAssertEqual(recorder.frame.width, 300, accuracy: 1, "the open list docks as the column")
    XCTAssertEqual(recorder.frame.maxX, 1200, accuracy: 1)

    state.chooseConversation(lena.id)
    settle()
    XCTAssertTrue(state.showsAgentMonitor, "wide selection keeps the list visible")
    XCTAssertEqual(recorder.frame.width, 300, accuracy: 1, "beside a conversation the column stays")

    state.showTranscription()
    settle()
    XCTAssertNil(state.selectedConversationID)
    XCTAssertEqual(recorder.frame, .zero, "back on the transcription the column is gone")
  }

  // MARK: - The keyboard follows the list (2026-10-19)

  func testTheListTakesTheKeyboardFromTheComposerAndGivesItBack() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let (broadcast, lena, astra) = try rosterFixture()
    let state = OverlayState.previewFormatted()
    state.applyChannelRoster(
      [broadcast, lena, astra].map {
        CsChannelRosterState(
          channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
          providerSessionId: $0.owner?.providerSessionID, open: false, loud: false,
          autosealDeadlineUnixMs: nil, followerAlive: $0.owner == nil ? nil : true,
          unreadSinceUnixMs: nil)
      })
    state.applyConversationSnapshot(
      .init(deliveries: [], conversations: [broadcast, lena, astra]))
    state.selectConversation(lena.id)
    let (panel, host) = keyboardPanel(DictationOverlayView(state: state), width: 380)
    let other = NSWindow(
      contentRect: CGRect(x: 0, y: 0, width: 200, height: 100), styleMask: [.titled],
      backing: .buffered, defer: false)
    other.isReleasedWhenClosed = false
    defer {
      panel.makeFirstResponder(nil)
      panel.releaseKeyAfterTranscript()
      panel.close()
      other.close()
    }
    let editor = try XCTUnwrap(composerEditor(in: host))
    panel.takeKeyForTranscript()
    XCTAssertTrue(panel.makeFirstResponder(editor))
    XCTAssertTrue(panel.isKeyWindow)

    let rosterFocus = state.agentDrawerFocusRevision
    state.showAgentMonitor()
    settle(host)
    XCTAssertFalse(
      panel.firstResponder === editor, "the explicitly folded composer releases the keyboard")
    XCTAssertGreaterThan(
      state.agentDrawerFocusRevision, rosterFocus, "the list's selected row is asked to take it")
    let key = try XCTUnwrap(
      NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
        windowNumber: panel.windowNumber, context: nil, characters: "x",
        charactersIgnoringModifiers: "x", isARepeat: false, keyCode: 7))
    panel.sendEvent(key)
    XCTAssertEqual(editor.string, "", "nothing is typed into a field nobody can see")

    state.chooseConversation(astra.id)
    settle(host)
    XCTAssertTrue(
      panel.firstResponder === editor,
      "choosing a row reveals the conversation and restores its keyboard")

    state.showAgentMonitor()
    settle(host)
    state.hideAgentSidebar()
    settle(host)
    XCTAssertTrue(panel.firstResponder === editor, "so does closing the list")

    // The keyboard went to another window: the list opens and closes without
    // taking it back.
    try BackgroundTestWindow.takeKeyboard(other)
    settle(host)
    XCTAssertFalse(panel.isKeyWindow)
    state.showAgentMonitor()
    settle(host)
    state.hideAgentSidebar()
    settle(host)
    XCTAssertFalse(panel.isKeyWindow, "a panel without the keyboard takes nothing")
    XCTAssertFalse(panel.firstResponder === editor)
  }

  func testTheRosterOverTheWholeWindowTakesTheKeyboardToo() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let defaults = try XCTUnwrap(UserDefaults(suiteName: UUID().uuidString))
    defaults.set(480.0, forKey: "Overlay.agentDrawerWidth.v1")
    let (broadcast, lena, _) = try rosterFixture()
    let state = OverlayState.previewFormatted()
    state.applyChannelRoster(
      [broadcast, lena].map {
        CsChannelRosterState(
          channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
          providerSessionId: $0.owner?.providerSessionID, open: false, loud: false,
          autosealDeadlineUnixMs: nil, followerAlive: $0.owner == nil ? nil : true,
          unreadSinceUnixMs: nil)
      })
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
    state.selectConversation(lena.id)
    let (panel, host) = keyboardPanel(
      DictationOverlayView(state: state).defaultAppStorage(defaults), width: 1200)
    defer {
      panel.makeFirstResponder(nil)
      panel.releaseKeyAfterTranscript()
      panel.close()
    }
    let editor = try XCTUnwrap(composerEditor(in: host))
    panel.takeKeyForTranscript()
    XCTAssertTrue(panel.makeFirstResponder(editor))

    // The covering panel takes focus; closing it restores the same editor.
    let rosterFocus = state.agentDrawerFocusRevision
    state.showAgentMonitor()
    panel.setContentSize(NSSize(width: 380, height: 500))
    settle(host)
    XCTAssertFalse(state.agentDrawerDocked)
    XCTAssertFalse(panel.firstResponder === editor, "the covered composer lets the keyboard go")
    XCTAssertGreaterThan(state.agentDrawerFocusRevision, rosterFocus)

    state.hideAgentSidebar()
    panel.setContentSize(NSSize(width: 1200, height: 500))
    settle(host)
    XCTAssertTrue(panel.firstResponder === editor, "the explicit reveal restores the keyboard")
  }

  /// A real overlay panel that can take the keyboard, as the composer tests use.
  private func keyboardPanel(_ root: some View, width: CGFloat) -> (FloatingOverlayPanel, NSView) {
    let host = NSHostingView(rootView: root)
    host.sizingOptions = []
    let panel = FloatingOverlayPanel(
      contentRect: CGRect(x: 0, y: 0, width: width, height: 500),
      styleMask: [.borderless, .nonactivatingPanel, .resizable], backing: .buffered,
      defer: false)
    panel.isReleasedWhenClosed = false
    panel.delegate = panel
    panel.becomesKeyOnlyIfNeeded = true
    panel.contentView = host
    BackgroundTestWindow.present(panel)
    settle(host)
    return (panel, host)
  }

  private func settle(_ host: NSView) {
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    host.layoutSubtreeIfNeeded()
  }

  private func composerEditor(in view: NSView) -> NSTextView? {
    if let text = view as? NSTextView, text.isEditable,
      String(describing: type(of: text)).contains("MessageTextView")
    {
      return text
    }
    for subview in view.subviews {
      if let text = composerEditor(in: subview) { return text }
    }
    return nil
  }

  // MARK: - Corner buttons and the rail (Maciej, 2026-10-19)

  func testPanelVisibilityIsRememberedAndPreviousPaneChoicesMigrateOnce() throws {
    for previous in ["split", "agents", "rail"] {
      let suite = UUID().uuidString
      let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
      defer { defaults.removePersistentDomain(forName: suite) }
      defaults.set(previous, forKey: "Overlay.agentsLayout.v1")
      let state = OverlayState()
      state.restoreAgentSidebar(defaults: defaults)
      XCTAssertEqual(state.showsAgentMonitor, previous != "rail")
      XCTAssertNil(defaults.object(forKey: "Overlay.agentsLayout.v1"))
      state.hideAgentSidebar()
      let relaunched = OverlayState()
      relaunched.restoreAgentSidebar(defaults: defaults)
      XCTAssertFalse(relaunched.showsAgentMonitor)
    }
    let suite = UUID().uuidString
    let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
    defer { defaults.removePersistentDomain(forName: suite) }
    defaults.set(100_000.0, forKey: OverlayAgentDrawerLayout.widthDefaultsKey)
    let state = OverlayState()
    state.restoreAgentSidebar(defaults: defaults)
    XCTAssertTrue(state.showsAgentMonitor)
    XCTAssertEqual(defaults.double(forKey: OverlayAgentDrawerLayout.widthDefaultsKey), 300)
    defaults.set(false, forKey: OverlayState.agentSidebarVisibleDefaultsKey)
    defaults.set("agents", forKey: "Overlay.agentsLayout.v1")
    state.restoreAgentSidebar(defaults: defaults)
    XCTAssertFalse(state.showsAgentMonitor, "the adopted preference owns all later launches")
  }

  /// The overlay over a suite of its own defaults, reporting the column's and
  /// the rail's frames.

  /// The numbered pills must take real mouseDown/mouseUp through the panel —
  /// an AXPress proves the action exists, not that a click reaches it past
  /// the panel's own resize band and window-drag routing. Channel 0 leads:
  /// it is the first pill under the disclosure control.

  // MARK: - Row model and broadcast regression (deliverable 1, 878667f9f)

  func testDockedRowsKeepProviderModelVisibleAlongsideTimePreviewAndBadge() throws {
    let source = try overlaySource("OverlayChannelStatusView.swift")
    let row = try section(
      of: source, from: "func channelRow(", to: "private func conversationRow(")
    let visual = try overlaySource("OverlayAgentRosterLabel.swift")
    XCTAssertTrue(visual.contains("lineLimit(2)"), "rows carry a two-line message preview")
    XCTAssertTrue(row.contains("drawerPreview("), "rows render the last message preview")
    XCTAssertTrue(
      visual.contains("overlay-agent-descriptor-"),
      "wide rows must show the same provider/model identity as narrow pills")
    // Astra's revised full-view contract (2026-10-22): the row's descriptor
    // and status carry NO metadata hint — the information is inline. Action
    // labels (like the archive control's help) keep their hints.
    XCTAssertFalse(
      row.contains(".overlayFastHint(tooltip"),
      "a metadata fast hint would duplicate the inline descriptor/status")
    XCTAssertTrue(
      visual.contains("overlay-channel-delivery-"), "the status dot keeps the delivery identifier")
    XCTAssertTrue(row.contains("overlay-agent-row-"), "row identifier kept")
  }

  func testCardMenuKeepsExactlyFourSecondaryActionsAndTheirFrozenOwner() throws {
    let (_, lena, _) = try rosterFixture()
    let owner = try XCTUnwrap(lena.owner)
    let row = OverlayChannelDelivery(
      channel: "2", agent: "lena", deliveryID: "card-menu", stage: .received, isOpen: false)
    var focusOwners: [OverlayConversationOwner] = []
    var archiveOwners: [OverlayConversationOwner] = []
    var primaryCalls = 0
    var view = OverlayChannelStatusView(
      channels: [row], unavailable: false, palette: .dark, animates: false,
      hudStates: ["2": hud(session: owner.providerSessionID, frameDetected: true)],
      onToggleChannel: { _ in primaryCalls += 1 }, conversations: [lena],
      onSelectConversation: { _ in primaryCalls += 1 })
    view.mutedChannels = ["2": false]
    view.onTogglePlayback = { _ in primaryCalls += 1 }
    view.onShowAgentInMonitor = { _ in primaryCalls += 1 }
    view.archiveCandidates = ["2": owner]
    view.onArchiveAgent = { archiveOwners.append($0) }
    view.onFocusAgentPanel = { focusOwners.append($0) }
    view.panelFocusAvailable = true
    let actions = try XCTUnwrap(view.agentPillMenuActions(for: "2", showsFocusState: true))
    let focus = try XCTUnwrap(actions.focus)
    let archive = try XCTUnwrap(actions.archive)
    XCTAssertTrue(focus.enabled)
    XCTAssertTrue(archive.enabled)
    XCTAssertTrue(view.cardAgentMenu(for: "2").secondaryActionsOnly)
    XCTAssertTrue(view.cardAgentMenu(for: "2", showsButton: true).secondaryActionsOnly)
    XCTAssertFalse(view.agentPillMenu(for: "2").secondaryActionsOnly)

    let host = NSHostingView(rootView: view.channelRow(row).frame(width: 340, height: 140))
    host.frame = NSRect(x: 0, y: 0, width: 340, height: 140)
    let window = NSWindow(
      contentRect: host.frame, styleMask: [.borderless], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    BackgroundTestWindow.present(window)
    defer { window.orderOut(nil) }
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.08))
    host.layoutSubtreeIfNeeded()
    var found: NSMenu?
    scan: for x in stride(from: CGFloat(6), to: host.bounds.width, by: 12) {
      for y in stride(from: CGFloat(6), to: host.bounds.height, by: 12) {
        let point = NSPoint(x: x, y: y)
        let event = try XCTUnwrap(
          NSEvent.mouseEvent(
            with: .rightMouseDown, location: host.convert(point, to: nil), modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
            context: nil, eventNumber: 9_400, clickCount: 1, pressure: 1))
        var hit = host.hitTest(point)
        while let current = hit {
          if let menu = current.menu(for: event) {
            found = menu
            break scan
          }
          hit = current.superview
        }
      }
    }
    let menu = try XCTUnwrap(found, "the real card must resolve a menu; absence is not a pass")
    menu.delegate?.menuNeedsUpdate?(menu)
    let items = menu.items.filter { !$0.isSeparatorItem }
    XCTAssertEqual(items.count, 4)
    XCTAssertEqual(
      Set(items.map(\.title)),
      Set([
        String(localized: "Channel 0"), String(localized: "Do not disturb"), focus.title,
        archive.title,
      ]))
    let focusIndex = try XCTUnwrap(menu.items.firstIndex { $0.title == focus.title })
    let archiveIndex = try XCTUnwrap(menu.items.firstIndex { $0.title == archive.title })
    menu.performActionForItem(at: focusIndex)
    menu.performActionForItem(at: archiveIndex)
    XCTAssertEqual(focusOwners, [owner])
    XCTAssertEqual(archiveOwners, [owner])
    XCTAssertEqual(primaryCalls, 0, "secondary actions cannot select, write, record or mute")
  }

  /// The 2026-10-29 card contract supersedes the older no-menu roster:
  /// More and right-click expose reception policy, workspace focus and archive.
  /// Write, microphone, playback and Show in Agents stay out of card menus.
  /// Both the permitted card menu and the full pill sentinel are required
  /// positive controls; inline metadata still has no system tooltip.
  func testFullRosterResolvesNoBigAgentMenuAndCarriesNoSystemTooltips() throws {
    let (broadcast, lena, astra) = try rosterFixture()
    let channels = [
      OverlayChannelDelivery(
        channel: "2", agent: "lena", deliveryID: "receipt-2", stage: .received, isOpen: false),
      OverlayChannelDelivery(
        channel: "3", agent: "astra", deliveryID: "receipt-3", stage: .received, isOpen: false),
    ]
    // The roster is fully armed (Astra on f105d23f4): every action callback
    // and owner fixture is present, so the absence of the big menu is proven
    // against the surface that could actually compose it.
    var view = OverlayChannelStatusView(
      channels: channels, unavailable: false, palette: .dark, animates: false,
      hudStates: ["2": hud(session: "lena-session"), "3": hud(session: "astra-session")],
      agentDescriptors: ["2": "codex · gpt-6", "3": "codex · gpt-6"],
      onToggleChannel: { _ in },
      conversations: [broadcast, lena, astra], selectedConversationID: broadcast.id,
      onSelectConversation: { _ in })
    view.mutedChannels = ["2": false, "3": false]
    view.onTogglePlayback = { _ in }
    view.onShowAgentInMonitor = { _ in }
    view.archiveCandidates = [
      "2": try XCTUnwrap(lena.owner), "3": try XCTUnwrap(astra.owner),
    ]
    view.onArchiveAgent = { _ in }
    view.onFocusAgentPanel = { _ in }
    view.panelFocusAvailable = true
    let host = NSHostingView(
      rootView: view.monitorBody(maximumHeight: 1600).environment(\.csTextScale, 1)
        .frame(width: 360))
    host.frame = NSRect(x: 0, y: 0, width: 360, height: 900)
    let window = NSWindow(
      contentRect: host.frame, styleMask: [.borderless], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    defer { window.orderOut(nil) }
    window.contentView = host
    BackgroundTestWindow.present(window)
    host.layoutSubtreeIfNeeded()

    // The same localization keys the production menu uses, so the markers
    // survive a Polish (or any) runtime language (Astra, T4).
    let forbidden = [
      String(localized: "Write / Forward…"), String(localized: "Speak to agent"),
      String(localized: "Stop speaking to agent"), String(localized: "Mute agent replies"),
      String(localized: "Unmute agent replies"), String(localized: "Show in Agents"),
    ]
    func titles(of menu: NSMenu) -> [String] {
      menu.items.flatMap { [$0.title] + ($0.submenu.map(titles(of:)) ?? []) }
    }
    var resolvedMenus = 0
    var cardMenus = 0
    let step: CGFloat = 12
    for x in stride(from: CGFloat(6), to: host.bounds.width, by: step) {
      for y in stride(from: CGFloat(6), to: host.bounds.height, by: step) {
        let point = NSPoint(x: x, y: y)
        guard
          let event = NSEvent.mouseEvent(
            with: .rightMouseDown, location: host.convert(point, to: nil),
            modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window.windowNumber, context: nil, eventNumber: 9_200,
            clickCount: 1, pressure: 1)
        else { continue }
        var view = host.hitTest(point)
        var menu: NSMenu?
        while let current = view, menu == nil {
          menu = current.menu(for: event)
          view = current.superview
        }
        guard let menu else { continue }
        resolvedMenus += 1
        menu.delegate?.menuNeedsUpdate?(menu)
        let resolved = titles(of: menu)
        if resolved.contains(String(localized: "Disconnect and archive agent")) {
          cardMenus += 1
          XCTAssertTrue(resolved.contains(String(localized: "Channel 0")))
          XCTAssertTrue(resolved.contains(String(localized: "Do not disturb")))
        }
        for title in resolved {
          for marker in forbidden {
            XCTAssertFalse(
              title.contains(marker),
              "the big agent menu leaked into the full roster at \(point): \(resolved)")
          }
        }
      }
    }
    XCTAssertGreaterThan(
      resolvedMenus, 0, "the scan resolved no menu at all, so absence proves nothing")
    XCTAssertGreaterThan(cardMenus, 0, "the roster must expose its permitted secondary menu")

    var tooltips: [String] = []
    func walk(_ view: NSView) {
      if let tip = view.toolTip, !tip.isEmpty { tooltips.append(tip) }
      for child in view.subviews { walk(child) }
    }
    walk(host)
    XCTAssertTrue(
      tooltips.isEmpty,
      "the full roster shows its information inline, never as system tooltips: \(tooltips)")

    // The matcher itself is proven on a sentinel first: a probe carrying the
    // REAL agentPillMenu(for: "2") must expose the forbidden commands, and
    // performing its write item must reach the spied conversation target.
    var sentinelSelected: String?
    var sentinel = OverlayChannelStatusView(
      channels: channels, unavailable: false, palette: .dark, animates: false,
      hudStates: ["2": hud(session: "lena-session"), "3": hud(session: "astra-session")],
      onToggleChannel: { _ in },
      conversations: [broadcast, lena, astra], selectedConversationID: broadcast.id,
      onSelectConversation: { sentinelSelected = $0 })
    sentinel.onTogglePlayback = { _ in }
    sentinel.mutedChannels = ["2": false]
    let probeHost = NSHostingView(
      rootView: VStack {
        Text(verbatim: "menu-probe").frame(width: 180, height: 40)
          .modifier(sentinel.agentPillMenu(for: "2", showsInlineError: false))
        Text(verbatim: "hint-probe").frame(width: 180, height: 40)
          .overlayFastHint("probe-hint", palette: .dark)
      }
      .frame(width: 200, height: 120))
    probeHost.frame = NSRect(x: 0, y: 0, width: 200, height: 120)
    let probeWindow = NSWindow(
      contentRect: probeHost.frame, styleMask: [.borderless], backing: .buffered, defer: false)
    probeWindow.isReleasedWhenClosed = false
    defer { probeWindow.orderOut(nil) }
    probeWindow.contentView = probeHost
    BackgroundTestWindow.present(probeWindow)
    probeHost.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.08))
    probeHost.layoutSubtreeIfNeeded()

    func resolvedMenu(in container: NSView, of window: NSWindow, at point: NSPoint) -> NSMenu? {
      guard
        let event = NSEvent.mouseEvent(
          with: .rightMouseDown, location: container.convert(point, to: nil),
          modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
          windowNumber: window.windowNumber, context: nil, eventNumber: 9_300,
          clickCount: 1, pressure: 1)
      else { return nil }
      var view = container.hitTest(point)
      while let current = view {
        if let menu = current.menu(for: event) { return menu }
        view = current.superview
      }
      return nil
    }
    // Resolve the mounted menu instead of assuming which y-coordinate the
    // hosting view gives its first VStack child in this macOS environment.
    var probeMenu: NSMenu?
    probeScan: for x in stride(from: CGFloat(6), to: probeHost.bounds.width, by: 12) {
      for y in stride(from: CGFloat(6), to: probeHost.bounds.height, by: 12) {
        if let menu = resolvedMenu(
          in: probeHost, of: probeWindow, at: NSPoint(x: x, y: y))
        {
          menu.delegate?.menuNeedsUpdate?(menu)
          probeMenu = menu
          break probeScan
        }
      }
    }
    let probeTitles = probeMenu.map(titles(of:)) ?? []
    XCTAssertTrue(
      forbidden.contains { marker in probeTitles.contains { $0.contains(marker) } },
      "the sentinel agent menu must trip the matcher, or absence proves nothing: \(probeTitles)")
    // Required, never conditional (Astra and Olga on bdadd84fc): another
    // forbidden command could satisfy the matcher while a missing Write
    // silently skipped the spy.
    let requiredMenu = try XCTUnwrap(probeMenu, "the sentinel resolves a real menu")
    let writeIndex = try XCTUnwrap(
      requiredMenu.items.firstIndex(where: {
        $0.title.contains(String(localized: "Write / Forward…"))
      }), "the sentinel menu must carry the Write command: \(probeTitles)")
    requiredMenu.performActionForItem(at: writeIndex)
    XCTAssertEqual(sentinelSelected, lena.id, "the write command reaches its exact conversation")

    // The custom hint panel is the other tooltip owner (overlayFastHint →
    // OverlayHintPanel). The probe proves hover really presents a readable
    // hint; then every roster region is hovered, and a hint repeating the
    // inline metadata — descriptor or session — is the duplicated tooltip
    // the contract forbids. Full last-message previews are allowed by the
    // sidebar request of 2026-10-29; action labels remain allowed as well.
    func regions(in view: NSView) -> [OverlayMenuCursorRegionView] {
      var found: [OverlayMenuCursorRegionView] = []
      if let region = view as? OverlayMenuCursorRegionView { found.append(region) }
      for child in view.subviews { found.append(contentsOf: regions(in: child)) }
      return found
    }
    let probeRegions = regions(in: probeHost)
    XCTAssertFalse(probeRegions.isEmpty, "the hint probe composes a hover region")
    var sawProbeHint = false
    for region in probeRegions {
      let center = region.convert(NSPoint(x: region.bounds.midX, y: region.bounds.midY), to: nil)
      region.updateHover(at: center)
      if OverlayHintPanel.shared.presentedTitle == "probe-hint" {
        sawProbeHint = true
        region.updateHover(at: NSPoint(x: -1_000, y: -1_000))
        XCTAssertNil(
          OverlayHintPanel.shared.presentedTitle,
          "leaving the control must withdraw its hint")
      } else {
        region.updateHover(at: NSPoint(x: -1_000, y: -1_000))
      }
    }
    XCTAssertTrue(sawProbeHint, "hover must present the probe hint, or absence proves nothing")

    let metadata = ["codex", "gpt-6", "lena-session", "astra-session"]
    let rosterRegions = regions(in: host)
    XCTAssertFalse(rosterRegions.isEmpty, "the roster composes hover regions")
    for region in rosterRegions {
      let center = region.convert(NSPoint(x: region.bounds.midX, y: region.bounds.midY), to: nil)
      region.updateHover(at: center)
      if let hint = OverlayHintPanel.shared.presentedTitle {
        for marker in metadata {
          XCTAssertFalse(
            hint.contains(marker), "a hover hint duplicates inline roster metadata: \(hint)")
        }
      }
      region.updateHover(at: NSPoint(x: -1_000, y: -1_000))
    }
  }

  /// Olga's chosen execution proof (2026-10-22): a real AX press on the
  /// composed roster's inline microphone reaches exactly its agent, exactly
  /// once. One agent and no All row keeps the shared identifier unique.
  /// Runs only in the isolated GUI window (make test-swift), through the
  /// independent AX client — never a self-query on the main thread.
  func testInlineMicrophonePressReachesItsAgentOnce() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let (_, lena, _) = try rosterFixture()
    var toggles: [UInt8] = []
    var view = OverlayChannelStatusView(
      channels: [
        OverlayChannelDelivery(
          channel: "2", agent: "lena", deliveryID: "receipt-2", stage: .received, isOpen: false)
      ],
      unavailable: false, palette: .dark, animates: false,
      hudStates: ["2": hud(session: "lena-session")],
      onToggleChannel: { toggles.append($0) },
      conversations: [lena], selectedConversationID: lena.id,
      onSelectConversation: { _ in })
    view.mutedChannels = ["2": false]
    view.onTogglePlayback = { _ in }
    let host = NSHostingView(
      rootView: view.monitorBody(maximumHeight: 800).environment(\.csTextScale, 1)
        .frame(width: 360))
    host.frame = NSRect(x: 0, y: 0, width: 360, height: 400)
    let window = NSWindow(
      contentRect: host.frame, styleMask: [.borderless], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    defer { window.orderOut(nil) }
    window.contentView = host
    try BackgroundTestWindow.takeKeyboard(window)
    host.layoutSubtreeIfNeeded()

    let elements = try NativeAccessibilityProbe.waitForElements("overlay-agent-microphone")
    XCTAssertEqual(
      elements.elements.count, 1,
      "one agent and no All row leave exactly one microphone: \(elements.elements)")
    let pressed = try NativeAccessibilityProbe.query("overlay-agent-microphone", press: true)
    XCTAssertEqual(pressed.pressStatus, 0, "the AX press must land: \(pressed.trace)")
    let deadline = Date().addingTimeInterval(2)
    while toggles.isEmpty && Date() < deadline {
      RunLoop.main.run(until: Date().addingTimeInterval(0.02))
    }
    XCTAssertEqual(toggles, [2], "the press reaches its exact agent, exactly once")
  }

  /// ⌘+ grew the docked column and the conversation, but the roster's own text
  /// stayed at 1.0 (Maciej, 2026-10-19): the overlay calls `monitorBody` on a
  /// status view that is never installed, so its environment read the default.
  func testDockedRosterTextFollowsTheOverlayTextScale() throws {
    let (broadcast, lena, astra) = try rosterFixture()
    // The roster's height is its text: a heading and three rows. Measured
    // alone at a fixed width, it grows with the scale only if the text does.
    // `fittingSize` reads 0 with `sizingOptions = []`, so the roster reports
    // its own ideal height.
    func rosterHeight(at scale: CGFloat) -> CGFloat {
      let recorder = OverlayRosterHeightRecorder()
      let view = OverlayChannelStatusView(
        channels: [
          OverlayChannelDelivery(
            channel: "2", agent: "lena", deliveryID: "receipt-2", stage: .received, isOpen: false),
          OverlayChannelDelivery(
            channel: "3", agent: "astra", deliveryID: "receipt-3", stage: .received, isOpen: false),
        ],
        unavailable: false, palette: .dark, animates: false,
        hudStates: ["2": hud(session: "lena-session"), "3": hud(session: "astra-session")],
        conversations: [broadcast, lena, astra], selectedConversationID: broadcast.id,
        onSelectConversation: { _ in })
      let host = NSHostingView(
        rootView:
          view.monitorBody(maximumHeight: 4000)
          .environment(\.csTextScale, scale)
          .frame(width: 300)
          .fixedSize(horizontal: false, vertical: true)
          .onGeometryChange(for: CGFloat.self) {
            $0.size.height
          } action: {
            recorder.height = $0
          })
      host.sizingOptions = []
      host.frame = CGRect(x: 0, y: 0, width: 300, height: 4000)
      // The scroll viewport takes the rows' measured height one pass later.
      for _ in 0..<3 {
        host.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
      }
      return recorder.height
    }
    let normal = rosterHeight(at: 1)
    let large = rosterHeight(at: 1.5)
    XCTAssertGreaterThan(normal, 40, "the roster lays out its heading and rows")
    XCTAssertLessThan(normal, 1000, "the viewport took the rows' height, not the 4000 pt bound")
    // Fixed paddings and glyphs do not scale, so the whole is below 1.5×; before
    // the fix nothing in the roster scaled and the ratio was 1.
    XCTAssertGreaterThan(
      large, normal * 1.1,
      "the docked roster's text grows with ⌘+ like the conversation: \(normal) → \(large)")
  }

  func testRememberedDrawerWidthLeavesConversationSpaceAndScalesWithText() throws {
    let defaults = try XCTUnwrap(UserDefaults(suiteName: UUID().uuidString))
    defaults.set(480.0, forKey: "Overlay.agentDrawerWidth.v1")
    let (broadcast, lena, _) = try rosterFixture()
    let state = OverlayState.previewFormatted()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
    state.selectConversation(lena.id)
    state.showAgentMonitor()
    state.conversationDrafts[lena.id] = "unfinished message"
    for (width, scale, expected) in [
      (1200.0, 1.0, 480.0), (900.0, 1.0, 480.0),
      (1400.0, 1.4, 672.0),
    ] as [(CGFloat, CGFloat, CGFloat)] {
      let recorder = OverlayDrawerFrameRecorder()
      let host = NSHostingView(
        rootView: DictationOverlayView(state: state)
          .defaultAppStorage(defaults)
          .environment(\.csTextScale, scale)
          .onPreferenceChange(OverlayDrawerFramePreferenceKey.self) { recorder.frame = $0 }
          .frame(width: width, height: 500))
      host.sizingOptions = []
      let panel = NSPanel(
        contentRect: CGRect(x: 0, y: 0, width: width, height: 500),
        styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
      panel.contentView = host
      BackgroundTestWindow.present(panel)
      host.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.1))
      host.layoutSubtreeIfNeeded()
      XCTAssertEqual(recorder.frame.width, expected, accuracy: 1)
      XCTAssertEqual(recorder.frame.minX, width - expected, accuracy: 1)
      XCTAssertGreaterThanOrEqual(recorder.frame.minX, 320 * scale - 1)
      XCTAssertEqual(state.selectedConversationID, lena.id)
      XCTAssertEqual(state.conversationDrafts[lena.id], "unfinished message")
      panel.orderOut(nil)
    }
    XCTAssertEqual(
      defaults.double(forKey: "Overlay.agentDrawerWidth.v1"), 480,
      "window clamping must not overwrite the user's remembered width")
  }

  func testDrawerPreviewNamesWhoAddressedWhomFromTheMessagesOwnRoute() throws {
    // Maciej (2026-10-17): "brak informacji jak who addressed whom". The
    // route is the message's source channel and frozen recipients, never the
    // row it is shown in.
    let (_, lena, astra) = try rosterFixture()
    let lenaOwner = try XCTUnwrap(lena.owner)
    let astraOwner = try XCTUnwrap(astra.owner)
    func recipient(_ owner: OverlayConversationOwner) -> OverlayConversationRecipient {
      OverlayConversationRecipient(
        owner: owner, deliveryID: nil, queued: false, accepted: false, acknowledged: false)
    }
    func message(
      _ kind: OverlayConversationMessage.Kind, owner: OverlayConversationOwner? = nil,
      to recipients: [OverlayConversationOwner] = [], source: String? = nil
    ) -> OverlayConversationMessage {
      var message = OverlayConversationMessage(
        id: "\(kind):1", kind: kind, text: "first line\nsecond line", order: 1,
        emittedAt: "2026-10-16T14:54:00Z", owner: owner, recipients: recipients.map(recipient),
        deliveryID: nil, replyTo: nil, unsolicited: false, playback: nil, busPath: "")
      message.sourceChannel = source
      return message
    }
    func preview(_ message: OverlayConversationMessage) -> String {
      OverlayChannelStatusView.drawerPreview(message: message)
    }
    XCTAssertEqual(
      preview(message(.user, to: [lenaOwner, astraOwner], source: "0")),
      "You → All: first line second line",
      "a broadcast stays → All in every recipient's row, one line")
    XCTAssertEqual(
      preview(message(.user, to: [lenaOwner], source: "2")), "You → lena: first line second line")
    XCTAssertEqual(
      preview(message(.user, source: "2")), "You → 2: first line second line",
      "without frozen recipients the channel stands, not the agent now on it")
    XCTAssertEqual(
      preview(message(.user)), "You: first line second line",
      "no route recorded, no arrow invented")
    XCTAssertEqual(
      preview(message(.reply, owner: lenaOwner)), "lena → You: first line second line",
      "a reply without a destination answered the Founder")
    XCTAssertEqual(
      preview(message(.reply, owner: lenaOwner, source: "0")), "lena → All: first line second line")
    XCTAssertEqual(
      preview(message(.reply, owner: lenaOwner, to: [astraOwner], source: "3")),
      "lena → astra: first line second line", "a peer message names its frozen recipient")
    XCTAssertEqual(
      preview(message(.reply, owner: lenaOwner, source: "3")),
      "lena → 3: first line second line")
    func route(_ message: OverlayConversationMessage) -> String {
      OverlayChannelStatusView.messageRoute(of: message).label
    }
    XCTAssertEqual(
      route(message(.user, to: [lenaOwner, astraOwner], source: "2")), "You → lena, astra")
    XCTAssertEqual(route(message(.reply, owner: lenaOwner, source: "10")), "lena → You")
    var attachmentOnly = message(.user, to: [lenaOwner], source: "2")
    attachmentOnly.text = "[attachment] shot.png (image/png, 4 bytes): /tmp/shot.png"
    attachmentOnly.attachments = [
      OverlayMessageAttachment(
        name: "shot.png", path: "/tmp/shot.png", mediaType: "image/png", bytes: 4)
    ]
    XCTAssertEqual(attachmentOnly.bodyText, "")
    XCTAssertEqual(
      route(attachmentOnly), "You → lena", "a file-only delivery still names its route")
    XCTAssertEqual(preview(attachmentOnly), "You → lena: ")
    let bubbles = try overlaySource("OverlayConversationView.swift")
    XCTAssertTrue(bubbles.contains("messageRoute(of: message)"))
    XCTAssertTrue(bubbles.contains("overlay-message-route-"))
    XCTAssertTrue(bubbles.contains("owner.id != conversation.owner?.id"))
    XCTAssertFalse(
      bubbles.contains("overlay-message-reply"),
      "a visual reply control is not a delivery until the envelope carries the source id")
  }

  func testCompactAgentRowDropsTheFooterAndKeepsUnreadApartFromReceipts() throws {
    // Identity may wrap; rendered card height and painted names are checked in
    // OverlayBusWindowTests rather than inferred from numeric lineLimit syntax.
    // Receipt and unread are distinct marks, neither adds a footer row.
    let source = try overlaySource("OverlayChannelStatusView.swift")
    let label = try section(
      of: source, from: "private func rosterRowLabel(", to: "func compactAgentActions(")
    let visual = try overlaySource("OverlayAgentRosterLabel.swift")
    XCTAssertTrue(
      visual.contains("OverlayAgentUnreadBadge("), "unread remains a badge distinct from receipts")
    XCTAssertFalse(label.contains("unread replies\")"), "no separate unread line")
    XCTAssertTrue(
      label.contains("deliveryReceiptDisplay("), "receipt follows frozen delivery evidence")
    let row = try section(of: source, from: "func channelRow(", to: "private func rosterRowLabel(")
    XCTAssertFalse(row.contains("rosterRowState("), "compact cards have no separate footer")
  }

  func testBroadcastPolicyRidesTheRowsContextMenuWithNoDisclosureList() throws {
    // Maciej (2026-10-17, "Słowo"): wake policy moves to the agent row's
    // right-click menu; the disclosure list under 0 · All is gone.
    let source = try overlaySource("OverlayChannelStatusView.swift")
    XCTAssertFalse(
      source.contains("overlay-broadcast-disclosure"),
      "the chevron list under 0 · All is gone; Saved conversations keeps its own disclosure")
    XCTAssertFalse(source.contains("OverlayBroadcastDisclosure"), "the old wrapper type is gone")
    let monitor = try section(
      of: source, from: "func monitorBody(maximumHeight:", to: "var currentConversations")
    XCTAssertTrue(
      monitor.contains("OverlayBroadcastPolicyMenu("),
      "the docked 0 · All row carries every member's wake policy")
    XCTAssertTrue(
      source.contains("members: broadcastMembers.filter { $0.channel == channel.channel }"),
      "an agent's own docked row carries only its own policy")
  }

  /// A bound channel whose reader died keeps its row, dimmed as one unit,
  /// and its microphone sleeps (Maciej, 2026-10-17).
  func testDockedRowDimsADisconnectedReaderAndUsesCentralAudioGuards() throws {
    let source = try overlaySource("OverlayChannelStatusView.swift")
    let modifier = try section(
      of: source, from: "struct OverlayDisconnectedRowDim: ViewModifier {", to: "\n}\n")
    XCTAssertTrue(modifier.contains(".opacity(disconnected ? Self.opacity : 1)"))
    XCTAssertTrue(modifier.contains(".grayscale(disconnected ? 1 : 0)"))
    let docked = try section(
      of: source, from: "func channelRow(", to: "private func rosterRowLabel(")
    XCTAssertTrue(
      docked.contains("compactAgentActions(for: channel)"),
      "the row uses the same guarded actions as its conversation header")
    let actions = try section(
      of: source, from: "func compactAgentActions(", to: "func compactReceptionIndicators(")
    XCTAssertTrue(actions.contains("agentMicrophoneEnabled(for: channel)"))
    XCTAssertTrue(actions.contains("agentPlaybackEnabled(for: channel)"))
    XCTAssertTrue(
      docked.contains("OverlayDisconnectedRowDim(disconnected: hasDeadFollower(channel))"),
      "the docked row dims as one unit")
  }

  // MARK: - Hosting structure (deliverables 1 and 4)

  func testOnePanelUsesTheExistingRosterForDockedAndCoveringPlacement() throws {
    let overlay = try overlaySource("DictationOverlayView.swift")
    XCTAssertFalse(overlay.contains("conversationEdgeControls"))
    XCTAssertFalse(overlay.contains("overlay-agent-rail"))
    XCTAssertFalse(overlay.contains("hideAgentsCanvas"))
    XCTAssertTrue(overlay.contains("overlay-agent-sidebar-toggle"))
    XCTAssertTrue(overlay.contains("resizable: true"))
    XCTAssertTrue(overlay.contains("resizable: false"))
    let column = try section(
      of: overlay, from: "private func agentDrawerColumn", to: "private func agentListHeaderControl"
    )
    XCTAssertTrue(column.contains("var roster = channelStatusView"))
    XCTAssertTrue(column.contains("return roster.monitorBody("))
  }

  // MARK: - Wide-window evacuation (Maciej, 2026-10-17)

  /// The docked column already names the agent and carries its microphone and
  /// speaker, so the identity pill must leave with it: the host hands the
  /// column's visibility to the conversation view, and the view drops the
  /// whole navigation row (controls and pill) with a collapsed measurement.
  func testHorizontalHeaderKeepsActionsWithFullAndFoldedSidebar() throws {
    let overlay = try overlaySource("DictationOverlayView.swift")
    let canvas = try section(
      of: overlay, from: "private func canvasStack", to: "private var header: some View")
    XCTAssertFalse(canvas.contains("hidesNavigation: columnShown"))
    XCTAssertTrue(canvas.contains("navigationActions: conversationHeaderActions("))
    XCTAssertTrue(
      canvas.contains("navigationIndicators: conversationHeaderIndicators(conversation)"))
    let conversation = try overlaySource("OverlayConversationView.swift")
    let anchor = try section(
      of: conversation, from: ".overlay(alignment: .top)", to: ".onChange(of: conversation.id)")
    XCTAssertTrue(
      anchor.contains("if hidesNavigation"),
      "the shared Intercom presentation retains its explicit navigation choice")
    XCTAssertTrue(
      anchor.contains("navigationHeight = 0"),
      "the content margin follows the hidden Intercom navigation row")
  }

  /// Broadcast-policy choices in a roster row's context menu are checkboxes, so
  /// they must use the menu Toggle's native state column like the header
  /// menu's checkboxes, not a checkmark squatting in the icon slot.
  func testPolicyMenuUsesNativeCheckboxToggles() throws {
    let source = try overlaySource("OverlayChannelStatusView.swift")
    let actions = try section(
      of: source, from: "private func policyActions", to: "private func apply")
    XCTAssertTrue(actions.contains("Toggle("), "policy rows are menu checkboxes")
    XCTAssertFalse(
      actions.contains("systemImage: \"checkmark\""),
      "no icon-slot checkmark impersonating a checkbox")
  }

  // MARK: - Fold keeps the panes' layout (Founder, 2026-10-26)

  /// The `sidebar.left` fold slides the panes, and the conversation inside
  /// its shrinking cell used to be laid out at the cell's current width: one
  /// character per line in the edge column, kept on screen by a stalled
  /// frame (recording 17:49, 23.25 s). The conversation now keeps its split
  /// width at every moment of the fold and the unfold; only the cell clips.

  /// The roster lays out at its new width at once; only the column's frame
  /// slides over it, anchored to the window edge, so no row re-wraps its
  /// title and preview inside a half-way frame.
  func testDrawerColumnSnapsItsListAndSlidesOnlyItsFrame() throws {
    let overlay = try overlaySource("DictationOverlayView.swift")
    let column = try section(
      of: overlay,
      from: "private func agentDrawerColumn",
      to: "private func agentListHeaderControl")
    XCTAssertTrue(column.contains(".animation(nil, value: width)"), "the list snaps")
    XCTAssertTrue(
      column.contains(".frame(width: width, alignment: .trailing)"),
      "the sliding frame is anchored to the window edge")
    let stack = try section(
      of: overlay, from: "private func canvasStack", to: "private func agentDrawerColumn")
    XCTAssertTrue(
      stack.contains(".frame(width: canvasWidth, alignment: .leading)"),
      "the conversation follows its actual available width")
    let clippedCell =
      ".frame(width: canvasWidth, height: geometry.size.height, alignment: .leading)\n"
      + "          .clipped()"
    XCTAssertTrue(stack.contains(clippedCell), "and its cell clips it while sliding")
  }
}

@MainActor
private final class OverlayDrawerFrameRecorder {
  var frame = CGRect.zero
}

@MainActor
private final class OverlayRosterHeightRecorder {
  var height: CGFloat = 0
}
