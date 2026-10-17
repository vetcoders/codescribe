import AppKit
import ApplicationServices
import Foundation

// Separate-process AX client for the isolated XCTest host. No input events,
// screenshots, real app lookup or global accessibility configuration.
struct Element: Codable {
  let label: String
  let value: String
  let x: Double
  let y: Double
  let width: Double
  let height: Double
}
struct Reception: Codable {
  let trusted: Bool
  let elements: [Element]
  let pressStatus: Int32?
  let trace: [String]
}
func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
  var result: CFTypeRef?
  guard AXUIElementCopyAttributeValue(element, name as CFString, &result) == .success else {
    return nil
  }
  return result
}
struct Request: Codable {
  let pid: Int32
  let identifier: String
  let press: Bool
}
func query(_ request: Request) throws -> Reception {
  let admittedIdentifier = request.identifier.hasPrefix("overlay-conversation-")
    || request.identifier.hasPrefix("overlay-view-conversation-")
  guard request.pid > 0, admittedIdentifier,
    let bundle = NSRunningApplication(processIdentifier: request.pid)?.bundleURL?.path,
    bundle.contains("/Xcode/DerivedData/Codescribe-"), bundle.hasSuffix("/Codescribe.app")
  else { throw CocoaError(.fileReadNoPermission) }
  let application = AXUIElementCreateApplication(request.pid)
  AXUIElementSetMessagingTimeout(application, 2)
  var trace: [String] = []
  var visited = 0
  var matches: [AXUIElement] = []
  func visit(_ element: AXUIElement, depth: Int) {
    guard depth < 24, visited < 10000 else { return }
    visited += 1
    if trace.count < 80 {
      trace.append(
        "depth=\(depth) role=\(attribute(element, kAXRoleAttribute) as? String ?? "nil") id=\(attribute(element, kAXIdentifierAttribute) as? String ?? "nil") label=\((attribute(element, kAXDescriptionAttribute) as? String ?? "").prefix(80))"
      )
    }
    if attribute(element, kAXIdentifierAttribute) as? String == request.identifier {
      matches.append(element)
    }
    let key = depth == 0 ? kAXWindowsAttribute : kAXChildrenAttribute
    for child in attribute(element, key) as? [AXUIElement] ?? [] { visit(child, depth: depth + 1) }
  }
  visit(application, depth: 0)
  let elements = matches.map { element in
    var point = CGPoint.zero
    var size = CGSize.zero
    if let value = attribute(element, kAXPositionAttribute),
      CFGetTypeID(value) == AXValueGetTypeID()
    {
      _ = AXValueGetValue(value as! AXValue, .cgPoint, &point)
    }
    if let value = attribute(element, kAXSizeAttribute), CFGetTypeID(value) == AXValueGetTypeID() {
      _ = AXValueGetValue(value as! AXValue, .cgSize, &size)
    }
    return Element(
      label: attribute(element, kAXTitleAttribute) as? String
        ?? attribute(element, kAXDescriptionAttribute) as? String ?? "",
      value: attribute(element, kAXValueAttribute) as? String ?? "",
      x: point.x, y: point.y, width: size.width, height: size.height)
  }
  var pressStatus: Int32?
  if request.press {
    guard matches.count == 1 else { throw CocoaError(.validationMultipleErrors) }
    pressStatus = AXUIElementPerformAction(matches[0], kAXPressAction as CFString).rawValue
  }
  return Reception(
    trusted: AXIsProcessTrusted(), elements: elements, pressStatus: pressStatus, trace: trace)
}
guard CommandLine.arguments.count == 2 else { exit(2) }
let directory = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try Data((AXIsProcessTrusted() ? "trusted" : "untrusted").utf8)
  .write(to: directory.appendingPathComponent("ready"), options: .atomic)
let deadline = Date().addingTimeInterval(900)
while Date() < deadline {
  for input in try FileManager.default.contentsOfDirectory(
    at: directory, includingPropertiesForKeys: nil)
  where input.lastPathComponent.hasSuffix(".request.json") {
    let request = try JSONDecoder().decode(Request.self, from: Data(contentsOf: input))
    let result = try query(request)
    let output = input.deletingPathExtension().deletingPathExtension().appendingPathExtension(
      "response.json")
    try JSONEncoder().encode(result).write(to: output, options: .atomic)
    try FileManager.default.removeItem(at: input)
  }
  Thread.sleep(forTimeInterval: 0.01)
}
