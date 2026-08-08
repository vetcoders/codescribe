import Combine
import XCTest
@testable import Codescribe

// Executed by `make test-swift`. The composer ReplaceRange assertions below run
// for real — 11 tests in this file, ~0.01 s. See CodescribeTests/README.md.

@MainActor
final class ComposerMicTests: XCTestCase {
    func testEveryStateKeepsTheMicrophoneGlyph() {
        for state in ComposerMicVisualState.allCases {
            guard case .mic = state.icon else {
                return XCTFail("\(state) lost the microphone glyph")
            }
        }
    }

    func testStateAccessibilityLabelsAreExplicit() {
        XCTAssertEqual(ComposerMicVisualState.idle.accessibilityLabel, "Start voice input")
        XCTAssertEqual(ComposerMicVisualState.preparing.accessibilityLabel, "Preparing voice input")
        XCTAssertEqual(ComposerMicVisualState.recording.accessibilityLabel, "Stop voice input")
        XCTAssertEqual(
            ComposerMicVisualState.blocked.accessibilityLabel,
            "Microphone busy with shortcut dictation"
        )
        XCTAssertEqual(ComposerAccessibility.micIdentifier, "agent-composer-mic")
    }

    func testOnlyIdleAndRecordingAreActionable() {
        XCTAssertTrue(ComposerMicVisualState.idle.isEnabled)
        XCTAssertTrue(ComposerMicVisualState.recording.isEnabled)
        XCTAssertFalse(ComposerMicVisualState.preparing.isEnabled)
        XCTAssertFalse(ComposerMicVisualState.blocked.isEnabled)
    }

    func testFinalPassRegressionKeepsLongerLiveTranscriptAndPreservesAlternative() {
        let store = AgentChatStore()
        store.beginDictationPreviewSession()
        store.updateDictationPreview("one two three four five six seven eight nine ten")

        let result = store.resolveDictationDelivery(final: "one two three", autoSend: true)

        XCTAssertEqual(result.text, "one two three four five six seven eight nine ten")
        XCTAssertTrue(result.autoSend)
        XCTAssertEqual(store.dictationFinalPreview, "one two three")
        XCTAssertTrue(store.dictationFinalChangedText)
        XCTAssertEqual(store.dictationDeliverySource, .live)
    }

    func testUserEditedPreviewWinsAndCancelsAssistiveAutoSend() {
        let store = AgentChatStore()
        store.beginDictationPreviewSession()
        store.updateDictationPreview("live machine text")
        store.editDictationPreview("human owned text")
        store.updateDictationPreview("later callback must not overwrite the edit")

        let result = store.resolveDictationDelivery(final: "final machine text", autoSend: true)

        XCTAssertEqual(result.text, "human owned text")
        XCTAssertFalse(result.autoSend)
        XCTAssertTrue(store.dictationPreviewUserEdited)
        XCTAssertEqual(store.dictationDeliverySource, .edited)
    }

    /// Guards the cursor-storm regression (2026-08-05): Apple live re-delivers the
    /// SAME partial ~25×/s during a pause. Each republish rebuilt the Agent window
    /// body and re-ran AppKit's deep `cursorUpdate:` walk, burning ~30% of the main
    /// thread and flickering the pointer between I-beam and arrow. Unchanged input
    /// must therefore emit ZERO `objectWillChange`.
    func testRepeatedIdenticalPartialsPublishNoChange() {
        let store = AgentChatStore()
        store.beginDictationPreviewSession()

        var publishes = 0
        let token = store.objectWillChange.sink { _ in publishes += 1 }
        defer { token.cancel() }

        // Assert on DELTAS, not absolute counts: one partial legitimately touches
        // more than one @Published property (live buffer + preview buffer).
        store.updateDictationPreview("mamy licencję")
        let afterFirst = publishes
        XCTAssertGreaterThan(afterFirst, 0, "first partial must publish")

        for _ in 0..<25 { store.updateDictationPreview("mamy licencję") }
        XCTAssertEqual(publishes, afterFirst, "25 identical partials must publish nothing")

        store.updateDictationPreview("mamy licencję i fajny format")
        XCTAssertGreaterThan(publishes, afterFirst, "a changed partial must publish")

        let afterChange = publishes
        for _ in 0..<25 { store.updateDictationPreview("mamy licencję i fajny format") }
        XCTAssertEqual(publishes, afterChange, "repeats of the new text must publish nothing")
    }

