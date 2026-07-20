import XCTest
@testable import Codescribe

/// U17 chat-presentation-truth: the You-bubble shows the spoken instruction,
/// never the assistive wire skeleton. These tests pin the parser to the exact
/// output of `app/os/selection.rs::build_assistive_input` (all four variants),
/// its refusal to touch non-skeleton text, and the restore path that rewrites
/// persisted history on load.
final class AssistivePromptParserTests: XCTestCase {

    // MARK: - Wire builders (byte-for-byte mirror of build_assistive_input)

    private func wire(
        instruction: String,
        selection: String? = nil,
        app: String? = nil
    ) -> String {
        var out = "INSTRUKCJA_UŻYTKOWNIKA:\n<<<\n\(instruction)\n>\n\n"
        if let selection {
            out += "ZAZNACZONY_TEKST:\n<<<\n\(selection)\n>\n"
        } else {
            out += "ZAZNACZONY_TEKST: brak dostępnego zaznaczenia.\n"
        }
        if let app {
            out += "\nKONTEKST:\n- frontmost_app: \(app)\n"
        }
        return out
    }

    // MARK: - Skeleton variants

    func testParsesSelectionAndContextVariant() {
        let parts = AssistivePromptParser.parse(
            wire(instruction: "popraw ten akapit", selection: "stary tekst do poprawy", app: "Safari")
        )
        XCTAssertEqual(parts?.instruction, "popraw ten akapit")
        XCTAssertEqual(parts?.selectedText, "stary tekst do poprawy")
        XCTAssertEqual(parts?.frontmostApp, "Safari")
    }

    func testParsesSelectionWithoutContextVariant() {
        let parts = AssistivePromptParser.parse(
            wire(instruction: "przetłumacz to", selection: "hello world")
        )
        XCTAssertEqual(parts?.instruction, "przetłumacz to")
        XCTAssertEqual(parts?.selectedText, "hello world")
        XCTAssertNil(parts?.frontmostApp)
    }

    func testParsesMissingSelectionWithContextVariant() {
        let parts = AssistivePromptParser.parse(
            wire(instruction: "napisz krótkie podsumowanie dnia", app: "Ghostty")
        )
        XCTAssertEqual(parts?.instruction, "napisz krótkie podsumowanie dnia")
        XCTAssertNil(parts?.selectedText)
        XCTAssertEqual(parts?.frontmostApp, "Ghostty")
    }

    func testParsesMissingSelectionWithoutContextVariant() {
        let parts = AssistivePromptParser.parse(wire(instruction: "co słychać"))
        XCTAssertEqual(parts?.instruction, "co słychać")
        XCTAssertNil(parts?.selectedText)
        XCTAssertNil(parts?.frontmostApp)
    }

    // MARK: - Multiline payloads

    func testMultilineInstructionAndSelectionSurviveIntact() {
        let instruction = "pierwsza myśl\n\ndruga myśl po pauzie"
        let selection = "linia 1\nlinia 2\n\nlinia 4 z > znakiem"
        let parts = AssistivePromptParser.parse(
            wire(instruction: instruction, selection: selection, app: "Xcode")
        )
        XCTAssertEqual(parts?.instruction, instruction)
        XCTAssertEqual(parts?.selectedText, selection)
        XCTAssertEqual(parts?.frontmostApp, "Xcode")
    }

    // MARK: - Non-skeleton text passes through

    func testPlainComposerTextIsNotParsed() {
        XCTAssertNil(AssistivePromptParser.parse("just a normal chat message"))
        XCTAssertNil(AssistivePromptParser.parse("mention of INSTRUKCJA_UŻYTKOWNIKA: mid-text"))
        XCTAssertNil(AssistivePromptParser.parse(""))
    }

    func testHeaderWithoutSelectionSectionIsNotParsed() {
        // A truncated/foreign prompt that opens like the skeleton but never
        // carries the mandatory ZAZNACZONY_TEKST section stays raw.
        XCTAssertNil(AssistivePromptParser.parse("INSTRUKCJA_UŻYTKOWNIKA:\n<<<\ncoś\n>\n"))
    }

    // MARK: - Message presentation (display/wire split)

