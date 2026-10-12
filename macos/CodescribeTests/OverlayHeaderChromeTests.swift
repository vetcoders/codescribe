import AppKit
import ObjectiveC
import SwiftUI
import XCTest

@testable import Codescribe

/// Header controls carry the action pill's hint and a hairline hover ring in
/// the hue of what they open. The hint floats in a child window because the
/// mini and compact header is the whole 46 pt panel.
@MainActor
final class OverlayHeaderChromeTests: XCTestCase {
  func testHintSitsBelowItsControlCenteredInsideTheScreen() {
    let visible = NSRect(x: 0, y: 0, width: 1_440, height: 900)
    let anchor = NSRect(x: 700, y: 800, width: 22, height: 22)
    let frame = OverlayHintPanel.frame(
      size: CGSize(width: 120, height: 20), below: anchor, within: visible)
    XCTAssertEqual(frame.maxY, anchor.minY - OverlayHintPanel.gap)
    XCTAssertEqual(frame.midX, anchor.midX)
  }

  func testHintFlipsAboveWhenTheScreenEndsBelowTheControl() {
    let visible = NSRect(x: 0, y: 0, width: 1_440, height: 900)
    let anchor = NSRect(x: 700, y: 10, width: 22, height: 22)
    let frame = OverlayHintPanel.frame(
      size: CGSize(width: 120, height: 20), below: anchor, within: visible)
    XCTAssertEqual(frame.minY, anchor.maxY + OverlayHintPanel.gap)
  }

  func testHintNeverRunsPastTheScreenEdge() {
    let visible = NSRect(x: 0, y: 0, width: 1_440, height: 900)
    let nearRight = OverlayHintPanel.frame(
      size: CGSize(width: 240, height: 20),
      below: NSRect(x: 1_420, y: 800, width: 22, height: 22), within: visible)
    XCTAssertEqual(nearRight.maxX, visible.maxX - OverlayHintPanel.screenInset)
    let nearLeft = OverlayHintPanel.frame(
      size: CGSize(width: 240, height: 20),
      below: NSRect(x: 0, y: 800, width: 22, height: 22), within: visible)
    XCTAssertEqual(nearLeft.minX, visible.minX + OverlayHintPanel.screenInset)
  }

