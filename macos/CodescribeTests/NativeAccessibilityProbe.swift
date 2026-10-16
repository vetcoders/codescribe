import AppKit
import Foundation
import XCTest

/// AX requests must cross a process boundary while the test host services its
/// main run loop. Querying our own SwiftUI AX graph can yield an empty tree or
/// deadlock; moving that self-query to another thread is not AppKit-safe.
@MainActor
enum NativeAccessibilityProbe {
  struct Element: Decodable {
    let label: String
    let value: String
    let x: Double
    let y: Double
    let width: Double
    let height: Double
    var frame: NSRect { NSRect(x: x, y: y, width: width, height: height) }
  }
  struct Reception: Decodable {
    let trusted: Bool
    let elements: [Element]
    let pressStatus: Int32?
    let trace: [String]
  }
  static func query(_ identifier: String, press: Bool = false) throws -> Reception {
    let path = try XCTUnwrap(
      ProcessInfo.processInfo.environment["CODESCRIBE_AX_PROBE"],
      "Run through make test-swift to build the independent AX client")
    struct Request: Encodable {
      let pid: Int32
      let identifier: String
      let press: Bool
    }
    let root = URL(fileURLWithPath: path, isDirectory: true)
    let id = UUID().uuidString
    let input = root.appendingPathComponent(id + ".request.json")
    let output = root.appendingPathComponent(id + ".response.json")
    let request = Request(
      pid: ProcessInfo.processInfo.processIdentifier, identifier: identifier, press: press)
    try JSONEncoder().encode(request).write(to: input, options: .atomic)
    defer { try? FileManager.default.removeItem(at: output) }
    let deadline = Date().addingTimeInterval(10)
    while !FileManager.default.fileExists(atPath: output.path) && Date() < deadline {
      RunLoop.main.run(until: Date().addingTimeInterval(0.005))
    }
    guard FileManager.default.fileExists(atPath: output.path) else {
      XCTFail("Independent AX probe exceeded its bounded deadline; see test-run AX log")
      throw CocoaError(.fileReadNoSuchFile)
    }
    let value = try JSONDecoder().decode(Reception.self, from: Data(contentsOf: output))
    XCTAssertTrue(value.trusted, "Independent AX client requires existing accessibility permission")
    return value
  }
}
