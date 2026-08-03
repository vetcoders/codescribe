import XCTest
@testable import Codescribe

final class ChatLayoutPolicyTests: XCTestCase {
    func testContentWidthSubtractsListPadding() {
        XCTAssertEqual(ChatLayoutPolicy.contentWidth(for: 800), 760)
        XCTAssertEqual(ChatLayoutPolicy.contentWidth(for: 0), 0)
    }

    func testYouBubbleTracksContainerWithoutFixed760Cap() {
        // Narrow: floor at minimumReadable.
        XCTAssertEqual(
            ChatLayoutPolicy.youBubbleMaxWidth(containerWidth: 320, mode: .wide),
            ChatLayoutPolicy.minimumReadable
        )

        // Mid column (~960): You stays chat-style (~72% of usable), which can be
        // *narrower* than the old fixed 760 cap — that is intentional.
        let mid = ChatLayoutPolicy.youBubbleMaxWidth(containerWidth: 960, mode: .wide)
        let usableMid = ChatLayoutPolicy.contentWidth(for: 960)
        XCTAssertEqual(mid, usableMid * ChatWidthMode.wide.youFraction, accuracy: 0.5)
        XCTAssertLessThan(mid, usableMid)

        // Wider viewport: proportional You exceeds the old 760 hard cap.
        let expanded = ChatLayoutPolicy.youBubbleMaxWidth(containerWidth: 1200, mode: .wide)
        XCTAssertGreaterThan(expanded, 760)

        // Ultrawide still never exceeds usable column.
        let wide = ChatLayoutPolicy.youBubbleMaxWidth(containerWidth: 1800, mode: .wide)
        XCTAssertLessThanOrEqual(wide, ChatLayoutPolicy.contentWidth(for: 1800))
    }

    func testLeadingColumnFillsUsableWidthUntilProseCap() {
        let laptop = ChatLayoutPolicy.leadingColumnMaxWidth(containerWidth: 960, mode: .wide)
        XCTAssertEqual(laptop, ChatLayoutPolicy.contentWidth(for: 960), accuracy: 0.5)
        XCTAssertGreaterThan(laptop, 900) // old assistant hard cap

        let ultrawide = ChatLayoutPolicy.leadingColumnMaxWidth(containerWidth: 2200, mode: .wide)
        XCTAssertEqual(ultrawide, ChatWidthMode.wide.proseComfortCap)
    }

    func testWidthModesOrderComfortableThenWideThenFull() {
        let container: CGFloat = 1400
        let comfortable = ChatLayoutPolicy.leadingColumnMaxWidth(
            containerWidth: container,
            mode: .comfortable
        )
        let wide = ChatLayoutPolicy.leadingColumnMaxWidth(containerWidth: container, mode: .wide)
        let full = ChatLayoutPolicy.leadingColumnMaxWidth(containerWidth: container, mode: .full)
        XCTAssertLessThan(comfortable, wide)
        XCTAssertLessThan(wide, full)
        XCTAssertEqual(full, ChatLayoutPolicy.contentWidth(for: container), accuracy: 0.5)
    }

    func testYouBubbleModeFractionsScale() {
        let container: CGFloat = 1200
        let c = ChatLayoutPolicy.youBubbleMaxWidth(containerWidth: container, mode: .comfortable)
        let w = ChatLayoutPolicy.youBubbleMaxWidth(containerWidth: container, mode: .wide)
        let f = ChatLayoutPolicy.youBubbleMaxWidth(containerWidth: container, mode: .full)
        XCTAssertLessThan(c, w)
        XCTAssertLessThan(w, f)
    }

    func testResolveUnknownWidthModeFallsBackToWide() {
        XCTAssertEqual(ChatWidthMode.resolve("nope"), .wide)
        XCTAssertEqual(ChatWidthMode.resolve(ChatWidthMode.full.rawValue), .full)
    }

    func testDefaultsKeyIsStableForAppStorage() {
        XCTAssertEqual(ChatLayoutPolicy.defaultsKey, "codescribe.chatWidthMode")
    }

    func testAttachmentMissingPathDoesNotClaimLocalFile() {
        let missing = MessageAttachment(name: "scan.png", url: nil, type: "image/png")
        XCTAssertNil(missing.url)
        XCTAssertEqual(missing.name, "scan.png")
        XCTAssertEqual(missing.type, "image/png")
    }

    func testAttachmentWithPathKeepsMetadataForPreview() {
        let url = URL(fileURLWithPath: "/tmp/codescribe-preview-fixture.png")
        let att = MessageAttachment(name: "codescribe-preview-fixture.png", url: url)
        XCTAssertEqual(att.url, url)
        XCTAssertEqual(att.type, "image/png")
    }
}