  /// The hint is a click-through child of the panel that owns the control: it
  /// follows that panel and leaves with it, and only its owner can hide it.
  func testHintIsAClickThroughChildOfTheOverlayPanelAndLeavesWithIt() throws {
    let parent = NSPanel(
      contentRect: NSRect(x: 300, y: 500, width: 400, height: 46),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    let control = NSView(frame: NSRect(x: 300, y: 12, width: 22, height: 22))
    parent.contentView?.addSubview(control)
    parent.orderFrontRegardless()
    defer { parent.orderOut(nil) }
    let owner = OverlayHintAnchor()
    owner.view = control
    let stranger = OverlayHintAnchor()

    owner.show("Show transcription", palette: .dark)
    let hint = try XCTUnwrap(OverlayHintPanel.shared.presentedPanel)
    XCTAssertTrue(hint.parent === parent)
    XCTAssertTrue(hint.ignoresMouseEvents)
    XCTAssertFalse(hint.canBecomeKey)
    let controlOnScreen = parent.convertToScreen(control.convert(control.bounds, to: nil))
    XCTAssertEqual(hint.frame.maxY, controlOnScreen.minY - OverlayHintPanel.gap, accuracy: 0.5)
    XCTAssertEqual(hint.appearance?.name, .darkAqua)

    stranger.hide()
    XCTAssertNotNil(OverlayHintPanel.shared.presentedPanel, "another control cannot hide it")

    owner.hide()
    XCTAssertNil(OverlayHintPanel.shared.presentedPanel)
    XCTAssertNil(hint.parent)

    owner.show("Agents", palette: .light)
    XCTAssertNotNil(OverlayHintPanel.shared.presentedPanel)
    parent.orderOut(nil)
    XCTAssertFalse(hint.isVisible, "no hint outlives its header")
    owner.hide()
  }

  func testHintIsNotShownForAControlOutsideAVisibleWindow() {
    let owner = OverlayHintAnchor()
    let detached = NSView(frame: NSRect(x: 0, y: 0, width: 22, height: 22))
    owner.view = detached
    owner.show("Pin", palette: .light)
    XCTAssertNil(OverlayHintPanel.shared.presentedPanel)
    withExtendedLifetime(detached) {}
  }

  func testAccentsShareOneHoverHue() {
    for palette in [OverlayAppearancePalette.light, .dark] {
      let hover = OverlayHeaderAccent.transcription.color(in: palette)
      XCTAssertEqual(OverlayHeaderAccent.agents.color(in: palette), hover)
      XCTAssertEqual(OverlayHeaderAccent.state.color(in: palette), hover)
      XCTAssertNotEqual(hover, palette.successStatus.color)
      XCTAssertNotEqual(hover, palette.agentAccent.color)
      XCTAssertEqual(hover, palette.listeningStatus.color)
    }
  }

  func testHeaderControlsRestHoverAndLeaveInLightAndDark() throws {
    let pinned = OverlayState()
    pinned.setKeepVisibleBetweenTakes(true)
    let loose = OverlayState()
    for palette in [OverlayAppearancePalette.light, OverlayAppearancePalette.dark] {
      let name = palette.appearance == .dark ? "dark" : "light"
      try assertNeutralHover(
        "transcription-\(name)", palette: palette, view: HeaderHoverTranscription(palette: palette))
      try assertNeutralHover(
        "agents-\(name)", palette: palette,
        view: HeaderHoverAgents(palette: palette, unread: true))
      try assertNeutralHover(
        "pin-selected-\(name)", palette: palette,
        view: OverlayPinButton(state: pinned, palette: palette))
      try assertNeutralHover(
        "pin-resting-\(name)", palette: palette,
        view: OverlayPinButton(state: loose, palette: palette))
      try assertNeutralHover(
        "presentation-\(name)", palette: palette,
        view: headerHoverControls(palette: palette, microphone: false, preview: true))
      try assertMicrophoneKeepsActivity(name, palette: palette)
      try assertDisabledMicrophoneStaysCalm(name, palette: palette)
    }
  }

  /// Every header control goes through the one hover chrome, in its family's
  /// hue; none relies on the native tooltip that a non-activating panel does
  /// not reliably present.
  func testEveryHeaderControlUsesTheHoverChromeInsteadOfANativeTooltip() throws {
    let view = try source("Overlay/DictationOverlayView.swift")
    XCTAssertTrue(
      view.contains("title: transcriptPreviewHelp, accent: .transcription, palette: palette"))
    XCTAssertTrue(view.contains("accent: recordingDisabled ? nil : .state, palette: palette"))
    XCTAssertTrue(
      view.contains("title: previewAccessibilityLabel, accent: .state, palette: palette"))
    XCTAssertTrue(
      view.contains("title: OverlayIntent.close.helpText, accent: nil, palette: palette"))
    for native in [
      ".help(transcriptPreviewHelp)", ".help(previewAccessibilityLabel)",
      ".help(OverlayIntent.close.helpText)", ".help(recordingLabel",
    ] {
      XCTAssertFalse(view.contains(native), native)
    }

    let roster = try source("Overlay/OverlayChannelStatusView.swift")
    XCTAssertTrue(
      roster.contains("title: String(localized: \"Agents\"), accent: .agents, palette: palette"))
    XCTAssertFalse(roster.contains(".help(\"Agents\")"))

    let menu = try source("Overlay/OverlayHeaderMenu.swift")
    XCTAssertTrue(
      menu.contains(
        ".modifier(OverlayHeaderControlChrome(title: help, accent: .state, palette: palette))"))
    XCTAssertFalse(menu.contains(".help(help)"))
  }

  /// The floating name pill opens the Agents drawer for All as for one agent;
  /// interactive glass gives it the system's hover and press feedback.
  func testConversationNamePillOpensTheAgentsDrawer() throws {
    let conversation = try source("Overlay/OverlayConversationView.swift")
    XCTAssertTrue(conversation.contains("Button(action: onShowAgents) { namePillLabel }"))
    XCTAssertTrue(conversation.contains(".modifier(OverlayNamePillGlass(interactive: true))"))
    XCTAssertTrue(
      conversation.contains(
        "content.glassEffect(interactive ? .regular.interactive() : .regular, in: Capsule())"))
    let overlay = try source("Overlay/DictationOverlayView.swift")
    XCTAssertTrue(overlay.contains("onShowAgents: { state.showAgentMonitor() },"))

    let state = OverlayState()
    let broadcast = OverlayConversation(
      id: "pill-all", channel: "0", name: "All", owner: nil, messages: [])
    state.applyConversationSnapshot(.init(deliveries: [], conversations: [broadcast]))
    state.selectConversation(broadcast.id)
    state.showAgentMonitor()
    XCTAssertTrue(state.showsAgentMonitor)
    XCTAssertEqual(state.selectedConversationID, broadcast.id, "the drawer keeps the conversation")
  }

  /// Every clickable drawer row carries the tray-like hover wash.
  func testClickableDrawerRowsCarryTheHoverWash() throws {
    let roster = try source("Overlay/OverlayChannelStatusView.swift")
    XCTAssertEqual(roster.components(separatedBy: "OverlayRowHoverHighlight(").count - 1, 3)
    XCTAssertTrue(roster.contains("enabled: conversation != nil && onSelectConversation != nil"))
    XCTAssertTrue(roster.contains("enabled: onShowTranscription != nil"))
    XCTAssertTrue(roster.contains("CSColor.surfaceRaised(0.05)"))
    XCTAssertTrue(
      roster.contains(
        ".contentShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))"))
    XCTAssertTrue(roster.contains(".onHover { hovered = enabled && $0 }"))
    XCTAssertFalse(roster.contains("opacity(hovered && enabled ? 0.06 : 0)"))
  }

  /// The production agent monitor washes the empty span of a roster row on a
  /// native pointer enter and clears it on leave. Light and dark both use the
  /// live panel appearance. A disabled monitor stays calm.
  func testProductionRosterRowHoverWashesTheWholeRow() throws {
    for palette in [OverlayAppearancePalette.light, OverlayAppearancePalette.dark] {
      let frames = try hoverFrames(
        try rosterMonitor(palette: palette, interactive: true),
        palette: palette,
        size: NSSize(width: 320, height: 280),
        minimumWidth: 180)
      let name = palette.appearance == .dark ? "dark" : "light"
      XCTAssertGreaterThan(
        headerBandCount(frames.rest, frames.hover, width: frames.width, from: 0.45, to: 0.75),
        400, "\(name) roster row did not wash its empty span")
      XCTAssertTrue(
        headerBandStaysOffRole(frames.rest, frames.hover, width: frames.width, palette: palette),
        "\(name) roster hover recolored the row")
      XCTAssertLessThan(
        headerDelta(frames.rest, frames.leave), 0.08, "\(name) roster hover did not clear")
    }
    let calm = try hoverFrames(
      try rosterMonitor(palette: .dark, interactive: false),
      palette: .dark,
      size: NSSize(width: 320, height: 280),
      requireTracking: false)
    XCTAssertLessThan(
      headerBandCount(calm.rest, calm.hover, width: calm.width, from: 0.45, to: 0.75),
      80, "disabled roster row took a hover wash")
  }

  private func assertNeutralHover<V: View>(
    _ name: String, palette: OverlayAppearancePalette, view: V
  ) throws {
    let frames = try hoverFrames(view, palette: palette)
    for role in [palette.successStatus, palette.agentAccent, palette.processingStatus] {
      XCTAssertFalse(
        headerContains(frames.rest, near: role, tolerance: 0.16),
        "\(name) rests on a role color")
    }
    XCTAssertGreaterThan(
      headerShift(frames.rest, frames.hover, toward: palette.listeningStatus), 0.04,
      "\(name) hover did not move toward the shared accent")
    XCTAssertTrue(
      headerContains(frames.hover, near: palette.listeningStatus, tolerance: 0.2),
      "\(name) hover has no shared accent pixel")
    XCTAssertLessThan(
      headerDelta(frames.rest, frames.leave), 0.08, "\(name) hover did not clear")
  }

  private func assertMicrophoneKeepsActivity(
    _ name: String, palette: OverlayAppearancePalette
  ) throws {
    let frames = try hoverFrames(
      headerHoverControls(
        palette: palette, microphone: true, preview: false, canFinish: true,
        recordingLight: .holdToTalk),
      palette: palette)
    XCTAssertTrue(
      headerContains(frames.rest, near: palette.errorStatus, tolerance: 0.2),
      "\(name) microphone lost its recording color before hover")
    XCTAssertTrue(
      headerContains(frames.hover, near: palette.errorStatus, tolerance: 0.2),
      "\(name) microphone lost its recording color on hover")
    XCTAssertGreaterThan(
      headerShift(frames.rest, frames.hover, toward: palette.listeningStatus), 0.03,
      "\(name) microphone hover ring did not appear")
    XCTAssertLessThan(
      headerDelta(frames.rest, frames.leave), 0.08, "\(name) microphone hover stuck")
  }

  private func assertDisabledMicrophoneStaysCalm(
    _ name: String, palette: OverlayAppearancePalette
  ) throws {
    let frames = try hoverFrames(
      headerHoverControls(
        palette: palette, microphone: true, preview: false, canFinish: false,
        isFinalizing: true, recordingLight: .processing),
      palette: palette)
    XCTAssertLessThan(
      headerShift(frames.rest, frames.hover, toward: palette.listeningStatus), 0.02,
      "\(name) disabled microphone took the hover accent")
    XCTAssertLessThan(headerDelta(frames.rest, frames.hover), 0.08)
  }

  private func hoverFrames<V: View>(
    _ view: V, palette: OverlayAppearancePalette,
    size: NSSize = NSSize(width: 64, height: 64),
    minimumWidth: CGFloat = 0,
    requireTracking: Bool = true
  ) throws -> (rest: [HeaderPixel], hover: [HeaderPixel], leave: [HeaderPixel], width: Int) {
    let root =
      view
      .frame(width: size.width, height: size.height)
      .background(palette.desktopBackground.color)
      .preferredColorScheme(palette.appearance == .dark ? .dark : .light)
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 280, y: 460, width: size.width, height: size.height),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.appearance = NSAppearance(named: palette.appearance == .dark ? .darkAqua : .aqua)
    panel.isOpaque = true
    panel.backgroundColor = palette.desktopBackground.nsColor
    let host = NSHostingView(rootView: root)
    host.frame = NSRect(x: 0, y: 0, width: size.width, height: size.height)
    panel.contentView = host
    panel.orderFrontRegardless()
    defer { panel.orderOut(nil) }
    headerSettle(host)
    let rest = headerRaster(host)
    XCTAssertFalse(rest.pixels.isEmpty, "control did not render")
    try headerPointer(
      true, on: host, minimumWidth: minimumWidth, requireTracking: requireTracking)
    headerSettle(host, seconds: 0.28)
    let hover = headerPixels(host)
    try headerPointer(
      false, on: host, minimumWidth: minimumWidth, requireTracking: requireTracking)
    headerSettle(host, seconds: 0.28)
    return (rest.pixels, hover, headerPixels(host), rest.width)
  }