    func testPresentedRewritesUserSkeletonMessage() {
        let raw = wire(instruction: "zrób listę zakupów", selection: "mleko, chleb", app: "Notes")
        let message = ChatMessage(role: .you, timestamp: "10:00", text: raw)

        let presented = AssistivePromptParser.presented(message)

        XCTAssertEqual(presented.text, "zrób listę zakupów")
        XCTAssertEqual(presented.wireText, raw)
        XCTAssertEqual(presented.contextSelection, "mleko, chleb")
        XCTAssertEqual(presented.contextApp, "Notes")
    }

    func testPresentedLeavesPlainUserMessageUntouched() {
        let message = ChatMessage(role: .you, timestamp: "10:00", text: "plain composer text")
        let presented = AssistivePromptParser.presented(message)
        XCTAssertEqual(presented.text, "plain composer text")
        XCTAssertNil(presented.wireText)
        XCTAssertNil(presented.contextSelection)
        XCTAssertNil(presented.contextApp)
    }

    func testPresentedLeavesAssistantMessageUntouched() {
        let raw = wire(instruction: "echo of the skeleton in a reply")
        let message = ChatMessage(role: .assistant, timestamp: "10:00", text: raw)
        let presented = AssistivePromptParser.presented(message)
        XCTAssertEqual(presented.text, raw)
        XCTAssertNil(presented.wireText)
    }

    // MARK: - Restore path (persisted threads render clean)

    @MainActor
    func testRestoredThreadMessagesRenderCleanFromWire() {
        let raw = wire(instruction: "przeczytaj tego maila", selection: "Dear team…", app: "Mail")
        let provider = StubThreadsProvider(
            thread: {
                var thread = ChatThread(title: "restored", meta: "yesterday")
                thread.backendId = "t_restore"
                return thread
            }(),
            messages: [
                ChatMessage(role: .you, timestamp: "09:00", text: raw),
                ChatMessage(role: .assistant, timestamp: "09:01", text: "Sure — summary follows."),
            ]
        )

        let store = AgentChatStore(threadsProvider: provider)

        let messages = store.threads.first { $0.backendId == "t_restore" }?.messages ?? []
        XCTAssertEqual(messages.count, 2)
        XCTAssertEqual(messages.first?.text, "przeczytaj tego maila")
        XCTAssertEqual(messages.first?.wireText, raw)
        XCTAssertEqual(messages.first?.contextSelection, "Dear team…")
        XCTAssertEqual(messages.first?.contextApp, "Mail")
        // The assistant turn is untouched by the rewrite.
        XCTAssertEqual(messages.last?.text, "Sure — summary follows.")
        XCTAssertNil(messages.last?.wireText)
    }

    @MainActor
    func testLiveVoiceTurnIngestsDisplayNotWire() {
        let raw = wire(instruction: "odpowiedz po polsku", selection: "some english text", app: "Slack")
        let store = AgentChatStore(threads: [])

        store.ingestVoiceTurn(threadId: "t_live", userText: raw)

        let thread = store.threads.first { $0.backendId == "t_live" }
        let you = thread?.messages.first { $0.role == .you }
        XCTAssertEqual(you?.text, "odpowiedz po polsku")
        XCTAssertEqual(you?.wireText, raw)
        XCTAssertEqual(you?.contextSelection, "some english text")
        XCTAssertEqual(you?.contextApp, "Slack")
        // The thread title comes from the spoken instruction, not the skeleton.
        XCTAssertEqual(thread?.title, "odpowiedz po polsku")
    }
}

/// Minimal threads provider: one persisted thread whose messages carry the wire
/// skeleton, standing in for ThreadStore JSON written before the display split.
private final class StubThreadsProvider: ChatThreadsProviding {
    private let thread: ChatThread
    private let messages: [ChatMessage]

    init(thread: ChatThread, messages: [ChatMessage]) {
        self.thread = thread
        self.messages = messages
    }

    func listThreads() -> [ChatThread] { [thread] }
    func searchThreads(query: String) -> [ChatThread] { [thread] }
    func loadMessages(backendId: String) -> [ChatMessage] { messages }
    func deleteThread(backendId: String) -> Bool { true }
    func setThreadFavorite(backendId: String, isFavorite: Bool) -> Bool { true }
    func renameThread(backendId: String, title: String) -> Bool { true }
    func setGeneratedTitle(backendId: String, title: String) -> Bool { true }
    func exportThreadMarkdown(backendId: String, assistantOnly: Bool) -> String? { nil }
    func generateThreadId() -> String { "t_generated" }
}
