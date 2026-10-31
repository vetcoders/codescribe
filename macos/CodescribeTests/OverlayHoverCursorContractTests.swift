import AppKit
import ObjectiveC
import SwiftUI
import XCTest

@testable import Codescribe

/// Hover-ink and pointer contract for the overlay's audio/panel controls and
/// the composer's corner controls. Written independently of the repair: the
/// author of the fix does not write its tests.
///
/// Maciej, 2026-10-31 21:48 CET: "nadal nie ma właściwych hoverów!! przyciski
/// są w stanie spoczynku takie jak powinny być na hover!! … wyślij, mikrofon
/// czy akcje przy wyślij … mają kursor tekstowy po najechaniu!"
/// Maciej, 2026-10-23 (quoted in OverlayAgentRosterLayoutTests): "Przydym też
/// troszkę te hiperbiałe przyciski".
///
/// Contract A: an available, unlit control rests visibly apart from its hover;
/// hover is lighter on the dark canvas and darker on the light one; the rest
/// stays readable on every sheet shade and distinct from disabled. Lit and
/// disabled inks keep their current values.
/// Contract B: over "+", the microphone, Send and the recipient chevron the
/// pointer is an arrow or a pointing hand, never the I-beam, in both the main
/// overlay panel and the Intercom panel. The text between the corners keeps its
/// I-beam, and the controls keep their own clicks.
@MainActor
final class OverlayHoverCursorContractTests: XCTestCase {
  // MARK: Thresholds

  /// Rest and hover differ by at least this WCAG ratio. Today the dark rest is
  /// the primary ink and its hover only 25 % whiter: 1.03:1, invisible. The
  /// light pair (muted → primary) measures 2.0:1 and the same pair in dark
  /// 2.3:1, so 1.5 admits any rest between muted and primary yet rejects an
  /// "almost primary" rest. With the sheet floor below, across all shades the
  /// admissible rest luminance is 0.24…0.58 (dark) and 0.04…0.11 (light);
  /// muted (0.36 and 0.07) sits inside both.
  static let restToHoverMinimum = 1.5
  /// The text floor Tokens.swift proves for its ink rungs. eb71d5617 gave the
  /// available controls a readable idle ink; a dimmer rest must keep that on
  /// every sheet shade, including warm carbon and travertine.
  static let restToSheetMinimum = 4.5
  /// An available control must not be read as a disabled one (muted at 35 %).
  /// Muted against disabled measures 3.3–4.4:1 across all shades.
  static let restToDisabledMinimum = 2.0
  /// Pins of the unchanged states. 8/255 per channel absorbs colour-space
  /// round trips; lit rest and lit hover differ by 18/255 (dark) and 42/255
  /// (light), so a pin still tells them apart.
  static let pinTolerance = 8.0 / 255

  static let audioNames = ["microphone", "speaker", "panel"]
  static let composerNames = ["plus", "microphone", "send", "recipients"]

  // MARK: A1 — rest versus hover

  /// RED today in dark: rest F4F2EC (primary), hover primary⊕25 % white,
  /// 1.03:1. Light passes (4B4D46 → 1D1B17, 2.0:1, darker).
  /// Kills (a) `idleInk = primaryText` restored in dark, and (b) hover equal
  /// to rest, in either appearance.
  func testAvailableAudioAndPanelControlsRestVisiblyApartFromHover() throws {
    let cases: [(OverlayAppearancePalette, SheetShade)] = [
      (.dark, .todayDark), (.light, .todayLight),
    ]
    for (palette, shade) in cases {
      let dark = palette.appearance == .dark
      let fixture = AudioControlsFixture(palette: palette, sheet: shade.hex, state: .available)
      defer { fixture.close() }
      let squares = try fixture.squares()
      let resting = try fixture.render()
      for (name, square) in zip(Self.audioNames, squares) {
        let rest = try fixture.ink(in: square, of: resting)
        fixture.hover(at: NSPoint(x: square.midX, y: square.midY))
        let hover = try fixture.ink(in: square, of: try fixture.render())
        let ratio = contrast(rest, hover)
        XCTAssertGreaterThanOrEqual(
          ratio, Self.restToHoverMinimum,
          "\(palette.appearance) \(name): rest \(hex(rest)) and hover \(hex(hover)) differ by "
            + "\(ratio):1; the resting control already looks hovered")
        if dark {
          XCTAssertGreaterThan(
            lightness(hover), lightness(rest),
            "dark \(name): hover \(hex(hover)) must be lighter than rest \(hex(rest))")
        } else {
          XCTAssertLessThan(
            lightness(hover), lightness(rest),
            "light \(name): hover \(hex(hover)) must be darker than rest \(hex(rest))")
        }
      }
      fixture.hover(at: nil)
    }
  }