  private func headerPointer(
    _ inside: Bool, on root: NSView, minimumWidth: CGFloat = 0, requireTracking: Bool = true
  ) throws {
    root.layoutSubtreeIfNeeded()
    headerUpdateTracking(root)
    var owners = headerTrackingOwners(in: root)
    if owners.isEmpty {
      headerSettle(root, seconds: 0.12)
      headerUpdateTracking(root)
      owners = headerTrackingOwners(in: root)
    }
    if minimumWidth > 0 {
      let wide = owners.filter { $0.1.rect.width >= minimumWidth && $0.1.rect.height <= 120 }
      XCTAssertFalse(
        wide.isEmpty,
        "no row-sized tracking area\n"
          + owners.map { "\($0.1.rect)" }.joined(separator: "\n")
          + "\n\(headerTree(root))")
      owners = wide
    }
    if owners.isEmpty, !requireTracking { return }
    XCTAssertFalse(owners.isEmpty, "no native tracking area\n\(headerTree(root))")
    for (view, area) in owners {
      let location = view.convert(NSPoint(x: area.rect.midX, y: area.rect.midY), to: nil)
      let event = try XCTUnwrap(
        NSEvent.enterExitEvent(
          with: inside ? .mouseEntered : .mouseExited,
          location: location,
          modifierFlags: [],
          timestamp: ProcessInfo.processInfo.systemUptime,
          windowNumber: view.window?.windowNumber ?? 0,
          context: nil,
          eventNumber: area.trackingNumber,
          trackingNumber: area.trackingNumber,
          userData: nil))
      try headerBind(area, to: event)
      if inside {
        view.mouseEntered(with: event)
      } else {
        view.mouseExited(with: event)
      }
    }
  }

