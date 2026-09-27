import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class OverlayTranscriptLayoutTests: XCTestCase {
  func testLongRefusalAndRevealedActionsFitWithoutCoveringTheTranscript() throws {
    for width in [280.0, 430.0] {
      for height in [150.0, 390.0] {
        let frames = Frames()
        let host = NSHostingView(
          rootView: VStack(spacing: 0) {
            OverlayTranscriptLayout {
              Color.clear
                .background(Probe(name: "transcript", frames: frames))
            } status: {
              VStack(spacing: 0) {
                Color.clear.frame(height: 1)
                  .background(Probe(name: "status-start", frames: frames))
                Text(String(repeating: "No seal was recorded for this take. ", count: 12))
                  .font(.system(size: 15))
                  .fixedSize(horizontal: false, vertical: true)
              }
            }
            .background(Probe(name: "body", frames: frames))
            OverlayIntentRail(
              phase: "Unsealed transcript",
              intents: [.copy, .retranscribe, .format, .sendToAgent],
              palette: .dark,
              footerEngineLabel: "Apple · local Whisper HQ format failed",
              historyAvailable: true,
              onIntent: { _ in }
            )
            .background(Probe(name: "actions", frames: frames))
          }
          .frame(width: width, height: height)
        )
        host.frame = CGRect(x: 0, y: 0, width: width, height: height)
        settle(host)
        let transcript = try XCTUnwrap(frames.values["transcript"])
        let body = try XCTUnwrap(frames.values["body"])
        let status = try XCTUnwrap(frames.values["status-start"])
        let actions = try XCTUnwrap(frames.values["actions"])
        XCTAssertGreaterThan(transcript.height, 8, "Transcript must remain reachable")
        XCTAssertGreaterThan(actions.height, 40, "The complete rail must be visible")
        XCTAssertLessThanOrEqual(transcript.maxY, status.minY + 0.5)
        XCTAssertLessThanOrEqual(body.maxY, actions.minY + 0.5)
        XCTAssertLessThan(status.minY, actions.minY)
        XCTAssertLessThanOrEqual(actions.maxY, height + 0.5)
        XCTAssertLessThanOrEqual(actions.width, width + 0.5)
      }
    }
  }

  func testRemovingStatusReturnsSpaceToTheTranscript() throws {
    let frames = Frames()
    let state = StatusState()
    let host = NSHostingView(
      rootView: TestBody(state: state, frames: frames).frame(width: 280, height: 200))
    host.frame = CGRect(x: 0, y: 0, width: 280, height: 200)
    settle(host)
    let withStatus = try XCTUnwrap(frames.values["transcript"])
    state.showStatus = false
    settle(host)
    let withoutStatus = try XCTUnwrap(frames.values["transcript"])
    XCTAssertGreaterThan(withoutStatus.height, withStatus.height + 20)
  }

  private func settle(_ host: NSView) {
    host.layoutSubtreeIfNeeded()
    RunLoop.main.run(until: Date().addingTimeInterval(0.06))
    host.layoutSubtreeIfNeeded()
  }

  private final class Frames {
    var values: [String: CGRect] = [:]
  }

  private struct Probe: View {
    let name: String
    let frames: Frames

    var body: some View {
      Color.clear.onGeometryChange(for: CGRect.self) {
        $0.frame(in: .global)
      } action: {
        frames.values[name] = $0
      }
    }
  }

  @Observable final class StatusState {
    var showStatus = true
  }

  private struct TestBody: View {
    let state: StatusState
    let frames: Frames

    var body: some View {
      OverlayTranscriptLayout {
        Color.clear.background(Probe(name: "transcript", frames: frames))
      } status: {
        if state.showStatus {
          Text("Incomplete coverage — these words were kept, not sealed")
            .font(.system(size: 15))
        }
      }
    }
  }
}