  // MARK: A2 — readable rest (guard for eb71d5617)

  /// GREEN today (dark rest is primary, light rest muted). Kills a repair that
  /// dims the rest into the canvas (faint ink, muted at low opacity) or makes
  /// it indistinguishable from the disabled state.
  func testRestingInkStaysReadableOnEverySheetAndApartFromDisabled() throws {
    for palette in [OverlayAppearancePalette.dark, .light] {
      let shades = palette.appearance == .dark ? SheetShade.darkChoices : SheetShade.lightChoices
      for shade in shades {
        let available = AudioControlsFixture(palette: palette, sheet: shade.hex, state: .available)
        let disabled = AudioControlsFixture(palette: palette, sheet: shade.hex, state: .disabled)
        defer {
          available.close()
          disabled.close()
        }
        let sheet = OverlayColorToken(shade.hex)
        let availableBitmap = try available.render()
        let disabledBitmap = try disabled.render()
        let pairs = zip(try available.squares(), try disabled.squares())
        for (name, (on, off)) in zip(Self.audioNames, pairs) {
          let rest = try available.ink(in: on, of: availableBitmap)
          let unavailable = try disabled.ink(in: off, of: disabledBitmap)
          XCTAssertGreaterThanOrEqual(
            contrast(rest, sheet), Self.restToSheetMinimum,
            "\(shade) \(name): rest \(hex(rest)) is \(contrast(rest, sheet)):1 on the sheet")
          XCTAssertGreaterThanOrEqual(
            contrast(rest, unavailable), Self.restToDisabledMinimum,
            "\(shade) \(name): available \(hex(rest)) reads as disabled \(hex(unavailable))")
        }
      }
    }
  }

  // MARK: A3 — lit and disabled stay as they are

  /// GREEN today. Lit rests in the agent accent and hovers 30 % toward the
  /// primary ink; disabled rests at muted 35 % and ignores hover. Kills a
  /// repair that solves the unlit rest by rewriting every state.
  func testLitAndDisabledControlsKeepTheirCurrentInk() throws {
    let cases: [(OverlayAppearancePalette, SheetShade)] = [
      (.dark, .todayDark), (.light, .todayLight),
    ]
    for (palette, shade) in cases {
      let sheet = OverlayColorToken(shade.hex)
      let lit = AudioControlsFixture(palette: palette, sheet: shade.hex, state: .lit)
      defer { lit.close() }
      let accent = NSColor(palette.agentAccent.color)
      let litHover = token(
        accent.blended(withFraction: 0.3, of: palette.primaryText.nsColor) ?? accent)
      let litSquares = try lit.squares()
      let litRest = try lit.render()
      // The open microphone and the speaking speaker; the panel is never lit.
      for (name, square) in zip(["microphone", "speaker"], litSquares) {
        let rest = try lit.ink(in: square, of: litRest)
        lit.hover(at: NSPoint(x: square.midX, y: square.midY))
        let hover = try lit.ink(in: square, of: try lit.render())
        XCTAssertLessThanOrEqual(
          channelDelta(rest, palette.agentAccent), Self.pinTolerance,
          "\(shade) lit \(name) rest \(hex(rest)) left the accent \(hex(palette.agentAccent))")
        XCTAssertLessThanOrEqual(
          channelDelta(hover, litHover), Self.pinTolerance,
          "\(shade) lit \(name) hover \(hex(hover)) left accent⊕30 % primary \(hex(litHover))")
      }
      lit.hover(at: nil)

      let disabled = AudioControlsFixture(palette: palette, sheet: shade.hex, state: .disabled)
      defer { disabled.close() }
      let expected = token(
        OverlayColorToken(palette.mutedText.rgb, alpha: 0.35).composited(over: sheet))
      let disabledSquares = try disabled.squares()
      let disabledRest = try disabled.render()
      for (name, square) in zip(Self.audioNames, disabledSquares) {
        let rest = try disabled.ink(in: square, of: disabledRest)
        disabled.hover(at: NSPoint(x: square.midX, y: square.midY))
        let hover = try disabled.ink(in: square, of: try disabled.render())
        XCTAssertLessThanOrEqual(
          channelDelta(rest, expected), Self.pinTolerance,
          "\(shade) disabled \(name) rest \(hex(rest)) left muted@35 % \(hex(expected))")
        XCTAssertLessThanOrEqual(
          channelDelta(hover, rest), 3.0 / 255,
          "\(shade) disabled \(name) must not answer hover: \(hex(rest)) → \(hex(hover))")
      }
      disabled.hover(at: nil)
    }
  }

