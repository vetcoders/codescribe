import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Falsifiers for the agents-drawer cut (Founder, 2026-10-16; mockup
/// `szuflada-agentow.html`): the roster docks as a right-hand column on wide
/// windows, covers the whole window as the same list on narrow ones
/// (2026-10-19), and the bubble cap follows ⌘+ text scale.
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

  private func hud(session: String) -> OverlayChannelHudProjection {
    OverlayChannelHudProjection(
      open: false, loud: false, autosealDeadline: nil, followerAlive: true,
      provider: "codex", providerSessionID: session)
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

  func testDockingThresholdFollowsTextScale() {
    XCTAssertTrue(
      OverlayAgentDrawerLayout.isDocked(windowWidth: 1200, textScale: 1.0),
      "1200 at 1.0 docks")
    XCTAssertFalse(
      OverlayAgentDrawerLayout.isDocked(windowWidth: 680, textScale: 1.0),
      "680 at 1.0 falls back to the full-width list")
    XCTAssertFalse(
      OverlayAgentDrawerLayout.isDocked(windowWidth: 1100, textScale: 1.6),
      "1100 at 1.6 needs 1152 — the full-width list, so large text never squeezes bubbles")
    XCTAssertTrue(
      OverlayAgentDrawerLayout.isDocked(windowWidth: 720, textScale: 1.0),
      "the exact 720 × scale boundary docks")
    XCTAssertTrue(OverlayAgentDrawerLayout.isDocked(windowWidth: 1152, textScale: 1.6))
    XCTAssertFalse(OverlayAgentDrawerLayout.isDocked(windowWidth: 719.9, textScale: 1.0))
    // Maciej 2026-10-19: a 900-point overlay at text 1.2 did not dock.
    XCTAssertTrue(OverlayAgentDrawerLayout.isDocked(windowWidth: 898, textScale: 1.2))
    XCTAssertEqual(OverlayAgentDrawerLayout.drawerWidth(textScale: 1.4), 420, accuracy: 0.001)
    // Maciej 2026-10-18: no cap; dragged fully left the roster is the window.
    XCTAssertEqual(
      OverlayAgentDrawerLayout.drawerWidth(
        textScale: 1.0, preferredWidth: 700, windowWidth: 1200), 700, accuracy: 0.001)
    XCTAssertEqual(
      OverlayAgentDrawerLayout.drawerWidth(
        textScale: 1.0, preferredWidth: 900, windowWidth: 1200), 1200, accuracy: 0.001,
      "less than 320 of conversation snaps to the agents view")
    XCTAssertNil(
      OverlayAgentDrawerLayout.rememberedWidth(
        textScale: 1.0, preferredWidth: 900, windowWidth: 1200),
      "a roster drawn across the window chooses the agents view, not a width")
    XCTAssertEqual(
      OverlayAgentDrawerLayout.rememberedWidth(
        textScale: 1.0, preferredWidth: 400, windowWidth: 1200) ?? 0, 400, accuracy: 0.001)
    XCTAssertEqual(OverlayAgentDrawerLayout.pillWidth(textScale: 1.0), 224, accuracy: 0.001)
  }

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
    state.applyConversationSnapshot(
      .init(deliveries: [], conversations: [broadcast, lena, astra]))
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
    defer { panel.orderOut(nil) }
    func settle() {
      host.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.1))
      host.layoutSubtreeIfNeeded()
    }
    settle()
    XCTAssertFalse(state.agentDrawerDocked, "700 wide at 1.0 is below the threshold")
    XCTAssertEqual(recorder.frame, .zero, "no list until it is asked for")

    state.showAgentMonitor()
    settle()
    XCTAssertTrue(state.showsAgentMonitor)
    XCTAssertEqual(recorder.frame.minX, 0, accuracy: 1, "the list starts at the left edge")
    XCTAssertEqual(recorder.frame.width, 700, accuracy: 1, "and takes the whole width")
    XCTAssertEqual(recorder.frame.maxY, 500, accuracy: 1, "and the full height")

    state.selectConversation(astra.id)
    settle()
    XCTAssertEqual(state.selectedConversationID, astra.id)
    XCTAssertFalse(state.showsAgentMonitor, "choosing a row returns to the conversation")
    XCTAssertEqual(recorder.frame, .zero, "the list leaves with the flag")
  }

  func testNarrowListIsTheDockedListAndThePillStackIsGone() throws {
    let overlay = try overlaySource("DictationOverlayView.swift")
    let canvas = try section(
      of: overlay, from: "private func canvasStack", to: "private var header: some View")
    XCTAssertTrue(canvas.contains("let listShown = !docked && listOnScreen"))
    XCTAssertTrue(
      canvas.contains("let canvasHidden = canvasWidth < 1 || listShown"),
      "the conversation or the transcription hides under the list, keeping its layout")
    XCTAssertTrue(
      canvas.contains("windowWidth: geometry.size.width, docked: false"),
      "the narrow list is the docked column's own view")
    XCTAssertTrue(canvas.contains(".onExitCommand { state.hideAgentSidebar() }"), "Esc closes it")
    let column = try section(
      of: overlay, from: "private func agentDrawerColumn",
      to: "private func agentDrawerResizeHandle")
    XCTAssertNotNil(
      column.range(of: #"if docked \{\s*agentDrawerResizeHandle"#, options: .regularExpression),
      "no edge to drag below the threshold")
    XCTAssertTrue(
      column.contains("showAgentsCanvas(windowWidth: windowWidth, docked: docked)"),
      "the corner button over the narrow list gives the window back")
    let state = try overlaySource("OverlayState.swift")
    let showCanvas = try section(
      of: state, from: "func showAgentsCanvas()", to: "func hideAgentsCanvas()")
    XCTAssertTrue(showCanvas.contains("if !agentDrawerDocked { hideAgentSidebar() }"))
    let conversation = try overlaySource("OverlayConversationView.swift")
    XCTAssertFalse(
      conversation.contains("agentPillStack"), "no second roster mounts under the name pill")
    let roster = try overlaySource("OverlayChannelStatusView.swift")
    for gone in [
      "OverlayAgentPillStack", "OverlayAgentPillEntry", "pillEntries", "overlay-agent-pill-stack",
    ] {
      XCTAssertFalse(roster.contains(gone), "\(gone) left with the pill stack")
    }
  }

  // MARK: - Docked column (deliverables 1 and 4)

  func testDockingEngagementClosesTheNarrowListAndKeepsSelectionAndDraft() throws {
    let (broadcast, lena, _) = try rosterFixture()
    let state = OverlayState()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
    state.selectConversation(lena.id)
    state.conversationDrafts[lena.id] = "half-typed draft"
    state.showAgentMonitor()
    XCTAssertTrue(state.showsAgentMonitor)
    let focusRevision = state.conversationFocusRevision

    state.setAgentDrawerDocked(true)

    XCTAssertFalse(state.showsAgentMonitor, "the narrow list closes when docking engages")
    XCTAssertEqual(state.selectedConversationID, lena.id, "the selection survives the switch")
    XCTAssertEqual(state.conversationDrafts[lena.id], "half-typed draft", "drafts survive")
    XCTAssertEqual(
      state.conversationFocusRevision, focusRevision,
      "no scroll-reset signal crosses a pure mode switch")

    state.setAgentDrawerDocked(false)
    XCTAssertFalse(state.showsAgentMonitor, "undocking does not reopen the list")
  }

  func testDockedNamePillFocusesTheSelectedRowInsteadOfOpening() throws {
    let (broadcast, lena, _) = try rosterFixture()
    let state = OverlayState()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
    state.selectConversation(lena.id)
    state.setAgentDrawerDocked(true)
    let focus = state.agentDrawerFocusRevision

    state.showAgentMonitor()

    XCTAssertFalse(state.showsAgentMonitor, "docked: nothing opens, the column is already there")
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
    XCTAssertEqual(recorder.frame, .zero, "700 wide at 1.0: no docked column")
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
    XCTAssertEqual(recorder.frame.width, 300, accuracy: 1, "the asked-for roster docks")

    state.hideAgentSidebar()
    settle()
    XCTAssertEqual(recorder.frame, .zero, "Esc gives the transcription its width back")

    state.selectConversation(lena.id)
    settle()
    XCTAssertFalse(state.showsAgentMonitor)
    XCTAssertEqual(recorder.frame.width, 300, accuracy: 1, "beside a conversation it is permanent")
  }

  func testWideningUnderTheOpenListDocksItAsTheColumn() throws {
    let (broadcast, lena, _) = try rosterFixture()
    let state = OverlayState.previewFormatted()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast, lena]))
    let recorder = OverlayDrawerFrameRecorder()
    // No fixed frame: the panel's content size is the window the layout reads.
    let host = NSHostingView(
      rootView:
        DictationOverlayView(state: state)
        .onPreferenceChange(OverlayDrawerFramePreferenceKey.self) { recorder.frame = $0 })
    host.sizingOptions = []
    let panel = NSPanel(
      contentRect: CGRect(x: 0, y: 0, width: 700, height: 500),
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
    XCTAssertEqual(recorder.frame.width, 700, accuracy: 1, "narrow: the list covers the window")

    // Docking engages while the list is open on the transcription: the list
    // becomes the column instead of vanishing.
    panel.setContentSize(NSSize(width: 1200, height: 500))
    settle()
    XCTAssertTrue(state.agentDrawerDocked)
    XCTAssertTrue(state.showsAgentMonitor)
    XCTAssertEqual(recorder.frame.width, 300, accuracy: 1, "the open list docks as the column")
    XCTAssertEqual(recorder.frame.maxX, 1200, accuracy: 1)

    state.selectConversation(lena.id)
    settle()
    XCTAssertFalse(state.showsAgentMonitor, "selection closes the flag")
    XCTAssertEqual(recorder.frame.width, 300, accuracy: 1, "beside a conversation the column stays")

    state.showTranscription()
    settle()
    XCTAssertNil(state.selectedConversationID)
    XCTAssertEqual(recorder.frame, .zero, "back on the transcription the column is gone")
  }

  // MARK: - The keyboard follows the list (Ada, 2026-10-19)

  func testTheListTakesTheKeyboardFromTheComposerAndGivesItBack() throws {
    let (broadcast, lena, astra) = try rosterFixture()
    let state = OverlayState.previewFormatted()
    state.applyConversationSnapshot(
      .init(deliveries: [], conversations: [broadcast, lena, astra]))
    state.selectConversation(lena.id)
    let (panel, host) = keyboardPanel(DictationOverlayView(state: state), width: 700)
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
    try BackgroundTestWindow.requireIsolatedGUI()
    panel.takeKeyForTranscript()
    XCTAssertTrue(panel.makeFirstResponder(editor))
    XCTAssertTrue(panel.isKeyWindow)

    let rosterFocus = state.agentDrawerFocusRevision
    state.showAgentMonitor()
    settle(host)
    XCTAssertFalse(panel.firstResponder === editor, "the covered composer lets the keyboard go")
    XCTAssertGreaterThan(
      state.agentDrawerFocusRevision, rosterFocus, "the list's selected row is asked to take it")
    let key = try XCTUnwrap(
      NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
        windowNumber: panel.windowNumber, context: nil, characters: "x",
        charactersIgnoringModifiers: "x", isARepeat: false, keyCode: 7))
    panel.sendEvent(key)
    XCTAssertEqual(editor.string, "", "nothing is typed into a field nobody can see")

    state.selectConversation(astra.id)
    settle(host)
    XCTAssertTrue(panel.firstResponder === editor, "choosing a row hands the keyboard back")

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
    let defaults = try XCTUnwrap(UserDefaults(suiteName: UUID().uuidString))
    defaults.set(480.0, forKey: "Overlay.agentDrawerWidth.v1")
    let (broadcast, lena, _) = try rosterFixture()
    let state = OverlayState.previewFormatted()
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
    try BackgroundTestWindow.requireIsolatedGUI()
    panel.takeKeyForTranscript()
    XCTAssertTrue(panel.makeFirstResponder(editor))

    // 780 still docks, but a 480 roster leaves the conversation under its
    // 320-point minimum: the roster takes the whole window, agents view only.
    let rosterFocus = state.agentDrawerFocusRevision
    panel.setContentSize(NSSize(width: 780, height: 500))
    settle(host)
    XCTAssertTrue(state.agentDrawerDocked)
    XCTAssertFalse(panel.firstResponder === editor, "the covered composer lets the keyboard go")
    XCTAssertGreaterThan(state.agentDrawerFocusRevision, rosterFocus)

    panel.setContentSize(NSSize(width: 1200, height: 500))
    settle(host)
    XCTAssertTrue(panel.firstResponder === editor, "the conversation returns with the keyboard")
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

  func testListPinnedFollowsTheLayoutAtEveryWidth() {
    for docked in [true, false] {
      XCTAssertTrue(
        OverlayAgentDrawerLayout.listPinned(
          layout: .agents, docked: docked, conversationSelected: true),
        "the agents view covers any width")
      XCTAssertFalse(
        OverlayAgentDrawerLayout.listPinned(
          layout: .rail, docked: docked, conversationSelected: true),
        "folded, the list comes only when asked for")
      for layout in [OverlayAgentsLayout.split, .agents, .rail] {
        XCTAssertFalse(
          OverlayAgentDrawerLayout.listPinned(
            layout: layout, docked: docked, conversationSelected: false),
          "the transcription keeps its width until the list is asked for")
      }
    }
    XCTAssertTrue(
      OverlayAgentDrawerLayout.listPinned(layout: .split, docked: true, conversationSelected: true))
    XCTAssertFalse(
      OverlayAgentDrawerLayout.listPinned(
        layout: .split, docked: false, conversationSelected: true),
      "below the threshold the split list comes when asked for")
    let widest = OverlayAgentDrawerLayout.widestSplitWidth(textScale: 1.0, windowWidth: 780)
    XCTAssertEqual(widest, 460, accuracy: 0.001)
    XCTAssertFalse(
      OverlayAgentDrawerLayout.fillsWindow(
        drawerWidth: OverlayAgentDrawerLayout.drawerWidth(
          textScale: 1.0, preferredWidth: widest, windowWidth: 780),
        windowWidth: 780),
      "the conversation slides out at its 320-point minimum")
    XCTAssertEqual(OverlayAgentDrawerLayout.railWidth(textScale: 1.2), 60.44, accuracy: 0.001)
  }

  func testTheLayoutIsRememberedAndTheDraggedAgentsViewMigrates() throws {
    let defaults = try XCTUnwrap(UserDefaults(suiteName: UUID().uuidString))
    defaults.set(
      OverlayAgentDrawerLayout.legacyFillsWindowWidth,
      forKey: OverlayAgentDrawerLayout.widthDefaultsKey)
    let state = OverlayState()
    state.restoreAgentsLayout(defaults: defaults)
    XCTAssertEqual(
      state.agentsLayout, .agents, "a roster once dragged across the window is the agents view")
    XCTAssertEqual(
      defaults.double(forKey: OverlayAgentDrawerLayout.widthDefaultsKey), 300,
      "and the stored width is a width again")
    XCTAssertEqual(defaults.string(forKey: OverlayState.agentsLayoutDefaultsKey), "agents")

    state.setAgentsLayout(.rail)
    let relaunched = OverlayState()
    relaunched.restoreAgentsLayout(defaults: defaults)
    XCTAssertEqual(relaunched.agentsLayout, .rail, "the choice survives a relaunch")
  }

  func testChoosingARowInTheAgentsViewSlidesTheConversationOut() throws {
    let (broadcast, lena, astra) = try rosterFixture()
    let state = OverlayState()
    state.applyConversationSnapshot(
      .init(deliveries: [], conversations: [broadcast, lena, astra]))
    state.selectConversation(lena.id)
    state.hideAgentsCanvas()
    XCTAssertEqual(state.agentsLayout, .agents)
    XCTAssertTrue(state.agentListPinned, "the agents view is on screen unasked, at any width")

    state.selectConversation(astra.id)
    XCTAssertEqual(
      state.agentsLayout, .agents, "the Agents button and a followed reply keep the agents view")

    state.chooseConversation(lena.id)
    XCTAssertEqual(state.selectedConversationID, lena.id)
    XCTAssertEqual(
      state.agentsLayout, .split, "a row chosen in the list slides the conversation out")

    state.foldAgentList()
    XCTAssertEqual(state.agentsLayout, .rail)
    state.chooseConversation(astra.id)
    XCTAssertEqual(state.selectedConversationID, astra.id)
    XCTAssertEqual(
      state.agentsLayout, .rail, "a pill switches the conversation and the list stays folded")
  }

  func testCornerButtonsMoveTheWideWindowBetweenTheThreeLayouts() throws {
    let (broadcast, lena, astra) = try rosterFixture()
    let state = OverlayState.previewFormatted()
    state.applyConversationSnapshot(
      .init(deliveries: [], conversations: [broadcast, lena, astra]))
    state.selectConversation(lena.id)
    let drawer = OverlayDrawerFrameRecorder()
    let rail = OverlayDrawerFrameRecorder()
    let (panel, settle) = try recordedPanel(state: state, width: 1200, drawer: drawer, rail: rail)
    defer { panel.orderOut(nil) }
    XCTAssertTrue(state.agentDrawerDocked)
    XCTAssertEqual(drawer.frame.width, 300, accuracy: 1, "split: the column beside the chat")

    state.hideAgentsCanvas()
    settle()
    XCTAssertEqual(drawer.frame.minX, 0, accuracy: 1, "sidebar.left: the agents view")
    XCTAssertEqual(drawer.frame.width, 1200, accuracy: 1)

    state.showAgentsCanvas()
    settle()
    XCTAssertEqual(drawer.frame.width, 300, accuracy: 1, "the chat slides back at the kept width")

    state.foldAgentList()
    settle()
    XCTAssertEqual(state.agentsLayout, .rail)
    XCTAssertEqual(drawer.frame, .zero, "sidebar.right folds the column away")
    XCTAssertEqual(
      rail.frame.width, OverlayAgentDrawerLayout.railWidth(textScale: 1), accuracy: 1,
      "into the rail of pills")
    XCTAssertEqual(rail.frame.maxX, 1200, accuracy: 1)

    state.showAgentMonitor()
    settle()
    XCTAssertEqual(drawer.frame.width, 300, accuracy: 1, "the Agents button still opens the list")
    XCTAssertEqual(rail.frame, .zero)
    state.chooseConversation(astra.id)
    settle()
    XCTAssertEqual(state.agentsLayout, .rail, "a choice in the asked-for list keeps the fold")
    XCTAssertEqual(drawer.frame, .zero)
    XCTAssertEqual(rail.frame.width, OverlayAgentDrawerLayout.railWidth(textScale: 1), accuracy: 1)

    state.unfoldAgentList()
    settle()
    XCTAssertEqual(state.agentsLayout, .split)
    XCTAssertEqual(drawer.frame.width, 300, accuracy: 1, "the rail's sidebar.right unfolds it")
    XCTAssertEqual(rail.frame, .zero)
  }

  func testTheRailAndTheAgentsViewHoldBelowTheThreshold() throws {
    let (broadcast, lena, astra) = try rosterFixture()
    let state = OverlayState.previewFormatted()
    state.applyConversationSnapshot(
      .init(deliveries: [], conversations: [broadcast, lena, astra]))
    state.selectConversation(lena.id)
    let drawer = OverlayDrawerFrameRecorder()
    let rail = OverlayDrawerFrameRecorder()
    let (panel, settle) = try recordedPanel(state: state, width: 700, drawer: drawer, rail: rail)
    defer { panel.orderOut(nil) }
    XCTAssertFalse(state.agentDrawerDocked)
    XCTAssertEqual(
      rail.frame.width, OverlayAgentDrawerLayout.railWidth(textScale: 1), accuracy: 1,
      "the rail is available before opening the list")

    state.showAgentMonitor()
    settle()
    XCTAssertEqual(drawer.frame.width, 700, accuracy: 1)
    state.foldAgentList()
    settle()
    XCTAssertFalse(state.showsAgentMonitor)
    XCTAssertEqual(drawer.frame, .zero)
    XCTAssertEqual(
      rail.frame.width, OverlayAgentDrawerLayout.railWidth(textScale: 1), accuracy: 1,
      "the fold fits a narrow window too")
    XCTAssertEqual(rail.frame.maxX, 700, accuracy: 1)

    state.unfoldAgentList()
    settle()
    XCTAssertEqual(
      state.agentsLayout, .rail, "no room beside the chat: the list covers it while asked for")
    XCTAssertEqual(drawer.frame.width, 700, accuracy: 1)
    state.chooseConversation(astra.id)
    settle()
    XCTAssertEqual(drawer.frame, .zero)
    XCTAssertEqual(
      rail.frame.width, OverlayAgentDrawerLayout.railWidth(textScale: 1), accuracy: 1,
      "and the choice returns to the rail")

    // Chosen on a wide window, the agents view stays the view when it narrows.
    state.setAgentsLayout(.agents)
    settle()
    XCTAssertFalse(state.showsAgentMonitor)
    XCTAssertEqual(drawer.frame.width, 700, accuracy: 1, "the agents view covers it unasked")
    XCTAssertEqual(rail.frame, .zero)
    state.chooseConversation(lena.id)
    settle()
    XCTAssertEqual(state.agentsLayout, .split)
    XCTAssertEqual(drawer.frame, .zero, "a chosen row gives the narrow window to the chat")
  }

  func testTheListHeaderCarriesBothCornerButtons() throws {
    let roster = try overlaySource("OverlayChannelStatusView.swift")
    let header = try section(
      of: roster, from: "private func scaledMonitorBody(", to: "} content: { rowFocus in")
    let left = try XCTUnwrap(header.range(of: "symbol: \"sidebar.left\""))
    let title = try XCTUnwrap(header.range(of: "Text(\"Agents\")"))
    let right = try XCTUnwrap(header.range(of: "symbol: \"sidebar.right\""))
    XCTAssertTrue(
      left.lowerBound < title.lowerBound && title.lowerBound < right.lowerBound,
      "sidebar.left in the leading corner, sidebar.right in the trailing one")
    let overlay = try overlaySource("DictationOverlayView.swift")
    let controls = try section(
      of: overlay, from: "private func agentsSidebarControls", to: "private func showAgentsCanvas")
    XCTAssertTrue(controls.contains("state.hideAgentsCanvas()"))
    XCTAssertTrue(controls.contains("onFoldList: { state.foldAgentList() }"))
    let rail = try section(
      of: overlay, from: "private func agentRail", to: "private func agentDrawerResizeHandle")
    XCTAssertTrue(rail.contains("onUnfold: { state.unfoldAgentList() }"))
    XCTAssertFalse(
      overlay.contains("overlay-agent-sidebar-show-conversation"),
      "the bottom corner button moved into the header")
  }

  /// The overlay over a suite of its own defaults, reporting the column's and
  /// the rail's frames.
  private func recordedPanel(
    state: OverlayState, width: CGFloat,
    drawer: OverlayDrawerFrameRecorder, rail: OverlayDrawerFrameRecorder
  ) throws -> (NSPanel, () -> Void) {
    let defaults = try XCTUnwrap(UserDefaults(suiteName: UUID().uuidString))
    let host = NSHostingView(
      rootView:
        DictationOverlayView(state: state)
        .defaultAppStorage(defaults)
        .onPreferenceChange(OverlayDrawerFramePreferenceKey.self) { drawer.frame = $0 }
        .onPreferenceChange(OverlayAgentRailFramePreferenceKey.self) { rail.frame = $0 }
        .frame(width: width, height: 500))
    host.sizingOptions = []
    let panel = NSPanel(
      contentRect: CGRect(x: 0, y: 0, width: width, height: 500),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.contentView = host
    BackgroundTestWindow.present(panel)
    // Long enough for the 0.24 s slide of a leaving pane to end.
    let settle = {
      host.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.35))
      host.layoutSubtreeIfNeeded()
    }
    settle()
    return (panel, settle)
  }

  func testNumberedRailButtonsStayOutsideResizeAndSelectWithoutUnfolding() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let (broadcast, lena, astra) = try rosterFixture()
    let savedCursor = NSCursor.current
    defer { savedCursor.set() }
    for scale: CGFloat in [0.8, 1, 1.6] {
      let state = OverlayState.previewFormatted()
      state.applyConversationSnapshot(
        .init(
          deliveries: [
            OverlayChannelDelivery(
              channel: "2", agent: "lena", deliveryID: "rail-lena", stage: .received, isOpen: false),
            OverlayChannelDelivery(
              channel: "3", agent: "astra", deliveryID: "rail-astra", stage: .received,
              isOpen: false),
          ], conversations: [broadcast, lena, astra]))
      state.applyChannelRoster(
        [broadcast, lena, astra].map {
          CsChannelRosterState(
            channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
            providerSessionId: $0.owner?.providerSessionID, open: false, loud: false,
            autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
        })
      state.selectConversation(lena.id)
      state.foldAgentList()
      state.conversationDrafts[lena.id] = "keep this draft"
      let generation = state.captureGeneration
      let host = NSHostingView(
        rootView: DictationOverlayView(state: state)
          .environment(\.csTextScale, scale)
          .defaultAppStorage(try XCTUnwrap(UserDefaults(suiteName: UUID().uuidString))))
      host.sizingOptions = []
      let panel = FloatingOverlayPanel(
        contentRect: NSRect(x: 100, y: 100, width: 1200, height: 500),
        styleMask: [.borderless, .nonactivatingPanel, .resizable], backing: .buffered,
        defer: false)
      panel.contentView = OverlayContentContainer(hosting: host)
      BackgroundTestWindow.present(panel)
      defer { panel.orderOut(nil) }
      host.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.1))
      for (digit, target) in [("3", astra.id), ("0", broadcast.id)] {
        let identifier = "overlay-agent-rail-\(digit)"
        let button = try XCTUnwrap(
          NativeAccessibilityProbe.waitForElements(identifier).elements.first)
        XCTAssertEqual(button.role, "AXButton")
        let screenTop = try XCTUnwrap(NSScreen.screens.first).frame.maxY
        let screenRect = NSRect(
          x: button.x, y: screenTop - button.y - button.height,
          width: button.width, height: button.height)
        let rect = panel.convertFromScreen(screenRect)
        XCTAssertLessThan(rect.maxX, panel.frame.width - OverlayResizeHit.band)
        for x in [rect.minX + 1, rect.midX, rect.maxX - 1] {
          let point = NSPoint(x: x, y: rect.midY)
          XCTAssertNil(OverlayResizeHit.edge(at: point, in: host.bounds))
          XCTAssertTrue(panel.cursor(at: point) === NSCursor.arrow)
          let event = try XCTUnwrap(
            NSEvent.mouseEvent(
              with: .mouseMoved, location: point, modifierFlags: [], timestamp: 1,
              windowNumber: panel.windowNumber, context: nil, eventNumber: 1,
              clickCount: 0, pressure: 0))
          panel.sendEvent(event)
          RunLoop.main.run(until: Date().addingTimeInterval(0.03))
          XCTAssertFalse(state.showsAgentMonitor, "hover must not replace the numbered target")
          XCTAssertEqual(state.agentsLayout, .rail)
          XCTAssertFalse(panel.isUserResizing)
        }
        _ = try NativeAccessibilityProbe.query(identifier, press: true)
        XCTAssertEqual(state.selectedConversationID, target)
        XCTAssertEqual(state.agentsLayout, .rail)
        XCTAssertFalse(state.showsAgentMonitor)
      }
      XCTAssertEqual(state.conversationDrafts[lena.id], "keep this draft")
      XCTAssertEqual(state.captureGeneration, generation)
      XCTAssertFalse(panel.isKeyWindow)
    }
  }

  /// The numbered pills must take real mouseDown/mouseUp through the panel —
  /// an AXPress proves the action exists, not that a click reaches it past
  /// the panel's own resize band and window-drag routing. Channel 0 leads:
  /// it is the first pill under the disclosure control.
  func testNumberedRailButtonsTakeNativeClicksWithoutResizeOrUnfolding() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let (broadcast, lena, astra) = try rosterFixture()
    for scale: CGFloat in [0.8, 1, 1.6] {
      let state = OverlayState.previewFormatted()
      state.applyConversationSnapshot(
        .init(
          deliveries: [
            OverlayChannelDelivery(
              channel: "2", agent: "lena", deliveryID: "rail-lena", stage: .received, isOpen: false),
            OverlayChannelDelivery(
              channel: "3", agent: "astra", deliveryID: "rail-astra", stage: .received,
              isOpen: false),
          ], conversations: [broadcast, lena, astra]))
      state.applyChannelRoster(
        [broadcast, lena, astra].map {
          CsChannelRosterState(
            channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
            providerSessionId: $0.owner?.providerSessionID, open: false, loud: false,
            autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
        })
      state.selectConversation(lena.id)
      state.foldAgentList()
      let host = NSHostingView(
        rootView: DictationOverlayView(state: state)
          .environment(\.csTextScale, scale)
          .defaultAppStorage(try XCTUnwrap(UserDefaults(suiteName: UUID().uuidString))))
      host.sizingOptions = []
      let panel = FloatingOverlayPanel(
        contentRect: NSRect(x: 100, y: 100, width: 1200, height: 500),
        styleMask: [.borderless, .nonactivatingPanel, .resizable], backing: .buffered,
        defer: false)
      panel.contentView = OverlayContentContainer(hosting: host)
      BackgroundTestWindow.present(panel)
      defer { panel.orderOut(nil) }
      host.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.1))
      let startFrame = panel.frame

      func windowRect(of identifier: String) throws -> NSRect {
        let element = try XCTUnwrap(
          NativeAccessibilityProbe.waitForElements(identifier).elements.first)
        let screenTop = try XCTUnwrap(NSScreen.screens.first).frame.maxY
        return panel.convertFromScreen(
          NSRect(
            x: element.x, y: screenTop - element.y - element.height,
            width: element.width, height: element.height))
      }
      func send(_ type: NSEvent.EventType, at point: NSPoint, clickCount: Int = 0) throws {
        let event = try XCTUnwrap(
          NSEvent.mouseEvent(
            with: type, location: point, modifierFlags: [], timestamp: 1,
            windowNumber: panel.windowNumber, context: nil, eventNumber: 1,
            clickCount: clickCount, pressure: type == .leftMouseUp ? 0 : 1))
        panel.sendEvent(event)
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
      }
      func nativeClick(at point: NSPoint) throws {
        try send(.leftMouseDown, at: point, clickCount: 1)
        XCTAssertFalse(panel.isUserResizing, "a pill click must not become a resize")
        try send(.leftMouseUp, at: point, clickCount: 1)
      }

      // Pointer travel from the disclosure control down to the first pill
      // keeps the pill as the click target; the list stays folded.
      let unfold = try windowRect(of: "overlay-agent-rail-unfold")
      let zero = try windowRect(of: "overlay-agent-rail-0")
      for step in 0...4 {
        let point = NSPoint(
          x: unfold.midX + (zero.midX - unfold.midX) * CGFloat(step) / 4,
          y: unfold.midY + (zero.midY - unfold.midY) * CGFloat(step) / 4)
        try send(.mouseMoved, at: point)
        XCTAssertFalse(state.showsAgentMonitor)
        XCTAssertEqual(state.agentsLayout, .rail)
      }

      // Unselected agent, then already-selected agent, then channel 0 twice:
      // every native click lands on its own conversation and nothing else.
      let three = try windowRect(of: "overlay-agent-rail-3")
      try nativeClick(at: NSPoint(x: three.midX, y: three.midY))
      XCTAssertEqual(state.selectedConversationID, astra.id)
      try nativeClick(at: NSPoint(x: three.midX, y: three.midY))
      XCTAssertEqual(state.selectedConversationID, astra.id)
      try nativeClick(at: NSPoint(x: zero.midX, y: zero.midY))
      XCTAssertEqual(state.selectedConversationID, broadcast.id)
      try nativeClick(at: NSPoint(x: zero.midX, y: zero.midY))
      XCTAssertEqual(state.selectedConversationID, broadcast.id)
      XCTAssertEqual(state.agentsLayout, .rail)
      XCTAssertFalse(state.showsAgentMonitor)
      XCTAssertEqual(panel.frame, startFrame)

      // The band itself still belongs to the native resize: a press there
      // starts a resize and selects nothing.
      let bandPoint = NSPoint(x: panel.frame.width - OverlayResizeHit.band / 2, y: zero.midY)
      try send(.leftMouseDown, at: bandPoint, clickCount: 1)
      XCTAssertTrue(panel.isUserResizing)
      XCTAssertEqual(state.selectedConversationID, broadcast.id)
      try send(.leftMouseUp, at: bandPoint, clickCount: 1)
      XCTAssertFalse(panel.isUserResizing)
      XCTAssertEqual(panel.frame, startFrame)
      XCTAssertFalse(panel.isKeyWindow)
    }
  }

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
    XCTAssertTrue(
      row.contains(".overlayFastHint(tooltip"),
      "descriptor and status use the same immediate hint as the header")
    XCTAssertTrue(
      visual.contains("overlay-channel-delivery-"), "the status dot keeps the delivery identifier")
    XCTAssertTrue(row.contains("overlay-agent-row-"), "row identifier kept")
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

  func testAgentRowStaysWithinFiveLinesAndKeepsUnreadApartFromReceipts() throws {
    // Maciej (2026-10-17): "ustalmy budżet 5 wierszy". Title, descriptor,
    // two preview lines, one state line; unread is a badge on the title line.
    let source = try overlaySource("OverlayChannelStatusView.swift")
    let label = try section(
      of: source, from: "private func rosterRowLabel(", to: "func rowStateText(")
    let visual = try overlaySource("OverlayAgentRosterLabel.swift")
    let limits = (visual + label).components(separatedBy: ".lineLimit(").dropFirst().compactMap {
      Int($0.prefix { $0.isNumber })
    }
    XCTAssertEqual(limits.reduce(0, +), 5, "every text in the row is capped, five lines in all")
    XCTAssertTrue(
      visual.contains("OverlayAgentUnreadBadge("), "unread rides the title line as a badge")
    XCTAssertFalse(label.contains("unread replies\")"), "no separate unread line")
    XCTAssertTrue(label.contains("rowStateText("), "receipt and reader share one state line")
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
  func testDockedRowDimsADisconnectedReaderAndSleepsItsMicrophone() throws {
    let source = try overlaySource("OverlayChannelStatusView.swift")
    let modifier = try section(
      of: source, from: "struct OverlayDisconnectedRowDim: ViewModifier {", to: "\n}\n")
    XCTAssertTrue(modifier.contains(".opacity(disconnected ? Self.opacity : 1)"))
    XCTAssertTrue(modifier.contains(".grayscale(disconnected ? 1 : 0)"))
    let docked = try section(
      of: source, from: "func channelRow(", to: "private func rosterRowLabel(")
    XCTAssertTrue(
      docked.contains("&& !hasDeadFollower(channel),"),
      "the docked row's microphone sleeps for a dead reader")
    XCTAssertTrue(
      docked.contains("OverlayDisconnectedRowDim(disconnected: hasDeadFollower(channel))"),
      "the docked row dims as one unit")
  }

  // MARK: - Hosting structure (deliverables 1 and 4)

  func testDockedColumnIsASiblingNotAnOverlayAndTheFloatingCardIsGone() throws {
    let overlay = try overlaySource("DictationOverlayView.swift")
    let canvas = try section(
      of: overlay, from: "private func canvasStack", to: "private var header: some View")
    XCTAssertFalse(
      canvas.contains("overlay-agent-drawer-dismiss"), "the floating card's backdrop is gone")
    // The roster enters the canvas only as the docked HStack sibling; the
    // actions handle's badge is the one remaining topTrailing overlay and it
    // stays empty of roster content.
    XCTAssertTrue(canvas.contains("maximumHeight: geometry.size.height, width: drawerWidth"))
    var rest = canvas
    while let range = rest.range(of: ".overlay(alignment: .topTrailing)") {
      let window = rest[range.lowerBound...].prefix(600)
      XCTAssertFalse(window.contains("monitorBody"), "the roster is not a floating overlay")
      XCTAssertFalse(window.contains("overlay-agent-sidebar"))
      rest = String(rest[range.upperBound...])
    }
    let column = try section(
      of: overlay, from: "private func agentDrawerColumn", to: "// MARK: Header")
    XCTAssertTrue(
      column.contains("var roster = channelStatusView")
        && column.contains("return roster.monitorBody("),
      "the column hosts the same roster body")
    XCTAssertTrue(column.contains("overlay-agent-sidebar"))
    XCTAssertTrue(
      canvas.contains("setAgentDrawerDocked"), "the canvas reports docked mode to the state")
    let conversation = try overlaySource("OverlayConversationView.swift")
    XCTAssertTrue(
      conversation.contains("OverlayAgentDrawerLayout.pillWidth(textScale: textScale)"),
      "the name pill keeps 224 × scale")
  }

  // MARK: - Wide-window evacuation (Maciej, 2026-10-17)

  /// The docked column already names the agent and carries its microphone and
  /// speaker, so the identity pill must leave with it: the host hands the
  /// column's visibility to the conversation view, and the view drops the
  /// whole navigation row (controls and pill) with a collapsed measurement.
  func testDockedColumnEvacuatesTheIdentityPillAndItsControls() throws {
    let overlay = try overlaySource("DictationOverlayView.swift")
    let canvas = try section(
      of: overlay, from: "private func canvasStack", to: "private var header: some View")
    XCTAssertTrue(
      canvas.contains("hidesNavigation: columnShown"),
      "the visible docked column is exactly when the pill evacuates")
    let conversation = try overlaySource("OverlayConversationView.swift")
    let anchor = try section(
      of: conversation, from: ".overlay(alignment: .top)", to: ".onChange(of: conversation.id)")
    XCTAssertTrue(
      anchor.contains("if hidesNavigation"),
      "the navigation row is conditional, not just transparent")
    XCTAssertTrue(
      anchor.contains("navigationHeight = 0"),
      "the content margin follows the evacuated row")
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
}

@MainActor
private final class OverlayDrawerFrameRecorder {
  var frame = CGRect.zero
}

@MainActor
private final class OverlayRosterHeightRecorder {
  var height: CGFloat = 0
}