  private func headerBind(_ area: NSTrackingArea, to event: NSEvent) throws {
    let selector = NSSelectorFromString("_setTrackingArea:")
    XCTAssertTrue(event.responds(to: selector), "hover event cannot carry a tracking area")
    _ = event.perform(selector, with: area)
    XCTAssertTrue(event.trackingArea === area, "hover event dropped its tracking area")
  }

  private func headerTrackingOwners(in view: NSView) -> [(NSView, NSTrackingArea)] {
    var found = view.trackingAreas.map { (view, $0) }
    for child in view.subviews {
      found.append(contentsOf: headerTrackingOwners(in: child))
    }
    return found
  }

  private func headerUpdateTracking(_ view: NSView) {
    view.updateTrackingAreas()
    view.subviews.forEach(headerUpdateTracking)
  }

  private func headerTree(_ view: NSView, depth: Int = 0) -> String {
    let line =
      String(repeating: " ", count: depth) + "\(type(of: view)) areas=\(view.trackingAreas.count)"
    return ([line] + view.subviews.map { headerTree($0, depth: depth + 1) }).joined(
      separator: "\n")
  }

  private func headerSettle(_ view: NSView, seconds: TimeInterval = 0.08) {
    view.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(seconds))
    view.layoutSubtreeIfNeeded()
  }

  private func headerPixels(_ view: NSView) -> [HeaderPixel] {
    headerRaster(view).pixels
  }

  /// Row-major sRGB samples. Every bitmap cell is kept so a column band is
  /// `index % width`.
  private func headerRaster(_ view: NSView) -> (width: Int, pixels: [HeaderPixel]) {
    guard let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) else {
      return (0, [])
    }
    view.cacheDisplay(in: view.bounds, to: bitmap)
    var pixels: [HeaderPixel] = []
    pixels.reserveCapacity(bitmap.pixelsWide * bitmap.pixelsHigh)
    for y in 0..<bitmap.pixelsHigh {
      for x in 0..<bitmap.pixelsWide {
        let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB)
        pixels.append(
          HeaderPixel(
            red: color?.redComponent ?? 0, green: color?.greenComponent ?? 0,
            blue: color?.blueComponent ?? 0, alpha: color?.alphaComponent ?? 0))
      }
    }
    return (bitmap.pixelsWide, pixels)
  }

  /// Pixels that moved inside a horizontal band. The band is the empty span
  /// of a full-width roster row: left of the audio controls, right of the label.
  private func headerBandCount(
    _ before: [HeaderPixel], _ after: [HeaderPixel], width: Int,
    from start: CGFloat, to end: CGFloat
  ) -> Int {
    guard width > 0, before.count == after.count, before.count.isMultiple(of: width) else {
      return 0
    }
    let lower = Int(CGFloat(width) * start)
    let upper = Int(CGFloat(width) * end)
    var count = 0
    for index in before.indices where headerDistance(before[index], after[index]) > 0.05 {
      let column = index % width
      if column >= lower && column < upper { count += 1 }
    }
    return count
  }

  private func headerBandStaysOffRole(
    _ before: [HeaderPixel], _ after: [HeaderPixel], width: Int,
    palette: OverlayAppearancePalette
  ) -> Bool {
    guard width > 0, before.count == after.count else { return false }
    let roles = [
      palette.successStatus, palette.agentAccent, palette.processingStatus,
      palette.listeningStatus,
    ].map { headerPixel($0.nsColor) }
    let lower = Int(CGFloat(width) * 0.45)
    let upper = Int(CGFloat(width) * 0.75)
    for index in before.indices {
      let column = index % width
      guard column >= lower, column < upper,
        headerDistance(before[index], after[index]) > 0.05
      else { continue }
      let pixel = after[index]
      if pixel.alpha > 0.45, roles.contains(where: { headerDistance(pixel, $0) < 0.16 }) {
        return false
      }
    }
    return true
  }

  /// Production monitor: the transcription row plus one live agent row.
  private func rosterMonitor(palette: OverlayAppearancePalette, interactive: Bool) throws
    -> some View
  {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "roster-hover",
        "lease_id": String(repeating: "c", count: 32),
        "channel": "1", "name": "Atlas",
      ]))
    let conversation = OverlayConversation(
      id: owner.id, channel: "1", name: "Atlas", owner: owner, messages: [])
    let channel = OverlayChannelDelivery(
      channel: "1", agent: "Atlas", deliveryID: nil, stage: nil, isOpen: false)
    var status = OverlayChannelStatusView(
      channels: [channel],
      unavailable: false,
      palette: palette,
      animates: false,
      hudStates: interactive
        ? [
          "1": .init(
            open: false, loud: false, autosealDeadline: nil, followerAlive: true,
            provider: owner.provider, providerSessionID: owner.providerSessionID)
        ] : [:],
      conversations: interactive ? [conversation] : [],
      onSelectConversation: interactive ? { _ in } : nil
    )
    if interactive { status.onShowTranscription = {} }
    return status.monitorBody(maximumHeight: 260)
  }

  private func headerContains(
    _ pixels: [HeaderPixel], near token: OverlayColorToken, tolerance: CGFloat
  ) -> Bool {
    let target = headerPixel(token.nsColor)
    return pixels.contains { pixel in
      pixel.alpha > 0.45 && headerDistance(pixel, target) < tolerance
    }
  }

  private func headerShift(
    _ before: [HeaderPixel], _ after: [HeaderPixel], toward token: OverlayColorToken
  ) -> CGFloat {
    let target = headerPixel(token.nsColor)
    var moved = 0
    var gain: CGFloat = 0
    for (lhs, rhs) in zip(before, after) where headerDistance(lhs, rhs) > 0.05 {
      gain += headerDistance(lhs, target) - headerDistance(rhs, target)
      moved += 1
    }
    guard moved > 8 else { return 0 }
    return gain / CGFloat(moved)
  }

  private func headerDelta(_ before: [HeaderPixel], _ after: [HeaderPixel]) -> CGFloat {
    zip(before, after).reduce(CGFloat(0)) { partial, pair in
      max(partial, headerDistance(pair.0, pair.1))
    }
  }

  private func headerPixel(_ color: NSColor) -> HeaderPixel {
    let resolved = color.usingColorSpace(.sRGB) ?? color
    return HeaderPixel(
      red: resolved.redComponent, green: resolved.greenComponent,
      blue: resolved.blueComponent, alpha: resolved.alphaComponent)
  }

  private func headerDistance(_ lhs: HeaderPixel, _ rhs: HeaderPixel) -> CGFloat {
    let red = lhs.red - rhs.red
    let green = lhs.green - rhs.green
    let blue = lhs.blue - rhs.blue
    return (red * red + green * green + blue * blue).squareRoot()
  }

  private func source(_ relative: String) throws -> String {
    let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
      .deletingLastPathComponent()
      .appendingPathComponent("Codescribe/Screens")
      .appendingPathComponent(relative)
    return try String(contentsOf: url, encoding: .utf8)
  }
}