  // MARK: B1 — the overlay panel's own pointer model

  /// Main overlay panel only (the Intercom panel has no pointer model).
  /// Under H1 (the hosting view's hitTest over a SwiftUI control in an
  /// `.overlay` returns the MessageTextView below it) this is RED today: the
  /// menu region yields to "editable text" and the model resolves the I-beam.
  /// Under H2 (hitTest returns the hosting view) it is GREEN today and acts as
  /// a guard. Kills (d) an arrow over the whole field (middle assertions) and
  /// (e) a control with hit testing switched off (hit-test assertions).
  func testOverlayPanelModelGivesControlsTheArrowAndTheTextMiddleTheIBeam() throws {
    let surface = try ComposerSurface(.overlay)
    defer { surface.close() }
    let panel = try XCTUnwrap(surface.panel as? FloatingOverlayPanel)
    let content = try XCTUnwrap(panel.contentView)
    let saved = NSCursor.current
    defer { saved.set() }

    let middle = try surface.textMiddle()
    XCTAssertEqual(panel.cursor(at: middle), .iBeam, "(d) the writable middle keeps the I-beam")
    XCTAssertTrue(
      content.hitTest(content.convert(middle, from: nil)) === surface.editor,
      "(d) the writable middle still hits the editor")

    for (name, point) in try surface.controlCenters() {
      let modelled = panel.cursor(at: point)
      XCTAssertTrue(
        modelled == .arrow || modelled == .pointingHand,
        "overlay \(name): the panel resolves \(modelled) over the control")
      let hit = content.hitTest(content.convert(point, from: nil))
      XCTAssertNotNil(hit, "(e) overlay \(name): nothing takes the control's hit")
      if let hit {
        XCTAssertTrue(hit.isDescendant(of: surface.host), "overlay \(name): hit \(hit)")
        XCTAssertFalse(
          hit.isDescendant(of: surface.textScope),
          "(e) overlay \(name): the control's point hits the text view \(hit) below it")
      }
      // A real approach: text first, then the control, then AppKit's update.
      NSCursor.iBeam.set()
      panel.sendEvent(try pointerEvent(.mouseMoved, at: middle, in: panel))
      panel.sendEvent(try pointerEvent(.mouseMoved, at: point, in: panel))
      XCTAssertNotEqual(NSCursor.current, .iBeam, "overlay \(name) after mouseMoved")
      panel.sendEvent(try pointerEvent(.cursorUpdate, at: point, in: panel))
      XCTAssertNotEqual(NSCursor.current, .iBeam, "overlay \(name) after cursorUpdate")
      XCTAssertFalse(
        panel.refreshCursor(at: middle), "(d) returning to the text must not write the arrow")
    }
  }

  // MARK: B2 — the controls keep their own clicks

  /// GREEN today; both surfaces. Kills (e): a repair that makes the arrow by
  /// removing the control from hit testing hands its clicks to the editor.
  func testComposerControlsKeepTheirOwnClicksOnBothSurfaces() throws {
    for kind in [Surface.overlay, .intercom] {
      let surface = try ComposerSurface(kind, probe: true)
      defer { surface.close() }
      BackgroundTestWindow.present(surface.panel)
      surface.settle()
      let end = NSRange(location: (surface.editor.string as NSString).length, length: 0)
      surface.editor.setSelectedRange(end)
      try click(at: try surface.probeCenter(), in: surface.panel)
      guard surface.counters.probe == 1 else {
        XCTFail("\(kind) fixture: a synthetic click did not reach a plain SwiftUI button")
        continue
      }
      let controls = Dictionary(uniqueKeysWithValues: try surface.controlCenters())
      // "+" opens NSOpenPanel and the chevron a modal menu: neither is clicked here.
      try click(at: try XCTUnwrap(controls["microphone"]), in: surface.panel)
      try click(at: try XCTUnwrap(controls["send"]), in: surface.panel)
      XCTAssertEqual(surface.counters.microphone, 1, "(e) \(kind): the microphone lost its click")
      XCTAssertEqual(surface.counters.sends, 1, "(e) \(kind): Send lost its click")
      XCTAssertEqual(surface.editor.string, "Szkic", "\(kind): a control click edited the draft")
      XCTAssertEqual(
        surface.editor.selectedRange(), end, "\(kind): a control click moved the caret")
    }
  }

  // MARK: B3 — AppKit's native cursor sources, per surface

