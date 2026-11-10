import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class ProviderReadinessAcceptanceTests: XCTestCase {
  /// A stored key or account is presence, not acceptance. Since the colour
  /// rule of 2026-10-26 (D2, carried into Settings by W0-02) a saved
  /// credential wears the olive state colour with the empty circle (`.saved`),
  /// so colour alone no longer separates it from an accepted probe: the sign
  /// does. The saved rows must render the olive circle and never the
  /// verified check; a rejected probe stays free of the state colour. The
  /// accepted probe wears the thin verified check and is the positive control
  /// that keeps an empty render from passing.
  func testStoredCredentialsWearTheSavedSignNotTheAcceptedOne() throws {
    let savedKey = KeyRow(
      account: "FIXTURE_API_KEY", label: "Fixture", isSet: true,
      probeResult: nil, probePending: false,
      onSave: { _ in }, onClear: {}, onTest: {})
    var provider = CsProviderOption.sampleProviders[1]
    provider.accountSignedIn = true
    provider.accountIdentity = "fixture@example.invalid"
    let savedAccount = AccountLoginRow(
      provider: provider, loginPending: false, loginNotice: nil, onStart: {}, onSignOut: {})
    let accepted = KeyProbeChip(result: CsApiKeyProbeResult(
      account: "FIXTURE_API_KEY", status: .ok, message: "fixture accepted", probedEndpoint: nil))
    let blocked = KeyProbeChip(result: CsApiKeyProbeResult(
      account: "FIXTURE_API_KEY", status: .invalid,
      message: "fixture invalid", probedEndpoint: nil))
    XCTAssertGreaterThan(try greenPixels(accepted, name: "accepted-probe"), 10)
    XCTAssertGreaterThan(try greenPixels(savedKey, name: "saved-key"), 0, "the saved circle")
    XCTAssertGreaterThan(
      try greenPixels(savedAccount, name: "saved-account"), 0, "the saved circle")
    XCTAssertLessThanOrEqual(try greenPixels(blocked, name: "blocked-probe"), 2)

    let keyRowsURL = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent()
      .deletingLastPathComponent()
      .appendingPathComponent("Codescribe/Screens/Settings/KeyRows.swift")
    let keyRows = try String(contentsOf: keyRowsURL, encoding: .utf8)
    XCTAssertTrue(
      keyRows.contains("CSStatusBadge(text: String(localized: \"Saved\"), mark: .saved)"),
      "a stored key is the saved mark")
    XCTAssertTrue(
      keyRows.contains("CSStatusBadge(text: Self.status(for: provider), mark: .saved)"),
      "a stored account is the saved mark")
    XCTAssertFalse(
      keyRows.contains("mark: .verified"), "no stored credential wears the verified check")
    // The check belongs to the probe that the provider accepted, and to
    // nothing else in the key rows; a refused key blocks, an untested one is
    // saved.
    XCTAssertTrue(keyRows.contains("case .ok: return .verified"), "an accepted probe is verified")
    XCTAssertEqual(
      keyRows.components(separatedBy: ".verified").count - 1, 1,
      "the accepted probe is the only verified sign in the key rows")
    XCTAssertTrue(keyRows.contains("case .invalid, .noQuota: return .blocked"))
    XCTAssertTrue(keyRows.contains("case .unsupported: return .saved"))
    XCTAssertTrue(keyRows.contains("case .rejected, .network, .missing: return .none"))
  }

  private func greenPixels<V: View>(_ view: V, name: String) throws -> Int {
    let host = NSHostingView(rootView: view.padding(16)
      .frame(width: 640, height: 140).background(Color.black).environment(\.colorScheme, .dark))
    host.appearance = NSAppearance(named: .darkAqua)
    host.frame = NSRect(x: 0, y: 0, width: 640, height: 140)
    host.layoutSubtreeIfNeeded()
    let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
    host.cacheDisplay(in: host.bounds, to: bitmap)
    let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
    XCTAssertGreaterThan(png.count, 500, "Native fixture must contain rendered content")
    let attachment = XCTAttachment(data: png, uniformTypeIdentifier: "public.png")
    attachment.name = name
    attachment.lifetime = .keepAlways
    add(attachment)
    var resolved: NSColor?
    host.appearance?.performAsCurrentDrawingAppearance {
      resolved = CSPalette.secondaryInk.usingColorSpace(.sRGB)
    }
    let secondary = try XCTUnwrap(resolved)
    let targetR = secondary.redComponent * secondary.alphaComponent
    let targetG = secondary.greenComponent * secondary.alphaComponent
    let targetB = secondary.blueComponent * secondary.alphaComponent
    var count = 0
    for y in 0..<bitmap.pixelsHigh {
      for x in 0..<bitmap.pixelsWide {
        guard let c = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
        if abs(c.redComponent - targetR) < 0.04,
          abs(c.greenComponent - targetG) < 0.04,
          abs(c.blueComponent - targetB) < 0.04 { count += 1 }
      }
    }
    return count
  }
}
