import XCTest
@testable import Codescribe

final class VocabularyABTests: XCTestCase {
  func testURLWhitelistAndLabGate() {
    func sample(_ raw: String, lab: Bool = true) -> UInt32? {
      VocabularyABRequest.sample(from: URL(string: raw)!, labEnabled: lab)
    }
    XCTAssertEqual(sample("codescribe://lab/vocabulary-ab?sample=30"), 30)
    XCTAssertEqual(sample("codescribe://lab/vocabulary-ab?output=/tmp/evil&sample=2"), 2)
    XCTAssertEqual(sample("codescribe://lab/vocabulary-ab"), 30)
    XCTAssertNil(sample("codescribe://lab/vocabulary-ab?sample=30", lab: false))
    for raw in [
      "https://lab/vocabulary-ab", "codescribe://other/vocabulary-ab",
      "codescribe://lab/record", "codescribe://user@lab/vocabulary-ab",
      "codescribe://lab:42/vocabulary-ab", "codescribe://lab/vocabulary-ab#x",
      "codescribe://lab/vocabulary-ab?sample=1", "codescribe://lab/vocabulary-ab?sample=101",
      "codescribe://lab/vocabulary-ab?sample=-2", "codescribe://lab/vocabulary-ab?sample=",
      "codescribe://lab/vocabulary-ab?sample", "codescribe://lab/vocabulary-ab?sample=2&sample=30",
    ] {
      XCTAssertNil(sample(raw), raw)
    }
  }

  @MainActor
  func testOneJobAtATimeAndReopensAfterCompletion() async {
    let channel = AsyncStream<Void>.makeStream()

    let action = VocabularyABAction { sample in
      XCTAssertEqual(sample, 30)

      for await _ in channel.stream { break }
    }
    let url = URL(string: "codescribe://lab/vocabulary-ab?sample=30")!
    XCTAssertFalse(action.receive(url, labEnabled: false))
    XCTAssertTrue(action.receive(url, labEnabled: true))
    XCTAssertFalse(action.receive(url, labEnabled: true))
    channel.continuation.finish()
    await action.job?.value
    XCTAssertTrue(action.receive(url, labEnabled: true))
  }
}