  /// RED today if the composer's NSTextView/NSClipView declares an I-beam
  /// cursor rect over its visible rect: the corner controls are drawn inside
  /// that rect. AppKit re-applies cursor rects outside `sendEvent` (key
  /// change, invalidation after typing), so the panel's no-ping-pong model
  /// cannot correct them. Kills (c) together with the Intercom twin, and (d).
  /// Skips when this host exposes neither native source on a plain NSTextView.
  func testOverlayPanelNativeCursorSourcesLeaveTheControlsAlone() throws {
    try assertNativeCursorSources(.overlay)
  }

  /// RED today: the Intercom panel has no pointer model, so both cursor rects
  /// and `.cursorUpdate` tracking areas of the text view act unfiltered over
  /// the corner controls. Kills (c): a repair confined to FloatingOverlayPanel
  /// (its sendEvent filter or `cursor(at:)`) leaves this red. Kills (d).
  func testIntercomPanelNativeCursorSourcesLeaveTheControlsAlone() throws {
    try assertNativeCursorSources(.intercom)
  }

  // MARK: B4 — real pointer (isolated GUI only)

  /// The only end-to-end probe: the real pointer and AppKit's own cursor
  /// machinery, including the cursor-rect reset that typing triggers. Expected
  /// RED today per Maciej's report. Skips outside an isolated GUI session, and
  /// when posted pointer events do not reach the app (no permission).
  func testRealPointerShowsNoIBeamOverComposerControlsOnBothSurfaces() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let saved = NSCursor.current
    defer { saved.set() }
    for kind in [Surface.overlay, .intercom] {
      let surface = try ComposerSurface(kind)
      defer { surface.close() }
      BackgroundTestWindow.present(surface.panel)
      if let overlay = surface.panel as? FloatingOverlayPanel {
        overlay.takeKeyForTranscript()
      } else {
        surface.panel.makeKey()
      }
      surface.panel.makeFirstResponder(surface.editor)
      surface.settle()
      let middle = try surface.textMiddle()
      NSCursor.arrow.set()
      movePointer(to: middle, in: surface.panel)
      guard NSCursor.current == .iBeam else {
        throw XCTSkip("\(kind): posted pointer motion did not reach the app; nothing to judge")
      }
      for (name, point) in try surface.controlCenters() {
        movePointer(to: point, in: surface.panel)
        XCTAssertNotEqual(
          NSCursor.current, .iBeam, "\(kind) \(name): real pointer over the control")
        surface.editor.insertText("x", replacementRange: surface.editor.selectedRange())
        surface.settle()
        XCTAssertNotEqual(
          NSCursor.current, .iBeam, "\(kind) \(name): typing re-applied the I-beam over the control"
        )
        movePointer(to: middle, in: surface.panel)
        XCTAssertEqual(NSCursor.current, .iBeam, "(d) \(kind): the text middle lost its I-beam")
      }
    }
  }

  // MARK: - Native cursor sources

  private func assertNativeCursorSources(_ kind: Surface) throws {
    let reference = try referenceTextCursorSources(kind)
    guard reference.rects || reference.tracking else {
      throw XCTSkip(
        "\(kind): a plain NSTextView declares the I-beam through neither cursor rects nor "
          + "cursorUpdate tracking in this host; the native sources cannot be judged here")
    }
    let surface = try ComposerSurface(kind)
    defer { surface.close() }
    let saved = NSCursor.current
    defer { saved.set() }
    refreshTrackingAreas(surface.panel.contentView)
    let beams = CursorRectLedger.declared(in: surface.panel).filter { $0.cursor == .iBeam }
      .map { $0.rect }
    let middle = try surface.textMiddle()
    // The overlay panel passes text-middle motion through to AppKit, so both
    // sources apply there. Over a control only the Intercom panel lacks a
    // filter; the overlay panel's filter is judged in B1.
    let middleTracking = try cursorAfterTrackingUpdate(at: middle, in: surface.panel)
    XCTAssertTrue(
      beams.contains { $0.contains(middle) } || middleTracking == .iBeam,
      "(d) \(kind): no native source gives the writable middle its I-beam")
    for (name, point) in try surface.controlCenters() {
      let covering = beams.filter { $0.contains(point) }
      XCTAssertTrue(
        covering.isEmpty,
        "\(kind) \(name): I-beam cursor rect(s) \(covering) cover the control at \(point)")
      if kind == .intercom {
        let tracked = try cursorAfterTrackingUpdate(at: point, in: surface.panel)
        XCTAssertNotEqual(
          tracked, .iBeam, "\(kind) \(name): a cursorUpdate owner sets the I-beam over the control")
      }
    }
  }

  /// Whether this host lets the test observe the two native sources at all,
  /// measured on an ordinary editable NSTextView in the same panel class.
  private func referenceTextCursorSources(_ kind: Surface) throws -> (rects: Bool, tracking: Bool) {
    let frame = NSRect(x: 240, y: 240, width: 300, height: 120)
    let panel: NSPanel
    switch kind {
    case .overlay:
      panel = FloatingOverlayPanel(
        contentRect: frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered,
        defer: false)
    case .intercom:
      let intercom = OverlayIntercomPanel()
      intercom.setFrame(frame, display: false)
      intercom.acceptsConversationFocus = true
      panel = intercom
    }
    panel.isReleasedWhenClosed = false
    let content = NSView(frame: NSRect(origin: .zero, size: frame.size))
    let scroll = NSScrollView(frame: content.bounds)
    let text = NSTextView(frame: scroll.bounds)
    text.isEditable = true
    text.string = "Reference"
    scroll.documentView = text
    content.addSubview(scroll)
    panel.contentView = content
    defer {
      panel.makeFirstResponder(nil)
      panel.contentView = nil
      panel.close()
    }
    panel.makeFirstResponder(text)
    content.layoutSubtreeIfNeeded()
    refreshTrackingAreas(content)
    let center = scroll.convert(NSPoint(x: scroll.bounds.midX, y: scroll.bounds.midY), to: nil)
    let saved = NSCursor.current
    defer { saved.set() }
    let rects = CursorRectLedger.declared(in: panel).contains {
      $0.cursor == .iBeam && $0.rect.contains(center)
    }
    let tracking = try cursorAfterTrackingUpdate(at: center, in: panel) == .iBeam
    return (rects, tracking)
  }

  /// AppKit's tracking-area source: owners of `.cursorUpdate` areas under the
  /// point receive `cursorUpdate(with:)`. Delivered back to front, so the
  /// frontmost owner writes last, as AppKit picks it. Nil: no such owner.
  private func cursorAfterTrackingUpdate(at point: NSPoint, in window: NSWindow) throws -> NSCursor?
  {
    guard let root = window.contentView else { return nil }
    var owners: [NSResponder] = []
    func visit(_ view: NSView) {
      for area in view.trackingAreas where area.options.contains(.cursorUpdate) {
        let rect = area.options.contains(.inVisibleRect) ? view.visibleRect : area.rect
        if view.convert(rect, to: nil).contains(point), let owner = area.owner as? NSResponder {
          owners.append(owner)
        }
      }
      for child in view.subviews { visit(child) }
    }
    visit(root)
    guard !owners.isEmpty else { return nil }
    NSCursor.arrow.set()
    let event = try pointerEvent(.cursorUpdate, at: point, in: window)
    for owner in owners { owner.cursorUpdate(with: event) }
    return NSCursor.current
  }

  private func refreshTrackingAreas(_ view: NSView?) {
    guard let view else { return }
    view.updateTrackingAreas()
    for child in view.subviews { refreshTrackingAreas(child) }
  }

  // MARK: - Events

  private func pointerEvent(
    _ type: NSEvent.EventType, at point: NSPoint, in window: NSWindow, number: Int = 1
  ) throws -> NSEvent {
    let now = ProcessInfo.processInfo.systemUptime
    let event =
      type == .cursorUpdate
      ? NSEvent.enterExitEvent(
        with: type, location: point, modifierFlags: [], timestamp: now,
        windowNumber: window.windowNumber, context: nil, eventNumber: number, trackingNumber: 0,
        userData: nil)
      : NSEvent.mouseEvent(
        with: type, location: point, modifierFlags: [], timestamp: now,
        windowNumber: window.windowNumber, context: nil, eventNumber: number,
        clickCount: type == .mouseMoved ? 0 : 1, pressure: type == .leftMouseDown ? 1 : 0)
    return try XCTUnwrap(event)
  }

  private func click(at point: NSPoint, in window: NSWindow) throws {
    for (type, number) in [(NSEvent.EventType.leftMouseDown, 2), (.leftMouseUp, 3)] {
      window.sendEvent(try pointerEvent(type, at: point, in: window, number: number))
      RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.03))
    }
    RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.08))
  }

  private func movePointer(to point: NSPoint, in window: NSWindow) {
    let screen = window.convertPoint(toScreen: point)
    let top = NSScreen.screens.first?.frame.maxY ?? 0
    let global = CGPoint(x: screen.x, y: top - screen.y)
    CGWarpMouseCursorPosition(global)
    CGEvent(
      mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: global,
      mouseButton: .left)?.post(tap: .cghidEventTap)
    RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.15))
  }
}

