import Foundation
import XCTest

@testable import Codescribe

/// The lease identity rule shared with Python and Rust
/// (`scripts/tests/fixtures/lease-identity-vectors.json`): every vector gives
/// the same lease id in `AgentPlaybackIdentity` as in `bus-demux.py` and in
/// `core::stt::active_names::follower_lease_id`.
final class LeaseIdentityVectorTests: XCTestCase {
  func testEveryVectorGivesItsLeaseID() throws {
    let root = URL(fileURLWithPath: #filePath)
      .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    let data = try Data(
      contentsOf: root.appendingPathComponent("scripts/tests/fixtures/lease-identity-vectors.json"))
    let fixture = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    let vectors = try XCTUnwrap(fixture["vectors"] as? [[String: String]])
    XCTAssertFalse(vectors.isEmpty)
    for vector in vectors {
      let provider = try XCTUnwrap(vector["provider"])
      let session = try XCTUnwrap(vector["provider_session_id"])
      XCTAssertEqual(
        AgentPlaybackIdentity.leaseIdentifier(provider: provider, session: session),
        vector["lease_id"], vector["case"] ?? "")
    }
  }
}
