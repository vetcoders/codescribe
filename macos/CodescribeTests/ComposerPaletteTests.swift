import XCTest

@testable import Codescribe

final class ComposerPaletteTests: XCTestCase {
    // MARK: Parsing — the palette must never hijack ordinary typing

    func testOnlyALeadingSlashActivatesThePalette() {
        XCTAssertEqual(ComposerPaletteQuery.parse(""), .inactive)
        XCTAssertEqual(ComposerPaletteQuery.parse("napisz kod"), .inactive)
        // The cases that would break everyday chat if the palette were greedy:
        XCTAssertEqual(ComposerPaletteQuery.parse("open path/to/file.rs"), .inactive)
        XCTAssertEqual(ComposerPaletteQuery.parse("and/or"), .inactive)
        XCTAssertEqual(ComposerPaletteQuery.parse("zrób to /model"), .inactive)
    }

    func testMultiLineDraftIsProseNotACommand() {
        XCTAssertEqual(ComposerPaletteQuery.parse("/model\nnadal piszę"), .inactive)
    }

    func testBareSlashOffersEveryCommand() {
        guard case .commands(let prefix) = ComposerPaletteQuery.parse("/") else {
            return XCTFail("bare slash must offer commands")
        }
        XCTAssertEqual(prefix, "")
        XCTAssertEqual(
            ComposerPaletteCommand.matching(prefix: prefix).count,
            ComposerPaletteCommand.allCases.count
        )
    }

    func testPartialKeywordNarrowsTheCommandList() {
        guard case .commands(let prefix) = ComposerPaletteQuery.parse("/mo") else {
            return XCTFail("partial keyword must stay in command mode")
        }
        XCTAssertEqual(ComposerPaletteCommand.matching(prefix: prefix), [.model])
        XCTAssertTrue(ComposerPaletteCommand.matching(prefix: "zzz").isEmpty)
    }

    func testCompleteKeywordOpensEntriesWithAndWithoutTrailingSpace() {
        XCTAssertEqual(ComposerPaletteQuery.parse("/model"), .entries(command: .model, filter: ""))
        XCTAssertEqual(ComposerPaletteQuery.parse("/model "), .entries(command: .model, filter: ""))
        XCTAssertEqual(
            ComposerPaletteQuery.parse("/model  gpt "),
            .entries(command: .model, filter: "gpt")
        )
        XCTAssertEqual(ComposerPaletteQuery.parse("/GRANTS"), .entries(command: .grants, filter: ""))
    }

    func testUnknownCommandWithAnArgumentFallsBackToPlainText() {
        // Trapping the user in an empty palette would make the draft unsendable.
        XCTAssertEqual(ComposerPaletteQuery.parse("/nosuch foo"), .inactive)
    }

    // MARK: Filtering + draft rewriting

    func testFilterMatchesTitleOrIdAndKeepsCallerOrdering() {
        let entries = [
            ComposerPaletteEntry(id: "gpt-5.6-sol", title: "GPT-5.6 Sol", subtitle: nil),
            ComposerPaletteEntry(id: "claude-fable-5", title: "Claude Fable 5", subtitle: nil),
            ComposerPaletteEntry(id: "gpt-5.6-luna", title: "GPT-5.6 Luna", subtitle: nil),
        ]
        XCTAssertEqual(ComposerPalette.filter(entries, by: "").map(\.id), entries.map(\.id))
        XCTAssertEqual(
            ComposerPalette.filter(entries, by: "gpt").map(\.id),
            ["gpt-5.6-sol", "gpt-5.6-luna"]
        )
        // Matching on the display title, not just the wire id.
        XCTAssertEqual(ComposerPalette.filter(entries, by: "fable").map(\.id), ["claude-fable-5"])
        XCTAssertTrue(ComposerPalette.filter(entries, by: "nope").isEmpty)
    }

    func testPickingACommandLeavesAKeywordAndSpaceSoEntriesOpen() {
        let draft = ComposerPalette.draft(afterPicking: .model)
        XCTAssertEqual(draft, "/model ")
        XCTAssertEqual(ComposerPaletteQuery.parse(draft), .entries(command: .model, filter: ""))
    }

    func testApplyingAnEntryClearsTheDraft() {
        // Otherwise the command text would be sent to the agent on the next Enter.
        XCTAssertEqual(ComposerPalette.draftAfterApplyingEntry, "")
        XCTAssertEqual(
            ComposerPaletteQuery.parse(ComposerPalette.draftAfterApplyingEntry),
            .inactive
        )
    }
}
