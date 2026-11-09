import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

/// Review evidence for the Agent context surface: the production Agent window
/// (`AgentWindowChrome.makeWindow`, header accessory included) rendered in
/// Light and Dark, at the window floor and the standard width, with the rail
/// expanded and collapsed, context closed and open (16 cells).
///
/// Runs only on request: without CODESCRIBE_VISUAL_EVIDENCE_DIR the test skips,
/// because 16 settled windows exceed the suite's per-test budget.
///
/// Two outputs per cell:
/// - Offscreen bitmap (always): drawn by AppKit from the frame view, not by the
///   compositor. It proves layout, proportions, typography and ink, not Liquid
///   Glass, vibrancy or Dark-mode glass.
/// - Live capture (when CODESCRIBE_EVIDENCE_LIVE_DIR is set): the window is
///   ordered front and `scripts/agent-window-capture.sh` (a separate process
///   holding Screen Recording) takes `screencapture -l` of it. This is the
///   compositor truth. The screen must be unlocked.
///
/// Recipe (the TEST_RUNNER_ prefix forwards a variable to the test process):
///   scripts/agent-window-capture.sh /path/live &
///   TEST_RUNNER_CODESCRIBE_VISUAL_EVIDENCE_DIR=/path/offscreen \
///   TEST_RUNNER_CODESCRIBE_EVIDENCE_LIVE_DIR=/path/live \
///   make test-swift SWIFT_TEST_MAX_TEST_SECONDS=300 \
///     SWIFT_TEST_ARGS='-only-testing:CodescribeTests/AgentContextEvidenceTests'
@MainActor
final class AgentContextEvidenceTests: XCTestCase {
  private let sidebarKey = "AgentChat.sidebarExpanded.v1"

  func testAgentContextEvidenceSheet() throws {
    let directory = try evidenceDirectory()
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    let previousSidebar = UserDefaults.standard.object(forKey: sidebarKey)
    defer {
      restore(previousSidebar, forKey: sidebarKey)
    }

    var written: [String] = []
    for scheme in ["light", "dark"] {
      for (widthName, width) in [("floor", AgentWindowMetrics.minWidth), ("standard", 1120)] {
        for expanded in [true, false] {
          for open in [false, true] {
            UserDefaults.standard.set(expanded, forKey: sidebarKey)
            let name =
              "agent-\(scheme)-\(widthName)-\(expanded ? "rail" : "norail")-\(open ? "open" : "closed").png"
            try render(
              scheme: scheme, contentWidth: width, open: open,
              to: directory.appendingPathComponent(name))
            written.append(name)
          }
        }
      }
    }
    print("agent-context-evidence: \(directory.path) \(written.count) files")
  }

