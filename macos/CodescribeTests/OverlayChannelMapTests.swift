import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Falsifiers for the bar's agent map (Monika's bar brief, 2026-10-30; bus
/// intercom spec §5.1, §5.5, §13, decision D1): one chip per bound agent,
/// channel 0 only while it broadcasts, the layout ladder `OverlayBarGeometry`
/// decides from measured widths (one row, two rows, truncated names,
/// overlapping circles), names that stay apart and a hint with the full name,
/// mini's Agents slot with its hover list and recipient digits, the three
/// marks of a circle kept apart, capitalised names, hints that follow the
/// Agent channel modifier, and one pill view shared with the Agents rail.
@MainActor
final class OverlayChannelMapTests: XCTestCase {

  // MARK: - Fixtures

  private func hud(
    open: Bool = false, alive: Bool? = true, session: String? = nil
  ) -> OverlayChannelHudProjection {
    OverlayChannelHudProjection(
      open: open, loud: false, autosealDeadline: nil, followerAlive: alive,
      provider: session == nil ? nil : "codex", providerSessionID: session)
  }

  private func delivery(_ channel: String, _ agent: String) -> OverlayChannelDelivery {
    OverlayChannelDelivery(
      channel: channel, agent: agent, deliveryID: nil, stage: nil, isOpen: false)
  }

  private func model(
    channels: [OverlayChannelDelivery],
    hudStates: [String: OverlayChannelHudProjection],
    selected: String? = nil,
    selectable: Set<String> = ["0", "2", "3", "5"],
    unread: Set<String> = []
  ) -> OverlayChannelMapModel {
    OverlayChannelMapModel(
      channels: channels, hudStates: hudStates, selectedDigit: selected,
      selectableDigits: selectable, unreadDigits: unread)
  }

  /// The acceptance scenario's roster: Miksa on 2, Igor on 3, Kuba on 5.
  private func scenario(
    open: Set<String> = [], dead: Set<String> = [], selected: String? = nil
  ) -> OverlayChannelMapModel {
    model(
      channels: [delivery("5", "Kuba"), delivery("2", "Miksa"), delivery("3", "Igor")],
      hudStates: [
        "0": hud(open: open.contains("0"), alive: nil),
        "2": hud(open: open.contains("2"), alive: !dead.contains("2"), session: "s2"),
        "3": hud(open: open.contains("3"), alive: !dead.contains("3"), session: "s3"),
        "5": hud(open: open.contains("5"), alive: !dead.contains("5"), session: "s5"),
      ], selected: selected)
  }

  /// Nine agents, every chord digit bound.
  private func fullRoster(selected: String? = nil) -> OverlayChannelMapModel {
    let names = ["Miksa", "Nora", "Igor", "Hania", "Kuba", "Konfig", "Klara", "Leon", "Zofia"]
    var hudStates: [String: OverlayChannelHudProjection] = ["0": hud(alive: nil)]
    var channels: [OverlayChannelDelivery] = []
    for (index, name) in names.enumerated() {
      let digit = String(index + 1)
      hudStates[digit] = hud(session: "s\(digit)")
      channels.append(delivery(digit, name))
    }
    return model(channels: channels, hudStates: hudStates, selected: selected)
  }

  private func ownerConversation(
    _ name: String, channel: String, session: String, lease: String
  ) throws -> OverlayConversation {
    let owner = try XCTUnwrap(
      OverlayConversationOwner(row: [
        "provider": "codex", "provider_session_id": session,
        "lease_id": String(repeating: lease, count: 32), "channel": channel, "name": name,
      ]))
    return OverlayConversation(
      id: owner.id, channel: channel, name: name, owner: owner, messages: [])
  }

