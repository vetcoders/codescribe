import AppKit
import ApplicationServices
import Foundation
import SwiftUI
import XCTest

@testable import Codescribe

final class OverlayConversationAcceptanceTests: XCTestCase {
  @MainActor
  func testWholeReplyBubbleKeepsPlaybackCopyAndBottomInsetCompact() throws {
    for state in ["spoken", "playing", "waiting", "stopped"] {
      for width in [CGFloat(262), 459] {
        for scale in [CGFloat(1), 1.6] {
          var bus = OverlayChannelDelivery.Bus()
          let replyID = String(repeating: "c", count: 24)
          var row = reply(replyID, delivery: "footer-measurement")
          row["text"] = "A retained reply with enough words to wrap onto another line."
          bus.consume(row)
          bus.consume(
            playback(replyID, ticket: "bubble-footer", state: state, time: "2026-10-05T04:00:01Z"))
          let conversation = try lenaConversation(bus)
          let message = try XCTUnwrap(conversation.messages.first { $0.kind == .reply })
          let view = OverlayConversationView(
            conversation: conversation, palette: .dark, topInset: 0, bottomInset: 0,
            pendingControls: [], controlErrors: [:], onControl: { _, _ in },
            draft: .constant(""), sending: false, sendError: nil, onSend: {})
          let user = OverlayConversationMessage(
            id: "user:footer", kind: .user, text: message.text, order: message.order,
            emittedAt: message.emittedAt, owner: nil, recipients: [], deliveryID: nil,
            replyTo: nil, unsolicited: false, playback: nil, busPath: "")
          let host = NSHostingView(
            rootView: view.messageBubble(message)
              .environment(\.csTextScale, scale).frame(width: width))
          let userHost = NSHostingView(
            rootView: view.messageBubble(user)
              .environment(\.csTextScale, scale).frame(width: width))
          let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: width, height: 200),
            styleMask: .borderless, backing: .buffered, defer: false)
          window.isReleasedWhenClosed = false
          window.contentView = host
          window.orderFrontRegardless()
          let userWindow = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: width, height: 200),
            styleMask: .borderless, backing: .buffered, defer: false)
          userWindow.isReleasedWhenClosed = false
          userWindow.contentView = userHost
          userWindow.orderFrontRegardless()
          defer {
            window.close()
            userWindow.close()
          }
          host.layoutSubtreeIfNeeded()
          userHost.layoutSubtreeIfNeeded()
          window.setContentSize(host.fittingSize)
          userWindow.setContentSize(userHost.fittingSize)
          RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
          host.layoutSubtreeIfNeeded()
          userHost.layoutSubtreeIfNeeded()
          let context = "state=\(state) width=\(width) scale=\(scale)"
          XCTAssertGreaterThan(userHost.fittingSize.height, 0, context)
          XCTAssertEqual(
            host.fittingSize.height, userHost.fittingSize.height, accuracy: 1,
            "Playback must not add height to an otherwise identical user bubble: \(context)")
        }
      }
    }
  }

  @MainActor
  func testReplyPlaybackAndCopyShareOneCompactFooter() throws {
    for state in ["spoken", "playing", "waiting", "stopped"] {
      for width in [CGFloat(262), 459] {
        for scale in [CGFloat(1), 1.6] {
          var bus = OverlayChannelDelivery.Bus()
          let replyID = String(repeating: "c", count: 24)
          bus.consume(reply(replyID))
          bus.consume(
            playback(replyID, ticket: "footer", state: state, time: "2026-10-05T04:00:01Z"))
          let conversation = try lenaConversation(bus)
          let message = try XCTUnwrap(conversation.messages.first { $0.kind == .reply })
          let view = OverlayConversationView(
            conversation: conversation, palette: .dark, topInset: 50, bottomInset: 20,
            pendingControls: [], controlErrors: [:], onControl: { _, _ in },
            draft: .constant(""), sending: false, sendError: nil, onSend: {})
          let host = NSHostingView(
            rootView: view.messageFooter(message)
              .environment(\.csTextScale, scale).frame(width: width).background(Color.black))
          let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: width, height: 120),
            styleMask: .borderless, backing: .buffered, defer: false)
          window.isReleasedWhenClosed = false
          window.contentView = host
          window.orderFrontRegardless()
          defer { window.close() }
          host.layoutSubtreeIfNeeded()
          RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
          host.layoutSubtreeIfNeeded()
          XCTAssertGreaterThan(host.fittingSize.height, 0)
          XCTAssertLessThanOrEqual(
            host.fittingSize.height, max(24, 16 * scale) + 2,
            "Playback and copy need one compact row: state=\(state) width=\(width) scale=\(scale)")
          let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
          host.cacheDisplay(in: host.bounds, to: bitmap)
          let pixelsPerPoint = CGFloat(bitmap.pixelsWide) / host.bounds.width
          var runs: [ClosedRange<Int>] = []
          var start: Int?
          for x in 0..<bitmap.pixelsWide {
            let ink = (0..<bitmap.pixelsHigh).contains { y in
              guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else {
                return false
              }
              return color.alphaComponent > 0.8
                && max(color.redComponent, color.greenComponent, color.blueComponent) > 0.15
            }
            if ink, start == nil { start = x }
            if !ink, let beginning = start {
              runs.append(beginning...(x - 1))
              start = nil
            }
          }
          XCTAssertGreaterThanOrEqual(runs.count, 2, "playback glyph and status must render")
          if runs.count >= 2 {
            let gap = CGFloat(runs[1].lowerBound - runs[0].upperBound - 1) / pixelsPerPoint
            XCTAssertGreaterThanOrEqual(gap, 2 * scale)
            XCTAssertLessThanOrEqual(
              gap, 6 * scale, "status stays within one character of the glyph")
          }
        }
      }
    }
  }

  @MainActor
  func testActivityCueTracksReceivingUserTurnAcrossRepliesAndTerminal() throws {
    var bus = OverlayChannelDelivery.Bus()
    let firstSession = "agent-channel-2-active-cue-a"
    let secondSession = "agent-channel-2-active-cue-b"
    func view(presented: Bool = true) throws -> OverlayConversationView {
      OverlayConversationView(
        conversation: try lenaConversation(bus), palette: .dark, topInset: 50, bottomInset: 20,
        pendingControls: [], controlErrors: [:], onControl: { _, _ in }, isPresented: presented,
        draft: .constant(""), sending: false, sendError: nil, onSend: {})
    }
    bus.consume(occurrence(0, revision: 1, text: "Pierwsza fraza", captureSession: firstSession))
    let user = try XCTUnwrap(lenaConversation(bus).messages.first { $0.kind == .user })
    XCTAssertEqual(try view().activeCaptureMessageID, user.id)
    bus.consume(reply(String(repeating: "d", count: 24)))
    XCTAssertEqual(
      try view().activeCaptureMessageID, user.id, "A reply does not own capture activity")
    XCTAssertNil(try view(presented: false).activeCaptureMessageID)
    endCapture(&bus, session: firstSession)
    XCTAssertNil(try view().activeCaptureMessageID, "Stop removes the activity cue")
    bus.consume(occurrence(0, revision: 2, text: "Późna poprawka", captureSession: firstSession))
    XCTAssertNil(try view().activeCaptureMessageID, "A late text revision cannot reopen capture")
    bus.consume(occurrence(3200, revision: 1, text: "Nowa fraza", captureSession: secondSession))
    let next = try XCTUnwrap(lenaConversation(bus).messages.last { $0.kind == .user })
    XCTAssertNotEqual(next.id, user.id)
    XCTAssertEqual(try view().activeCaptureMessageID, next.id)
    XCTAssertEqual(next.text, "Nowa fraza", "The decorative cue never changes transcript bytes")
  }

  @MainActor
  func testLongConversationResizePreservesLiveEdgeHistoryAndComposer() throws {
    var bus = OverlayChannelDelivery.Bus()
    for index in 0..<30 {
      var row = reply(String(format: "%024x", index + 1))
      row["text"] = String(repeating: "Retained **conversation** paragraph.\n\n", count: 8)
      bus.consume(row)
    }
    let conversation = try lenaConversation(bus)
    let host = NSHostingView(
      rootView: OverlayConversationView(
        conversation: conversation, palette: .light, topInset: 50, bottomInset: 20,
        pendingControls: [], controlErrors: [:], onControl: { _, _ in },
        draft: .constant("Retained draft"), sending: false, sendError: nil, onSend: {}))
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: 532, height: 360)
    let window = NSWindow(
      contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    window.orderFrontRegardless()
    defer { window.close() }
    func settle() {
      host.layoutSubtreeIfNeeded()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
      host.layoutSubtreeIfNeeded()
    }
    func scrollViews(_ root: NSView) -> [NSScrollView] {
      (root as? NSScrollView).map { [$0] } ?? root.subviews.flatMap(scrollViews)
    }
    func editor(_ root: NSView) -> NSTextView? {
      if let text = root as? NSTextView, text.isEditable { return text }
      return root.subviews.lazy.compactMap { editor($0) }.first
    }
    settle()
    let scroll = try XCTUnwrap(scrollViews(host).first { $0.bounds.height > 150 })
    let document = try XCTUnwrap(scroll.documentView)
    let originalEditor = try XCTUnwrap(editor(host))
    originalEditor.setSelectedRange(NSRange(location: 2, length: 5))
    for followsLatest in [true, false] {
      if !followsLatest {
        NotificationCenter.default.post(
          name: NSScrollView.willStartLiveScrollNotification, object: scroll)
        scroll.contentView.scroll(to: .zero)
        scroll.reflectScrolledClipView(scroll.contentView)
        NotificationCenter.default.post(
          name: NSScrollView.didLiveScrollNotification, object: scroll)
        NotificationCenter.default.post(
          name: NSScrollView.didEndLiveScrollNotification, object: scroll)
        settle()
      }
      for width in [CGFloat(360), 680, 420, 600, 360, 532] {
        window.setContentSize(NSSize(width: width, height: 360))
        settle()
        if followsLatest {
          XCTAssertGreaterThan(scroll.documentVisibleRect.maxY, document.bounds.height - 50)
        } else {
          XCTAssertLessThan(scroll.documentVisibleRect.maxY, document.bounds.height - 100)
        }
        XCTAssertTrue(try XCTUnwrap(editor(host)) === originalEditor)
        XCTAssertEqual(originalEditor.string, "Retained draft")
        XCTAssertEqual(originalEditor.selectedRange(), NSRange(location: 2, length: 5))
        XCTAssertLessThanOrEqual(document.bounds.width, scroll.bounds.width + 1)
      }
    }
  }

  @MainActor
  func testConversationNamePillExposesChannelAtNormalAndLargeTextSizes() throws {
    func elements(_ object: Any) -> [any NSAccessibilityProtocol] {
      guard let element = object as? any NSAccessibilityProtocol else { return [] }
      let native = (object as? NSView)?.subviews ?? []
      return [element] + ((element.accessibilityChildren() ?? []) + native).flatMap(elements)
    }
    for channel in ["0", "2", "7"] {
      let conversationOwner = OverlayConversationOwner(row: owner(leaseA, channel: channel))
      let conversation = OverlayConversation(
        id: "pill-\(channel)", channel: channel, name: "Lena",
        owner: channel == "0" ? nil : conversationOwner, messages: [])
      let expected = channel == "0" ? String(localized: "0 · All") : "\(channel) · Lena"
      for scale in [CGFloat(1), CGFloat(1.6)] {
        let view = OverlayConversationView(
          conversation: conversation, palette: .dark, topInset: 50, bottomInset: 20,
          pendingControls: [], controlErrors: [:], onControl: { _, _ in },
          draft: .constant(""), sending: false, sendError: nil, onSend: {})
        let host = NSHostingView(rootView: view.environment(\.csTextScale, scale))
        host.sizingOptions = []
        host.frame = NSRect(x: 0, y: 0, width: 360, height: 400)
        let window = NSWindow(
          contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        defer { window.close() }
        window.orderFrontRegardless()
        host.layoutSubtreeIfNeeded()
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
        host.layoutSubtreeIfNeeded()
        let pill = try XCTUnwrap(
          elements(host).first { $0.accessibilityIdentifier() == "overlay-conversation-name" })
        let text = [pill.accessibilityLabel(), pill.accessibilityValue() as? String]
          .compactMap { $0 }.joined(separator: " ")
        XCTAssertTrue(text.contains(expected), "channel=\(channel) scale=\(scale): \(text)")
        XCTAssertGreaterThan(pill.accessibilityFrame().width, 0)
        XCTAssertLessThanOrEqual(pill.accessibilityFrame().width, host.bounds.width)
      }
    }
  }

  @MainActor
  func testSelectableMessageTextOpensOverlayKeyboardGate() throws {
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 400, height: 300),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    defer { panel.close() }
    let host = NSHostingView(rootView: MarkdownText(raw: "Wiadomość agenta"))
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: 400, height: 300)
    panel.contentView = host
    panel.orderFrontRegardless()
    host.layoutSubtreeIfNeeded()
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
    func selectableText(in view: NSView) -> NSView? {
      if view.acceptsFirstResponder && view.responds(to: #selector(NSText.copy(_:))) {
        return view
      }
      return view.subviews.lazy.compactMap { selectableText(in: $0) }.first
    }
    let field = try XCTUnwrap(selectableText(in: host))
    let point = field.convert(NSPoint(x: field.bounds.midX, y: field.bounds.midY), to: nil)
    XCTAssertFalse(panel.canBecomeKey)
    let event = try XCTUnwrap(
      NSEvent.mouseEvent(
        with: .leftMouseDown, location: point, modifierFlags: [],
        timestamp: 0, windowNumber: panel.windowNumber, context: nil,
        eventNumber: 0, clickCount: 1, pressure: 1))
    let mouseUp = try XCTUnwrap(
      NSEvent.mouseEvent(
        with: .leftMouseUp, location: point, modifierFlags: [],
        timestamp: 0.01, windowNumber: panel.windowNumber, context: nil,
        eventNumber: 1, clickCount: 1, pressure: 0))
    NSApp.postEvent(mouseUp, atStart: true)
    panel.sendEvent(event)
    XCTAssertTrue(panel.allowsKeyForTranscript)
    panel.releaseKeyAfterTranscript()
    XCTAssertFalse(panel.canBecomeKey)
  }

  @MainActor
  func testCommandCCopiesThroughFocusedNativeResponder() throws {
    class CopyTarget: NSTextView {
      var copiedText: String?
      override func copy(_ sender: Any?) {
        copiedText = (string as NSString).substring(with: selectedRange())
      }
    }
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 0, y: 0, width: 400, height: 300),
      styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    defer { panel.close() }
    let text = CopyTarget(frame: panel.contentLayoutRect)
    text.string = "alpha beta gamma"
    text.isEditable = false
    text.isSelectable = true
    panel.contentView = text
    panel.orderFrontRegardless()
    panel.takeKeyForTranscript()
    XCTAssertTrue(panel.makeFirstResponder(text))
    text.setSelectedRange(NSRange(location: 6, length: 4))
    let key = try XCTUnwrap(
      NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: .command, timestamp: 0,
        windowNumber: panel.windowNumber, context: nil, characters: "c",
        charactersIgnoringModifiers: "c", isARepeat: false, keyCode: 8))
    XCTAssertTrue(panel.performKeyEquivalent(with: key))
    XCTAssertEqual(text.copiedText, "beta")
  }

  @MainActor
  func testMessageCopyPreservesFullRawMarkdown() {
    let pasteboard = NSPasteboard.withUniqueName()
    defer { pasteboard.releaseGlobally() }
    pasteboard.setString("poprzednia zawartość", forType: .string)
    let raw = "Pierwszy akapit.\n\n```swift\nlet wynik = \"całość\"\n```\n\nOstatni akapit."
    chatCopy(raw, to: pasteboard)
    XCTAssertEqual(pasteboard.string(forType: .string), raw)
  }

  @MainActor
  func testChannelZeroHasEditableComposerAndReturnSendsDraft() throws {
    var draft = ""
    var sent: [String] = []
    let conversation = OverlayConversation(
      id: "0", channel: "0", name: "All", owner: nil, messages: [])
    let view = OverlayConversationView(
      conversation: conversation, palette: .dark, topInset: 50, bottomInset: 20,
      pendingControls: [], controlErrors: [:], onControl: { _, _ in },
      draft: Binding(get: { draft }, set: { draft = $0 }), sending: false,
      sendError: nil, onSend: { sent.append(draft) })
    let host = NSHostingView(rootView: view)
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: 480, height: 500)
    let window = NSWindow(
      contentRect: host.frame, styleMask: [.titled], backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    defer { window.close() }
    window.orderFrontRegardless()
    func editor(in view: NSView) -> NSTextView? {
      if let text = view as? NSTextView, text.isEditable { return text }
      return view.subviews.lazy.compactMap { editor(in: $0) }.first
    }
    let deadline = Date(timeIntervalSinceNow: 1)
    while editor(in: host) == nil, Date() < deadline {
      host.layoutSubtreeIfNeeded()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.02))
    }
    let text = try XCTUnwrap(editor(in: host))
    window.makeFirstResponder(text)
    text.insertText("Do wszystkich", replacementRange: text.selectedRange())
    text.keyDown(with: try returnEvent())
    XCTAssertEqual(sent, ["Do wszystkich"])
  }

  @MainActor
  func testBusMarkdownUsesChatCodeWellAndKeepsResizeMarginAcrossZoom() throws {
    let raw = """
      ## Wynik pracy

      **Gotowe** i [źródło](https://example.test/receipt).

      - pierwszy krok
      - drugi krok

      ```swift
      let result = "przeczytano"
      print(result)
      ```
      """
    var bus = OverlayChannelDelivery.Bus()
    var row = reply(String(repeating: "9", count: 24))
    row["text"] = raw
    row.removeValue(forKey: "tts_vendor")
    bus.consume(row)
    let restored = try JSONDecoder().decode(
      OverlayChannelDelivery.Bus.self, from: JSONEncoder().encode(bus))
    let conversation = try lenaConversation(restored)
    XCTAssertEqual(
      conversation.messages.first?.text, raw, "rendering must preserve the exact receipt")
    let key = "OverlayMarkdownZoom.\(UUID().uuidString)"
    defer { UserDefaults.standard.removeObject(forKey: key) }
    let scale = TextScaleController(key: key)
    let view = OverlayConversationView(
      conversation: conversation, palette: .light, topInset: 50, bottomInset: 20,
      pendingControls: [], controlErrors: [:], onControl: { _, _ in },
      draft: .constant("Zachowaj szkic"), sending: false, sendError: nil, onSend: {})
    let host = NSHostingView(
      rootView: TextScaleRoot(controller: scale) { view }
        .background(OverlayAppearancePalette.light.desktopBackground.color)
        .preferredColorScheme(.light))
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: 532, height: 500)
    let window = NSWindow(
      contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    defer { window.close() }
    func settle() {
      host.layoutSubtreeIfNeeded()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
      host.layoutSubtreeIfNeeded()
    }
    func scrollViews(_ root: NSView) -> [NSScrollView] {
      (root as? NSScrollView).map { [$0] } ?? root.subviews.flatMap(scrollViews)
    }
    func editor(_ root: NSView) -> NSTextView? {
      if let text = root as? NSTextView, text.isEditable { return text }
      return root.subviews.lazy.compactMap { editor($0) }.first
    }
    settle()
    let scroll = try XCTUnwrap(scrollViews(host).first { $0.bounds.height > 150 })
    let document = try XCTUnwrap(scroll.documentView)
    let originalEditor = try XCTUnwrap(editor(host))
    originalEditor.setSelectedRange(NSRange(location: 2, length: 5))
    let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
    host.cacheDisplay(in: host.bounds, to: bitmap)
    var codeWellPixels = 0
    for y in stride(from: 100, to: bitmap.pixelsHigh - 100, by: 4) {
      for x in stride(from: 30, to: bitmap.pixelsWide / 2, by: 4) {
        guard let color = bitmap.colorAt(x: x, y: y) else { continue }
        if min(color.redComponent, color.greenComponent, color.blueComponent) > 0.97 {
          codeWellPixels += 1
        }
      }
    }
    XCTAssertGreaterThan(
      codeWellPixels, 100, "a fenced block must render the shared native code well")
    for _ in 0..<6 { scale.increase() }
    settle()
    let viewport = scroll.convert(scroll.bounds, to: host)
    XCTAssertEqual(host.bounds.maxX - viewport.maxX, 15, accuracy: 1)
    XCTAssertEqual(viewport.height, host.bounds.height, accuracy: 1)
    XCTAssertLessThanOrEqual(document.bounds.width, viewport.width + 1)
    XCTAssertTrue(try XCTUnwrap(editor(host)) === originalEditor)
    XCTAssertEqual(originalEditor.string, "Zachowaj szkic")
    XCTAssertEqual(originalEditor.selectedRange(), NSRange(location: 2, length: 5))
    XCTAssertEqual(try XCTUnwrap(originalEditor.font).pointSize, 22.4, accuracy: 0.05)
  }

  @MainActor
  func testConversationZoomRetainsEditorDraftSelectionAndTypingFont() throws {
    let key = "OverlayConversationZoom.\(UUID().uuidString)"
    defer { UserDefaults.standard.removeObject(forKey: key) }
    let scale = TextScaleController(key: key)
    var draft = "Pierwsza linia i druga wypowiedź"
    let identity = try XCTUnwrap(OverlayConversationOwner(row: owner(leaseA)))
    let view = OverlayConversationView(
      conversation: .init(
        id: identity.id, channel: "2", name: "Lena", owner: identity, messages: []),
      palette: .light, topInset: 50, bottomInset: 20, pendingControls: [], controlErrors: [:],
      onControl: { _, _ in }, draft: Binding(get: { draft }, set: { draft = $0 }),
      sending: false, sendError: nil, onSend: {})
    let host = NSHostingView(rootView: TextScaleRoot(controller: scale) { view })
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: 470, height: 280)
    let window = NSWindow(
      contentRect: host.frame, styleMask: .borderless,
      backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    defer { window.close() }
    func settle() {
      host.layoutSubtreeIfNeeded()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.04))
      host.layoutSubtreeIfNeeded()
    }
    func editor(in root: NSView) -> NSTextView? {
      if let text = root as? NSTextView, text.isEditable { return text }
      return root.subviews.lazy.compactMap { editor(in: $0) }.first
    }
    settle()
    let original = try XCTUnwrap(editor(in: host))
    original.setSelectedRange(NSRange(location: 5, length: 7))
    for _ in 0..<6 { scale.increase() }
    settle()
    let enlarged = try XCTUnwrap(editor(in: host))
    XCTAssertTrue(enlarged === original)
    XCTAssertEqual(enlarged.string, draft)
    XCTAssertEqual(enlarged.selectedRange(), NSRange(location: 5, length: 7))
    XCTAssertEqual(try XCTUnwrap(enlarged.font).pointSize, 22.4, accuracy: 0.05)
    XCTAssertEqual(
      (enlarged.typingAttributes[.font] as? NSFont)?.pointSize, enlarged.font?.pointSize)
    XCTAssertLessThanOrEqual(try XCTUnwrap(enlarged.enclosingScrollView).frame.height, 112)
    scale.reset()
    settle()
    XCTAssertTrue(editor(in: host) === original)
    XCTAssertEqual(try XCTUnwrap(original.font).pointSize, 14, accuracy: 0.05)
    XCTAssertEqual(original.selectedRange(), NSRange(location: 5, length: 7))
    XCTAssertEqual(draft, "Pierwsza linia i druga wypowiedź")
  }

  func testTextReplyDoesNotAcquireSpeechFromItsTextOrPlaybackFailure() throws {
    var bus = OverlayChannelDelivery.Bus()
    let id = String(repeating: "d", count: 24)
    var text = reply(id)
    text.removeValue(forKey: "tts_vendor")
    text.removeValue(forKey: "voice")
    text.removeValue(forKey: "speed")
    bus.consume(text)
    bus.consume(
      playback(
        id, ticket: String(repeating: "b", count: 24), state: "failed",
        time: "2026-10-05T04:00:01Z"))
    let message = try XCTUnwrap(lenaConversation(bus).messages.first)
    XCTAssertEqual(message.text, "Odpowiedź")
    XCTAssertEqual(message.playback?.state, "failed")
    XCTAssertFalse(message.supportsSpeechPlayback)
  }

  func testSpeechReplyKeepsPlaybackCapabilityBeforeSynthesisAndAcrossMirrors() throws {
    var bus = OverlayChannelDelivery.Bus()
    let id = String(repeating: "d", count: 24)
    let speech = reply(id)
    bus.consume(speech)
    XCTAssertTrue(try XCTUnwrap(lenaConversation(bus).messages.first).supportsSpeechPlayback)
    var mirror = speech
    mirror.removeValue(forKey: "tts_vendor")
    mirror.removeValue(forKey: "voice")
    bus.consume(mirror)
    let restored = try JSONDecoder().decode(
      OverlayChannelDelivery.Bus.self,
      from: JSONEncoder().encode(bus))
    XCTAssertTrue(try XCTUnwrap(lenaConversation(restored).messages.first).supportsSpeechPlayback)
  }

  @MainActor
  func testTextReplyCannotInvokePlaybackEvenThroughAStaleControl() async throws {
    var bus = OverlayChannelDelivery.Bus()
    let id = String(repeating: "d", count: 24)
    var text = reply(id)
    text.removeValue(forKey: "tts_vendor")
    text.removeValue(forKey: "voice")
    bus.consume(text)
    let message = try XCTUnwrap(lenaConversation(bus).messages.first)
    let state = OverlayState()
    await state.controlReply(message, stop: false)
    await state.controlReply(message, stop: true)
    XCTAssertTrue(state.pendingReplyControls.isEmpty)
    XCTAssertTrue(state.replyControlErrors.isEmpty)
  }

  @MainActor
  func testReadLabelRequiresCanonicalAcknowledgmentRatherThanQueueAcceptance() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(occurrence(0, revision: 1))
    var recipient = try XCTUnwrap(lenaConversation(bus).messages.first?.recipients.first)
    let delivery = try XCTUnwrap(recipient.deliveryID)
    XCTAssertEqual(OverlayConversationView.receiptStatusText(for: recipient), "Addressed")
    recipient.queued = true
    XCTAssertEqual(OverlayConversationView.receiptStatusText(for: recipient), "Queued")
    var accepted = owner(leaseA)
    accepted.merge([
      "schema": "codescribe.native-queue.receipt.v1", "disposition": "provider_accepted",
      "delivery_id": delivery,
    ]) { _, new in new }
    bus.observeAcceptance(accepted)
    recipient = try XCTUnwrap(lenaConversation(bus).messages.first?.recipients.first)
    XCTAssertEqual(OverlayConversationView.receiptStatusText(for: recipient), "Queue accepted")
    var foreign = owner(leaseB, session: "foreign")
    foreign.merge(["schema": "codescribe.agent-ack.v1", "delivery_id": delivery]) { _, new in new }
    bus.consume(foreign)
    recipient = try XCTUnwrap(lenaConversation(bus).messages.first?.recipients.first)
    XCTAssertEqual(OverlayConversationView.receiptStatusText(for: recipient), "Queue accepted")
    var ack = owner(leaseA)
    ack.merge(["schema": "codescribe.agent-ack.v1", "delivery_id": delivery]) { _, new in new }
    bus.consume(ack)
    recipient = try XCTUnwrap(lenaConversation(bus).messages.first?.recipients.first)
    XCTAssertEqual(OverlayConversationView.receiptStatusText(for: recipient), "Read")
  }

  @MainActor
  func testBroadcastReceiptMenuStaysCompactInAllAndAgentViewsAcrossAcknowledgments() throws {
    let recipients = [
      owner(leaseA), owner(leaseB, session: "agent-b", name: "Adam", channel: "4"),
      owner(String(repeating: "c", count: 32), session: "agent-c", name: "Astra", channel: "3"),
      owner(String(repeating: "d", count: 32), session: "agent-d", name: "Bruno", channel: "1"),
    ]
    var bus = OverlayChannelDelivery.Bus()
    var question = occurrence(0, revision: 1, text: "Krótka wiadomość", recipients: recipients)
    question["audience"] = "*"
    question["session_id"] = "agent-channel-0-test"
    question["occurrence_session_id"] = "agent-channel-0-test"
    bus.consume(question)
    bus.consume([
      "schema": "codescribe.channel-session.v1", "channel": "0",
      "session_id": "agent-channel-0-test", "state": "sealed",
    ])

    func elements(_ object: Any) -> [any NSAccessibilityProtocol] {
      guard let element = object as? any NSAccessibilityProtocol else { return [] }
      let native = (object as? NSView)?.subviews ?? []
      return [element] + ((element.accessibilityChildren() ?? []) + native).flatMap(elements)
    }
    func scrollViews(_ root: NSView) -> [NSScrollView] {
      (root as? NSScrollView).map { [$0] } ?? root.subviews.flatMap(scrollViews)
    }
    for readCount in 0...4 {
      if readCount > 0 {
        let prior = try lenaConversation(bus).messages[0]
        var ack = recipients[readCount - 1]
        ack.merge([
          "schema": "codescribe.agent-ack.v1",
          "delivery_id": try XCTUnwrap(prior.recipients[readCount - 1].deliveryID),
        ]) { _, new in new }
        bus.consume(ack)
      }
      for channel in ["0", "2"] {
        let conversation = try XCTUnwrap(
          bus.conversations(busPath: busPath).first { $0.channel == channel })
        let message = try XCTUnwrap(conversation.messages.first)
        XCTAssertEqual(message.recipients.count, 4)
        for scale in [CGFloat(1), CGFloat(1.6)] {
          let view = OverlayConversationView(
            conversation: conversation, palette: .light, topInset: 0, bottomInset: 0,
            pendingControls: [], controlErrors: [:], onControl: { _, _ in },
            draft: .constant("Zachowaj szkic"), sending: false, sendError: nil, onSend: {})
          let host = NSHostingView(rootView: view.environment(\.csTextScale, scale))
          host.sizingOptions = []
          host.frame = NSRect(x: 0, y: 0, width: 360, height: 400)
          let window = NSWindow(
            contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
          window.isReleasedWhenClosed = false
          window.contentView = host
          defer { window.close() }
          window.orderFrontRegardless()
          host.layoutSubtreeIfNeeded()
          RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
          host.layoutSubtreeIfNeeded()
          let tree = elements(host)
          let expected = String(localized: "Read \(readCount)/\(4)")
          let control = try XCTUnwrap(
            tree.first {
              $0.accessibilityRole() == .menuButton && $0.accessibilityLabel() == expected
            },
            "channel=\(channel) scale=\(scale): native menu missing; roles=\(tree.map { $0.accessibilityRole()?.rawValue ?? "" })"
          )
          XCTAssertEqual(control.accessibilityLabel(), expected)
          let frame = control.accessibilityFrame()
          XCTAssertGreaterThan(frame.height, 0)
          XCTAssertLessThanOrEqual(frame.height, 32, "receipts must occupy one line")
          XCTAssertLessThan(frame.width, 230, "receipt must fit a narrow message card")
          let scroll = try XCTUnwrap(scrollViews(host).first)
          XCTAssertNotNil(scroll.documentView)
        }
      }
    }
  }

  func testFollowerDeliveryIdentitySurvivesPrunedMailboxAndQuietRestart() throws {
    // Golden IDs produced by the canonical Python follower, including Unicode case folding.
    let cases: [(String, Bool, String, String)] = [
      ("Lena", false, "a3ef146175321188c29696c2", "0071dabf01108c70423514ac"),
      ("Lena", true, "f3dc2f9c6f8361352456291b", "0071dabf01108c70423514ac"),
      ("Straße", false, "21f5bc73227810fd65926352", "3c55192d29e8a0186f69cbc9"),
      ("Straße", true, "18d07635403401be1687107e", "3c55192d29e8a0186f69cbc9"),
      ("*", false, "293a28be203f6fbd5a2fa823", "9bc72fbcce2cebd795f86e24"),
      ("*", true, "441fab6ca0f6c7e77c4dee1f", "9bc72fbcce2cebd795f86e24"),
    ]
    for (audience, terminal, preview, delivery) in cases {
      var bus = OverlayChannelDelivery.Bus()
      var row = occurrence(0, revision: 1)
      row["audience"] = audience
      if !terminal { row["reducer_action"] = "apply_manual_edit" }
      bus.consume(row)
      XCTAssertEqual(
        try lenaConversation(bus).messages.first?.recipients.first?.deliveryID, preview)
      endCapture(&bus, session: "agent-channel-2-take-a")
      XCTAssertEqual(
        try lenaConversation(bus).messages.first?.recipients.first?.deliveryID, delivery)
      bus = try JSONDecoder().decode(
        OverlayChannelDelivery.Bus.self, from: JSONEncoder().encode(bus))
      var ack = owner(leaseA)
      ack.merge(["schema": "codescribe.agent-ack.v1", "delivery_id": delivery]) { _, new in new }
      bus.consume(ack)
      bus.consume(reply(String(repeating: "d", count: 24), delivery: delivery))
      let messages = try lenaConversation(bus).messages
      XCTAssertEqual(messages.first?.recipients.first?.acknowledged, true)
      XCTAssertEqual(messages.last?.replyTo, messages.first?.recipients.first?.deliveryID)
    }
  }

  func testTypedMessagesRetainDistinctIdentityAndCausalReceipt() throws {
    var bus = OverlayChannelDelivery.Bus()
    for index in 0..<5 {
      let identity = String(format: "%024x", index + 1)
      bus.consume([
        "schema": "codescribe.agent-user-message.v1", "kind": "agent_user_message",
        "message_id": identity, "text": "Iwo", "audience": "lena", "channel": "2",
        "recipients": [owner(leaseA)], "emitted_at": "2026-10-05T10:00:00Z",
      ])
    }
    let rows = try lenaConversation(bus).messages
    XCTAssertEqual(rows.count, 5)
    XCTAssertEqual(Set(rows.map(\.id)).count, 5)
    let delivery = try XCTUnwrap(rows.first?.recipients.first?.deliveryID)
    bus.consume(reply(String(repeating: "d", count: 24), delivery: delivery))
    XCTAssertEqual(try lenaConversation(bus).messages.last?.replyTo, delivery)
    bus = try JSONDecoder().decode(OverlayChannelDelivery.Bus.self, from: JSONEncoder().encode(bus))
    XCTAssertEqual(try lenaConversation(bus).messages.count, 6)
  }

  func testTypedBroadcastMergesRecipientCopiesIntoOneQuestion() throws {
    var bus = OverlayChannelDelivery.Bus()
    let identity = String(repeating: "e", count: 24)
    for (lease, channel) in [(leaseA, "2"), (String(repeating: "b", count: 32), "7")] {
      let recipient = owner(lease, channel: channel)
      let row: [String: Any] = [
        "schema": "codescribe.agent-user-message.v1", "kind": "agent_user_message",
        "message_id": identity, "source_event_id": identity, "source": "typed",
        "text": "Do wszystkich", "audience": "lena", "channel": channel,
        "origin_channel": "0", "recipients": [recipient],
      ]
      bus.consume(row)
      bus.consume(row)
    }
    let all = try XCTUnwrap(bus.conversations(busPath: "fixture").first { $0.channel == "0" })
    XCTAssertEqual(all.messages.count, 1)
    XCTAssertEqual(all.messages.first?.recipients.count, 2)
    XCTAssertEqual(Set(all.messages[0].recipients.compactMap(\.deliveryID)).count, 2)
  }

  @MainActor
  func testComposerReturnPublishesEditorBytesWithoutEndingEditing() throws {
    var draft = "Szkic"
    var sent: [String] = []
    let onSend = { sent.append(draft) }
    try withComposer(
      draft: Binding(get: { draft }, set: { draft = $0 }), onSend: onSend
    ) {
      scroll, editor in
      editor.string = "Ostatnie słowa z edytora"
      editor.keyDown(with: try returnEvent())
      XCTAssertEqual(sent, ["Ostatnie słowa z edytora"])
      XCTAssertTrue(editor.window?.firstResponder === editor)
      XCTAssertLessThan(scroll.bounds.height, 40, "a single-line draft must keep the input compact")
      XCTAssertEqual(scroll.borderType, .noBorder, "the shared composer supplies chrome")
    }
  }

  @MainActor
  func testComposerShiftReturnInsertsLineAndReturnSendsBothLines() throws {
    var draft = "Pierwsza"
    var sent: [String] = []
    let onSend = { sent.append(draft) }
    try withComposer(
      draft: Binding(get: { draft }, set: { draft = $0 }), onSend: onSend
    ) {
      _, editor in
      editor.setSelectedRange(NSRange(location: editor.string.utf16.count, length: 0))
      editor.keyDown(with: try returnEvent(modifiers: .shift))
      XCTAssertTrue(sent.isEmpty)
      XCTAssertEqual(editor.string, "Pierwsza\n")
      editor.insertText("Druga", replacementRange: editor.selectedRange())
      editor.keyDown(with: try returnEvent())
      XCTAssertEqual(sent, ["Pierwsza\nDruga"])
    }
  }

  @MainActor
  func testComposerEmptyOrPendingReturnDoesNotSend() throws {
    for pending in [false, true] {
      var draft = pending ? "Już wysyłana" : "  "
      var sends = 0
      let onSend = { sends += 1 }
      try withComposer(
        draft: Binding(get: { draft }, set: { draft = $0 }), sending: pending,
        onSend: onSend
      ) { _, editor in
        editor.keyDown(with: try returnEvent())
        XCTAssertEqual(sends, 0)
      }
    }
  }

  @MainActor
  func testComposerMarkedTextAndOtherCommandsRemainNative() throws {
    var draft = "Tekst"
    var sends = 0
    let onSend = { sends += 1 }
    try withComposer(draft: Binding(get: { draft }, set: { draft = $0 }), onSend: onSend) {
      _, editor in
      editor.setMarkedText(
        "拼", selectedRange: NSRange(location: 1, length: 0),
        replacementRange: editor.selectedRange())
      XCTAssertTrue(editor.hasMarkedText())
      editor.keyDown(with: try returnEvent())
      XCTAssertEqual(sends, 0, "Return commits composition without submitting")
      editor.unmarkText()
      editor.setSelectedRange(NSRange(location: editor.string.utf16.count, length: 0))
      let end = editor.selectedRange().location
      editor.moveLeft(nil)
      XCTAssertLessThan(editor.selectedRange().location, end)
      XCTAssertEqual(sends, 0)
    }
  }

  @MainActor
  func testNewAgentMessageRevealsTheLatestRowAfterReadingEarlierHistory() throws {
    var bus = OverlayChannelDelivery.Bus()
    for index in 0..<18 {
      bus.consume(
        occurrence(
          index * 3200, revision: index + 1,
          text: String(repeating: "Wcześniejsza wiadomość w historii. ", count: 8),
          captureSession: "agent-channel-2-take-\(index)"))
      endCapture(&bus, session: "agent-channel-2-take-\(index)")
    }
    func view() throws -> OverlayConversationView {
      OverlayConversationView(
        conversation: try lenaConversation(bus), palette: .light, topInset: 50, bottomInset: 20,
        pendingControls: [], controlErrors: [:], onControl: { _, _ in },
        draft: .constant("Zachowaj ten szkic"), sending: false, sendError: nil, onSend: {})
    }
    let host = NSHostingView(rootView: try view())
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: 532, height: 300)
    let window = NSWindow(
      contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    defer { window.close() }
    func settle() {
      host.layoutSubtreeIfNeeded()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
      host.layoutSubtreeIfNeeded()
    }
    func scrollViews(_ root: NSView) -> [NSScrollView] {
      (root as? NSScrollView).map { [$0] } ?? root.subviews.flatMap(scrollViews)
    }
    func editor(_ root: NSView) -> NSTextView? {
      if let text = root as? NSTextView, text.isEditable { return text }
      return root.subviews.lazy.compactMap { editor($0) }.first
    }
    settle()
    let scroll = try XCTUnwrap(scrollViews(host).first { $0.bounds.height > 150 })
    let document = try XCTUnwrap(scroll.documentView)
    let originalEditor = try XCTUnwrap(editor(host))
    originalEditor.setSelectedRange(NSRange(location: 5, length: 7))
    scroll.contentView.scroll(to: .zero)
    scroll.reflectScrolledClipView(scroll.contentView)
    settle()
    XCTAssertLessThan(scroll.documentVisibleRect.maxY, document.bounds.height - 100)

    bus.consume(
      occurrence(
        18 * 3200, revision: 19,
        text: String(repeating: "Najnowsza wiadomość ma być widoczna bez szukania. ", count: 10),
        captureSession: "agent-channel-2-take-18"))
    endCapture(&bus, session: "agent-channel-2-take-18")
    host.rootView = try view()
    settle()
    XCTAssertGreaterThan(
      scroll.documentVisibleRect.maxY, document.bounds.height - 50,
      "a newly admitted message must reveal the newest row even after reading history")
    XCTAssertTrue(try XCTUnwrap(editor(host)) === originalEditor)
    XCTAssertEqual(originalEditor.string, "Zachowaj ten szkic")
    XCTAssertEqual(originalEditor.selectedRange(), NSRange(location: 5, length: 7))

    scroll.contentView.scroll(to: .zero)
    scroll.reflectScrolledClipView(scroll.contentView)
    settle()
    let delivery = try XCTUnwrap(
      try lenaConversation(bus).messages.last?.recipients.first?.deliveryID)
    bus.consume(reply(String(repeating: "8", count: 24), delivery: delivery))
    host.rootView = try view()
    settle()
    XCTAssertGreaterThan(
      scroll.documentVisibleRect.maxY, document.bounds.height - 50,
      "an agent reply must also reveal the latest row")
    XCTAssertTrue(try XCTUnwrap(editor(host)) === originalEditor)
    XCTAssertEqual(originalEditor.selectedRange(), NSRange(location: 5, length: 7))
  }

  @MainActor
  func testJumpToCurrentReturnsFromHistoryWithoutReplacingComposer() throws {
    var bus = OverlayChannelDelivery.Bus()
    for index in 0..<30 {
      var row = reply(String(format: "%024x", index + 1))
      row["text"] = String(repeating: "Retained conversation text.\n\n", count: 4)
      bus.consume(row)
    }
    let conversation = try lenaConversation(bus)
    let host = NSHostingView(
      rootView: OverlayConversationView(
        conversation: conversation, palette: .light, topInset: 50, bottomInset: 20,
        pendingControls: [], controlErrors: [:], onControl: { _, _ in },
        draft: .constant("Retained draft"), sending: false, sendError: nil, onSend: {}))
    host.sizingOptions = []
    host.frame = NSRect(x: 0, y: 0, width: 532, height: 300)
    let window = NSWindow(
      contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    defer { window.close() }
    func settle() {
      host.layoutSubtreeIfNeeded()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.2))
      host.layoutSubtreeIfNeeded()
    }
    func scrollViews(_ root: NSView) -> [NSScrollView] {
      (root as? NSScrollView).map { [$0] } ?? root.subviews.flatMap(scrollViews)
    }
    func editor(_ root: NSView) -> NSTextView? {
      if let text = root as? NSTextView, text.isEditable { return text }
      return root.subviews.lazy.compactMap { editor($0) }.first
    }
    let application = AXUIElementCreateApplication(ProcessInfo.processInfo.processIdentifier)
    func axAttribute(_ element: AXUIElement, _ key: String) -> CFTypeRef? {
      var value: CFTypeRef?
      guard AXUIElementCopyAttributeValue(element, key as CFString, &value) == .success else {
        return nil
      }
      return value
    }
    func jumpButtons(_ element: AXUIElement? = nil, depth: Int = 0) -> [AXUIElement] {
      let element = element ?? application
      guard depth < 16 else { return [] }
      if axAttribute(element, kAXIdentifierAttribute) as? String
        == "overlay-conversation-jump-to-current"
      {
        return [element]
      }
      let key = depth == 0 ? kAXWindowsAttribute : kAXChildrenAttribute
      let children = axAttribute(element, key) as? [AXUIElement] ?? []
      return children.flatMap { jumpButtons($0, depth: depth + 1) }
    }
    window.orderFrontRegardless()
    settle()
    let scroll = try XCTUnwrap(scrollViews(host).first { $0.bounds.height > 150 })
    let document = try XCTUnwrap(scroll.documentView)
    let originalEditor = try XCTUnwrap(editor(host))
    originalEditor.setSelectedRange(NSRange(location: 2, length: 5))
    XCTAssertTrue(jumpButtons().isEmpty, "The live edge needs no return affordance")
    NotificationCenter.default.post(
      name: NSScrollView.willStartLiveScrollNotification, object: scroll)
    scroll.contentView.scroll(to: .zero)
    scroll.reflectScrolledClipView(scroll.contentView)
    NotificationCenter.default.post(name: NSScrollView.didLiveScrollNotification, object: scroll)
    NotificationCenter.default.post(name: NSScrollView.didEndLiveScrollNotification, object: scroll)
    settle()
    XCTAssertLessThan(scroll.documentVisibleRect.maxY, document.bounds.height - 100)
    XCTAssertEqual(
      jumpButtons().count, 1, "Manual history reading exposes one centered return pill")
    // Reflow a long mounted conversation, not an empty geometry stand-in.
    // Reading history must not turn resize into a jump-to-current command.
    for width in [CGFloat(360), 680, 420, 600, 360, 532] {
      window.setContentSize(NSSize(width: width, height: 360))
      settle()
      XCTAssertLessThan(scroll.documentVisibleRect.maxY, document.bounds.height - 100)
      XCTAssertTrue(try XCTUnwrap(editor(host)) === originalEditor)
      XCTAssertEqual(originalEditor.string, "Retained draft")
      XCTAssertEqual(originalEditor.selectedRange(), NSRange(location: 2, length: 5))
      XCTAssertLessThanOrEqual(document.bounds.width, scroll.bounds.width + 1)
    }
    let button = try XCTUnwrap(jumpButtons().first)
    let input = try XCTUnwrap(originalEditor.enclosingScrollView)
    var buttonPosition = CGPoint.zero
    var buttonSize = CGSize.zero
    let position = try XCTUnwrap(axAttribute(button, kAXPositionAttribute))
    let size = try XCTUnwrap(axAttribute(button, kAXSizeAttribute))
    XCTAssertEqual(CFGetTypeID(position), AXValueGetTypeID())
    XCTAssertEqual(CFGetTypeID(size), AXValueGetTypeID())
    XCTAssertTrue(AXValueGetValue(position as! AXValue, .cgPoint, &buttonPosition))
    XCTAssertTrue(AXValueGetValue(size as! AXValue, .cgSize, &buttonSize))
    let buttonFrame = NSRect(origin: buttonPosition, size: buttonSize)
    XCTAssertGreaterThan(
      buttonFrame.minY,
      window.convertToScreen(input.convert(input.bounds, to: nil)).maxY,
      "The pill sits above the measured composer")
    XCTAssertEqual(AXUIElementPerformAction(button, kAXPressAction as CFString), .success)
    settle()
    XCTAssertGreaterThan(scroll.documentVisibleRect.maxY, document.bounds.height - 50)
    for width in [CGFloat(360), 680, 420, 600, 360, 532] {
      window.setContentSize(NSSize(width: width, height: 360))
      settle()
      XCTAssertGreaterThan(
        scroll.documentVisibleRect.maxY, document.bounds.height - 50,
        "Native size-change anchoring must keep the live edge through reflow")
    }
    XCTAssertTrue(jumpButtons().isEmpty)
    XCTAssertTrue(try XCTUnwrap(editor(host)) === originalEditor)
    XCTAssertEqual(originalEditor.string, "Retained draft")
    XCTAssertEqual(originalEditor.selectedRange(), NSRange(location: 2, length: 5))
  }

  @MainActor
  func testConversationOpensAtLatestAndBubblesHaveOppositeEdges() throws {
    var bus = OverlayChannelDelivery.Bus()
    for index in 0..<12 {
      bus.consume(
        occurrence(
          index * 3200, revision: index + 1,
          text: "Wiadomość człowieka, która ma pozostać po prawej stronie rozmowy.",
          captureSession: "agent-channel-2-take-\(index)"))
      endCapture(&bus, session: "agent-channel-2-take-\(index)")
      let delivery = try XCTUnwrap(
        try lenaConversation(bus).messages.last?.recipients.first?.deliveryID)
      bus.consume(reply(String(format: "%024x", index + 1), delivery: delivery))
    }
    let conversation = try XCTUnwrap(
      bus.conversations(busPath: busPath).first { $0.channel == "2" })
    for scheme in [ColorScheme.dark, .light] {
      let view = OverlayConversationView(
        conversation: conversation, palette: .resolve(scheme), topInset: 50, bottomInset: 20,
        pendingControls: [], controlErrors: [:], onControl: { _, _ in },
        draft: .constant(""), sending: false, sendError: nil, onSend: {})
      let host = NSHostingView(
        rootView: view.background(OverlayAppearancePalette.resolve(scheme).desktopBackground.color)
          .preferredColorScheme(scheme))
      host.frame = NSRect(x: 0, y: 0, width: 600, height: 500)
      let window = NSWindow(
        contentRect: host.frame, styleMask: .borderless,
        backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      defer { window.close() }
      host.layoutSubtreeIfNeeded()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.08))
      func scrollViews(_ root: NSView) -> [NSScrollView] {
        (root as? NSScrollView).map { [$0] } ?? root.subviews.flatMap(scrollViews)
      }
      let scroll = try XCTUnwrap(scrollViews(host).first { $0.bounds.height > 150 })
      let viewport = scroll.convert(scroll.bounds, to: host)
      XCTAssertEqual(
        viewport.height, host.bounds.height, accuracy: 1,
        "messages must scroll beneath the fixed header and composer, without a clipped middle strip"
      )
      XCTAssertEqual(viewport.minY, host.bounds.minY, accuracy: 1)
      let document = try XCTUnwrap(scroll.documentView)
      XCTAssertGreaterThan(
        scroll.documentVisibleRect.maxY, document.bounds.height - 50,
        "opening a channel must reveal the latest row")
      let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
      host.cacheDisplay(in: host.bounds, to: bitmap)
      let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
      try png.write(
        to: FileManager.default.temporaryDirectory
          .appendingPathComponent("codescribe-conversation-\(scheme).png"))
      var warmCount = 0
      var warmX: Double = 0
      var neutralCount = 0
      var neutralLeft = 0
      var neutralRight = 0
      // Rendered interiors must match; the user's brand hairline stays sparse and on the right.
      for y in stride(from: 90, to: bitmap.pixelsHigh - 30, by: 4) {
        for x in stride(from: 0, to: bitmap.pixelsWide, by: 4) {
          guard let color = bitmap.colorAt(x: x, y: y) else {
            continue
          }
          let r = color.redComponent
          let g = color.greenComponent
          let b = color.blueComponent
          if r - g > 0.035 && g - b > 0.025 {
            warmCount += 1
            warmX += Double(x)
          }
          let agentFill = scheme == .dark ? 0.149 : 0.914
          if abs(r - agentFill) < 0.018 && abs(r - g) < 0.03 {
            neutralCount += 1
            if x < bitmap.pixelsWide / 2 { neutralLeft += 1 } else { neutralRight += 1 }
          }
        }
      }
      XCTAssertGreaterThan(warmCount, 10, "the human bubble retains its brand outline")
      XCTAssertGreaterThan(neutralLeft, 100, "agent bubble has a neutral interior")
      XCTAssertGreaterThan(neutralRight, 100, "human bubble has the same neutral interior")
      XCTAssertLessThan(warmCount * 10, neutralCount, "orange is a hairline, never a filled bubble")
      let viewportMidpoint = Double(viewport.midX) * Double(bitmap.pixelsWide) / host.bounds.width
      XCTAssertGreaterThan(warmX / Double(max(1, warmCount)), viewportMidpoint)

    }
  }

  @MainActor
  func testCompactConversationLeavesReadingSpaceBetweenGlassEdges() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(occurrence(0, revision: 1, text: "Czytelna wiadomość w małym panelu."))
    let conversation = try lenaConversation(bus)
    for scheme in [ColorScheme.dark, .light] {
      let view = OverlayConversationView(
        conversation: conversation, palette: .resolve(scheme), topInset: 50, bottomInset: 20,
        pendingControls: [], controlErrors: [:], onControl: { _, _ in },
        draft: .constant(""), sending: false, sendError: nil, onSend: {})
      let host = NSHostingView(rootView: view.preferredColorScheme(scheme))
      host.frame = NSRect(x: 0, y: 0, width: 532, height: 260)
      let window = NSWindow(
        contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      defer { window.close() }
      host.layoutSubtreeIfNeeded()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.08))
      func scrollViews(_ root: NSView) -> [NSScrollView] {
        (root as? NSScrollView).map { [$0] } ?? root.subviews.flatMap(scrollViews)
      }
      let scrolls = scrollViews(host)
      let messages = try XCTUnwrap(scrolls.first { $0.bounds.height > 150 })
      let input = try XCTUnwrap(scrolls.first { $0.bounds.height < 80 })
      XCTAssertEqual(host.bounds.height, 260, "do not enlarge the window to hide excess chrome")
      XCTAssertEqual(messages.bounds.height, 260, accuracy: 1, "messages use the full viewport")
      XCTAssertLessThan(input.bounds.height, 40, "one compact single-line input")
      XCTAssertGreaterThanOrEqual(messages.documentVisibleRect.height, 120)
    }
  }

  @MainActor
  func testConversationGlassDoesNotPaintAFullWidthComposerBar() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(occurrence(0, revision: 1, text: "Wiadomość pod pływającym polem tekstowym."))
    let conversation = try lenaConversation(bus)
    for scheme in [ColorScheme.dark, .light] {
      let host = NSHostingView(
        rootView: OverlayConversationView(
          conversation: conversation, palette: .resolve(scheme), topInset: 50, bottomInset: 20,
          pendingControls: [], controlErrors: [:], onControl: { _, _ in },
          draft: .constant(""), sending: false, sendError: nil, onSend: {}
        )
        .preferredColorScheme(scheme))
      host.frame = NSRect(x: 0, y: 0, width: 532, height: 260)
      let window = NSWindow(
        contentRect: host.frame, styleMask: .borderless, backing: .buffered, defer: false)
      window.isReleasedWhenClosed = false
      window.contentView = host
      defer { window.close() }
      host.layoutSubtreeIfNeeded()
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.08))
      func scrollViews(_ root: NSView) -> [NSScrollView] {
        (root as? NSScrollView).map { [$0] } ?? root.subviews.flatMap(scrollViews)
      }
      let scrolls = scrollViews(host)
      let messages = try XCTUnwrap(scrolls.first { $0.bounds.height > 150 })
      let input = try XCTUnwrap(scrolls.first { $0.bounds.height < 80 })
      let inputFrame = input.convert(input.bounds, to: host)
      XCTAssertEqual(messages.bounds.height, host.bounds.height, accuracy: 1)
      XCTAssertGreaterThanOrEqual(inputFrame.minX, 20)
      XCTAssertLessThanOrEqual(inputFrame.maxX, host.bounds.width - 20)
      XCTAssertLessThan(input.bounds.height, 40, "input glass remains one compact field")
    }
  }

  @MainActor
  private func returnEvent(modifiers: NSEvent.ModifierFlags = []) throws -> NSEvent {
    try XCTUnwrap(
      NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: modifiers,
        timestamp: 0, windowNumber: 0, context: nil, characters: "\r",
        charactersIgnoringModifiers: "\r",
        isARepeat: false, keyCode: 36))
  }

  @MainActor
  func testRestoredConversationLatestBubbleIsVisibleOnFullCanvas() throws {
    for count in [1, 173] {
      for initiallyMini in [false, true] {
        var bus = OverlayChannelDelivery.Bus()
        for index in 0..<count {
          var row = reply(String(format: "%024x", index + 1))
          row["text"] =
            index == count - 1
            ? "LATEST VISIBLE BUBBLE\n\nLast retained message must appear immediately."
            : String(repeating: "Retained multiline conversation.\n\n", count: 5)
          bus.consume(row)
        }
        let restored = try JSONDecoder().decode(
          OverlayChannelDelivery.Bus.self, from: JSONEncoder().encode(bus))
        let conversation = try lenaConversation(restored)
        XCTAssertEqual(conversation.messages.count, count)
        XCTAssertNotNil(conversation.messages.last)
        let state = OverlayState.previewFormatted()
        state.applyConversationSnapshot(.init(deliveries: [], conversations: [conversation]))
        state.selectConversation(conversation.id, expand: false)
        state.conversationDrafts[conversation.id] = "Retained unsent draft"
        state.setPresentationMode(initiallyMini ? .mini : .expanded)
        let host = NSHostingView(rootView: DictationOverlayView(state: state))
        host.sizingOptions = []
        let panel = FloatingOverlayPanel(
          contentRect: NSRect(x: 800, y: 300, width: 600, height: 500),
          styleMask: [.borderless, .nonactivatingPanel, .resizable],
          backing: .buffered, defer: false)
        panel.isReleasedWhenClosed = false
        panel.contentView = host
        panel.setPresentationMode(initiallyMini ? .mini : .expanded)
        state.onPresentationModeChanged = { [weak panel] mode in
          panel?.setPresentationMode(mode, animated: false)
        }
        panel.orderFrontRegardless()
        defer { panel.close() }
        host.layoutSubtreeIfNeeded()
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.08))
        let revision = state.conversationFocusRevision
        if initiallyMini {
          state.setPresentationMode(.expanded)
          XCTAssertEqual(state.conversationFocusRevision, revision)
        }
        // Permit the existing presentation animation and native layout to
        // settle, without a wheel, reselect, forced scroll or identity reset.
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.35))
        func elements(_ object: Any) -> [any NSAccessibilityProtocol] {
          guard let element = object as? any NSAccessibilityProtocol else { return [] }
          let native = (object as? NSView)?.subviews ?? []
          return [element] + ((element.accessibilityChildren() ?? []) + native).flatMap(elements)
        }
        let tree = elements(host)
        let candidates = tree.filter {
          $0.accessibilityRole() == .staticText
            && (($0.accessibilityLabel() ?? "").contains("LATEST VISIBLE BUBBLE")
              || ($0.accessibilityValue() as? String ?? "").contains("LATEST VISIBLE BUBBLE"))
        }
        let nativeViewport = panel.convertToScreen(host.bounds)
          .insetBy(dx: 20, dy: 90)
        XCTAssertTrue(
          candidates.contains { element in
            let frame = element.accessibilityFrame()
            return !frame.isEmpty && nativeViewport.intersects(frame)
          },
          "count=\(count) mini=\(initiallyMini): latest bubble missing from viewport \(nativeViewport); frames=\(candidates.map { $0.accessibilityFrame() })"
        )
        XCTAssertEqual(state.selectedConversationID, conversation.id)
        XCTAssertEqual(state.selectedConversation?.messages, conversation.messages)
        XCTAssertEqual(state.conversationDrafts[conversation.id], "Retained unsent draft")
      }
    }
  }

  @MainActor
  func testRepeatedAgentSwitchPaintsLatestBubbleWithoutInputAndKeepsComposer() throws {
    let conversations = [173, 1, 35].enumerated().map { agent, count in
      OverlayConversation(
        id: "switch-agent-\(agent)", channel: "0", name: "Agent \(agent)", owner: nil,
        messages: (0..<count).map { index in
          OverlayConversationMessage(
            id: "reply:switch-\(agent)-\(index)", kind: .reply,
            text: index == count - 1
              ? "VISIBLE AGENT \(agent)"
              : String(repeating: "Retained multiline conversation.\n\n", count: 5),
            order: UInt64(index), emittedAt: "2026-10-13T07:00:00Z", owner: nil,
            recipients: [], deliveryID: nil, replyTo: nil, unsolicited: false,
            playback: nil, busPath: "")
        })
    }
    let state = OverlayState.previewFormatted()
    state.applyConversationSnapshot(.init(deliveries: [], conversations: conversations))
    for conversation in conversations {
      state.conversationDrafts[conversation.id] = "Draft for \(conversation.id)"
    }
    state.selectConversation(conversations[0].id)
    let host = NSHostingView(rootView: DictationOverlayView(state: state))
    host.sizingOptions = []
    let panel = FloatingOverlayPanel(
      contentRect: NSRect(x: 800, y: 300, width: 600, height: 500),
      styleMask: [.borderless, .nonactivatingPanel, .resizable],
      backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    panel.contentView = host
    panel.setPresentationMode(.expanded)
    panel.orderFrontRegardless()
    defer { panel.close() }

    func elements(_ object: Any) -> [any NSAccessibilityProtocol] {
      guard let element = object as? any NSAccessibilityProtocol else { return [] }
      let native = (object as? NSView)?.subviews ?? []
      return [element] + ((element.accessibilityChildren() ?? []) + native).flatMap(elements)
    }
    func editor(_ root: NSView) -> NSTextView? {
      if let text = root as? NSTextView, text.isEditable,
        text.accessibilityLabel() == String(localized: "Send to everyone")
      {
        return text
      }
      return root.subviews.lazy.compactMap(editor).first
    }
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.35))
    let originalEditor = try XCTUnwrap(editor(host))
    for index in [1, 0, 2, 1, 2, 0, 1, 0] {
      let target = conversations[index]
      let loading = OverlayConversation(
        id: target.id, channel: target.channel, name: target.name, owner: target.owner,
        messages: [], historyLoaded: false)
      state.applyConversationSnapshot(
        .init(
          deliveries: [], conversations: conversations.map { $0.id == target.id ? loading : $0 }))
      state.selectConversation(conversations[index].id)
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.02))
      state.applyConversationSnapshot(.init(deliveries: [], conversations: conversations))
      // No wheel, typing, reselect, explicit layout or forced redraw.
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.35))
      let viewport = panel.convertToScreen(host.bounds).insetBy(dx: 20, dy: 90)
      let bubbles = elements(host).filter {
        $0.accessibilityRole() == .staticText
          && (($0.accessibilityValue() as? String ?? "").contains("VISIBLE AGENT \(index)")
            || ($0.accessibilityLabel() ?? "").contains("VISIBLE AGENT \(index)"))
      }
      XCTAssertTrue(
        bubbles.contains {
          !$0.accessibilityFrame().isEmpty && viewport.intersects($0.accessibilityFrame())
        },
        "agent \(index) has no painted latest bubble after switching")
      XCTAssertTrue(
        try XCTUnwrap(editor(host)) === originalEditor, "switch must not recreate input")
      XCTAssertEqual(originalEditor.string, "Draft for \(conversations[index].id)")
      XCTAssertEqual(state.selectedConversation?.messages, conversations[index].messages)
    }
  }

  @MainActor
  func testActualPanelInitialComposerClickTakesKeyAndUpdatesDraft() throws {
    try withPanelComposer(transcriptFirst: false)
  }

  @MainActor
  func testActualPanelTranscriptToComposerClickRetainsKeyAndUpdatesDraft() throws {
    try withPanelComposer(transcriptFirst: true)
  }

  @MainActor
  func testActualComposerRetainsRepliesWhileActiveThenReleasesAfterRenewedTypingHorizon() throws {
    try withPanelComposer(transcriptFirst: true, exerciseReplyProtection: true)
  }

  @MainActor
  private func withPanelComposer(
    transcriptFirst: Bool, exerciseReplyProtection: Bool = false
  ) throws {
    var draft = ""
    var now: TimeInterval = 100
    let state = OverlayState(nowProvider: { now })
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(reply(String(repeating: "c", count: 24)))
    state.applyConversationSnapshot(
      .init(deliveries: [], conversations: bus.conversations(busPath: busPath)))
    let selected = try lenaConversation(bus)
    state.selectConversation(selected.id)
    state.setConversationVisible(true)
    let host = NSHostingView(
      rootView: OverlayConversationComposer(
        palette: .dark, draft: Binding(get: { draft }, set: { draft = $0 }),
        sending: false, onSubmit: {},
        onEditorActive: { state.noteComposerEditorActive($0) },
        onTypingActivity: { state.noteComposerTypingActivity() }))
    host.frame = NSRect(x: 0, y: 0, width: 600, height: 100)
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 400))
    content.addSubview(host)
    let transcript = LiveTranscriptTextView.makeTextView()
    transcript.frame = NSRect(x: 20, y: 160, width: 550, height: 180)
    transcript.string = "Retained transcript"
    content.addSubview(transcript)
    let panel = FloatingOverlayPanel(
      contentRect: content.frame, styleMask: [.borderless, .nonactivatingPanel, .resizable],
      backing: .buffered, defer: false)
    panel.isReleasedWhenClosed = false
    panel.delegate = panel
    panel.becomesKeyOnlyIfNeeded = true
    panel.contentView = content
    defer {
      panel.makeFirstResponder(nil)
      panel.releaseKeyAfterTranscript()
      panel.close()
    }
    panel.orderFront(nil)
    host.layoutSubtreeIfNeeded()
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.03))
    func editable(_ view: NSView) -> NSTextView? {
      if let text = view as? NSTextView, text.isEditable { return text }
      return view.subviews.lazy.compactMap(editable).first
    }
    let editor = try XCTUnwrap(editable(host))
    func click(_ view: NSView) throws {
      let local = NSPoint(x: view.bounds.midX, y: view.bounds.midY)
      let location = view.convert(local, to: nil)
      let contentLocation = content.convert(location, from: nil)
      let hit = try XCTUnwrap(content.hitTest(contentLocation))
      XCTAssertTrue(hit === view || hit.isDescendant(of: view))
      let down = try XCTUnwrap(
        NSEvent.mouseEvent(
          with: .leftMouseDown, location: location, modifierFlags: [],
          timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: panel.windowNumber,
          context: nil, eventNumber: 1, clickCount: 1, pressure: 1))
      let up = try XCTUnwrap(
        NSEvent.mouseEvent(
          with: .leftMouseUp, location: location, modifierFlags: [],
          timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: panel.windowNumber,
          context: nil, eventNumber: 2, clickCount: 1, pressure: 0))
      NSApp.postEvent(up, atStart: true)
      panel.sendEvent(down)
      RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.01))
    }
    if transcriptFirst {
      try click(transcript)
      XCTAssertTrue(panel.firstResponder === transcript)
      XCTAssertTrue(panel.isKeyWindow)
    }
    try click(editor)
    XCTAssertTrue(panel.allowsKeyForTranscript)
    XCTAssertTrue(panel.isKeyWindow)
    XCTAssertTrue(panel.firstResponder === editor)
    var nextReply = 100
    func receiveOtherOwnerReply() throws -> OverlayConversation {
      nextReply += 1
      var row = reply(String(format: "%024x", nextReply))
      row.merge(owner(leaseB, session: "agent-b", name: "Astra", channel: "3")) {
        _, new in new
      }
      bus.consume(row)
      let conversations = bus.conversations(busPath: busPath)
      state.applyConversationSnapshot(.init(deliveries: [], conversations: conversations))
      return try XCTUnwrap(conversations.first { $0.owner?.leaseID == leaseB })
    }
    if exerciseReplyProtection {
      now = 1000
      let other = try receiveOtherOwnerReply()
      XCTAssertEqual(state.selectedConversationID, selected.id, "blank active field owns focus")
      XCTAssertEqual(state.unreadReplies(in: other), 1)
      XCTAssertTrue(panel.firstResponder === editor)
      XCTAssertTrue(panel.isKeyWindow)
      XCTAssertEqual(draft, "")
    }
    let key = try XCTUnwrap(
      NSEvent.keyEvent(
        with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
        windowNumber: panel.windowNumber, context: nil, characters: "x",
        charactersIgnoringModifiers: "x", isARepeat: false, keyCode: 7))
    panel.sendEvent(key)
    XCTAssertEqual(editor.string, "x")
    XCTAssertEqual(draft, "x")
    if exerciseReplyProtection {
      editor.setSelectedRange(NSRange(location: 0, length: 1))
      let focus = state.conversationFocusRevision
      let other = try receiveOtherOwnerReply()
      XCTAssertEqual(state.selectedConversationID, selected.id)
      XCTAssertEqual(state.conversationFocusRevision, focus)
      XCTAssertEqual(state.unreadReplies(in: other), 2)
      XCTAssertTrue(panel.firstResponder === editor)
      XCTAssertEqual(editor.selectedRange(), NSRange(location: 0, length: 1))
      XCTAssertEqual(draft, "x")
      now = 1009
      panel.sendEvent(key)
      XCTAssertEqual(draft, "x", "typing replaces the selection and renews the horizon")
      XCTAssertTrue(panel.makeFirstResponder(transcript), "real native responder transition")
      now = 1023.999
      let beforeExpiry = try receiveOtherOwnerReply()
      XCTAssertEqual(state.selectedConversationID, selected.id)
      XCTAssertEqual(state.unreadReplies(in: beforeExpiry), 3)
      // Manual navigation remains available even during the hold.
      state.selectConversation(beforeExpiry.id)
      XCTAssertEqual(state.selectedConversationID, beforeExpiry.id)
      state.selectConversation(selected.id)
      now = 1024
      state.applyConversationSnapshot(
        .init(deliveries: [], conversations: bus.conversations(busPath: busPath)))
      XCTAssertEqual(state.selectedConversationID, selected.id, "expiry never replays a reply")
      let atExpiry = try receiveOtherOwnerReply()
      XCTAssertEqual(state.selectedConversationID, atExpiry.id, "fresh reply follows at deadline")
      XCTAssertTrue(panel.firstResponder === transcript)
      XCTAssertEqual(draft, "x")
    }
    // Leaving the panel still returns keyboard ownership to the destination.
    panel.windowDidResignKey(Notification(name: NSWindow.didResignKeyNotification, object: panel))
    XCTAssertFalse(panel.allowsKeyForTranscript)
    XCTAssertFalse(panel.firstResponder === editor)
  }

  @MainActor
  private func withComposer(
    draft: Binding<String>, sending: Bool = false, onSend: @escaping () -> Void,
    inspect: (NSScrollView, NSTextView) throws -> Void
  ) throws {
    let owner = try XCTUnwrap(OverlayConversationOwner(row: owner(leaseA)))
    let view = OverlayConversationView(
      conversation: .init(id: owner.id, channel: "2", name: "Lena", owner: owner, messages: []),
      palette: .dark, topInset: 50, bottomInset: 20, pendingControls: [], controlErrors: [:],
      onControl: { _, _ in }, draft: draft, sending: sending, sendError: nil,
      onSend: onSend)
    let host = NSHostingView(rootView: view)
    host.frame = NSRect(x: 0, y: 0, width: 600, height: 500)
    let window = NSWindow(
      contentRect: host.frame, styleMask: .borderless,
      backing: .buffered, defer: false)
    window.isReleasedWhenClosed = false
    window.contentView = host
    defer { window.close() }
    host.layoutSubtreeIfNeeded()
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.03))
    func editableTextView(_ root: NSView) -> NSTextView? {
      if let editor = root as? NSTextView, editor.isEditable { return editor }
      return root.subviews.lazy.compactMap(editableTextView).first
    }
    let editor = try XCTUnwrap(editableTextView(host))
    let scroll = try XCTUnwrap(editor.enclosingScrollView)
    XCTAssertTrue(window.makeFirstResponder(editor))
    try inspect(scroll, editor)
  }

  private let busPath = "/fixture/transcript-events.jsonl"
  private let leaseA = String(repeating: "a", count: 32)
  private let leaseB = String(repeating: "b", count: 32)

  private func owner(
    _ lease: String, session: String = "agent-a", name: String = "Lena", channel: String = "2"
  ) -> [String: Any] {
    [
      "provider": "codex", "provider_session_id": session, "lease_id": lease,
      "name": name, "channel": channel,
    ]
  }

  private func occurrence(
    _ start: Int, revision: Int, text: String = "Iwo", recipients: [[String: Any]]? = nil,
    captureSession: String = "agent-channel-2-take-a"
  ) -> [String: Any] {
    [
      "schema": "codescribe.transcript-evidence.v1", "session_id": captureSession,
      "occurrence_session_id": captureSession, "capture_epoch": 1,
      "sample_start": start, "sample_end": start + 1600, "document_index": 0,
      "reducer_revision": revision, "sequence": revision,
      "reducer_action": "record_ledger_terminal_seal", "audience": "Lena",
      "rendered_text": text, "recipients": recipients ?? [owner(leaseA)],
    ]
  }

  private func endCapture(_ bus: inout OverlayChannelDelivery.Bus, session: String) {
    bus.consume([
      "schema": "codescribe.transcript.v1", "session_id": session, "status": "session_ended",
    ])
  }

  private func reply(_ replyID: String, delivery: String? = nil) -> [String: Any] {
    var row = owner(leaseA)
    row.merge([
      "schema": "codescribe.agent-reply.v1", "reply_id": replyID,
      "text": "Odpowiedź", "association": delivery == nil ? "unsolicited" : "addressed",
      "emitted_at": "2026-10-05T04:00:00Z",
      "tts_vendor": "xai", "voice": "ara", "speed": 1.25,
    ]) { _, new in new }
    if let delivery { row["delivery_id"] = delivery }
    return row
  }

  private func playback(_ replyID: String, ticket: String, state: String, time: String) -> [String:
    Any]
  {
    var row = owner(leaseA)
    row.merge([
      "schema": "codescribe.agent-reply-playback.v1", "reply_id": replyID,
      "playback_ticket": ticket, "state": state, "emitted_at": time,
      "spoken": state == "spoken",
    ]) { _, new in new }
    return row
  }

  private func lenaConversation(_ bus: OverlayChannelDelivery.Bus) throws -> OverlayConversation {
    try XCTUnwrap(bus.conversations(busPath: busPath).first { $0.owner?.leaseID == leaseA })
  }

  func testFiveEqualLabelsInOneCaptureRetainFiveRangesInsideOneMessage() throws {
    var bus = OverlayChannelDelivery.Bus()
    let full = "Iwo Iwo Iwo Iwo Iwo"
    for index in 0..<5 {
      bus.consume(occurrence(index * 3200, revision: index + 1, text: full))
    }
    XCTAssertEqual(try lenaConversation(bus).messages.count, 1)
    let message = try XCTUnwrap(try lenaConversation(bus).messages.first)
    let identities = try XCTUnwrap(message.occurrenceIDs)
    XCTAssertEqual(identities.count, 5)
    XCTAssertEqual(Set(identities).count, 5, "document_index0 must not overwrite physical ranges")
    XCTAssertEqual(message.text, full, "copy the full canonical render once")
    bus.consume(occurrence(0, revision: 10, text: "Iwo poprawione"))
    bus.consume(occurrence(0, revision: 1, text: "stara odpowiedź"))
    XCTAssertEqual(try lenaConversation(bus).messages.count, 1)
    let revised = try XCTUnwrap(try lenaConversation(bus).messages.first)
    XCTAssertEqual(revised.id, message.id)
    XCTAssertEqual(revised.text, "Iwo poprawione")
    XCTAssertEqual(revised.occurrenceIDs, identities)
    bus = try JSONDecoder().decode(OverlayChannelDelivery.Bus.self, from: JSONEncoder().encode(bus))
    XCTAssertEqual(try lenaConversation(bus).messages.first?.occurrenceIDs, identities)
  }

  func testFiveEqualCapturesRemainFiveDistinctMessages() throws {
    var bus = OverlayChannelDelivery.Bus()
    for index in 0..<5 {
      let session = "agent-channel-2-identical-\(index)"
      bus.consume(occurrence(0, revision: 1, captureSession: session))
      endCapture(&bus, session: session)
    }
    let rows = try lenaConversation(bus).messages
    XCTAssertEqual(rows.count, 5)
    XCTAssertEqual(Set(rows.map(\.id)).count, 5)
    XCTAssertEqual(rows.map(\.text), Array(repeating: "Iwo", count: 5))
    XCTAssertEqual(Set(rows.flatMap { $0.occurrenceIDs ?? [] }).count, 5)
    XCTAssertEqual(Set(rows.flatMap { $0.recipients.compactMap(\.deliveryID) }).count, 5)
  }

  func testBroadcastIsOneQuestionWithFrozenRecipientsAndNoLateInheritance() throws {
    var bus = OverlayChannelDelivery.Bus()
    let recipients = [owner(leaseA), owner(leaseB, session: "agent-b", name: "Adam", channel: "4")]
    var question = occurrence(0, revision: 1, recipients: recipients)
    question["audience"] = "*"
    question["session_id"] = "agent-channel-0-test"
    question["occurrence_session_id"] = "agent-channel-0-test"
    bus.consume(question)
    XCTAssertEqual(try lenaConversation(bus).messages.count, 1)
    XCTAssertEqual(try lenaConversation(bus).messages.first?.recipients.count, 2)
    let leaseC = String(repeating: "c", count: 32)
    bus.observeLease(
      [
        "schema": "codescribe.agent-bridge.lease.v1", "provider": "codex",
        "provider_session_id": "agent-c", "lease_id": leaseC, "bus": busPath,
        "pending": [],
      ], channel: "3",
      binding: ["provider": "codex", "provider_session_id": "agent-c", "audience": "Astra"])
    let conversations = bus.conversations(busPath: busPath)
    XCTAssertEqual(conversations.filter { $0.owner != nil && !$0.messages.isEmpty }.count, 2)
    XCTAssertTrue(
      conversations.filter { $0.owner?.leaseID == leaseC }.allSatisfy { $0.messages.isEmpty })
    XCTAssertEqual(try lenaConversation(bus).messages.first?.recipients.count, 2)
  }

  func testAllShowsOnlyBroadcastAndOwnedCausalRepliesAcrossRestart() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(occurrence(0, revision: 1, text: "Tylko do Leny"))
    let privateDelivery = try XCTUnwrap(
      try lenaConversation(bus).messages.last?.recipients.first?.deliveryID)
    bus.consume(reply(String(repeating: "d", count: 24), delivery: privateDelivery))
    var broadcast = occurrence(3200, revision: 2, text: "Do wszystkich")
    broadcast["session_id"] = "agent-channel-0-test"
    broadcast["occurrence_session_id"] = "agent-channel-0-test"
    broadcast["audience"] = "*"
    bus.consume(broadcast)
    let delivery = try XCTUnwrap(
      try lenaConversation(bus).messages.last?.recipients.first?.deliveryID)
    bus.consume(reply(String(repeating: "e", count: 24), delivery: delivery))
    bus.consume(reply(String(repeating: "f", count: 24)))
    for projection in [
      bus,
      try JSONDecoder().decode(
        OverlayChannelDelivery.Bus.self,
        from: JSONEncoder().encode(bus)),
    ] {
      let zero = try XCTUnwrap(
        projection.conversations(busPath: busPath).first { $0.channel == "0" })
      XCTAssertEqual(zero.messages.map(\.text), ["Do wszystkich", "Odpowiedź"])
      XCTAssertEqual(zero.messages.last?.replyTo, delivery)
      XCTAssertEqual(
        try lenaConversation(projection).messages.count, 5, "private conversation retains every row"
      )
    }
  }

  func testExplicitPrivatePeerRouteOverridesBroadcastAssociation() throws {
    var bus = OverlayChannelDelivery.Bus()
    var question = occurrence(0, revision: 1, text: "Do wszystkich")
    question["session_id"] = "agent-channel-0-test"
    question["occurrence_session_id"] = "agent-channel-0-test"
    question["audience"] = "*"
    bus.consume(question)
    let delivery = try XCTUnwrap(
      try lenaConversation(bus).messages.first?.recipients.first?.deliveryID)
    // An ordinary associated response still follows its question into All.
    let associatedID = String(repeating: "d", count: 24)
    bus.consume(reply(associatedID, delivery: delivery))
    // Explicit peer addressing wins even if an older association names All.
    let privateID = String(repeating: "e", count: 24)
    var privateReply = reply(privateID, delivery: delivery)
    privateReply["sender"] = owner(leaseA)
    privateReply["channel"] = "4"
    privateReply["origin_channel"] = "4"
    privateReply["peer_to"] = "Adam"
    privateReply["recipients"] = [owner(leaseB, session: "agent-b", name: "Adam", channel: "4")]
    privateReply["text"] = "Tylko do Adama"
    bus.consume(privateReply)
    for projection in [
      bus,
      try JSONDecoder().decode(OverlayChannelDelivery.Bus.self, from: JSONEncoder().encode(bus)),
    ] {
      let conversations = projection.conversations(busPath: busPath)
      let all = try XCTUnwrap(conversations.first { $0.id == "0" })
      XCTAssertEqual(all.messages.map(\.text), ["Do wszystkich", "Odpowiedź"])
      XCTAssertEqual(all.messages.last?.id, "reply:" + associatedID)
      XCTAssertEqual(all.messages.last?.replyTo, delivery)
      XCTAssertFalse(
        try lenaConversation(projection).messages.contains { $0.id == "reply:" + privateID })
      let adam = try XCTUnwrap(conversations.first { $0.owner?.leaseID == leaseB })
      XCTAssertEqual(adam.messages.map(\.id), ["reply:" + privateID])
      XCTAssertEqual(adam.messages.first?.owner?.name, "Lena")
      XCTAssertEqual(adam.messages.first?.sourceChannel, "4")
    }
  }

  func testLateAcknowledgmentRetainsReplyAndForeignOwnerCannotAcknowledge() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(occurrence(0, revision: 1))
    let delivery = try XCTUnwrap(
      try lenaConversation(bus).messages.first?.recipients.first?.deliveryID)
    bus.consume(reply(String(repeating: "d", count: 24), delivery: delivery))
    var foreign = owner(leaseB, session: "agent-b")
    foreign.merge(["schema": "codescribe.agent-ack.v1", "delivery_id": delivery]) { _, new in new }
    bus.consume(foreign)
    XCTAssertEqual(try lenaConversation(bus).messages.first?.recipients.first?.acknowledged, false)
    var ack = owner(leaseA)
    ack.merge(["schema": "codescribe.agent-ack.v1", "delivery_id": delivery]) { _, new in new }
    bus.consume(ack)
    let rows = try lenaConversation(bus).messages
    XCTAssertEqual(rows.count, 2)
    XCTAssertEqual(rows.first?.recipients.first?.acknowledged, true)
    XCTAssertEqual(rows.last?.text, "Odpowiedź")
    XCTAssertEqual(rows.last?.replyTo, delivery)
  }

  func testRebindingAndRenamingDoNotRelabelHistoricalReply() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(reply(String(repeating: "d", count: 24)))
    bus.observeLease(
      [
        "schema": "codescribe.agent-bridge.lease.v1", "provider": "codex",
        "provider_session_id": "agent-b", "lease_id": leaseB, "bus": busPath,
        "pending": [],
      ], channel: "2",
      binding: ["provider": "codex", "provider_session_id": "agent-b", "audience": "Nowy"])
    let old = try XCTUnwrap(
      bus.conversations(busPath: busPath).first { $0.owner?.leaseID == leaseA })
    XCTAssertEqual(old.name, "Lena")
    XCTAssertEqual(old.messages.first?.owner?.providerSessionID, "agent-a")
    XCTAssertTrue(
      bus.conversations(busPath: busPath).filter { $0.owner?.leaseID == leaseB }.allSatisfy {
        $0.messages.isEmpty
      })
  }

  func testPriorPlaybackTicketCannotCompleteOrRearmNewPlayback() throws {
    var bus = OverlayChannelDelivery.Bus()
    bus.consume(reply(String(repeating: "d", count: 24)))
    bus.consume(
      playback(
        String(repeating: "d", count: 24), ticket: "old", state: "waiting",
        time: "2026-10-05T04:00:00Z"))
    bus.consume(
      playback(
        String(repeating: "d", count: 24), ticket: "new", state: "playing",
        time: "2026-10-05T04:00:02Z"))
    bus.consume(
      playback(
        String(repeating: "d", count: 24), ticket: "old", state: "spoken",
        time: "2026-10-05T04:00:03Z"))
    XCTAssertEqual(try lenaConversation(bus).messages.last?.playback?.ticket, "new")
    bus.consume(
      playback(
        String(repeating: "d", count: 24), ticket: "old", state: "waiting",
        time: "2026-10-05T04:00:04Z"))
    XCTAssertEqual(try lenaConversation(bus).messages.last?.playback?.ticket, "new")
    XCTAssertEqual(try lenaConversation(bus).messages.last?.playback?.state, "playing")
  }

  /// Founder, 2026-10-14: the speaker shows who is talking right now.
  func testOnlyAPlayingReplyMarksItsConversationAsSpeaking() throws {
    var bus = OverlayChannelDelivery.Bus()
    let replyID = String(repeating: "e", count: 24)
    bus.consume(reply(replyID))
    XCTAssertFalse(try lenaConversation(bus).replySpeaking, "Reply text alone is not speech")
    bus.consume(
      playback(replyID, ticket: "voice", state: "waiting", time: "2026-10-05T04:00:01Z"))
    XCTAssertFalse(
      try lenaConversation(bus).replySpeaking, "Synthesis and the player queue are silent")
    bus.consume(
      playback(replyID, ticket: "voice", state: "playing", time: "2026-10-05T04:00:02Z"))
    XCTAssertTrue(try lenaConversation(bus).replySpeaking)
    bus.consume(
      playback(replyID, ticket: "voice", state: "spoken", time: "2026-10-05T04:00:03Z"))
    XCTAssertFalse(try lenaConversation(bus).replySpeaking, "The indicator ends with the speech")
  }

  func testProjectionRoundTripPreservesDistinctRowsAndCausalOwnership() throws {
    var bus = OverlayChannelDelivery.Bus()
    for index in 0..<5 { bus.consume(occurrence(index * 3200, revision: index + 1)) }
    let delivery = try XCTUnwrap(
      try lenaConversation(bus).messages.last?.recipients.first?.deliveryID)
    bus.consume(reply(String(repeating: "d", count: 24), delivery: delivery))
    let restored = try JSONDecoder().decode(
      OverlayChannelDelivery.Bus.self, from: JSONEncoder().encode(bus))
    XCTAssertEqual(restored.conversations(busPath: busPath), bus.conversations(busPath: busPath))
  }

  func testRawObservationsCannotCreateConversationText() throws {
    var bus = OverlayChannelDelivery.Bus()
    var observation = occurrence(0, revision: 1)
    observation["schema"] = "codescribe.raw.v1"
    observation["kind"] = "correction"
    bus.consume(observation)
    XCTAssertTrue(bus.conversations(busPath: busPath).allSatisfy { $0.messages.isEmpty })
  }

  func testHistoryRetainsOnlyTheLatestBoundedRows() throws {
    var bus = OverlayChannelDelivery.Bus()
    for index in 0..<300 {
      bus.consume(
        occurrence(
          index * 3200, revision: index + 1,
          captureSession: "agent-channel-2-history-\(index)"))
    }
    let rows = try lenaConversation(bus).messages
    XCTAssertEqual(rows.count, 256)
    XCTAssertEqual(rows.first?.order, 45)
    XCTAssertEqual(rows.last?.order, 300)
  }

  func testReplyIdentifierCannotMasqueradeAsLeaseIdentifier() throws {
    XCTAssertNil(OverlayConversationOwner(row: owner(String(repeating: "a", count: 24))))
    XCTAssertNotNil(OverlayConversationOwner(row: owner(leaseA)))
  }
}