// MARK: - Fixtures

private enum AudioState { case available, disabled, lit }

private enum Surface: String { case overlay, intercom }

private struct ContractFixtureError: Error, CustomStringConvertible {
  let description: String
}

/// The conversation header's flat trio (microphone, speaker, panel) on an
/// explicit sheet shade. Never ordered in, so hover hints stay withdrawn.
@MainActor
private final class AudioControlsFixture {
  static let diameter: CGFloat = 56
  let sheet: OverlayColorToken
  let host: NSHostingView<AnyView>
  let window: NSWindow

  init(palette: OverlayAppearancePalette, sheet hexValue: UInt32, state: AudioState) {
    sheet = OverlayColorToken(hexValue)
    let dark = palette.appearance == .dark
    let controls = OverlayAgentAudioControls(
      open: state == .lit, muted: false,
      microphoneEnabled: state != .disabled, playbackEnabled: state != .disabled,
      palette: palette, onMicrophone: {}, onPlayback: {}, speaking: state == .lit,
      chrome: false, diameter: Self.diameter,
      panel: OverlayAgentPanelControl(
        symbol: "macwindow", hint: "Panel", enabled: state != .disabled, action: {}))
    let root =
      controls
      .padding(24)
      .frame(maxWidth: .infinity, maxHeight: .infinity)
      .background(Color(nsColor: NSColor(hex: hexValue)))
      .symbolEffectsRemoved()
      .transaction {
        $0.animation = nil
        $0.disablesAnimations = true
      }
      .environment(\.colorScheme, dark ? .dark : .light)
    host = NSHostingView(rootView: AnyView(root))
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: 240, height: 110)
    window = NSWindow(
      contentRect: NSRect(x: -10000, y: -10000, width: 240, height: 110), styleMask: .borderless,
      backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
    window.contentView = host
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.05))
    host.layoutSubtreeIfNeeded()
  }

  func close() {
    window.contentView = nil
    window.close()
  }

  /// The three hit squares, left to right, from their native hover owners.
  func squares() throws -> [NSRect] {
    let side = Self.diameter
    let frames = hoverRegions(in: host).map { $0.convert($0.bounds, to: host) }
      .filter { abs($0.width - side) <= 2 && abs($0.height - side) <= 2 }
    let squares = deduplicated(frames).sorted { $0.midX < $1.midX }
    guard squares.count == 3 else {
      throw ContractFixtureError(description: "expected microphone, speaker, panel; got \(squares)")
    }
    return squares
  }

  /// The pointer as AppKit's tracking areas report it; nil leaves.
  func hover(at point: NSPoint?) {
    let location = point.map { host.convert($0, to: nil) }
    for region in hoverRegions(in: host) { region.updateHover(at: location) }
    // Visual delivery is coalesced to one display beat.
    RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.25))
    host.layoutSubtreeIfNeeded()
  }

  func render() throws -> NSBitmapImageRep {
    host.layoutSubtreeIfNeeded()
    let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
    host.cacheDisplay(in: host.bounds, to: bitmap)
    let corner = try XCTUnwrap(bitmap.colorAt(x: 2, y: 2))
    XCTAssertLessThanOrEqual(
      channelDelta(token(corner), sheet), OverlayHoverCursorContractTests.pinTolerance,
      "fixture: the sheet did not render")
    return bitmap
  }

  /// The glyph's strongest pixel against the sheet: its full-coverage ink.
  func ink(in square: NSRect, of bitmap: NSBitmapImageRep) throws -> OverlayColorToken {
    let scaleX = CGFloat(bitmap.pixelsWide) / host.bounds.width
    let scaleY = CGFloat(bitmap.pixelsHigh) / host.bounds.height
    let rect = square.insetBy(dx: 2, dy: 2)
    let top = host.isFlipped ? rect.minY : host.bounds.height - rect.maxY
    var strongest = 0.0
    var ink = sheet
    var covered = 0
    for y in Int(top * scaleY)..<Int((top + rect.height) * scaleY) {
      for x in Int(rect.minX * scaleX)..<Int(rect.maxX * scaleX) {
        guard let color = bitmap.colorAt(x: x, y: y) else { continue }
        let pixel = token(color)
        let delta = channelDelta(pixel, sheet)
        if delta > 0.08 { covered += 1 }
        if delta > strongest {
          strongest = delta
          ink = pixel
        }
      }
    }
    guard covered > 20 else {
      throw ContractFixtureError(description: "no glyph rendered in \(square)")
    }
    return ink
  }
}