  private func armedState() throws -> (
    state: OverlayState, broadcast: OverlayConversation, lena: OverlayConversation,
    astra: OverlayConversation
  ) {
    let broadcast = OverlayConversation(
      id: "map-all", channel: "0", name: "All", owner: nil, messages: [])
    let lena = try ownerConversation("lena", channel: "2", session: "lena-session", lease: "a")
    let astra = try ownerConversation("astra", channel: "3", session: "astra-session", lease: "b")
    let state = OverlayState.previewFormatted()
    optIntoChannelMap(state)
    state.applyConversationSnapshot(
      .init(
        deliveries: [
          OverlayChannelDelivery(
            channel: "2", agent: "lena", deliveryID: "map-lena", stage: .received, isOpen: false),
          OverlayChannelDelivery(
            channel: "3", agent: "astra", deliveryID: "map-astra", stage: .received,
            isOpen: false),
        ], conversations: [broadcast, lena, astra]))
    state.applyChannelRoster(
      [broadcast, lena, astra].map {
        CsChannelRosterState(
          channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
          providerSessionId: $0.owner?.providerSessionID, open: false, loud: false,
          autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
      })
    state.agentChannelModifierSetting = { "ctrl" }
    return (state, broadcast, lena, astra)
  }

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

  /// A roster of agents on digits 1… with a current conversation each, so
  /// every chip is selectable; the map is on in the horizontal layout.
  private func crowdedState(_ names: [String]) throws -> (
    state: OverlayState, conversations: [String: OverlayConversation]
  ) {
    var conversations: [String: OverlayConversation] = [:]
    for (index, name) in names.enumerated() {
      let digit = String(index + 1)
      conversations[digit] = try ownerConversation(
        name.lowercased(), channel: digit, session: "crowd-\(digit)",
        lease: String(index, radix: 16))
    }
    let ordered = conversations.keys.sorted().compactMap { conversations[$0] }
    let state = OverlayState.previewFormatted()
    optIntoChannelMap(state)
    state.applyConversationSnapshot(
      .init(
        deliveries: ordered.map {
          OverlayChannelDelivery(
            channel: $0.channel, agent: $0.name, deliveryID: "crowd-\($0.channel)",
            stage: .received, isOpen: false)
        }, conversations: ordered))
    state.applyChannelRoster(
      ordered.map {
        CsChannelRosterState(
          channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
          providerSessionId: $0.owner?.providerSessionID, open: false, loud: false,
          autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
      })
    state.agentChannelModifierSetting = { "ctrl" }
    return (state, conversations)
  }

  // MARK: Geometry fixtures

  /// The expanded bar without a map, measured by the product: padding, the
  /// lockup and the tools. Ladder fixtures add synthetic names to it.
  private var barBase: CGFloat {
    DictationOverlayWindow.midiBarGeometry(chips: [], textScale: 1, showsMeter: false).baseWidth
  }

  private func geometry(names: [CGFloat], budget: CGFloat? = nil) -> OverlayBarGeometry {
    var geometry = OverlayBarGeometry(
      baseWidth: barBase, horizontalPadding: DictationOverlayWindow.miniOuterPadding,
      nameWidths: names, circle: OverlayChannelPill.digitCircle,
      inlineGap: OverlayChannelPill.chipGap)
    if let budget { geometry.oneRowBudget = budget }
    return geometry
  }

  /// A name width at which `count` agents still need the whole ladder: the
  /// two-row map is wider than the densest one-row map, so narrowing past
  /// two rows truncates and then overlaps. Derived from the product's own
  /// constants, never from a pixel value.
  private func ladderNameWidth(count: Int) -> CGFloat {
    let n = CGFloat(count)
    let densest =
      barBase + OverlayBarGeometry.groupGap + n * OverlayBarGeometry.densestSlot
      + (n - 1) * OverlayBarGeometry.oneRowMinimumGap
    let perName =
      (densest - DictationOverlayWindow.miniOuterPadding
        - n * (OverlayChannelPill.digitCircle + OverlayChannelPill.chipGap)
        - (n - 1) * OverlayBarGeometry.twoRowMinimumGap) / n
    return ceil(perName) + 8
  }

  private enum Rung: Equatable {
    /// One row, every name in full.
    case oneRow
    /// The map on its own second row, names beside circles.
    case twoRows
    /// One row, at least one name cut, no circle overlapping.
    case truncated
    /// One row, circles overlapping.
    case overlapped
  }

  private func rung(_ layout: OverlayChannelMapLayout, full: [CGFloat]) -> Rung {
    if layout.arrangement == .twoRows { return .twoRows }
    if layout.overlaps { return .overlapped }
    return zip(layout.nameWidths, full).contains { $0 + 0.5 < $1 } ? .truncated : .oneRow
  }

  /// Walks the bar from `start` (past its natural width by default) down to
  /// its floor, one point at a time, checking the invariants that hold at every width, and returns
  /// the rungs in the order the narrowing bar meets them.
  private func walkLadder(
    _ geometry: OverlayBarGeometry, from start: CGFloat? = nil,
    file: StaticString = #filePath, line: UInt = #line
  ) -> [Rung] {
    var rungs: [Rung] = []
    let full = geometry.nameWidths
    var width = start ?? ceil(geometry.naturalWidth) + 40
    while width >= geometry.floorWidth - 0.01 {
      let layout = geometry.layout(width: width)
      let current = rung(layout, full: full)
      if rungs.last != current { rungs.append(current) }
      XCTAssertEqual(
        layout.slots.count, full.count, "\(width) pt: a slot per agent", file: file, line: line)
      XCTAssertEqual(
        geometry.extraHeight(width: width),
        current == .twoRows ? OverlayBarGeometry.rowGap + OverlayBarGeometry.secondRowHeight : 0,
        "\(width) pt: only the second row adds height", file: file, line: line)
      for (shown, name) in zip(layout.nameWidths, full) {
        XCTAssertLessThanOrEqual(
          shown, name + 0.01, "\(width) pt: a name never grows", file: file, line: line)
      }
      switch layout.arrangement {
      case .oneRow:
        XCTAssertLessThanOrEqual(
          geometry.baseWidth + OverlayBarGeometry.groupGap + layout.width, width + 0.5,
          "\(width) pt: the one-row map fits beside the tools", file: file, line: line)
        XCTAssertGreaterThanOrEqual(
          layout.spacing, OverlayBarGeometry.oneRowMinimumGap - 0.01,
          "\(width) pt: names never run together", file: file, line: line)
        let origins = layout.origins
        for index in layout.slots.indices {
          XCTAssertGreaterThanOrEqual(
            layout.slots[index], OverlayBarGeometry.densestSlot - 0.01,
            "\(width) pt: every channel keeps a hit area", file: file, line: line)
          XCTAssertLessThanOrEqual(
            layout.nameWidths[index], layout.slots[index] + 0.5,
            "\(width) pt: a name stays inside its own slot", file: file, line: line)
          if index > 0 {
            XCTAssertGreaterThanOrEqual(
              origins[index] - (origins[index - 1] + layout.slots[index - 1]),
              OverlayBarGeometry.oneRowMinimumGap - 0.01,
              "\(width) pt: neighbouring slots never touch", file: file, line: line)
          }
        }
        if current == .truncated {
          for (shown, name) in zip(layout.nameWidths, full) {
            XCTAssertGreaterThanOrEqual(
              shown, min(name, layout.circle) - 0.5,
              "\(width) pt: a cut name keeps at least the circle's width", file: file, line: line)
          }
        }
      case .twoRows:
        XCTAssertLessThanOrEqual(
          geometry.horizontalPadding + layout.width, width + 0.5,
          "\(width) pt: the second row fits the bar", file: file, line: line)
        XCTAssertLessThanOrEqual(geometry.baseWidth, width + 0.5, file: file, line: line)
        XCTAssertGreaterThanOrEqual(
          layout.spacing, OverlayBarGeometry.twoRowMinimumGap - 0.01, file: file, line: line)
        XCTAssertEqual(layout.nameWidths, full, "two rows keep full names", file: file, line: line)
      }
      width -= 1
    }
    return rungs
  }

  // MARK: Rendering fixtures

  private struct Raster {
    let width: Int
    let height: Int
    let scale: CGFloat
    let pixels: [SIMD4<Double>]

    /// The pixel under a point in the view's top-left coordinates.
    func at(_ x: CGFloat, _ y: CGFloat) -> SIMD4<Double> {
      let column = min(width - 1, max(0, Int(x * scale)))
      let row = min(height - 1, max(0, Int(y * scale)))
      return pixels[row * width + column]
    }

    func count(near color: NSColor, within tolerance: Double) -> Int {
      let target = Self.rgba(color)
      return pixels.filter { $0.w > 0.5 && Self.distance($0, target) < tolerance }.count
    }

    static func rgba(_ color: NSColor) -> SIMD4<Double> {
      let srgb = color.usingColorSpace(.sRGB) ?? color
      return SIMD4(
        Double(srgb.redComponent), Double(srgb.greenComponent), Double(srgb.blueComponent),
        Double(srgb.alphaComponent))
    }

    static func distance(_ a: SIMD4<Double>, _ b: SIMD4<Double>) -> Double {
      let d = a - b
      return (d.x * d.x + d.y * d.y + d.z * d.z).squareRoot()
    }
  }

  /// Draws a view on the dark sheet colour in a presented borderless window
  /// and reads its pixels back.
  private func render(_ content: some View, size: CGSize) throws -> Raster {
    let palette = OverlayAppearancePalette.dark
    let host = NSHostingView(
      rootView: content
        .frame(width: size.width, height: size.height, alignment: .topLeading)
        .background(palette.desktopBackground.color)
        .preferredColorScheme(.dark))
    host.frame = CGRect(origin: .zero, size: size)
    let window = NSWindow(
      contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.appearance = NSAppearance(named: .darkAqua)
    window.contentView = host
    BackgroundTestWindow.present(window)
    defer { window.close() }
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    host.layoutSubtreeIfNeeded()
    let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
    host.cacheDisplay(in: host.bounds, to: bitmap)
    var pixels: [SIMD4<Double>] = []
    pixels.reserveCapacity(bitmap.pixelsWide * bitmap.pixelsHigh)
    for y in 0..<bitmap.pixelsHigh {
      for x in 0..<bitmap.pixelsWide {
        pixels.append(
          bitmap.colorAt(x: x, y: y).map(Raster.rgba) ?? SIMD4(0, 0, 0, 0))
      }
    }
    return Raster(
      width: bitmap.pixelsWide, height: bitmap.pixelsHigh,
      scale: CGFloat(bitmap.pixelsWide) / size.width, pixels: pixels)
  }

  // MARK: - Projection (§13.2)

  func testOnlyBoundAgentDigitsFollowInOrderWithoutChannelZero() {
    let map = model(
      channels: [
        delivery("5", "Kuba"), delivery("3", "Igor"),
        // Delivery history alone does not bind a digit.
        delivery("7", "ghost"),
        // Channel 10 has no chord, so it stays off the map.
        delivery("10", "Founder"),
      ],
      hudStates: [
        "0": hud(alive: nil), "3": hud(session: "s3"), "5": hud(session: "s5"),
        "10": hud(session: "s10"),
      ])
    XCTAssertEqual(map.chips.map(\.digit), ["3", "5"], "no 0 All on the map")
    XCTAssertEqual(map.chips.map(\.name), ["Igor", "Kuba"])
    XCTAssertFalse(map.chips.contains(where: \.isBroadcast))
    XCTAssertEqual(map.count, 2, "the counter counts agents")
    XCTAssertEqual(map.counterFigure, "2")
  }

  func testNoRosterMeansNoMap() {
    let map = model(channels: [delivery("2", "Miksa")], hudStates: [:])
    XCTAssertTrue(map.chips.isEmpty)
    let bar = DictationOverlayWindow.midiBarGeometry(
      chips: map.chips, textScale: 1, showsMeter: false)
    XCTAssertEqual(bar.naturalWidth, bar.baseWidth, "no map: the bar is its base, no hole")
    XCTAssertEqual(bar.floorWidth, bar.baseWidth)
    XCTAssertEqual(bar.layout(width: bar.naturalWidth).slots, [])
    XCTAssertEqual(bar.extraHeight(width: bar.naturalWidth), 0)
  }

  func testChannelZeroAloneIsNoMap() {
    let map = model(channels: [], hudStates: ["0": hud(alive: nil)])
    XCTAssertTrue(map.chips.isEmpty, "channel 0 is not an agent")
  }

  func testChannelZeroAppearsOnlyWhileTheMicrophoneBroadcasts() {
    let map = scenario(open: ["0"])
    XCTAssertEqual(map.chips.map(\.digit), ["0", "2", "3", "5"])
    let zero = map.chips[0]
    XCTAssertTrue(zero.isBroadcast)
    XCTAssertTrue(zero.microphoneOpen)
    XCTAssertEqual(zero.name, "", "no All label")
    XCTAssertEqual(
      DictationOverlayWindow.midiBarGeometry(chips: map.chips, textScale: 1, showsMeter: false)
        .nameWidths.first, 0, "channel 0 is a circle with its badge and no name")
    XCTAssertEqual(map.count, 3, "channel 0 is not counted")
    XCTAssertEqual(map.agents.map(\.digit), ["2", "3", "5"])
    XCTAssertEqual(map.openDigits, ["0"])
    XCTAssertEqual(map.counterFigure, "0", "the counter shows 0 with the badge")
    XCTAssertEqual(map.counterHint, "2 Miksa · 3 Igor · 5 Kuba", "the hint lists agents only")
    XCTAssertEqual(zero.hint(modifier: .control), "0 · Ctrl+0")
    XCTAssertEqual(
      zero.accessibilityLabel(modifier: .control), "Channel 0, shortcut Ctrl 0, microphone open")
    // Broadcasting with no agent bound still shows the open channel.
    let alone = model(channels: [], hudStates: ["0": hud(open: true, alive: nil)])
    XCTAssertEqual(alone.chips.map(\.digit), ["0"])
    XCTAssertEqual(alone.count, 0)
    XCTAssertEqual(alone.counterAccessibilityValue, "Microphone open on channel 0")
  }

  func testDeadFollowerDimsItsChipAndKeepsItsDigit() {
    let map = scenario(dead: ["3"])
    XCTAssertEqual(map.chips.map(\.digit), ["2", "3", "5"])
    XCTAssertEqual(map.chips.map(\.disconnected), [false, true, false])
  }

  func testUnreadSelectableAndSelectedComeFromTheRoster() {
    let map = model(
      channels: [delivery("2", "Miksa"), delivery("3", "Igor")],
      hudStates: ["0": hud(alive: nil), "2": hud(session: "s2"), "3": hud(session: "s3")],
      selected: "3", selectable: ["0", "2"], unread: ["3"])
    XCTAssertEqual(map.chips.map(\.selectable), [true, false])
    XCTAssertEqual(map.chips.map(\.unread), [false, true])
    XCTAssertEqual(map.chips.map(\.selected), [false, true])
    XCTAssertTrue(map.anyUnread)
  }

  // MARK: - Names (§13.5)

  func testLeaseNamesAreCapitalisedOnTheMapOnly() {
    let map = model(
      channels: [delivery("3", "igor"), delivery("4", "hania"), delivery("6", "konfig")],
      hudStates: [
        "0": hud(alive: nil), "3": hud(session: "s3"), "4": hud(session: "s4"),
        "6": hud(session: "s6"),
      ])
    XCTAssertEqual(map.chips.map(\.name), ["Igor", "Hania", "Konfig"])
    XCTAssertEqual(map.chips.map(\.digit), ["3", "4", "6"], "identity stays the digit")
    XCTAssertEqual(map.counterHint, "3 Igor · 4 Hania · 6 Konfig")
    XCTAssertEqual(map.chips[1].hint(modifier: .control), "4 · Hania · Ctrl+4")
    XCTAssertEqual(OverlayChannelMapChip.displayName("ąbc"), "Ąbc", "the first grapheme only")
    XCTAssertEqual(OverlayChannelMapChip.displayName("miksa-igor"), "Miksa-igor")
    XCTAssertEqual(OverlayChannelMapChip.displayName("Kuba"), "Kuba")
    XCTAssertEqual(OverlayChannelMapChip.displayName(""), "")
  }

  func testCapitalisationLeavesTheRosterLeaseNamesAlone() {
    let state = OverlayState.previewFormatted()
    state.applyChannelRoster([
      CsChannelRosterState(
        channel: "3", audience: "igor", provider: "codex", providerSessionId: "igor-session",
        open: false, loud: false, autosealDeadlineUnixMs: nil, followerAlive: true,
        unreadSinceUnixMs: nil)
    ])
    XCTAssertEqual(state.channelMapLayoutModel.chips.map(\.name), ["Igor"])
    XCTAssertEqual(state.channelRosterNames["3"], "igor", "the lease keeps its name")
  }

  // MARK: - Open-channel dot (§13.3, §14)

  func testOpenChannelCarriesTheDotInPlace() {
    let map = scenario(open: ["5"])
    XCTAssertEqual(map.chips.map(\.digit), ["2", "3", "5"], "no second row, no reorder")
    XCTAssertEqual(map.chips.map(\.microphoneOpen), [false, false, true])
    XCTAssertEqual(map.openDigits, ["5"])
    XCTAssertEqual(map.counterFigure, "5", "the collapsed counter shows the open digit")
    XCTAssertEqual(map.count, 3, "the agent count returns when the channel closes")
    XCTAssertEqual(
      map.chips[2].accessibilityLabel(modifier: .control),
      "Channel 5, Kuba, shortcut Ctrl 5, microphone open")
    XCTAssertTrue(map.counterAccessibilityValue.hasPrefix("Microphone open on channel 5"))
    XCTAssertEqual(
      DictationOverlayWindow.midiBarGeometry(chips: map.chips, textScale: 1, showsMeter: false),
      DictationOverlayWindow.midiBarGeometry(
        chips: scenario().chips, textScale: 1, showsMeter: false),
      "the ring and the dot overlap the circle and add no width")
  }

  func testClosedChannelReturnsThePlainChipAndCounter() {
    let map = scenario()
    XCTAssertFalse(map.microphoneOpen)
    XCTAssertEqual(map.counterFigure, "3", "the counter counts agents")
    XCTAssertEqual(
      map.chips[2].accessibilityLabel(modifier: .control), "Channel 5, Kuba, shortcut Ctrl 5")
  }

  func testRecordingCounterHugsItsCircledDigit() {
    let one = OverlayChannelPill.counterWidth(figure: "3", openDigitCount: 1, scale: 1)
    XCTAssertEqual(
      one,
      ceil(2 * OverlayChannelPill.counterRecordingPadding + OverlayChannelPill.digitCircle))
    XCTAssertGreaterThan(
      OverlayChannelPill.counterWidth(figure: "3", openDigitCount: 2, scale: 1), one)
    XCTAssertLessThan(
      one, OverlayChannelPill.counterWidth(figure: "3", openDigitCount: 0, scale: 1),
      "the open digit is narrower than 3 ▾")
  }

  // MARK: - Mini (brief §2)

  /// Mini is as wide as its content (Monika's bar brief, 2026-10-30): the
  /// brand dot, transcription, microphone, the Agents slot and the chevron.
  /// Bound agents add no counter in the classic and horizontal layouts; the
  /// vertical layout adds its stacked counter beside the dot.
  func testMiniWidthIsItsControlsPlusTheCounterAsDrawn() {
    let floor = DictationOverlayWindow.miniFloorWidth
    let control = OverlayMicrophoneGlyph.diameter
    XCTAssertEqual(
      DictationOverlayWindow.miniControlCount, 4, "transcription, microphone, Agents, chevron")
    XCTAssertEqual(
      floor,
      DictationOverlayWindow.miniOuterPadding + DictationOverlayWindow.miniBrandDot
        + DictationOverlayWindow.miniControlCount
        * (DictationOverlayWindow.miniControlSpacing + control))
    let empty = model(channels: [], hudStates: [:])
    XCTAssertEqual(
      DictationOverlayWindow.miniNaturalWidth(map: empty, textScale: 1), floor,
      "no channel: mini is its controls")
    XCTAssertEqual(
      DictationOverlayWindow.miniNaturalWidth(map: scenario(), textScale: 1), floor,
      "bound agents add no counter to mini: the ✦ stands for them")
    XCTAssertEqual(
      DictationOverlayWindow.miniNaturalWidth(map: fullRoster(), textScale: 1.2), floor,
      "a resting mini does not grow with the team or the text")

    for open: Set<String> in [["5"], ["2", "5"], ["0"], ["0", "2", "3", "5"]] {
      let map = scenario(open: open)
      XCTAssertEqual(
        DictationOverlayWindow.miniNaturalWidth(map: map, textScale: 1),
        ceil(
          floor - control
            + OverlayChannelStatusView.miniAgentsButtonWidth(map: map, textScale: 1)),
        "\(open.sorted()): mini swaps the ✦ for the recipients' digits as drawn")
    }
    let one = DictationOverlayWindow.miniNaturalWidth(map: scenario(open: ["5"]), textScale: 1)
    let two = DictationOverlayWindow.miniNaturalWidth(
      map: scenario(open: ["2", "5"]), textScale: 1)
    let four = DictationOverlayWindow.miniNaturalWidth(
      map: scenario(open: ["0", "2", "3", "5"]), textScale: 1)
    XCTAssertGreaterThanOrEqual(one, floor, "the slot never gets narrower than the ✦")
    XCTAssertGreaterThan(two, one, "two recipients widen the capsule instead of clipping it")
    XCTAssertGreaterThan(four, two)

    XCTAssertEqual(
      DictationOverlayWindow.miniNaturalWidth(map: scenario(), textScale: 1, stacked: true),
      ceil(
        floor + DictationOverlayWindow.miniBrandGap
          + OverlayChannelPill.counterWidth(
            figure: "3", openDigitCount: 0, scale: 1, stacked: true)),
      "the vertical layout's stacked counter follows the dot")
    XCTAssertEqual(
      DictationOverlayWindow.miniNaturalWidth(map: empty, textScale: 1, stacked: true), floor,
      "no agent, no counter")
    XCTAssertEqual(
      DictationOverlayWindow.minimumSize(for: .mini).width, floor,
      "mini's floor is its controls")
  }

  /// Monika, 2026-10-30: while she speaks to agents mini stays mini and the
  /// ✦ slot shows the violet digits of every channel the microphone feeds,
  /// overlapping when there are several, `0` for everyone; the hint carries
  /// their full names.
  func testMiniAgentsSlotShowsTheDigitsOfEveryRecipient() {
    let control = OverlayMicrophoneGlyph.diameter
    XCTAssertEqual(
      OverlayChannelStatusView.miniAgentsButtonWidth(map: scenario(), textScale: 1), control,
      "at rest the slot is the ✦")
    let one = scenario(open: ["3"])
    XCTAssertEqual(one.openDigits, ["3"])
    XCTAssertEqual(one.recipientsHint, "3 · Igor")
    let several = scenario(open: ["2", "5"])
    XCTAssertEqual(several.openDigits, ["2", "5"], "every open channel, in channel order")
    XCTAssertEqual(several.recipientsHint, "2 · Miksa, 5 · Kuba")
    let all = scenario(open: ["0"])
    XCTAssertEqual(all.openDigits, ["0"], "everyone reads 0")
    XCTAssertEqual(all.recipientsHint, "0 · All")
    let mixed = scenario(open: ["0", "3"])
    XCTAssertEqual(mixed.openDigits, ["0", "3"])
    XCTAssertEqual(mixed.recipientsHint, "0 · All, 3 · Igor")

    let single = OverlayChannelPill.counterWidth(figure: "3", openDigitCount: 1, scale: 1)
    let pair = OverlayChannelPill.counterWidth(figure: "3", openDigitCount: 2, scale: 1)
    XCTAssertGreaterThan(pair, single)
    XCTAssertLessThan(
      pair - single, OverlayChannelPill.digitCircle, "a further recipient's circle overlaps")
    XCTAssertEqual(
      OverlayChannelStatusView.miniAgentsButtonWidth(map: several, textScale: 1),
      max(control, pair))
    XCTAssertEqual(
      OverlayChannelStatusView.miniAgentsButtonWidth(map: one, textScale: 1),
      max(control, single))
  }

  /// Pointing at mini's ✦ lists the present agents, `N · Name` in channel
  /// order with the yellow dot beside an unread reply; channel 0 is not an
  /// agent and has no row, even while it broadcasts.
  func testAgentsHoverListNamesEveryAgentWithItsDigitAndUnreadDot() {
    let map = model(
      channels: [delivery("5", "kuba"), delivery("2", "lena"), delivery("4", "lena2")],
      hudStates: [
        "0": hud(open: true, alive: nil), "2": hud(session: "s2"), "4": hud(session: "s4"),
        "5": hud(session: "s5"),
      ], selectable: ["2", "4", "5"], unread: ["4"])
    XCTAssertEqual(map.chips.map(\.digit), ["0", "2", "4", "5"])
    let rows = map.agents
    XCTAssertEqual(
      rows.map { OverlayAgentsHoverList.rowTitle($0) }, ["2 · Lena", "4 · Lena2", "5 · Kuba"])
    XCTAssertEqual(rows.map(\.unread), [false, true, false])
    XCTAssertEqual(rows.map(\.selectable), [true, true, true])
    let size = OverlayAgentsHoverList.size(agents: rows)
    XCTAssertEqual(
      size.height,
      2 * OverlayAgentsHoverList.verticalPadding + 3 * OverlayAgentsHoverList.rowHeight)
    let widest =
      rows.map {
        OverlayChannelPill.textWidth(
          OverlayAgentsHoverList.rowTitle($0), size: OverlayAgentsHoverList.fontSize)
      }.max() ?? 0
    XCTAssertGreaterThanOrEqual(
      size.width, widest + 2 * OverlayAgentsHoverList.horizontalPadding,
      "every row fits its title")
    XCTAssertGreaterThan(
      OverlayAgentsHoverList.size(agents: fullRoster().agents).height, size.height,
      "the list grows with the team")
  }

  // MARK: - Bar geometry (brief §3–§5)

  func testBarGeometryMeasuresTheBarAndEveryName() {
    let map = scenario(open: ["0"])
    let bar = DictationOverlayWindow.midiBarGeometry(
      chips: map.chips, textScale: 1, showsMeter: false)
    XCTAssertEqual(
      bar.nameWidths,
      map.chips.map { $0.isBroadcast ? 0 : OverlayChannelPill.nameWidth($0.name, scale: 1) })
    XCTAssertEqual(bar.circle, OverlayChannelPill.digitCircle)
    XCTAssertEqual(bar.inlineGap, OverlayChannelPill.chipGap)
    XCTAssertEqual(bar.horizontalPadding, DictationOverlayWindow.miniOuterPadding)
    XCTAssertEqual(bar.baseWidth, barBase, "the map's names are not part of the base")
    XCTAssertEqual(
      barBase,
      ceil(
        DictationOverlayWindow.miniOuterPadding + DictationOverlayWindow.barBrandWidth
          + OverlayBarGeometry.groupGap + DictationOverlayWindow.barTools))

    let meter = DictationOverlayWindow.midiBarGeometry(
      chips: map.chips, textScale: 1, showsMeter: true)
    XCTAssertEqual(
      meter.baseWidth - bar.baseWidth,
      DictationOverlayWindow.barWaveformWidth + DictationOverlayWindow.barMeterSpacing
        + DictationOverlayWindow.barClockWidth + OverlayBarGeometry.groupGap,
      accuracy: 1, "the waveform and the clock take room only while recording")
    XCTAssertEqual(meter.nameWidths, bar.nameWidths)
    let counter = DictationOverlayWindow.midiBarGeometry(
      chips: [], textScale: 1, showsMeter: false,
      leadingCounter: OverlayChannelPill.headerHeight)
    XCTAssertEqual(
      counter.baseWidth - barBase,
      DictationOverlayWindow.miniBrandGap + OverlayChannelPill.headerHeight, accuracy: 1,
      "the vertical layout's counter beside the brand")

    let larger = DictationOverlayWindow.midiBarGeometry(
      chips: map.chips, textScale: 1.2, showsMeter: false)
    XCTAssertGreaterThan(larger.nameWidths[1], bar.nameWidths[1])
    XCTAssertEqual(larger.circle, OverlayChannelPill.digitCircle * 1.2, accuracy: 0.001)
    XCTAssertEqual(
      DictationOverlayWindow.midiBarGeometry(chips: map.chips, textScale: 3, showsMeter: false),
      DictationOverlayWindow.midiBarGeometry(
        chips: map.chips, textScale: OverlayChannelPill.headerScaleCeiling, showsMeter: false),
      "the map stops scaling where its row would grow")
  }

  /// The width at which `geometry` lays every name out in one row at the
  /// natural gap, plus room to start the walk above it.
  private func walkStart(_ geometry: OverlayBarGeometry) -> CGFloat {
    let row =
      geometry.baseWidth + OverlayBarGeometry.groupGap
      + geometry.nameWidths.reduce(0) { $0 + max(geometry.circle, $1) }
      + CGFloat(geometry.nameWidths.count - 1) * OverlayBarGeometry.oneRowNaturalGap
    return max(geometry.naturalWidth, row) + 20
  }

  /// The rungs from a bar wide enough for one row with every full name down
  /// to the floor.
  private func ladder(
    _ geometry: OverlayBarGeometry, file: StaticString = #filePath, line: UInt = #line
  ) -> [Rung] {
    walkLadder(geometry, from: ceil(walkStart(geometry)), file: file, line: line)
  }

  /// Nine agents narrowing from a bar wide enough for one row down to the
  /// floor: one row, two rows (nine full names pass the one-row budget), one
  /// row with cut names, then overlapping circles down to the densest slot.
  func testNineAgentsWalkTheWholeLadderToOverlappingCircles() {
    let names = Array(repeating: ladderNameWidth(count: 9), count: 9)
    let bar = geometry(names: names)
    XCTAssertEqual(ladder(bar), [.oneRow, .twoRows, .truncated, .overlapped])
    XCTAssertEqual(
      bar.layout(width: bar.naturalWidth).arrangement, .twoRows,
      "a row past the budget makes the natural bar two rows")
    let densest =
      bar.baseWidth + OverlayBarGeometry.groupGap + 9 * OverlayBarGeometry.densestSlot
      + 8 * OverlayBarGeometry.oneRowMinimumGap
    XCTAssertEqual(bar.floorWidth, densest, accuracy: 0.5, "the floor is the densest layout")
    let floor = bar.layout(width: bar.floorWidth)
    XCTAssertTrue(floor.overlaps)
    XCTAssertEqual(floor.slots, Array(repeating: OverlayBarGeometry.densestSlot, count: 9))
    XCTAssertEqual(floor.spacing, OverlayBarGeometry.oneRowMinimumGap)

    // The real roster measures the same ladder.
    let roster = DictationOverlayWindow.midiBarGeometry(
      chips: fullRoster().chips, textScale: 1, showsMeter: false)
    let rungs = ladder(roster)
    XCTAssertEqual(rungs.first, .oneRow)
    XCTAssertEqual(rungs.last, .overlapped, "nine named agents reach overlapping circles")
    XCTAssertTrue(rungs.contains(.twoRows))
  }

  /// Four agents whose two-row map is wider than the densest one-row map
  /// walk the same ladder: the count does not choose the rungs.
  func testFourAgentsWithLongNamesWalkTheWholeLadder() {
    let bar = geometry(names: Array(repeating: ladderNameWidth(count: 4), count: 4))
    XCTAssertEqual(ladder(bar), [.oneRow, .twoRows, .truncated, .overlapped])
    let floor = bar.layout(width: bar.floorWidth)
    XCTAssertTrue(floor.overlaps)
    XCTAssertEqual(floor.slots, Array(repeating: OverlayBarGeometry.densestSlot, count: 4))
  }

  /// Four short names fit one row within the budget: narrowing first closes
  /// the gaps from 8 to 3 pt, then moves the map to a second row, where the
  /// full names stay down to the bar's base width.
  func testFourShortNamesCloseTheirGapsBeforeTakingTwoRows() {
    let name = 2 * OverlayChannelPill.digitCircle
    let bar = geometry(names: Array(repeating: name, count: 4))
    let natural =
      bar.baseWidth + OverlayBarGeometry.groupGap + 4 * name
      + 3 * OverlayBarGeometry.oneRowNaturalGap
    XCTAssertEqual(bar.naturalWidth, natural, accuracy: 0.01, "one row, never wider than it")
    XCTAssertEqual(bar.layout(width: natural).spacing, OverlayBarGeometry.oneRowNaturalGap)
    let tight =
      bar.baseWidth + OverlayBarGeometry.groupGap + 4 * name
      + 3 * OverlayBarGeometry.oneRowMinimumGap
    let closed = bar.layout(width: tight)
    XCTAssertEqual(closed.arrangement, .oneRow)
    XCTAssertEqual(closed.spacing, OverlayBarGeometry.oneRowMinimumGap, accuracy: 0.01)
    XCTAssertEqual(closed.nameWidths, bar.nameWidths, "gaps close before any name is cut")
    XCTAssertEqual(bar.layout(width: tight - 1).arrangement, .twoRows)
    XCTAssertEqual(walkLadder(bar), [.oneRow, .twoRows])
  }

  /// Brief §5: whether the map takes a second row follows the measured names
  /// and the width, not the number of agents.
  func testRowsAreDecidedByWidthsNotByTheNumberOfAgents() {
    let circle = OverlayChannelPill.digitCircle
    let tiny = Array(repeating: circle - 2, count: 9)
    let nineInOneRow =
      barBase + OverlayBarGeometry.groupGap + 9 * circle
      + 8 * OverlayBarGeometry.oneRowNaturalGap
    let budget = nineInOneRow + 1
    let nine = geometry(names: tiny, budget: budget)
    XCTAssertEqual(
      nine.layout(width: nine.naturalWidth).arrangement, .oneRow,
      "nine agents with short names keep one row")
    XCTAssertEqual(nine.naturalWidth, nineInOneRow, accuracy: 0.01)
    let long = (budget - barBase - OverlayBarGeometry.groupGap) / 4 + 10
    let four = geometry(names: Array(repeating: long, count: 4), budget: budget)
    XCTAssertEqual(
      four.layout(width: four.naturalWidth).arrangement, .twoRows,
      "four agents with long names take two rows at the same budget")
    // Widening the bar to the full one-row width brings the one row back.
    let fourInOneRow =
      barBase + OverlayBarGeometry.groupGap + 4 * long
      + 3 * OverlayBarGeometry.oneRowNaturalGap
    XCTAssertEqual(four.layout(width: fourInOneRow).arrangement, .oneRow)
    XCTAssertEqual(four.layout(width: fourInOneRow).nameWidths, four.nameWidths)
  }

  func testChipWidthsAreMeasuredFromTheirText() {
    let short = OverlayChannelPill.nameWidth("Igor", scale: 1)
    let long = OverlayChannelPill.nameWidth("Miksa-Igor", scale: 1)
    XCTAssertGreaterThan(long, short)
    XCTAssertEqual(
      short, OverlayChannelPill.textWidth("Igor", size: OverlayChannelPill.nameSize))
    XCTAssertGreaterThan(OverlayChannelPill.nameWidth("Igor", scale: 1.2), short)
    // A one-row slot is the wider of the circle and the full name.
    let tiny = OverlayChannelPill.nameWidth("A", scale: 1)
    let bar = geometry(names: [tiny, long])
    let layout = bar.layout(width: bar.naturalWidth)
    XCTAssertEqual(layout.arrangement, .oneRow)
    XCTAssertEqual(layout.slots, [max(OverlayChannelPill.digitCircle, tiny), long])
    XCTAssertEqual(
      layout.width, layout.slots.reduce(0, +) + OverlayBarGeometry.oneRowNaturalGap)
    XCTAssertEqual(layout.origins, [0, layout.slots[0] + OverlayBarGeometry.oneRowNaturalGap])
  }

  /// Brief §4: similar names are told apart by the channel number, and the
  /// hint names the full agent, also for Lena and Lena2.
  func testTruncatedNamesStayApartAndTheHintCarriesTheFullName() throws {
    let lena = OverlayChannelPill.nameWidth("Lena", scale: 1)
    XCTAssertEqual(OverlayChannelPill.truncatedName("Lena", width: lena, scale: 1), "Lena")
    let cut = OverlayChannelPill.truncatedName("Lena2", width: lena, scale: 1)
    XCTAssertNotEqual(cut, "Lena", "a cut name never reads as another agent's full name")
    XCTAssertNotEqual(cut, "Lena2")
    XCTAssertTrue(cut.hasPrefix("L"))
    var width = OverlayChannelPill.nameWidth("Lena2", scale: 1)
    while width > 0 {
      let shown = OverlayChannelPill.truncatedName("Lena2", width: width, scale: 1)
      XCTAssertFalse(shown.isEmpty, "\(width) pt: every slot keeps a mark of its own")
      if shown.hasSuffix("\u{2026}") {
        XCTAssertTrue("Lena2".hasPrefix(String(shown.dropLast())), shown)
        XCTAssertLessThanOrEqual(OverlayChannelPill.nameWidth(shown, scale: 1), width + 0.5)
      } else if shown != "Lena2" {
        XCTAssertEqual(shown, "L", "\(width) pt: below any prefix the initial stays")
      }
      width -= 1
    }

    let map = model(
      channels: [delivery("2", "lena"), delivery("4", "lena2")],
      hudStates: ["2": hud(session: "s2"), "4": hud(session: "s4")], selectable: ["2", "4"])
    XCTAssertEqual(
      map.chips.map { $0.hint(modifier: .control) },
      ["2 · Lena · Ctrl+2", "4 · Lena2 · Ctrl+4"])
    XCTAssertEqual(
      map.chips.map { OverlayAgentsHoverList.rowTitle($0) }, ["2 · Lena", "4 · Lena2"])
    XCTAssertEqual(
      map.chips[1].accessibilityLabel(modifier: .control), "Channel 4, Lena2, shortcut Ctrl 4")
    // The map hands the chip its cut name and the hint its full one.
    let view = try overlaySource("OverlayChannelMapView.swift")
    XCTAssertTrue(view.contains("chip.name, width: layout.nameWidths[index], scale: scale"))
    XCTAssertTrue(view.contains("hint: chip.hint(modifier: modifier)"))
  }

  func testStripPillsStopScalingWhereTheRowWouldGrow() {
    XCTAssertEqual(OverlayChannelPill.headerScale(1), 1)
    XCTAssertEqual(OverlayChannelPill.headerScale(0.8), 0.8)
    XCTAssertEqual(OverlayChannelPill.headerScale(2), OverlayChannelPill.headerScaleCeiling)
    XCTAssertLessThanOrEqual(
      OverlayChannelPill.digitCircle * OverlayChannelPill.headerScaleCeiling,
      OverlayChannelPill.headerHeight)
    XCTAssertEqual(OverlayChannelPill.headerHeight, OverlayMicrophoneGlyph.diameter)
  }

  // MARK: - Shortcut hints (§5.1, §6)

  func testModifierFollowsTheSettingsNormalization() {
    XCTAssertEqual(OverlayChannelChordModifier(setting: "ctrl"), .control)
    XCTAssertEqual(OverlayChannelChordModifier(setting: "fn"), .function)
    XCTAssertEqual(OverlayChannelChordModifier(setting: "FN"), .function)
    XCTAssertEqual(OverlayChannelChordModifier(setting: "cmd"), .control, "Command is not a value")
    XCTAssertEqual(OverlayChannelChordModifier(setting: ""), .control)
  }

  func testChipHintNamesTheChordFromTheSetting() {
    let kuba = scenario().chips[2]
    XCTAssertEqual(kuba.hint(modifier: .control), "5 · Kuba · Ctrl+5")
    XCTAssertEqual(kuba.hint(modifier: .function), "5 · Kuba · Fn+5")
    XCTAssertEqual(
      kuba.accessibilityLabel(modifier: .function), "Channel 5, Kuba, shortcut Fn 5")
  }

  /// Both keys record dictation: the core resolves the modifier to `off`,
  /// and no hint names a chord that does not exist.
  func testNoChordWhenBothKeysRecord() {
    XCTAssertEqual(OverlayChannelChordModifier(setting: "off"), .unbound)
    let kuba = scenario().chips[2]
    XCTAssertEqual(kuba.hint(modifier: .unbound), "5 · Kuba")
    XCTAssertEqual(kuba.accessibilityLabel(modifier: .unbound), "Channel 5, Kuba")
    XCTAssertFalse(kuba.accessibilityLabel(modifier: .unbound).contains("shortcut"))
  }

  func testCounterHintListsTheAgents() {
    XCTAssertEqual(scenario().counterHint, "2 Miksa · 3 Igor · 5 Kuba")
    XCTAssertEqual(scenario().counterAccessibilityValue, "2 Miksa · 3 Igor · 5 Kuba")
    XCTAssertFalse(scenario().counterHint.contains("All"))
  }

  func testOverlayReadsTheModifierFromSettings() {
    let state = OverlayState.previewFormatted()
    XCTAssertEqual(state.agentChannelModifier, .control)
    state.agentChannelModifierSetting = { "fn" }
    state.refreshAgentChannelModifier()
    XCTAssertEqual(state.agentChannelModifier, .function)
    state.agentChannelModifierSetting = { "ctrl" }
    state.refreshAgentChannelModifier()
    XCTAssertEqual(state.agentChannelModifier, .control)
  }

  // MARK: - Click (decision D1, §13.4)

  func testChipSelectsItsConversationLikeTheRailPillAndNeverTouchesCapture() throws {
    let (state, broadcast, _, astra) = try armedState()
    state.setPresentationMode(.midi)
    let generation = state.captureGeneration
    let view = DictationOverlayView(state: state)
    let roster = view.channelStatusView
    let map = roster.channelMapModel
    XCTAssertEqual(map.chips.map(\.digit), ["2", "3"])
    XCTAssertEqual(map.chips.map(\.name), ["Lena", "Astra"])
    XCTAssertEqual(map.chips.map(\.selectable), [true, true])
    XCTAssertEqual(map.chips.map(\.selected), [false, false])

    roster.selectChannelConversation("3")
    XCTAssertEqual(state.selectedConversationID, astra.id, "the exact owner's conversation")
    XCTAssertEqual(state.windowContentSurface, .agents)
    XCTAssertEqual(state.presentationMode, .expanded, "the Agents canvas unfolds")
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty, "a chip is not a microphone")
    XCTAssertFalse(state.channelHudStates.values.contains(where: \.open))
    XCTAssertEqual(
      DictationOverlayView(state: state).channelStatusView.channelMapModel.chips.map(\.selected),
      [false, true], "the viewed conversation's chip wears the fill")

    roster.selectChannelConversation("0")
    XCTAssertEqual(state.selectedConversationID, broadcast.id)
    // A digit with no current conversation is not an identity.
    roster.selectChannelConversation("7")
    XCTAssertEqual(state.selectedConversationID, broadcast.id)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty)
  }

  /// A row of mini's ✦ list selects its conversation through the same
  /// roster path a map chip takes, so the two never disagree about the owner.
  func testAgentsListRowSelectsThroughTheChipsPath() throws {
    let (state, _, lena, _) = try armedState()
    state.setPresentationMode(.mini)
    let generation = state.captureGeneration
    let roster = DictationOverlayView(state: state).channelStatusView
    XCTAssertEqual(
      roster.channelMapModel.agents.map { OverlayAgentsHoverList.rowTitle($0) },
      ["2 · Lena", "3 · Astra"])
    roster.selectChannelConversation("2")
    XCTAssertEqual(state.selectedConversationID, lena.id, "the exact owner's conversation")
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty, "a row is not a microphone")

    let status = try overlaySource("OverlayChannelStatusView.swift")
    let button = try section(
      of: status, from: "private var agentsButton: some View", to: "private func recipientDigits(")
    XCTAssertTrue(button.contains("onSelect: { selectChannelConversation($0) }"))
    XCTAssertTrue(
      button.contains("hintInteractive: list != nil"), "the pointer can move into the list")
    XCTAssertTrue(
      button.contains("let agents = miniAgentsButton == nil ? [] : channelMapModel.agents"),
      "only mini's ✦ lists the agents")
    let header = try overlaySource("DictationOverlayView.swift")
    let strip = try section(
      of: header, from: "private func channelStrip(", to: "private func refreshingChordModifier")
    XCTAssertTrue(strip.contains("onSelectChannel: { roster.selectChannelConversation($0) }"))
  }

