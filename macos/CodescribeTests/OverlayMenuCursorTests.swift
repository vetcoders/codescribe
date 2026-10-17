import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayMenuCursorTests: XCTestCase {
  func testSidebarDividerCursorWinsInsideDrawerAndYieldsWhenRemoved() {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 1000, height: 500),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 1000, height: 500))
    panel.contentView = content
    let drawer = OverlayMenuCursorRegionView(
      frame: NSRect(x: 700, y: 0, width: 300, height: 400))
    let divider = OverlayMenuCursorRegionView(
      frame: NSRect(x: 700, y: 0, width: 10, height: 400))
    divider.cursor = .resizeLeftRight
    content.addSubview(drawer)
    content.addSubview(divider)
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 705, y: 200)), .resizeLeftRight)
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 800, y: 200)), .arrow)
    XCTAssertEqual(
      panel.cursor(at: NSPoint(x: 705, y: 450)), .arrow,
      "the divider does not own the header")
    divider.removeFromSuperview()
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 705, y: 200)), .arrow)
  }

  func testHoverUsesPanelMotionAndClearsOnNativeExitAndRemoval() throws {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 100, y: 100, width: 300, height: 200),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 300, height: 200))
    panel.contentView = content
    let region = OverlayMenuCursorRegionView(frame: NSRect(x: 20, y: 30, width: 200, height: 40))
    var transitions: [Bool] = []
    region.onHover = { transitions.append($0) }
    content.addSubview(region)
    panel.orderFrontRegardless()
    content.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    defer { panel.close() }
    func move(_ point: NSPoint) throws {
      panel.sendEvent(
        try XCTUnwrap(
          NSEvent.mouseEvent(
            with: .mouseMoved, location: point, modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: panel.windowNumber,
            context: nil, eventNumber: 1, clickCount: 0, pressure: 0)))
    }
    try move(NSPoint(x: 100, y: 50))
    try move(NSPoint(x: 110, y: 50))
    XCTAssertEqual(transitions, [true], "Motion inside does not repeat the enter")
    try move(NSPoint(x: 350, y: 250))
    XCTAssertEqual(transitions, [true, false])
    try move(NSPoint(x: 100, y: 50))
    region.mouseExited(
      with: try XCTUnwrap(
        NSEvent.enterExitEvent(
          with: .mouseExited, location: NSPoint(x: 350, y: 250), modifierFlags: [],
          timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: panel.windowNumber,
          context: nil, eventNumber: 2, trackingNumber: 0, userData: nil)))
    XCTAssertEqual(transitions, [true, false, true, false])
    try move(NSPoint(x: 100, y: 50))
    region.removeFromSuperview()
    XCTAssertEqual(transitions, [true, false, true, false, true, false])
    XCTAssertTrue(panel.menuCursorRegions.allObjects.isEmpty)
  }

  func testProductionHeaderRegistersItsVisibleControlsWithThePanel() throws {
    let state = OverlayState()
    state.setPresentationMode(.midi)
    let panel = try XCTUnwrap(
      DictationOverlayWindow.make(
        state: state, textScale: TextScaleController(key: "OverlayMenuCursorTests.header"))
        as? FloatingOverlayPanel)
    defer {
      panel.orderOut(nil)
      panel.invalidatePresence()
    }
    panel.orderFrontRegardless()
    let content = try XCTUnwrap(panel.contentView)
    content.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    content.layoutSubtreeIfNeeded()
    let controls = panel.menuCursorRegions.allObjects.filter {
      $0.window === panel && !$0.isHiddenOrHasHiddenAncestor && !$0.visibleRect.isEmpty
    }
    XCTAssertGreaterThanOrEqual(
      controls.count, 4, "The real header must wire its controls, not only the drawer")
    let saved = NSCursor.current
    defer { saved.set() }
    for control in controls {
      let point = control.convert(NSPoint(x: control.bounds.midX, y: control.bounds.midY), to: nil)
      NSCursor.iBeam.set()
      XCTAssertTrue(panel.refreshCursor(at: point), "Mounted control at \(point)")
      XCTAssertEqual(NSCursor.current, .arrow)
    }
  }

  /// Founder, 2026-10-15: an I-beam showed between and over the agent's
  /// microphone and speaker, which float above message text. The pair and
  /// the gap between its buttons claim the arrow from the text below.
  func testAgentAudioControlsClaimTheArrowAboveText() throws {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 300, height: 120),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 300, height: 120))
    let text = NSTextView(frame: content.bounds)
    text.isHorizontallyResizable = false
    text.isVerticallyResizable = false
    text.isEditable = false
    text.string = String(repeating: "Wiadomość od agenta pod przyciskami. ", count: 12)
    content.addSubview(text)
    let controls = NSHostingView(
      rootView: OverlayAgentAudioControls(
        open: false, muted: false, microphoneEnabled: true, playbackEnabled: true,
        palette: .dark, onMicrophone: {}, onPlayback: {}))
    controls.frame = NSRect(x: 180, y: 70, width: 100, height: 40)
    content.addSubview(controls)
    panel.contentView = content
    content.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    content.layoutSubtreeIfNeeded()

    let region = try XCTUnwrap(
      panel.menuCursorRegions.allObjects.first { $0.window === panel },
      "the controls register a cursor region")
    let frame = region.convert(region.bounds, to: nil)
    XCTAssertGreaterThan(frame.width, 50, "both buttons and their gap, got \(frame)")
    let gap = NSPoint(x: frame.midX, y: frame.midY)
    XCTAssertEqual(panel.cursor(at: gap), .arrow, "region \(frame)")
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 40, y: 30)), .iBeam, "text keeps its cursor")
  }

  // MARK: - Pill stack, composer chips, bubble chips (Maciej, 2026-10-17 01:15Z)

  /// Maciej, 2026-10-17 01:15Z (channel 0, three screenshots): an I-beam sat
  /// on the agent pills and their microphone/speaker in the unfolded stack and
  /// on the attachment chip in the composer. The non-activating panel gets no
  /// native cursor updates, so the I-beam a text view set stays until the
  /// panel resets it, and the panel resets it only above registered regions.
  /// Each pill row claims the arrow the way the audio pair already does.
  func testAgentPillRowsClaimTheArrowAboveTheTranscript() throws {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 440, height: 260),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 440, height: 260))
    let text = NSTextView(frame: content.bounds)
    text.isHorizontallyResizable = false
    text.isVerticallyResizable = false
    text.isEditable = false
    text.string = String(repeating: "Transkrypcja pod stosem pigułek agentów. ", count: 14)
    content.addSubview(text)
    // Rows without audio controls: two listeners whose controls stayed home.
    // Any region found belongs to the row itself.
    func entry(_ channel: String, _ name: String, unread: Int) -> OverlayAgentPillEntry {
      OverlayAgentPillEntry(
        id: "channel-\(channel)", conversationID: "codex:\(name)", title: "\(channel) · \(name)",
        descriptor: nil, unread: unread, dot: .listening, quietSymbol: nil, isBroadcast: false,
        disconnected: false,
        controls: .init(
          channel: channel, open: false, muted: nil, microphoneEnabled: false,
          playbackEnabled: false, broadcast: false, speaking: false,
          onMicrophone: {}, onPlayback: {}))
    }
    let entries = [entry("2", "stasiu", unread: 5), entry("3", "lena", unread: 0)]
    let stack = NSHostingView(
      rootView: OverlayAgentPillStack(
        palette: .dark, entries: entries, broadcastMembers: [], broadcastEnabled: true,
        onSelect: { _ in }, onDismiss: {}))
    stack.frame = NSRect(x: 190, y: 30, width: 240, height: 210)
    content.addSubview(stack)
    panel.contentView = content
    content.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    content.layoutSubtreeIfNeeded()

    let rows = panel.menuCursorRegions.allObjects.filter {
      $0.window === panel && !$0.isHiddenOrHasHiddenAncestor && !$0.visibleRect.isEmpty
    }
    // The audio pair registers its own small regions; only the row-wide ones count.
    let frames = rows.map { $0.convert($0.bounds, to: nil) }.filter { $0.width > 120 }
    XCTAssertEqual(frames.count, entries.count, "one region per pill row, got \(frames)")
    let saved = NSCursor.current
    defer { saved.set() }
    for frame in frames {
      XCTAssertGreaterThan(frame.width, 120, "the row covers the pill, not a glyph: \(frame)")
      NSCursor.iBeam.set()
      XCTAssertTrue(panel.refreshCursor(at: NSPoint(x: frame.midX, y: frame.midY)), "row \(frame)")
      XCTAssertEqual(NSCursor.current, .arrow)
    }
    NSCursor.iBeam.set()
    XCTAssertFalse(
      panel.refreshCursor(at: NSPoint(x: 40, y: 30)), "the transcript keeps native tracking")
    XCTAssertEqual(NSCursor.current, .iBeam)
  }

  /// The composer's attachment chips sit above the editor whose tracking set
  /// the I-beam. The chip row claims the arrow; the editable field under it
  /// keeps its text cursor, like the drawer's live search does.
  func testComposerAttachmentChipsClaimTheArrowAndTheEditorKeepsItsTextCursor() throws {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 440, height: 180),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 440, height: 180))
    // Only pointers into Codescribe's own stores become pills, so the fixture
    // lives under the isolated config directory's assets store.
    let configDirectory = CodescribeConfig().configDir()
    guard configDirectory != NSHomeDirectory() + "/.codescribe" else {
      throw XCTSkip("needs an isolated CODESCRIBE_DATA_DIR, as make test-swift provides")
    }
    let store = URL(fileURLWithPath: configDirectory, isDirectory: true)
      .appendingPathComponent("assets", isDirectory: true)
    try FileManager.default.createDirectory(at: store, withIntermediateDirectories: true)
    let asset = store.appendingPathComponent("inline_cursor_\(UUID().uuidString).png")
    try Data([0x89, 0x50, 0x4E, 0x47]).write(to: asset)
    defer { try? FileManager.default.removeItem(at: asset) }
    let draft = ConversationDraft.compose(
      text: "Również pilla informacyjna", attachments: [asset.path])
    XCTAssertEqual(ConversationDraft.split(draft).attachments.count, 1, "fixture is a pill")
    let composer = NSHostingView(
      rootView: OverlayConversationComposer(
        palette: .dark, draft: .constant(draft), sending: false, onSubmit: {}))
    composer.frame = NSRect(x: 10, y: 10, width: 420, height: 160)
    content.addSubview(composer)
    panel.contentView = content
    content.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    content.layoutSubtreeIfNeeded()

    let chips = panel.menuCursorRegions.allObjects.filter {
      $0.window === panel && !$0.isHiddenOrHasHiddenAncestor && !$0.visibleRect.isEmpty
    }
    let chipRow = try XCTUnwrap(chips.first, "the attachment row registers a cursor region")
    XCTAssertEqual(chips.count, 1, "only the chip row, never the editor")
    let frame = chipRow.convert(chipRow.bounds, to: nil)
    let saved = NSCursor.current
    defer { saved.set() }
    NSCursor.iBeam.set()
    XCTAssertTrue(panel.refreshCursor(at: NSPoint(x: frame.midX, y: frame.midY)), "chips \(frame)")
    XCTAssertEqual(NSCursor.current, .arrow)

    let editor = try XCTUnwrap(editableTextView(in: content), "the composer mounts its editor")
    let typing = editor.convert(NSPoint(x: editor.bounds.midX, y: editor.bounds.midY), to: nil)
    XCTAssertFalse(frame.contains(typing), "chips do not cover the editor: \(frame) vs \(typing)")
    XCTAssertEqual(panel.cursor(at: typing), .iBeam)
    NSCursor.iBeam.set()
    XCTAssertFalse(panel.refreshCursor(at: typing), "typing keeps native tracking")
    XCTAssertEqual(NSCursor.current, .iBeam)
  }

  /// A bubble's attachment chip floats above message text in the same panel.
  func testConversationAttachmentChipClaimsTheArrowAboveMessageText() throws {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 300, height: 120),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 300, height: 120))
    let text = NSTextView(frame: content.bounds)
    text.isHorizontallyResizable = false
    text.isVerticallyResizable = false
    text.isEditable = false
    text.string = String(repeating: "Wiadomość z załącznikiem pod chipem. ", count: 12)
    content.addSubview(text)
    let chip = NSHostingView(
      rootView: AttachmentChip(
        attachment: MessageAttachment(
          name: "inline_d0d0…09372302.png", url: URL(fileURLWithPath: "/tmp/inline_d0d0.png"),
          type: "image/png")))
    chip.frame = NSRect(x: 120, y: 70, width: 170, height: 32)
    content.addSubview(chip)
    panel.contentView = content
    content.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.05))
    content.layoutSubtreeIfNeeded()

    let region = try XCTUnwrap(
      panel.menuCursorRegions.allObjects.first { $0.window === panel },
      "the chip registers a cursor region")
    let frame = region.convert(region.bounds, to: nil)
    XCTAssertGreaterThan(frame.width, 60, "thumbnail and name, got \(frame)")
    let saved = NSCursor.current
    defer { saved.set() }
    NSCursor.iBeam.set()
    XCTAssertTrue(panel.refreshCursor(at: NSPoint(x: frame.midX, y: frame.midY)), "chip \(frame)")
    XCTAssertEqual(NSCursor.current, .arrow)
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 40, y: 30)), .iBeam, "text keeps its cursor")
  }

  private func editableTextView(in view: NSView) -> NSTextView? {
    if let text = view as? NSTextView, text.isEditable { return text }
    for child in view.subviews {
      if let found = editableTextView(in: child) { return found }
    }
    return nil
  }

  func testDrawerTextInputKeepsItsTextCursor() {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 500))
    panel.contentView = content
    content.addSubview(
      OverlayMenuCursorRegionView(
        frame: NSRect(x: 300, y: 100, width: 280, height: 350)))
    let search = NSTextField(frame: NSRect(x: 320, y: 400, width: 220, height: 24))
    search.isEditable = true
    content.addSubview(search)
    let point = NSPoint(x: 350, y: 412)
    XCTAssertTrue(content.hitTest(point) === search)
    XCTAssertEqual(
      panel.cursor(at: point), .iBeam, "Live search remains a text input inside chrome")
    let saved = NSCursor.current
    defer { saved.set() }
    NSCursor.iBeam.set()
    XCTAssertFalse(panel.refreshCursor(at: point), "The panel leaves native input tracking alone")
    XCTAssertEqual(NSCursor.current, .iBeam)
  }

  func testHeaderAndDrawerKeepIndependentCursorRegions() {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel, .resizable],
      backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 500))
    panel.contentView = content
    let header = OverlayMenuCursorRegionView(frame: NSRect(x: 100, y: 450, width: 22, height: 22))
    let drawer = OverlayMenuCursorRegionView(frame: NSRect(x: 300, y: 100, width: 280, height: 350))
    content.addSubview(header)
    content.addSubview(drawer)
    let saved = NSCursor.current
    defer {
      panel.close()
      saved.set()
    }

    NSCursor.iBeam.set()
    XCTAssertTrue(panel.refreshCursor(at: NSPoint(x: 110, y: 460)))
    XCTAssertEqual(NSCursor.current, .arrow, "Mounting the drawer cannot disown header controls")
    NSCursor.iBeam.set()
    XCTAssertTrue(panel.refreshCursor(at: NSPoint(x: 400, y: 200)))
    XCTAssertEqual(NSCursor.current, .arrow)

    drawer.removeFromSuperview()
    NSCursor.iBeam.set()
    XCTAssertTrue(panel.refreshCursor(at: NSPoint(x: 110, y: 460)))
    XCTAssertEqual(NSCursor.current, .arrow, "Removing one region retains the other")
    header.isHidden = true
    NSCursor.iBeam.set()
    XCTAssertFalse(panel.refreshCursor(at: NSPoint(x: 110, y: 460)))
    XCTAssertEqual(NSCursor.current, .iBeam, "Hidden chrome cannot own the cursor")
    header.isHidden = false
    header.removeFromSuperview()
    XCTAssertFalse(panel.refreshCursor(at: NSPoint(x: 110, y: 460)))
  }

  func testMountedMenuOwnsArrowAboveSelectableTextAndReleasesOnRemoval() {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 500))
    let text = NSTextView(frame: content.bounds)
    // This fixture tests panel hit ownership, not TextKit's fit-to-content
    // resizing. Keep the selectable surface fixed while layout is realized.
    text.isHorizontallyResizable = false
    text.isVerticallyResizable = false
    text.isEditable = false
    text.string = "Selectable transcript behind the drawer"
    content.addSubview(text)
    panel.contentView = content
    text.frame = content.bounds
    if let container = text.textContainer {
      text.layoutManager?.ensureLayout(for: container)
    }
    content.layoutSubtreeIfNeeded()
    let menu = OverlayMenuCursorRegionView(frame: NSRect(x: 300, y: 100, width: 280, height: 350))
    content.addSubview(menu)

    let outsideMenu = text.convert(NSPoint(x: 100, y: 200), to: nil)
    XCTAssertTrue(
      content.hitTest(outsideMenu) === text,
      "Fixture must hit native text outside menu; text frame=\(text.frame), visible=\(text.visibleRect), menu frame=\(menu.frame)"
    )
    XCTAssertTrue(panel.menuCursorRegions.contains(menu))
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 400, y: 200)), .arrow)
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 100, y: 200)), .iBeam)
    XCTAssertEqual(panel.cursor(at: outsideMenu), .iBeam)
    XCTAssertNil(menu.hitTest(NSPoint(x: 10, y: 10)), "Drawer controls retain their own hit path")

    menu.isHidden = true
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 400, y: 200)), .iBeam)
    menu.isHidden = false
    menu.removeFromSuperview()
    XCTAssertTrue(panel.menuCursorRegions.allObjects.isEmpty)
    XCTAssertEqual(panel.cursor(at: NSPoint(x: 400, y: 200)), .iBeam)
  }

  func testMenuMotionDoesNotDispatchOverlappingTextTrackingAndReleasesOwnership() throws {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 100, y: 100, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel, .resizable],
      backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 500))
    let text = DrawerCursorTrackingTextView(frame: content.bounds)
    text.isHorizontallyResizable = false
    text.isVerticallyResizable = false
    text.isEditable = false
    text.string = "Selectable transcript behind the agent drawer"
    content.addSubview(text)
    panel.contentView = content
    text.frame = content.bounds
    panel.acceptsMouseMovedEvents = true
    panel.orderFrontRegardless()
    let saved = NSCursor.current
    defer {
      panel.orderOut(nil)
      saved.set()
    }
    text.addTrackingArea(
      NSTrackingArea(
        rect: text.bounds, options: [.mouseMoved, .activeAlways], owner: text, userInfo: nil))
    let selection = NSRange(location: 2, length: 5)
    text.setSelectedRange(selection)
    let menu = OverlayMenuCursorRegionView(frame: NSRect(x: 300, y: 100, width: 280, height: 350))
    content.addSubview(menu)
    let button = NSButton(title: "Agent", target: nil, action: nil)
    button.frame = NSRect(x: 320, y: 120, width: 120, height: 30)
    content.addSubview(button)
    XCTAssertTrue(content.hitTest(NSPoint(x: 350, y: 135)) === button)

    func move(to point: NSPoint) throws {
      let event = try XCTUnwrap(
        NSEvent.mouseEvent(
          with: .mouseMoved, location: point, modifierFlags: [],
          timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: panel.windowNumber,
          context: nil, eventNumber: 1, clickCount: 0, pressure: 0))
      panel.sendEvent(event)
    }

    // Prove the mounted native tracking path is active before testing its exclusion.
    try move(to: NSPoint(x: 100, y: 200))
    XCTAssertGreaterThan(text.motions, 0)
    text.motions = 0
    let inside = NSPoint(x: 400, y: 200)
    for _ in 0..<20 {
      try move(to: inside)
      XCTAssertEqual(NSCursor.current, .arrow)
    }
    XCTAssertEqual(text.motions, 0, "Drawer motion must not first set the text cursor")
    XCTAssertEqual(text.selectedRange(), selection)
    XCTAssertFalse(panel.refreshCursor(at: inside), "An unchanged arrow needs no second write")

    try move(to: NSPoint(x: 100, y: 200))
    XCTAssertGreaterThan(text.motions, 0, "Text outside the drawer retains native motion")
    text.motions = 0
    menu.isHidden = true
    try move(to: inside)
    XCTAssertGreaterThan(text.motions, 0, "Hiding the drawer releases native tracking")
    text.motions = 0
    menu.isHidden = false
    try move(to: inside)
    XCTAssertEqual(text.motions, 0, "Showing the drawer restores exclusive cursor ownership")
    menu.removeFromSuperview()
    try move(to: inside)
    XCTAssertGreaterThan(text.motions, 0, "Removing the drawer releases native tracking")
    XCTAssertEqual(text.selectedRange(), selection)
  }

  func testMenuDoesNotTakeResizeOwnershipFromPanelEdges() {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel, .resizable], backing: .buffered, defer: false)
    defer { panel.close() }
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 500))
    panel.contentView = content
    content.addSubview(OverlayMenuCursorRegionView(frame: content.bounds))

    XCTAssertEqual(panel.cursor(at: NSPoint(x: 400, y: 200)), .arrow)
    XCTAssertEqual(
      panel.cursor(at: NSPoint(x: 599, y: 200)), OverlayResizeHit.cursor(for: .right))
    XCTAssertEqual(
      panel.cursor(at: NSPoint(x: 400, y: 499)), OverlayResizeHit.cursor(for: .top))
  }
}

@MainActor
private final class DrawerCursorTrackingTextView: NSTextView {
  var motions = 0

  override func mouseMoved(with event: NSEvent) {
    motions += 1
    NSCursor.iBeam.set()
    super.mouseMoved(with: event)
  }
}