private final class ContractFirstMouseHostingView<Content: View>: NSHostingView<Content> {
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

@MainActor
private final class ComposerCounters {
  var draft = "Szkic"
  var microphone = 0
  var sends = 0
  var probe = 0
}

/// One composer with all four corner controls, hosted as production hosts it:
/// OverlayContentContainer in FloatingOverlayPanel, or a bare NSHostingView in
/// the Intercom panel with the compact editor budget.
@MainActor
private final class ComposerSurface {
  let kind: Surface
  let counters: ComposerCounters
  let host: NSHostingView<AnyView>
  let panel: NSPanel
  let editor: NSTextView

  init(_ kind: Surface, probe: Bool = false) throws {
    let counters = ComposerCounters()
    let composer = OverlayConversationComposer(
      palette: .dark,
      draft: Binding(get: { counters.draft }, set: { counters.draft = $0 }),
      maximumEditorHeight: kind == .intercom ? 64 : nil,
      sending: false, onSubmit: { counters.sends += 1 },
      onToggleDraftMicrophone: { counters.microphone += 1 },
      draftSyncChoices: [
        OverlayComposerSyncChoice(id: "peer", title: "Peer", selected: false, enabled: true)
      ],
      onToggleDraftSync: { _ in },
      recipientsOnSend: true)
    let root = VStack(spacing: 12) {
      if probe {
        Button {
          counters.probe += 1
        } label: {
          Text(verbatim: "Probe")
        }
        .buttonStyle(.plain)
        .frame(width: 80, height: 28)
        .contentShape(Rectangle())
        .conversationActionHover(restingColor: .gray, hoverColor: .white)
      }
      composer
    }
    .padding(16)
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .environment(\.csTextScale, 1)
    .environment(\.colorScheme, .dark)
    let host = ContractFirstMouseHostingView(rootView: AnyView(root))
    host.sizingOptions = []
    let frame = NSRect(x: 240, y: 240, width: 420, height: 200)
    let panel: NSPanel
    switch kind {
    case .overlay:
      let overlay = FloatingOverlayPanel(
        contentRect: frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered,
        defer: false)
      overlay.acceptsMouseMovedEvents = true
      overlay.contentView = OverlayContentContainer(hosting: host)
      panel = overlay
    case .intercom:
      let intercom = OverlayIntercomPanel()
      intercom.setFrame(frame, display: false)
      // An opened conversation card: the one that takes typing.
      intercom.acceptsConversationFocus = true
      intercom.contentView = host
      panel = intercom
    }
    panel.isReleasedWhenClosed = false
    for _ in 0..<2 {
      host.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.06))
    }
    self.kind = kind
    self.counters = counters
    self.host = host
    self.panel = panel
    editor = try XCTUnwrap(editableText(in: host), "\(kind): composer editor")
    panel.makeFirstResponder(editor)
    settle()
  }

  var textScope: NSView { editor.enclosingScrollView ?? editor }

  func settle() {
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.1))
    host.layoutSubtreeIfNeeded()
  }

  func close() {
    for region in hoverRegions(in: host) { region.updateHover(at: nil) }
    panel.makeFirstResponder(nil)
    panel.orderOut(nil)
    panel.contentView = nil
    panel.close()
  }

  /// "+", microphone, Send and the chevron, left to right, in window points.
  func controlCenters() throws -> [(String, NSPoint)] {
    let field = textScope.convert(textScope.bounds, to: nil)
    let frames = hoverRegions(in: host).map { $0.convert($0.bounds, to: nil) }
      .filter { field.contains(NSPoint(x: $0.midX, y: $0.midY)) && $0.width < field.width / 2 }
    let controls = deduplicated(frames).sorted { $0.midX < $1.midX }
    guard controls.count == OverlayHoverCursorContractTests.composerNames.count else {
      throw ContractFixtureError(
        description: "\(kind): expected the four corner controls, got \(controls)")
    }
    return zip(OverlayHoverCursorContractTests.composerNames, controls).map {
      ($0, NSPoint(x: $1.midX, y: $1.midY))
    }
  }

  func probeCenter() throws -> NSPoint {
    let field = textScope.convert(textScope.bounds, to: nil)
    let probe = hoverRegions(in: host).map { $0.convert($0.bounds, to: nil) }
      .first { !field.contains(NSPoint(x: $0.midX, y: $0.midY)) }
    let frame = try XCTUnwrap(probe, "\(kind): probe button")
    return NSPoint(x: frame.midX, y: frame.midY)
  }

  /// The writable text between the two corner exclusions, on the control row.
  func textMiddle() throws -> NSPoint {
    let container = try XCTUnwrap(editor.textContainer)
    let paths = container.exclusionPaths
    guard paths.count == 2 else {
      throw ContractFixtureError(
        description: "\(kind): expected two corner exclusions, got \(paths.count)")
    }
    let left = paths[0].bounds
    let right = paths[1].bounds
    let origin = editor.textContainerOrigin
    let local = NSPoint(x: (left.maxX + right.minX) / 2 + origin.x, y: left.midY + origin.y)
    return editor.convert(local, to: nil)
  }
}