  /// One owner of the ✦ button (Monika's bar brief, 2026-10-30): every face
  /// of the Agents button, mini's recipient digits included, is drawn by
  /// `OverlayChannelStatusView` under one identifier and one action; the
  /// header draws none of its own, and the vertical layout leaves mini's
  /// list and digits off because its counter shows them.
  func testAgentsButtonHasOneOwnerInEveryForm() throws {
    let status = try overlaySource("OverlayChannelStatusView.swift")
    let faces = try section(
      of: status, from: "  var body: some View {\n    if let mini = miniAgentsButton",
      to: "  var monitorBody: some View")
    XCTAssertEqual(faces.components(separatedBy: "\"overlay-agent-glyph\"").count - 1, 2)
    XCTAssertTrue(faces.contains("Button(action: showMonitor)"))
    XCTAssertTrue(faces.contains("showsHint: false, action: showMonitor"))
    XCTAssertTrue(
      faces.contains(
        "if let mini = miniAgentsButton, agentCaptureOpen, channelMapModel.microphoneOpen"),
      "the digits replace the ✦ only in mini and only while agent channels are open")
    let header = try overlaySource("DictationOverlayView.swift")
    XCTAssertFalse(header.contains("overlay-agent-glyph"), "the header draws no second ✦")
    XCTAssertFalse(header.contains("OverlayHeaderGlyph.agents"))
    let slot = try section(
      of: header, from: "private var miniAgentsSlot: some View",
      to: "private var transcriptionPreviewButton")
    XCTAssertTrue(slot.contains("if !state.channelMapCounterStacked {"))
    XCTAssertTrue(
      slot.contains("roster.miniAgentsButton = OverlayChannelStatusView.MiniAgentsButton("))
    let tools = try section(
      of: header, from: "private func barTools(roster: OverlayChannelStatusView)",
      to: "private var miniAgentsSlot: some View")
    XCTAssertFalse(tools.contains("miniAgentsButton"), "the expanded bar keeps the plain ✦")
  }