    func testRepeatedVadFlagsPublishNoChange() {
        let store = AgentChatStore()
        store.beginDictationPreviewSession()

        var publishes = 0
        let token = store.objectWillChange.sink { _ in publishes += 1 }
        defer { token.cancel() }

        for _ in 0..<10 { store.setDictationVadActive(false) }
        XCTAssertEqual(publishes, 0, "unchanged VAD flag must not republish")

        store.setDictationVadActive(true)
        XCTAssertEqual(publishes, 1, "VAD transition must publish once")
    }

    // ── Composer × layered transcription ────────────────────────────────────
    //
    // The composer's live preview is assembled Swift-side by
    // `ComposerDictationListener` from `onFinal`/`onPreview`. Layer 1 corrects
    // sealed utterances afterwards via `onReplaceRange`, so a listener that
    // ignores those events shows the user text the engine has already
    // retracted — split-brain against the overlay, which does apply them.

    /// Drain queued main-actor hops until the store settles on `expected`.
    /// `publishPreview` hands off through `Task { @MainActor … }`, so the
    /// update is one scheduling hop away from the call that caused it.
    private func awaitDictationPreview(
        _ store: AgentChatStore,
        equals expected: String,
        file: StaticString = #filePath,
        line: UInt = #line
    ) async {
        for _ in 0..<500 {
            if store.dictationPreview == expected { return }
            await Task.yield()
        }
        XCTAssertEqual(store.dictationPreview, expected, file: file, line: line)
    }

    func testTailPatchCorrectsTheComposerLivePreviewInPlace() async {
        let store = AgentChatStore()
        store.beginDictationPreviewSession()
        let listener = ComposerDictationListener(store: store, onError: { _ in })

        listener.onFinal(
            utteranceId: 1, text: "korzystając z Tulczajn 2024",
            avgLogprob: nil, speechPct: nil, confidenceFlags: []
        )
        await awaitDictationPreview(store, equals: "korzystając z Tulczajn 2024")

        listener.onReplaceRange(
            utteranceId: 1, start: 14, end: 22, text: "Toolchain", source: .tailPatch
        )
        await awaitDictationPreview(store, equals: "korzystając z Toolchain 2024")
    }

    func testComposerTailPatchTargetsOnlyItsOwnUtterance() async {
        let store = AgentChatStore()
        store.beginDictationPreviewSession()
        let listener = ComposerDictationListener(store: store, onError: { _ in })

        listener.onFinal(
            utteranceId: 1, text: "pierwsze zdanie",
            avgLogprob: nil, speechPct: nil, confidenceFlags: []
        )
        listener.onFinal(
            utteranceId: 2, text: "drugie zdanie",
            avgLogprob: nil, speechPct: nil, confidenceFlags: []
        )
        listener.onReplaceRange(
            utteranceId: 2, start: 0, end: 6, text: "trzecie", source: .tailPatch
        )

        await awaitDictationPreview(store, equals: "pierwsze zdanie trzecie zdanie")
    }

    /// An unbound or overrunning window is dropped, never half-applied — the
    /// same rule the overlay and the Rust committed buffer follow.
    func testComposerDropsUnboundOrOutOfRangePatches() async {
        let store = AgentChatStore()
        store.beginDictationPreviewSession()
        let listener = ComposerDictationListener(store: store, onError: { _ in })

        listener.onFinal(
            utteranceId: 1, text: "krótkie",
            avgLogprob: nil, speechPct: nil, confidenceFlags: []
        )
        listener.onReplaceRange(utteranceId: 9, start: 0, end: 3, text: "X", source: .tailPatch)
        listener.onReplaceRange(utteranceId: 1, start: 0, end: 999, text: "X", source: .tailPatch)

        await awaitDictationPreview(store, equals: "krótkie")
    }

    func testPendingAttachmentPreviewUsesExactStagedURL() {
        let url = URL(fileURLWithPath: "/tmp/exact-staged-preview.png")
        let pending = PendingAttachment(url: url)

        XCTAssertEqual(pending.previewAttachment.url, url)
        XCTAssertEqual(pending.previewAttachment.name, url.lastPathComponent)
        XCTAssertEqual(pending.previewAttachment.type, "image/png")
    }
}
