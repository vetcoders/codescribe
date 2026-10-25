import AppKit
import SwiftUI
import XCTest

@testable import Codescribe

@MainActor
final class ProviderReadinessAcceptanceTests: XCTestCase {
  /// A positive control prevents an empty render from passing the absence checks.
  func testStoredCredentialsRemainNeutralWhileAnAcceptedProbeIsGreen() throws {
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
    let rejected = KeyProbeChip(result: CsApiKeyProbeResult(
      account: "FIXTURE_API_KEY", status: .rejected,
      message: "fixture rejected", probedEndpoint: nil))
    XCTAssertGreaterThan(try greenPixels(accepted, name: "accepted-probe"), 10)
    XCTAssertLessThanOrEqual(try greenPixels(savedKey, name: "saved-key"), 2)
    XCTAssertLessThanOrEqual(try greenPixels(savedAccount, name: "saved-account"), 2)
    XCTAssertLessThanOrEqual(try greenPixels(rejected, name: "rejected-probe"), 2)
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
    let green = try XCTUnwrap(NSColor(CSColor.oliveLight).usingColorSpace(.sRGB))
    var count = 0
    for y in 0..<bitmap.pixelsHigh {
      for x in 0..<bitmap.pixelsWide {
        guard let c = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
        if abs(c.redComponent - green.redComponent) < 0.04,
          abs(c.greenComponent - green.greenComponent) < 0.04,
          abs(c.blueComponent - green.blueComponent) < 0.04 { count += 1 }
      }
    }
    return count
  }
}
