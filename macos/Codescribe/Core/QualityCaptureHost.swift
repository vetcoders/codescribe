import Foundation

/// Are we running inside the XCTest host rather than the product?
///
/// The Swift suite runs *inside the app bundle*, so anything that reaches the
/// filesystem through a free FFI function — rather than through an injected
/// collaborator a test can mock — writes to the operator's REAL data directory.
/// That is not theoretical: `OverlayState.captureQualityIfEdited` calls the free
/// `commitOverlayQualityRecord`, and every `make test-swift` run appended two
/// synthetic corrections to `~/.codescribe/quality/corrections.jsonl`. By the
/// time it was caught, 276 of 501 rows in the live store were test residue and
/// they were rendering in Settings › Dictionary as the user's own corrections.
///
/// `core/config/keychain.rs::in_xctest_host` is the Rust half of the same idea
/// (it keys on the XCTest environment markers, pinned by a test). This is the
/// Swift half: one definition, so a new write path has an obvious thing to ask.
///
/// Fails CLOSED for the user's data: when the marker is absent we assume
/// production and write, which is the safe direction for a real dictation.
enum QualityCaptureHost {
    /// `XCTestCase` only exists in the process once the test bundle is loaded.
    static let isRunningTests = NSClassFromString("XCTestCase") != nil
}