  private func render(scheme: String, contentWidth: CGFloat, open: Bool, to url: URL) throws {
    let appearance = NSAppearance(named: scheme == "dark" ? .darkAqua : .aqua)!
    let colorScheme: ColorScheme = scheme == "dark" ? .dark : .light
    let card = AgentContextCardPresenter()
    let hosting = NSHostingController(
      rootView: AgentChatView(store: fixtureStore(), contextCard: card)
        .environment(\.csReduceTransparencyOverride, false)
        .environment(\.csContrastOverride, .standard)
        .environment(\.colorScheme, colorScheme)
        .preferredColorScheme(colorScheme)
        .transaction { $0.disablesAnimations = true }
    )
    // Resolve the appearance before SwiftUI mounts: dynamic NSColor and the
    // header accessory read it from the view tree, not from the environment.
    hosting.view.appearance = appearance
    let window = AgentWindowChrome.makeWindow(content: hosting)
    window.appearance = appearance
    window.setContentSize(NSSize(width: contentWidth, height: 620))
    window.setFrameOrigin(NSPoint(x: 80, y: 80))
    BackgroundTestWindow.present(window)
    defer {
      window.orderOut(nil)
      window.close()
    }
    settle(window)
    if open {
      card.open()
      settle(window)
    }
    if let live = ProcessInfo.processInfo.environment["CODESCRIBE_EVIDENCE_LIVE_DIR"] {
      try captureLive(window, name: url.lastPathComponent, in: URL(fileURLWithPath: live))
    }
    let frameView = try XCTUnwrap(window.contentView?.superview, "no window frame view")
    frameView.layoutSubtreeIfNeeded()
    let bitmap = try XCTUnwrap(
      frameView.bitmapImageRepForCachingDisplay(in: frameView.bounds), "no bitmap")
    frameView.cacheDisplay(in: frameView.bounds, to: bitmap)
    let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]), "no png")
    try png.write(to: url)
  }

  /// A conversation with enough turns to show the reading column, a stored
  /// update time and no queued turns. The model is set to prove the card
  /// does not show it.
  private func fixtureStore() -> AgentChatStore {
    let stamp = Date(timeIntervalSince1970: 1_794_200_000)
    let messages = [
      ChatMessage(
        role: .you, timestamp: stamp,
        text: "Przygotuj krótki plan wdrożenia nowego panelu kontekstu."),
      ChatMessage(
        role: .assistant, timestamp: stamp,
        text:
          "Plan ma trzy kroki: zastąpić kolumnę kartą otwieraną z nagłówka, oddać rozmowie odzyskaną szerokość i sprawdzić układ w jasnym i ciemnym motywie, przy minimalnej i standardowej szerokości okna."
      ),
      ChatMessage(role: .you, timestamp: stamp, text: "Co z dostępnością?"),
      ChatMessage(
        role: .assistant, timestamp: stamp,
        text:
          "Karta zamyka się Escape i kliknięciem obok, a focus wraca tam, gdzie był. Fonty pozostają w skali systemu."
      ),
    ]
    var thread = ChatThread(
      title: "Redesign panelu kontekstu", meta: "", isRestored: false, messagesLoaded: true,
      messages: messages)
    thread.model = "claude-opus-5-5"
    thread.updatedAt = stamp
    return AgentChatStore(engine: nil, threads: [thread])
  }

  /// Compositor evidence: show the window without activating the app, ask an
  /// outside capturer (which holds Screen Recording) for `screencapture -l`,
  /// and wait for its file. The request file carries "<windowNumber> <name>";
  /// `scripts/agent-window-capture.sh` serves it.
  private func captureLive(_ window: NSWindow, name: String, in directory: URL) throws {
    window.alphaValue = 1
    window.ignoresMouseEvents = false
    window.orderFrontRegardless()
    let deadline = Date().addingTimeInterval(1.5)
    while Date() < deadline {
      RunLoop.current.run(until: Date().addingTimeInterval(0.05))
      window.displayIfNeeded()
    }
    let result = directory.appendingPathComponent(name)
    try "\(window.windowNumber) \(name)\n".write(
      to: directory.appendingPathComponent("request"), atomically: true, encoding: .utf8)
    let wait = Date().addingTimeInterval(20)
    while Date() < wait, !FileManager.default.fileExists(atPath: result.path) {
      RunLoop.current.run(until: Date().addingTimeInterval(0.1))
    }
    XCTAssertTrue(FileManager.default.fileExists(atPath: result.path), "no live capture \(name)")
  }

  private func settle(_ window: NSWindow) {
    let deadline = Date().addingTimeInterval(1.2)
    while Date() < deadline {
      RunLoop.current.run(until: Date().addingTimeInterval(0.03))
      window.contentView?.layoutSubtreeIfNeeded()
      window.displayIfNeeded()
    }
  }

  private func evidenceDirectory() throws -> URL {
    guard let path = ProcessInfo.processInfo.environment["CODESCRIBE_VISUAL_EVIDENCE_DIR"],
      !path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    else { throw XCTSkip("Agent context evidence runs only on request") }
    return URL(fileURLWithPath: path, isDirectory: true)
  }

  private func restore(_ previous: Any?, forKey key: String) {
    if let previous {
      UserDefaults.standard.set(previous, forKey: key)
    } else {
      UserDefaults.standard.removeObject(forKey: key)
    }
  }
}