private struct HeaderPixel {
  var red: CGFloat
  var green: CGFloat
  var blue: CGFloat
  var alpha: CGFloat
}

/// The transcription button is private to the overlay. This mounts the same
/// production glyph and chrome the header uses.
private struct HeaderHoverTranscription: View {
  let palette: OverlayAppearancePalette

  var body: some View {
    Button(action: {}) {
      OverlayMicrophoneGlyph(
        character: OverlayHeaderGlyph.transcription, tint: palette.mutedText.color)
    }
    .buttonStyle(.plain)
    .modifier(
      OverlayHeaderControlChrome(
        title: "Show transcription", accent: .transcription, palette: palette))
  }
}

private struct HeaderHoverAgents: View {
  let palette: OverlayAppearancePalette
  let unread: Bool

  var body: some View {
    OverlayChannelStatusView(
      channels: [],
      unavailable: false,
      palette: palette,
      animates: false,
      unreadCounts: unread ? ["thread": 2] : [:],
      onShowMonitor: {}
    )
  }
}

private func headerHoverControls(
  palette: OverlayAppearancePalette,
  microphone: Bool,
  preview: Bool,
  canFinish: Bool = false,
  isFinalizing: Bool = false,
  recordingLight: OverlayRecordingLight? = nil
) -> OverlayRecordingControls {
  var controls = OverlayRecordingControls(
    canFinish: canFinish,
    presentationMode: .mini,
    compact: false,
    palette: palette,
    onIntent: { _ in },
    onPreviewToggle: {},
    isFinalizing: isFinalizing,
    recordingLight: recordingLight,
    animates: false
  )
  controls.showsRecordingButton = microphone
  controls.showsPreviewButton = preview
  return controls
}
