import XCTest

@testable import Codescribe

/// Status words and repair notes of the permission rows (smoke review
/// 2026-11-09). One owner, `PermissionKind`, serves Setup and Settings, so the
/// rules are checked there rather than in either view.
final class PermissionPresentationTests: XCTestCase {
  private let en = Locale(identifier: "en")
  private let pl = Locale(identifier: "pl")

  /// A probe that only proves missing effective access never says the person
  /// refused; a scope whose API records a decision keeps "Denied".
  func testDeniedReadsAsNoAccessOnlyWhereTheProbeCannotSeeADecision() {
    for kind in [PermissionKind.accessibility, .screenRecording, .fullDiskAccess] {
      XCTAssertEqual(kind.statusLabel(for: .denied, locale: en), "No access", "\(kind)")
      XCTAssertEqual(kind.statusLabel(for: .denied, locale: pl), "Brak dostępu", "\(kind)")
    }
    for kind in [PermissionKind.microphone, .speechRecognition, .inputMonitoring, .spotifyAutomation]
    {
      XCTAssertEqual(kind.statusLabel(for: .denied, locale: en), "Denied", "\(kind)")
      XCTAssertEqual(kind.statusLabel(for: .denied, locale: pl), "Odmówiono", "\(kind)")
    }
    for kind in PermissionKind.allCases {
      XCTAssertEqual(
        kind.statusLabel(for: .granted, locale: en), PermissionState.granted.label(locale: en))
      XCTAssertEqual(
        kind.statusLabel(for: .notDetermined, locale: en),
        PermissionState.notDetermined.label(locale: en))
    }
    XCTAssertEqual(PermissionState.denied.label(locale: en), "Denied", "the state itself is kept")
  }

  /// Only a required refusal macOS will not ask about again is a problem.
  /// The reopen hint and the Spotify guidance are information.
  func testOnlyAFinalRequiredRefusalIsAProblem() {
    XCTAssertEqual(
      PermissionKind.microphone.repairNoteKind(for: .denied, requested: false), .settingsOnly)
    XCTAssertTrue(PermissionKind.microphone.repairNoteIsProblem(for: .denied, requested: false))
    XCTAssertTrue(
      PermissionKind.speechRecognition.repairNoteIsProblem(for: .denied, requested: true))

    for kind in [PermissionKind.accessibility, .inputMonitoring, .screenRecording] {
      XCTAssertEqual(kind.repairNoteKind(for: .denied, requested: true), .mayNeedReopen, "\(kind)")
      XCTAssertFalse(kind.repairNoteIsProblem(for: .denied, requested: true), "\(kind)")
      XCTAssertNil(kind.repairNoteKind(for: .denied, requested: false), "\(kind) unrequested")
    }
    XCTAssertEqual(
      PermissionKind.spotifyAutomation.repairNoteKind(for: .denied, requested: false),
      .spotifySetup)
    XCTAssertFalse(
      PermissionKind.spotifyAutomation.repairNoteIsProblem(for: .denied, requested: false))
    for kind in PermissionKind.allCases {
      XCTAssertNil(kind.repairNoteKind(for: .granted, requested: true), "\(kind) granted")
      XCTAssertNil(kind.repairNote(for: .granted, requested: true, locale: en), "\(kind)")
    }
  }

  /// The reopen hint says what is known and what may help, in both languages,
  /// and promises nothing.
  func testReopenHintStatesMissingAccessWithoutPromisingARestartFix() throws {
    let english = try XCTUnwrap(
      PermissionKind.accessibility.repairNote(for: .denied, requested: true, locale: en))
    XCTAssertTrue(english.hasPrefix("Codescribe doesn’t have access yet."), english)
    XCTAssertTrue(english.contains("may"), english)
    XCTAssertFalse(english.hasPrefix("Already turned on?"), english)

    let polish = try XCTUnwrap(
      PermissionKind.accessibility.repairNote(for: .denied, requested: true, locale: pl))
    XCTAssertNotEqual(polish, english, "the hint has a Polish translation")
    XCTAssertTrue(polish.contains("może"), polish)
  }
}