// MARK: - Cursor-rect ledger

/// Records every `addCursorRect(_:cursor:)` a window's views make on a fresh
/// reset. The swap lasts for one call and is undone before it returns.
@MainActor
private enum CursorRectLedger {
  struct Entry {
    let view: NSView
    let rect: NSRect
    let cursor: NSCursor
  }

  static var recording = false
  static var entries: [Entry] = []

  static func declared(in window: NSWindow) -> [(rect: NSRect, cursor: NSCursor)] {
    guard
      let original = class_getInstanceMethod(
        NSView.self, #selector(NSView.addCursorRect(_:cursor:))),
      let recorder = class_getInstanceMethod(
        NSView.self, #selector(NSView.codescribeContractAddCursorRect(_:cursor:)))
    else { return [] }
    method_exchangeImplementations(original, recorder)
    recording = true
    entries = []
    defer {
      recording = false
      method_exchangeImplementations(original, recorder)
    }
    if let root = window.contentView?.superview ?? window.contentView { reset(root) }
    return entries.filter { $0.view.window === window }.compactMap {
      entry -> (rect: NSRect, cursor: NSCursor)? in
      let visible = entry.rect.intersection(entry.view.visibleRect)
      guard !visible.isEmpty else { return nil }
      return (rect: entry.view.convert(visible, to: nil), cursor: entry.cursor)
    }
  }

  private static func reset(_ view: NSView) {
    view.discardCursorRects()
    view.resetCursorRects()
    for child in view.subviews { reset(child) }
  }
}

extension NSView {
  /// After the swap this selector names AppKit's original implementation.
  @objc dynamic fileprivate func codescribeContractAddCursorRect(_ rect: NSRect, cursor: NSCursor) {
    if CursorRectLedger.recording {
      CursorRectLedger.entries.append(.init(view: self, rect: rect, cursor: cursor))
    }
    codescribeContractAddCursorRect(rect, cursor: cursor)
  }
}

// MARK: - Helpers

@MainActor
private func hoverRegions(in view: NSView) -> [OverlayMenuCursorRegionView] {
  ((view as? OverlayMenuCursorRegionView).map { [$0] } ?? [])
    + view.subviews.flatMap { hoverRegions(in: $0) }
}

@MainActor
private func editableText(in view: NSView) -> NSTextView? {
  if let text = view as? NSTextView, text.isEditable, !(text is LiveTranscriptNativeTextView) {
    return text
  }
  return view.subviews.lazy.compactMap { editableText(in: $0) }.first
}

/// A control's colour owner and its hint owner share one frame.
private func deduplicated(_ frames: [NSRect]) -> [NSRect] {
  var unique: [NSRect] = []
  for frame in frames
  where !unique.contains(where: {
    abs($0.midX - frame.midX) <= 1 && abs($0.midY - frame.midY) <= 1
      && abs($0.width - frame.width) <= 2 && abs($0.height - frame.height) <= 2
  }) {
    unique.append(frame)
  }
  return unique
}

private func token(_ color: NSColor) -> OverlayColorToken {
  let srgb = color.usingColorSpace(.sRGB) ?? color
  func byte(_ value: CGFloat) -> UInt32 { UInt32(max(0, min(255, (value * 255).rounded()))) }
  return OverlayColorToken(
    byte(srgb.redComponent) << 16 | byte(srgb.greenComponent) << 8 | byte(srgb.blueComponent))
}

private func token(_ rgb: (red: Double, green: Double, blue: Double)) -> OverlayColorToken {
  token(NSColor(srgbRed: rgb.red, green: rgb.green, blue: rgb.blue, alpha: 1))
}

/// The product's own WCAG ratio between two opaque inks.
private func contrast(_ a: OverlayColorToken, _ b: OverlayColorToken) -> Double {
  OverlayColorToken.contrastRatio(foreground: a, surface: b, background: b)
}

/// Monotonic in relative luminance: the ratio against black.
private func lightness(_ ink: OverlayColorToken) -> Double {
  contrast(ink, OverlayColorToken(0x000000))
}

private func channelDelta(_ a: OverlayColorToken, _ b: OverlayColorToken) -> Double {
  let x = a.components
  let y = b.components
  return max(abs(x.red - y.red), abs(x.green - y.green), abs(x.blue - y.blue))
}

private func hex(_ ink: OverlayColorToken) -> String { String(format: "%06X", ink.rgb) }
