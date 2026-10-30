import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Review renders of the bar (Monika's bar brief, 2026-10-30): mini and the
/// expanded bar with a horizontal agent map, at rest and while Monika speaks
/// to agents, the narrowing ladder, the map's marks and the ✦ hover list.
/// Each test attaches a PNG (`bar-…`, kept always) for a person to look at;
/// the assertions only make sure the render shows the intended state.
///
/// `cacheDisplay` draws the SwiftUI layers but not the behind-window glass
/// material (`OverlayPopoverGlass`, the sheet glass), so the glass is judged
/// live on screen, not from these images.
@MainActor
final class OverlayBarReviewRenderTests: XCTestCase {

  // MARK: Fixtures

  private let fourAgents = [("1", "As"), ("3", "Lena"), ("4", "Lena2"), ("6", "Nina")]
  private let nineAgents = [
    ("1", "As"), ("2", "Konstantyna"), ("3", "Lena"), ("4", "Lena2"), ("5", "Bo"),
    ("6", "Nina"), ("7", "Maksymilian"), ("8", "Igor"), ("9", "Hania"),
  ]

  private func ownerConversation(
    _ name: String, channel: String, lease: String
  ) throws -> OverlayConversation {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": "review-\(channel)",
        "lease_id": String(repeating: lease, count: 32), "channel": channel, "name": name,
      ]))
    return OverlayConversation(
      id: owner.id, channel: channel, name: name, owner: owner, messages: [])
  }

  /// Agents bound on their digits with a current conversation each, the map
  /// on in the horizontal layout, the chord read as Ctrl.
  private func barState(
    _ agents: [(String, String)], mode: OverlayPresentationMode
  ) throws -> (OverlayState, [OverlayConversation]) {
    let conversations = try agents.enumerated().map { index, agent in
      try ownerConversation(
        agent.1.lowercased(), channel: agent.0, lease: String(index, radix: 16))
    }
    let state = OverlayState()
    optIntoChannelMap(state)
    state.applyConversationSnapshot(
      .init(
        deliveries: conversations.map {
          OverlayChannelDelivery(
            channel: $0.channel, agent: $0.name, deliveryID: "review-\($0.channel)",
            stage: .received, isOpen: false)
        }, conversations: conversations))
    state.agentChannelModifierSetting = { "ctrl" }
    state.setPresentationMode(mode)
    apply(conversations, open: [], to: state)
    return (state, conversations)
  }

  private func apply(
    _ conversations: [OverlayConversation], open: Set<String>, to state: OverlayState
  ) {
    state.applyChannelRoster(
      conversations.map {
        CsChannelRosterState(
          channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
          providerSessionId: $0.owner?.providerSessionID, open: open.contains($0.channel),
          loud: false, autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
      })
  }

  /// The expanded bar's geometry as the window owner computes it.
  private func barGeometry(_ state: OverlayState) -> OverlayBarGeometry {
    DictationOverlayWindow.midiBarGeometry(
      chips: state.channelMapLayoutModel.chips, textScale: 1,
      showsMeter: state.channelMapBarShowsMeter)
  }

  private func image(of view: NSView) throws -> NSImage {
    let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
    view.cacheDisplay(in: view.bounds, to: bitmap)
    let image = NSImage(size: view.bounds.size)
    image.addRepresentation(bitmap)
    return image
  }

  private func attach(_ image: NSImage, _ name: String) {
    let attachment = XCTAttachment(image: image)
    attachment.name = name
    attachment.lifetime = .keepAlways
    add(attachment)
  }

  /// The real overlay view in a borderless overlay panel of the given size,
  /// in one appearance, rendered from its content view.
  private func renderOverlay(
    _ state: OverlayState, width: CGFloat, height: CGFloat, dark: Bool = true, name: String
  ) throws {
    let host = NSHostingView(
      rootView: DictationOverlayView(state: state)
        .environment(\.csTextScale, 1)
        .environment(\.colorScheme, dark ? .dark : .light)
        .defaultAppStorage(try XCTUnwrap(UserDefaults(suiteName: UUID().uuidString)))
        .transaction { $0.disablesAnimations = true })
    host.sizingOptions = []
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 200, y: 200, width: width, height: height),
      styleMask: [.borderless, .nonactivatingPanel, .resizable], backing: .buffered,
      defer: false)
    panel.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
    panel.contentView = OverlayContentContainer(hosting: host)
    BackgroundTestWindow.present(panel)
    defer { panel.orderOut(nil) }
    panel.setFrame(NSRect(x: 200, y: 200, width: width, height: height), display: true)
    let root = try XCTUnwrap(panel.contentView)
    root.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.15))
    root.layoutSubtreeIfNeeded()
    attach(try image(of: root), name)
  }

  /// A SwiftUI view alone on the palette's sheet colour.
  private func renderAlone(
    _ content: some View, size: CGSize, dark: Bool = true, name: String
  ) throws {
    let palette: OverlayAppearancePalette = dark ? .dark : .light
    let host = NSHostingView(
      rootView: content
        .frame(width: size.width, height: size.height, alignment: .topLeading)
        .background(palette.desktopBackground.color)
        .environment(\.colorScheme, dark ? .dark : .light))
    host.frame = NSRect(origin: .zero, size: size)
    let window = NSWindow(
      contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
    window.contentView = host
    BackgroundTestWindow.present(window)
    defer { window.close() }
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.12))
    host.layoutSubtreeIfNeeded()
    attach(try image(of: host), name)
  }

  private func renderMini(_ state: OverlayState, name: String) throws {
    XCTAssertEqual(state.presentationMode, .mini)
    try renderOverlay(
      state,
      width: DictationOverlayWindow.miniNaturalWidth(
        map: state.channelMapLayoutModel, textScale: 1),
      height: DictationOverlayWindow.collapsedHeight, name: name)
  }

  private func renderExpanded(
    _ state: OverlayState, width: CGFloat? = nil, dark: Bool = true, name: String
  ) throws -> OverlayChannelMapLayout {
    XCTAssertEqual(state.presentationMode, .midi)
    XCTAssertTrue(state.channelMapInHeader, "the horizontal strip carries the map")
    let geometry = barGeometry(state)
    let width = max(width ?? geometry.naturalWidth, geometry.floorWidth)
    try renderOverlay(
      state, width: width,
      height: DictationOverlayWindow.collapsedHeight + geometry.extraHeight(width: width),
      dark: dark, name: name)
    return geometry.layout(width: width)
  }

  // MARK: Mini

  func testRenderMiniAtRestWithFourAgents() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let (state, _) = try barState(fourAgents, mode: .mini)
    XCTAssertEqual(state.channelMapLayoutModel.count, 4)
    XCTAssertFalse(state.channelMapLayoutModel.microphoneOpen)
    try renderMini(state, name: "bar-mini-rest-4-agents")
  }

  func testRenderMiniSpeakingToOneAgent() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let (state, conversations) = try barState(fourAgents, mode: .mini)
    apply(conversations, open: ["4"], to: state)
    XCTAssertEqual(state.presentationMode, .mini, "speaking keeps mini")
    XCTAssertEqual(state.channelMapLayoutModel.openDigits, ["4"])
    try renderMini(state, name: "bar-mini-speaking-to-1")
  }

  func testRenderMiniSpeakingToTwoAgents() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let (state, conversations) = try barState(fourAgents, mode: .mini)
    apply(conversations, open: ["4", "6"], to: state)
    XCTAssertEqual(state.presentationMode, .mini, "speaking keeps mini")
    XCTAssertEqual(state.channelMapLayoutModel.openDigits, ["4", "6"])
    try renderMini(state, name: "bar-mini-speaking-to-2")
  }

  /// The ✦ list's content alone; on screen it sits in the hint panel's glass.
  func testRenderMiniAgentsHoverList() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let map = OverlayChannelMapModel(
      channels: fourAgents.map {
        OverlayChannelDelivery(
          channel: $0.0, agent: $0.1.lowercased(), deliveryID: nil, stage: nil, isOpen: false)
      },
      hudStates: Dictionary(
        uniqueKeysWithValues: fourAgents.map {
          (
            $0.0,
            OverlayChannelHudProjection(
              open: false, loud: false, autosealDeadline: nil, followerAlive: true)
          )
        }),
      selectableDigits: Set(fourAgents.map(\.0)), unreadDigits: ["4"])
    XCTAssertEqual(map.agents.map(\.unread), [false, false, true, false])
    try renderAlone(
      OverlayAgentsHoverList(
        agents: map.agents, modifier: .control, palette: .dark, onSelect: { _ in }),
      size: OverlayAgentsHoverList.size(agents: map.agents), name: "bar-mini-agents-hover-list")
  }

  // MARK: Expanded bar

  func testRenderExpandedBarWithFourAgentsAtRest() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let (state, _) = try barState(fourAgents, mode: .midi)
    XCTAssertFalse(state.channelMapBarShowsMeter, "no waveform or clock at rest")
    let layout = try renderExpanded(state, name: "bar-expanded-4-agents-rest")
    XCTAssertEqual(layout.slots.count, 4)
  }

  func testRenderExpandedBarWithFourAgentsSpeakingToTwo() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let (state, conversations) = try barState(fourAgents, mode: .midi)
    apply(conversations, open: ["4", "6"], to: state)
    XCTAssertEqual(state.presentationMode, .midi, "speaking keeps the bar")
    XCTAssertTrue(state.channelMapBarShowsMeter, "the waveform and the clock show")
    XCTAssertEqual(state.channelMapLayoutModel.openDigits, ["4", "6"])
    _ = try renderExpanded(state, name: "bar-expanded-4-agents-speaking-to-2")
  }

  func testRenderExpandedBarWithFourAgentsInLight() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let (state, _) = try barState(fourAgents, mode: .midi)
    _ = try renderExpanded(state, dark: false, name: "bar-expanded-4-agents-light")
  }

  func testRenderExpandedBarWithNineAgentsAtItsNaturalWidth() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let (state, _) = try barState(nineAgents, mode: .midi)
    XCTAssertEqual(state.channelMapLayoutModel.count, 9)
    let layout = try renderExpanded(state, name: "bar-expanded-9-agents-natural")
    XCTAssertEqual(layout.arrangement, .twoRows, "nine full names take a second row")
  }

  /// The ladder as the bar narrows: the narrowest two-row width, a width
  /// halfway through the cut names, the first with overlapping circles, and
  /// the floor. Each attachment carries its width in points.
  func testRenderExpandedBarWithNineAgentsNarrowingToItsFloor() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let (state, _) = try barState(nineAgents, mode: .midi)
    let geometry = barGeometry(state)
    let full = geometry.nameWidths
    var lastTwoRows: CGFloat?
    var firstTruncated: CGFloat?
    var firstOverlap: CGFloat?
    var width = floor(geometry.naturalWidth)
    while width >= geometry.floorWidth {
      let layout = geometry.layout(width: width)
      if layout.arrangement == .twoRows {
        lastTwoRows = width
      } else if layout.overlaps {
        if firstOverlap == nil { firstOverlap = width }
      } else if zip(layout.nameWidths, full).contains(where: { $0 + 0.5 < $1 }),
        firstTruncated == nil
      {
        firstTruncated = width
      }
      width -= 1
    }
    let steps: [(CGFloat?, String)] = [
      (lastTwoRows, "two rows"),
      (
        firstTruncated.flatMap { truncated in firstOverlap.map { floor((truncated + $0) / 2) } },
        "truncated names"
      ),
      (firstOverlap, "overlapping circles"), (geometry.floorWidth, "floor"),
    ]
    for (step, rung) in steps {
      let target = try XCTUnwrap(step, "the ladder reaches \(rung)")
      let shown = max(target, geometry.floorWidth)
      let layout = try renderExpanded(
        state, width: shown, name: "bar-expanded-9-agents-narrowing-\(Int(shown))")
      switch rung {
      case "two rows": XCTAssertEqual(layout.arrangement, .twoRows)
      case "overlapping circles", "floor": XCTAssertTrue(layout.overlaps, rung)
      default: XCTAssertEqual(layout.arrangement, .oneRow, rung)
      }
    }
  }

  // MARK: The map's marks

  /// Unread replies, recipients and the viewed conversation need replies and
  /// a selection the panel fixtures do not have, so the strip is rendered
  /// from a crafted model: 2 unread, 3 recipient, 4 viewed (top rows) or 5
  /// with all three (bottom rows), each at one-row and overlapping density.
  func testRenderMapMarks() throws {
    try BackgroundTestWindow.requireIsolatedGUI()
    let agents = [("1", "As"), ("2", "Lena"), ("3", "Lena2"), ("4", "Nina"), ("5", "Konstantyna")]
    let channels = agents.map {
      OverlayChannelDelivery(
        channel: $0.0, agent: $0.1.lowercased(), deliveryID: nil, stage: nil, isOpen: false)
    }
    func map(selected: String) -> OverlayChannelMapModel {
      OverlayChannelMapModel(
        channels: channels,
        hudStates: Dictionary(
          uniqueKeysWithValues: agents.map {
            (
              $0.0,
              OverlayChannelHudProjection(
                open: $0.0 == "3" || $0.0 == "5", loud: false, autosealDeadline: nil,
                followerAlive: true)
            )
          }),
        selectedDigit: selected, selectableDigits: Set(agents.map(\.0)),
        unreadDigits: ["2", "5"])
    }
    let names = map(selected: "4").chips.map { OverlayChannelPill.nameWidth($0.name, scale: 1) }
    let geometry = OverlayBarGeometry(
      baseWidth: 0, horizontalPadding: 0, nameWidths: names,
      circle: OverlayChannelPill.digitCircle, inlineGap: OverlayChannelPill.chipGap)
    let open = geometry.layout(width: geometry.naturalWidth)
    let dense = geometry.layout(width: geometry.floorWidth)
    XCTAssertEqual(open.arrangement, .oneRow)
    XCTAssertFalse(open.overlaps)
    XCTAssertTrue(dense.overlaps)
    let marked = map(selected: "5").chips.first { $0.digit == "5" }
    XCTAssertEqual(marked?.unread, true)
    XCTAssertEqual(marked?.microphoneOpen, true)
    XCTAssertEqual(marked?.selected, true)
    func strip(_ layout: OverlayChannelMapLayout, selected: String) -> some View {
      OverlayChannelMapView(
        model: map(selected: selected), placement: .strip(layout), modifier: .control,
        palette: .dark, textScale: 1, animates: false, onSelectChannel: { _ in },
        onCounter: {}
      )
      .padding(8)
    }
    let rows = VStack(alignment: .leading, spacing: 4) {
      strip(open, selected: "4")
      strip(open, selected: "5")
      strip(dense, selected: "4")
      strip(dense, selected: "5")
    }
    try renderAlone(
      rows,
      size: CGSize(
        width: open.width + 24, height: 4 * (OverlayChannelPill.mapRowHeight + 16) + 12),
      name: "bar-map-marks")
  }
}