  func testVerticalMiniCounterOpensTheIntercomAndNothingElse() throws {
    let (state, _, _, _) = try armedState()
    state.channelMapPreference.selectLayout(.vertical)
    state.setPresentationMode(.mini)
    let surface = state.windowContentSurface
    let generation = state.captureGeneration
    var revealed = 0
    state.onRevealIntercom = { revealed += 1 }
    let map = try XCTUnwrap(DictationOverlayView(state: state).channelCounter)
    XCTAssertEqual(map.placement, .counter)
    XCTAssertEqual(map.model.counterFigure, "2", "the badge carries the same agent count")
    map.onCounter()
    XCTAssertEqual(revealed, 1, "the counter leads to the Intercom group")
    XCTAssertEqual(state.presentationMode, .mini, "mini stays mini")
    XCTAssertNil(state.selectedConversationID)
    XCTAssertEqual(state.windowContentSurface, surface)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty)
  }

  /// The horizontal and classic layouts have no counter in any form: mini's
  /// ✦ stands for the agents and the expanded bar draws every one of them. A
  /// stray counter call only brings the Intercom group forward.
  func testHorizontalBarHasNoCounterInAnyForm() throws {
    let (state, _, _, _) = try armedState()
    XCTAssertTrue(state.channelMapInHeader)
    var revealed = 0
    state.onRevealIntercom = { revealed += 1 }
    for mode: OverlayPresentationMode in [.mini, .midi, .expanded] {
      state.setPresentationMode(mode)
      XCTAssertNil(DictationOverlayView(state: state).channelCounter, "\(mode)")
      state.activateChannelMapCounter()
      XCTAssertEqual(state.presentationMode, mode, "\(mode): no counter click changes the form")
    }
    XCTAssertEqual(revealed, 3)
    state.channelMapPreference.select(false)
    state.setPresentationMode(.mini)
    XCTAssertNil(DictationOverlayView(state: state).channelCounter, "classic")
  }

  /// One mini in every layout (Maciej, 2026-10-23): the vertical layout's
  /// mini is the same capsule plus its stacked counter beside the dot.
  func testMiniIsAsWideInBothLayoutsUpToTheCounter() throws {
    let (state, _, _, _) = try armedState()
    let map = state.channelMapLayoutModel
    let strip = DictationOverlayWindow.miniNaturalWidth(map: map, textScale: 1)
    let stacked = DictationOverlayWindow.miniNaturalWidth(map: map, textScale: 1, stacked: true)
    XCTAssertEqual(strip, DictationOverlayWindow.miniFloorWidth)
    XCTAssertEqual(
      stacked - strip, DictationOverlayWindow.miniBrandGap + OverlayChannelPill.headerHeight,
      accuracy: 0.5, "one mini; the vertical counter is the only difference")
  }

  func testNoMapBeforeTheRosterNamesAChannel() {
    let state = OverlayState.previewFormatted()
    optIntoChannelMap(state)
    state.setPresentationMode(.midi)
    XCTAssertTrue(
      DictationOverlayView(state: state).channelStatusView.channelMapModel.chips.isEmpty)
    state.channelMapPreference.selectLayout(.vertical)
    XCTAssertNil(DictationOverlayView(state: state).channelCounter)
  }

  // MARK: - The chord keeps the collapsed form (§1, §14 W3)

  /// The controller's roster after a chord: the conversations' channels, with
  /// `open` naming the channels the microphone feeds.
  private func chordRoster(
    _ conversations: [OverlayConversation], open: Set<String>
  ) -> [CsChannelRosterState] {
    conversations.map {
      CsChannelRosterState(
        channel: $0.channel, audience: $0.name, provider: $0.owner?.provider,
        providerSessionId: $0.owner?.providerSessionID, open: open.contains($0.channel),
        loud: false, autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
    }
  }

  /// With the map on, a chord in mini or compact keeps the form: the strip's
  /// chip or counter shows the open digit, so the chord is readable without
  /// unfolding. The selection follows the open channel as before.
  func testChordKeepsMiniAndCompactWhileTheMapIsOn() throws {
    for form in [OverlayPresentationMode.mini, .midi] {
      let (state, broadcast, lena, astra) = try armedState()
      let conversations = [broadcast, lena, astra]
      XCTAssertTrue(state.channelMapEnabled)
      state.setPresentationMode(form)
      XCTAssertEqual(state.presentationMode, form)

      state.applyChannelRoster(chordRoster(conversations, open: ["3"]))
      XCTAssertEqual(state.presentationMode, form, "\(form): the chord keeps the form")
      let map = state.channelMapLayoutModel
      XCTAssertEqual(map.openDigits, ["3"], "\(form): the strip shows the open digit")
      XCTAssertEqual(map.counterFigure, "3", "\(form): the counter shows the open digit")
      XCTAssertEqual(map.chips.first { $0.digit == "3" }?.microphoneOpen, true)
      XCTAssertEqual(state.selectedConversationID, astra.id, "the selection follows the chord")

      state.applyChannelRoster(chordRoster(conversations, open: []))
      XCTAssertEqual(state.presentationMode, form, "\(form): closing keeps the form")
      let closed = state.channelMapLayoutModel
      XCTAssertFalse(closed.microphoneOpen)
      XCTAssertEqual(closed.counterFigure, String(closed.count), "the agent count returns")

      // A broadcast on 0 by chord keeps the form as well.
      state.applyChannelRoster(chordRoster(conversations, open: ["0"]))
      XCTAssertEqual(state.presentationMode, form, "\(form): broadcasting keeps the form")
      XCTAssertEqual(state.channelMapLayoutModel.openDigits, ["0"])
    }
  }

  /// With the map off, the chord unfolds mini and compact onto the Agents
  /// canvas, as before the map; an expanded overlay stays expanded either way.
  func testChordExpandsAsBeforeWithTheMapOffOrWhenAlreadyExpanded() throws {
    for form in [OverlayPresentationMode.mini, .midi] {
      let (state, broadcast, lena, astra) = try armedState()
      state.channelMapPreference.select(false)
      XCTAssertFalse(state.channelMapEnabled)
      state.setPresentationMode(form)
      state.applyChannelRoster(chordRoster([broadcast, lena, astra], open: ["3"]))
      XCTAssertEqual(state.presentationMode, .expanded, "\(form): the map is off")
      XCTAssertEqual(state.windowContentSurface, .agents)
      XCTAssertEqual(state.selectedConversationID, astra.id)
    }
    let (state, broadcast, lena, astra) = try armedState()
    XCTAssertTrue(state.channelMapEnabled)
    state.setPresentationMode(.expanded)
    state.applyChannelRoster(chordRoster([broadcast, lena, astra], open: ["3"]))
    XCTAssertEqual(state.presentationMode, .expanded)
    XCTAssertEqual(state.windowContentSurface, .agents)
    XCTAssertEqual(state.selectedConversationID, astra.id)
  }

  // MARK: - Hosted bar

  /// The real overlay view in a borderless panel of the given size; the
  /// header lays its form out at that width.
  private func hostOverlay(
    _ state: OverlayState, width: CGFloat, height: CGFloat
  ) throws -> FloatingOverlayPanel {
    let host = NSHostingView(
      rootView: DictationOverlayView(state: state)
        .defaultAppStorage(try XCTUnwrap(UserDefaults(suiteName: UUID().uuidString))))
    host.sizingOptions = []
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 100, y: 100, width: width, height: height),
      styleMask: [.borderless, .nonactivatingPanel, .resizable], backing: .buffered,
      defer: false)
    panel.contentView = OverlayContentContainer(hosting: host)
    BackgroundTestWindow.present(panel)
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.1))
    return panel
  }

  func testWideCompactStripShowsChipsAndAChipClickSelects() throws {
    let (state, _, _, astra) = try armedState()
    state.setPresentationMode(.midi)
    let panel = try hostOverlay(state, width: 900, height: DictationOverlayWindow.collapsedHeight)
    defer { panel.orderOut(nil) }
    for (digit, name) in [("2", "Lena"), ("3", "Astra")] {
      // The probe's XCTSkip (no isolated GUI session) must reach the runner;
      // inside XCTUnwrap's autoclosure it would be recorded as a failure.
      let elements = try NativeAccessibilityProbe.waitForElements(
        "overlay-channel-chip-\(digit)"
      ).elements
      let chip = try XCTUnwrap(elements.first, "chip \(digit)")
      XCTAssertEqual(chip.role, "AXButton")
      XCTAssertLessThanOrEqual(
        chip.height, Double(OverlayChannelPill.mapRowHeight) + 0.5,
        "a circle and its name fit the bar's one row")
      XCTAssertGreaterThanOrEqual(
        chip.width, Double(OverlayChannelPill.nameWidth(name, scale: 1)) - 0.5,
        "a wide bar gives every name its full width")
    }
    XCTAssertTrue(
      try NativeAccessibilityProbe.query("overlay-channel-chip-0").elements.isEmpty,
      "no 0 All chip while channel 0 is closed")
    XCTAssertTrue(
      try NativeAccessibilityProbe.query("overlay-channel-map-toggle").elements.isEmpty,
      "the horizontal bar shows the chips, never a counter")
    _ = try NativeAccessibilityProbe.query("overlay-channel-chip-3", press: true)
    XCTAssertEqual(state.selectedConversationID, astra.id)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty)
    XCTAssertFalse(panel.isKeyWindow)
  }

  /// Brief §4: circles may overlap, and every channel stays selectable. At
  /// the bar's floor nine circles overlap; each chip's hit area is its own
  /// slot, the slots stay apart, and pressing each selects its own channel.
  func testOverlappingCirclesKeepEveryChannelsOwnHitArea() throws {
    let names = ["Miksa", "Nora", "Igor", "Hania", "Kuba", "Konfig", "Klara", "Leon", "Zofia"]
    let (state, conversations) = try crowdedState(names)
    state.setPresentationMode(.midi)
    let model = DictationOverlayView(state: state).channelStatusView.channelMapModel
    XCTAssertEqual(model.chips.map(\.digit), (1...9).map(String.init))
    let bar = DictationOverlayWindow.midiBarGeometry(
      chips: model.chips, textScale: 1, showsMeter: state.channelMapBarShowsMeter)
    let layout = bar.layout(width: bar.floorWidth)
    XCTAssertTrue(layout.overlaps, "nine named agents at the floor overlap their circles")
    let origins = layout.origins
    for index in layout.slots.indices.dropFirst() {
      XCTAssertGreaterThanOrEqual(
        origins[index] - origins[index - 1] - layout.slots[index - 1],
        OverlayBarGeometry.oneRowMinimumGap - 0.01, "slot \(index): hit areas stay apart")
    }

    let panel = try hostOverlay(
      state, width: bar.floorWidth, height: DictationOverlayWindow.collapsedHeight)
    defer { panel.orderOut(nil) }
    var frames: [NSRect] = []
    for digit in model.chips.map(\.digit) {
      let elements = try NativeAccessibilityProbe.waitForElements(
        "overlay-channel-chip-\(digit)"
      ).elements
      let chip = try XCTUnwrap(elements.first, "chip \(digit)")
      XCTAssertLessThanOrEqual(
        chip.width, Double(OverlayBarGeometry.densestSlot) + 0.5,
        "chip \(digit): the slot, not the overlapping circle, is the hit area")
      frames.append(chip.frame)
    }
    for (left, right) in zip(frames, frames.dropFirst()) {
      XCTAssertLessThanOrEqual(left.maxX, right.minX + 0.5, "hit areas never overlap")
    }
    for digit in model.chips.map(\.digit) {
      // A selection unfolds the Agents canvas; the bar comes back for the
      // next press.
      state.setPresentationMode(.midi)
      panel.contentView?.layoutSubtreeIfNeeded()
      RunLoop.main.run(until: Date().addingTimeInterval(0.1))
      _ = try NativeAccessibilityProbe.query("overlay-channel-chip-\(digit)", press: true)
      XCTAssertEqual(
        state.selectedConversationID, conversations[digit]?.id,
        "chip \(digit) selects its own channel")
    }
    XCTAssertTrue(state.pendingChannelToggles.isEmpty, "a chip is not a microphone")
  }

  /// Mini stays mini while Monika speaks to agents (her decision,
  /// 2026-10-30): its one Agents button shows the digits of every recipient
  /// and names them in its value; mini has no map and no counter.
  func testMiniShowsOneAgentsButtonWithTheDigitsOfEveryRecipient() throws {
    let (state, broadcast, lena, astra) = try armedState()
    state.setPresentationMode(.mini)
    state.applyChannelRoster(chordRoster([broadcast, lena, astra], open: ["2", "3"]))
    XCTAssertEqual(state.presentationMode, .mini, "the chord keeps mini")
    XCTAssertEqual(state.channelMapLayoutModel.openDigits, ["2", "3"])
    let panel = try hostOverlay(
      state,
      width: DictationOverlayWindow.miniNaturalWidth(
        map: state.channelMapLayoutModel, textScale: 1),
      height: DictationOverlayWindow.collapsedHeight)
    defer { panel.orderOut(nil) }
    let elements = try NativeAccessibilityProbe.waitForElements("overlay-agent-glyph").elements
    XCTAssertEqual(elements.count, 1, "one Agents button")
    let button = try XCTUnwrap(elements.first)
    XCTAssertTrue(button.value.hasPrefix("Microphone open on channel 2, 3"), button.value)
    XCTAssertTrue(
      try NativeAccessibilityProbe.query("overlay-channel-chip-2").elements.isEmpty,
      "mini has no map")
    XCTAssertTrue(
      try NativeAccessibilityProbe.query("overlay-channel-map-toggle").elements.isEmpty,
      "mini has no counter")
  }

  // MARK: - Marks (brief §6–§7)

  /// The recipient of the current utterance wears the violet ring, the
  /// viewed conversation the muted violet fill: two marks a glance tells
  /// apart, and neither borrows the other's.
  func testRecipientRingAndViewedFillAreDistinctMarks() throws {
    let slot: CGFloat = 40
    func chip(open: Bool, selected: Bool) throws -> Raster {
      try render(
        OverlayChannelPill(
          form: .chip,
          display: OverlayChannelPill.Display(
            figure: "4", nameBelow: true, slotWidth: slot, microphoneOpen: open,
            selected: selected),
          hint: "", accessibilityText: "", identifier: "mark-probe", palette: .dark,
          textScale: 1, showsHint: false, action: {}),
        size: CGSize(width: slot, height: OverlayChannelPill.mapRowHeight))
    }
    let plain = try chip(open: false, selected: false)
    let recipient = try chip(open: true, selected: false)
    let viewed = try chip(open: false, selected: true)
    let radius = OverlayChannelPill.digitCircle / 2
    let center = CGPoint(x: slot / 2, y: radius)
    // On the ring, clear of the open dot (bottom right) and unread dot (top
    // right); inside the circle, clear of the digit.
    let rim = [
      CGPoint(x: center.x - radius + 0.75, y: center.y),
      CGPoint(x: center.x, y: center.y - radius + 0.75),
    ]
    let inside = [
      CGPoint(x: center.x - radius * 0.66, y: center.y),
      CGPoint(x: center.x + radius * 0.66, y: center.y),
    ]
    for point in rim {
      XCTAssertGreaterThan(
        Raster.distance(plain.at(point.x, point.y), recipient.at(point.x, point.y)), 0.08,
        "\(point): the recipient wears the ring")
    }
    for point in inside {
      XCTAssertGreaterThan(
        Raster.distance(plain.at(point.x, point.y), viewed.at(point.x, point.y)), 0.04,
        "\(point): the viewed conversation wears the fill")
      XCTAssertLessThan(
        Raster.distance(plain.at(point.x, point.y), recipient.at(point.x, point.y)), 0.03,
        "\(point): the recipient's mark is the ring, not a fill")
      XCTAssertGreaterThan(
        Raster.distance(recipient.at(point.x, point.y), viewed.at(point.x, point.y)), 0.04,
        "\(point): recipient and viewed read apart")
    }
  }

  /// Brief §6: overlapping circles never hide an unread dot. The one-row map
  /// draws the dots on a layer above every circle, so a dot reads as much of
  /// itself over a neighbour's circle as it does in the open row.
  func testUnreadDotsStayDrawnOverOverlappingCircles() throws {
    var states: [String: OverlayChannelHudProjection] = [:]
    var channels: [OverlayChannelDelivery] = []
    for (index, name) in ["Miksa", "Nora", "Igor", "Hania", "Kuba"].enumerated() {
      let digit = String(index + 1)
      states[digit] = hud(session: "s\(digit)")
      channels.append(delivery(digit, name))
    }
    func map(unread: Set<String>) -> OverlayChannelMapModel {
      model(channels: channels, hudStates: states, selectable: Set(states.keys), unread: unread)
    }
    let bar = OverlayBarGeometry(
      baseWidth: 0, horizontalPadding: 0,
      nameWidths: map(unread: []).chips.map { OverlayChannelPill.nameWidth($0.name, scale: 1) },
      circle: OverlayChannelPill.digitCircle, inlineGap: OverlayChannelPill.chipGap)
    let open = bar.layout(width: bar.naturalWidth)
    let dense = bar.layout(width: bar.floorWidth)
    XCTAssertEqual(open.arrangement, .oneRow)
    XCTAssertFalse(open.overlaps)
    XCTAssertTrue(dense.overlaps)
    let yellow = OverlayAppearancePalette.dark.processingStatus.nsColor
    func dots(_ layout: OverlayChannelMapLayout, unread: Set<String>) throws -> Int {
      try render(
        OverlayChannelMapView(
          model: map(unread: unread), placement: .strip(layout), modifier: .control,
          palette: .dark, textScale: 1, animates: false, onSelectChannel: { _ in },
          onCounter: {}),
        size: CGSize(width: layout.width + 8, height: OverlayChannelPill.mapRowHeight)
      ).count(near: yellow, within: 0.15)
    }
    XCTAssertEqual(try dots(dense, unread: []), 0, "no unread reply, no yellow")
    let spread = try dots(open, unread: ["2", "3"])
    let stacked = try dots(dense, unread: ["2", "3"])
    XCTAssertGreaterThan(spread, 0)
    XCTAssertGreaterThanOrEqual(
      Double(stacked), Double(spread) * 0.8, "a neighbour's circle hides none of a dot")
    let view = try overlaySource("OverlayChannelMapView.swift")
    XCTAssertTrue(view.contains("drawsUnreadDot: !oneRow"))
  }

  // MARK: - Source shape (§5.5, §13.3, §13.4, invariants)

  func testPillWriteKeepsTheSelectedOwnerAndDoesNotFocusItsTerminalOrRecord() throws {
    let (state, _, _, astra) = try armedState()
    state.chooseConversation(astra.id)
    let generation = state.captureGeneration
    var selections: [String?] = []
    var focused: [OverlayConversationOwner] = []
    var toggles: [UInt8] = []
    var roster = OverlayChannelStatusView(
      channels: state.visibleChannelRows.filter { $0.channel != "0" }, unavailable: false,
      palette: .dark, animates: false, hudStates: state.channelHudStates,
      onToggleChannel: { toggles.append($0) }, conversations: state.conversations,
      selectedConversationID: astra.id,
      onSelectConversation: { selections.append($0) })
    roster.onFocusAgentPanel = { focused.append($0) }
    let actions = try XCTUnwrap(roster.agentPillMenu(for: "3").agentActions)
    XCTAssertTrue(actions.write.enabled)
    actions.write.perform()
    XCTAssertEqual(selections, [astra.id])
    XCTAssertTrue(focused.isEmpty, "writing to an already selected agent is still composing")
    XCTAssertTrue(toggles.isEmpty)
    XCTAssertEqual(state.captureGeneration, generation)
    XCTAssertTrue(roster.agentPillMenu(for: "0").members.isEmpty)
    XCTAssertNil(roster.agentPillMenu(for: "7").agentActions)
  }

  func testPillShowInAgentsDuringCaptureCannotBecomeTheHeaderStopAction() throws {
    let (state, _, _, astra) = try armedState()
    state.applyChannelRoster([
      CsChannelRosterState(
        channel: "3", audience: "astra", provider: "codex",
        providerSessionId: "astra-session", open: true, loud: false,
        autosealDeadlineUnixMs: nil, followerAlive: true, unreadSinceUnixMs: nil)
    ])
    state.chooseConversation(astra.id)
    state.setPresentationMode(.midi)
    let generation = state.captureGeneration
    let roster = DictationOverlayView(state: state).channelStatusView
    let show = try XCTUnwrap(roster.agentPillMenu(for: "3").agentActions?.showInAgents)
    XCTAssertTrue(show.enabled)
    show.perform()
    XCTAssertEqual(state.selectedConversationID, astra.id)
    XCTAssertEqual(state.presentationMode, .expanded)
    XCTAssertEqual(state.windowContentSurface, .agents)
    XCTAssertTrue(state.showsAgentMonitor)
    XCTAssertTrue(state.channelHudStates["3"]?.open == true)
    XCTAssertTrue(state.pendingChannelToggles.isEmpty)
    XCTAssertEqual(state.captureGeneration, generation)
  }

  func testPillAudioCommandsRefusePendingAndUnknownPlaybackWithoutOptimisticState() throws {
    let (state, _, _, _) = try armedState()
    var toggles: [UInt8] = []
    var playback: [String] = []
    var roster = OverlayChannelStatusView(
      channels: state.visibleChannelRows.filter { $0.channel != "0" }, unavailable: false,
      palette: .dark, animates: false, hudStates: state.channelHudStates,
      onToggleChannel: { toggles.append($0) }, conversations: state.conversations)
    roster.onTogglePlayback = { playback.append($0) }
    roster.pendingToggleChannels = ["3"]
    roster.pendingMuteChannels = ["3"]
    roster.mutedChannels = ["3": false]
    let pending = try XCTUnwrap(roster.agentPillMenu(for: "3").agentActions)
    XCTAssertFalse(try XCTUnwrap(pending.microphone).enabled)
    XCTAssertFalse(try XCTUnwrap(pending.playback).enabled)
    pending.microphone?.perform()
    pending.playback?.perform()
    XCTAssertTrue(toggles.isEmpty)
    XCTAssertTrue(playback.isEmpty)
    roster.pendingToggleChannels = []
    roster.pendingMuteChannels = []
    roster.mutedChannels = [:]
    let unknown = try XCTUnwrap(roster.agentPillMenu(for: "3").agentActions?.playback)
    XCTAssertFalse(unknown.enabled)
    unknown.perform()
    XCTAssertTrue(playback.isEmpty, "missing receipt is not permission to toggle")
    roster.mutedChannels = ["3": true]
    let ready = try XCTUnwrap(roster.agentPillMenu(for: "3").agentActions)
    XCTAssertTrue(try XCTUnwrap(ready.microphone).enabled)
    XCTAssertTrue(try XCTUnwrap(ready.playback).enabled)
    ready.microphone?.perform()
    ready.playback?.perform()
    XCTAssertEqual(toggles, [3])
    XCTAssertEqual(playback, ["3"])
    XCTAssertFalse(roster.hudStates["3"]?.open == true)
  }

  func testPillArchiveUsesExactOwnerAndRefusesPendingOrOpenCapture() throws {
    let (state, _, _, astra) = try armedState()
    let owner = try XCTUnwrap(astra.owner)
    var archived: [OverlayConversationOwner] = []
    func roster(open: Bool = false, pending: Bool = false) -> OverlayChannelStatusView {
      var view = OverlayChannelStatusView(
        channels: state.visibleChannelRows.filter { $0.channel != "0" }, unavailable: false,
        palette: .dark, animates: false,
        hudStates: ["3": hud(open: open, session: "astra-session")],
        conversations: state.conversations)
      view.archiveCandidates = ["3": owner]
      view.onArchiveAgent = { archived.append($0) }
      view.pendingArchives = pending ? [owner] : []
      return view
    }
    XCTAssertNil(
      roster().agentPillMenu(for: "3").agentActions?.focus,
      "unknown terminal must not inherit a navigation action")
    for view in [roster(pending: true), roster(open: true)] {
      let action = try XCTUnwrap(view.agentPillMenu(for: "3").agentActions?.archive)
      XCTAssertFalse(action.enabled)
      action.perform()
    }
    XCTAssertTrue(archived.isEmpty)
    let ready = try XCTUnwrap(roster().agentPillMenu(for: "3").agentActions?.archive)
    XCTAssertTrue(ready.enabled)
    ready.perform()
    XCTAssertEqual(archived, [owner])
  }

  func testMapChipsAndCountersShareOnePillView() throws {
    let mapView = try overlaySource("OverlayChannelMapView.swift")
    XCTAssertTrue(mapView.contains("form: .chip"))
    XCTAssertTrue(mapView.contains("form: .counter"))
    let mapOnly = try section(
      of: mapView, from: "struct OverlayChannelMapView: View",
      to: "struct OverlayAgentsHoverList: View")
    XCTAssertFalse(mapOnly.contains("Text(verbatim:"), "the map does not draw a digit itself")
    let status = try overlaySource("OverlayChannelStatusView.swift")
    XCTAssertTrue(
      status.contains("OverlayChannelPill(\n      form: .counter"),
      "mini's recipient digits are the same pill")
    let pill = try overlaySource("OverlayChannelPill.swift")
    XCTAssertEqual(
      pill.components(separatedBy: "Text(verbatim: text)").count - 1, 1,
      "one drawing of the digit")
    // Spec §14: the open channel's digit carries one violet dot, drawn once,
    // and no symbol; the microphone and megaphone signs live on the Agents
    // button only.
    XCTAssertEqual(
      pill.components(separatedBy: "Circle()\n      .fill(palette.agentAccent.color)").count - 1, 1,
      "one drawing of the open-channel dot")
    let pillView = try section(
      of: pill, from: "struct OverlayChannelPill: View", to: "// MARK: Header metrics")
    // Symbols replace a figure: the stacked counter and the numberless human
    // mailbox. Neither adds a glyph to a numbered agent's digit.
    XCTAssertEqual(
      pillView.components(separatedBy: "Image(systemName:").count - 1, 2,
      "only the stacked counter and numberless mailbox carry symbols")
    XCTAssertTrue(pillView.contains("Image(systemName: Self.stackedCardsSymbol)"))
    XCTAssertFalse(pillView.contains("mic.fill"), "the microphone glyph left the digit")
    XCTAssertFalse(pillView.contains("megaphone"), "the megaphone stays on the Agents button")
    XCTAssertTrue(
      pillView.contains("if display.animates && !reduceMotion"),
      "the dot pulses only while the pill animates and Reduce Motion is off")
    for forbidden in [
      "OverlayState", "onToggleChannel", "toggleAgentChannel", "OverlayConversation",
      "onSelectConversation",
    ] {
      XCTAssertFalse(pill.contains(forbidden), "the pill takes display values only: \(forbidden)")
      XCTAssertFalse(
        mapView.contains(forbidden), "the map routes through the roster: \(forbidden)")
    }
    // The badge replaced the route text, and no popover came with it.
    for source in [pill, mapView] {
      XCTAssertFalse(source.contains("\\u{2192}"), "no MIC → arrow")
      XCTAssertFalse(source.contains("MIC"), "no MIC text")
      XCTAssertFalse(source.contains(".popover("), "the map popover is deferred")
      XCTAssertFalse(source.contains("NSPopover"), "the map popover is deferred")
    }
    let label = try overlaySource("OverlayAgentRosterLabel.swift")
    XCTAssertFalse(label.contains("OverlayState"))
  }

  /// A chip selects its conversation and the vertical counter brings the
  /// Intercom group forward; neither opens a microphone or retargets a
  /// capture. The agent's own actions stay in the chip's secondary-click menu.
  func testMapSelectionNeverOpensAMicrophone() throws {
    let overlay = try overlaySource("DictationOverlayView.swift")
    let strip = try section(
      of: overlay, from: "private func channelStrip(", to: "private func refreshingChordModifier")
    let counter = try section(
      of: overlay, from: "var channelCounter: OverlayChannelMapView?",
      to: "private func channelStrip(")
    for source in [strip, counter] {
      XCTAssertTrue(source.contains("onSelectChannel: { roster.selectChannelConversation($0) }"))
      XCTAssertTrue(source.contains("onCounter: { state.activateChannelMapCounter() }"))
      XCTAssertFalse(source.contains("toggle"), "selection never touches capture")
      XCTAssertFalse(source.contains("showAgentMonitor"), "the W1 seam is gone")
    }
    XCTAssertTrue(
      strip.contains("agentPillMenu: { roster.agentPillMenu(for: $0, showsInlineError: false) }"),
      "the agent's actions are one secondary click away")
    let state = try overlaySource("OverlayState.swift")
    let activate = try section(
      of: state, from: "func activateChannelMapCounter()", to: "func refreshAgentChannelModifier()")
    XCTAssertTrue(activate.contains("onRevealIntercom?()"))
    XCTAssertFalse(activate.contains("toggle"))
    XCTAssertFalse(activate.contains("setPresentationMode"), "the counter changes no form")
    // The intent rail stays out of the bar.
    let header = try section(
      of: overlay, from: "private func justifiedHeader(density: OverlayHeaderDensity)",
      to: "private var transcriptionPreviewButton")
    XCTAssertFalse(header.contains("OverlayIntentRail("))
  }
}
