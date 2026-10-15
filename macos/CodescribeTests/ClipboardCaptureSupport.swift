import AppKit
import XCTest

@testable import Codescribe

/// Every user copy crosses the Rust copy authority (`UserTextClipboard`), which
/// the test host neither runs nor may let near the user's real clipboard.
/// Tests route the bridge write into a private pasteboard instead and keep
/// asserting the payload exactly as before.
@MainActor
enum ClipboardCapture {
  /// Routes copies into `board` until the returned closure restores the bridge.
  static func route(into board: NSPasteboard) -> () -> Void {
    let previous = UserTextClipboard.bridgeWrite
    UserTextClipboard.bridgeWrite = { text in
      board.clearContents()
      board.setString(text, forType: .string)
    }
    return { UserTextClipboard.bridgeWrite = previous }
  }
}

extension XCTestCase {
  /// A copy is a main-actor task; let it settle before reading the capture.
  /// `nil` means the view had nothing copyable, which these tests treat as a failure.
  @MainActor
  func settle(
    _ copy: Task<Void, Never>?, file: StaticString = #filePath, line: UInt = #line
  ) async throws {
    let copy = try XCTUnwrap(copy, "the selection must be copyable", file: file, line: line)
    await copy.value
  }
}

/// Records what `OverlayState.recoveryClipboardWriter` was asked to write and
/// answers with a settable outcome. A reference, so the writer closure can
/// observe a later change of mind (the retry after a refused write).
@MainActor
final class RecoveryWriteRecorder {
  var texts: [String] = []
  var succeeds: Bool
  init(succeeds: Bool = true) { self.succeeds = succeeds }
  func writer(_ text: String) async -> Bool {
    texts.append(text)
    return succeeds
  }
}
