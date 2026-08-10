import XCTest
@testable import Codescribe

/// U17 chat-presentation-truth: the You-bubble shows the spoken instruction,
/// never the assistive wire skeleton. These tests pin the parser to the exact
/// output of `app/os/selection.rs::build_assistive_input` (all four variants),
/// its refusal to touch non-skeleton text, and the restore path that rewrites
/// persisted history on load.
final class AssistivePromptParserTests: XCTestCase {

    // MARK: - Wire builders (byte-for-byte mirror of build_assistive_input)

    /// Legacy Polish dialect — threads persisted before the EN label rename.
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

    /// Canonical English dialect — current `build_assistive_input` output.
    private func englishWire(
        instruction: String,
        selection: String? = nil,
        selectionCarriedInContextTags: Bool = false,
        app: String? = nil
    ) -> String {
        var out = "USER_INSTRUCTION:\n<<<\n\(instruction)\n>\n\n"
        if let selection {
            out += "SELECTED_TEXT:\n<<<\n\(selection)\n>\n"
        } else if selectionCarriedInContextTags {
            out += "SELECTED_TEXT: carried in <codescribe_context>.\n"
        } else {
            out += "SELECTED_TEXT: no selection available.\n"
        }
        if let app {
            out += "\nCONTEXT:\n- frontmost_app: \(app)\n"
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

    // MARK: - Canonical English dialect (current wires)

    func testParsesEnglishSelectionAndContextVariant() {
        let parts = AssistivePromptParser.parse(
            englishWire(instruction: "popraw ten akapit", selection: "stary tekst", app: "Safari")
        )
        XCTAssertEqual(parts?.instruction, "popraw ten akapit")
        XCTAssertEqual(parts?.selectedText, "stary tekst")
        XCTAssertEqual(parts?.frontmostApp, "Safari")
    }

    func testParsesEnglishMissingSelectionVariants() {
        let missing = AssistivePromptParser.parse(
            englishWire(instruction: "summarize the day", app: "Ghostty")
        )
        XCTAssertEqual(missing?.instruction, "summarize the day")
        XCTAssertNil(missing?.selectedText)
        XCTAssertEqual(missing?.frontmostApp, "Ghostty")

        // Bucket-carried selections live in <codescribe_context> tags appended
        // after the skeleton; the header line itself parses like "missing".
        let carried = AssistivePromptParser.parse(
            englishWire(instruction: "compare all three", selectionCarriedInContextTags: true)
        )
        XCTAssertEqual(carried?.instruction, "compare all three")
        XCTAssertNil(carried?.selectedText)
        XCTAssertNil(carried?.frontmostApp)
    }

    func testParsesCarriedSelectionVariantWithLiveCount() {
        // The carried-line suffix carries an honest selection count, so the
        // parser matches by prefix and consumes through the end of the line.
        let parts = AssistivePromptParser.parse(
            "USER_INSTRUCTION:\n<<<\npieguski przede wszystkim\n>\n\n"
                + "SELECTED_TEXT: carried in <codescribe_context> (3 selections).\n"
                + "\nCONTEXT:\n- frontmost_app: iTerm2\n"
        )
        XCTAssertEqual(parts?.instruction, "pieguski przede wszystkim")
        XCTAssertNil(parts?.selectedText)
        XCTAssertEqual(parts?.frontmostApp, "iTerm2")

        let singular = AssistivePromptParser.parse(
            "USER_INSTRUCTION:\n<<<\njedno zaznaczenie\n>\n\n"
                + "SELECTED_TEXT: carried in <codescribe_context> (1 selection).\n"
        )
        XCTAssertEqual(singular?.instruction, "jedno zaznaczenie")
        XCTAssertNil(singular?.selectedText)
        XCTAssertNil(singular?.frontmostApp)
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

    func testHeaderWithoutSelectionSectionSalvagesInstruction() {
        // Truncated wire (header + instruction, no selection section) used to
        // stay raw and dump the skeleton into the You bubble — that collapsed
        // the Agent window on June-era restores (R1). Salvage the spoken text.
        let parts = AssistivePromptParser.parse("INSTRUKCJA_UŻYTKOWNIKA:\n<<<\ncoś\n>\n")
        XCTAssertEqual(parts?.instruction, "coś")
        XCTAssertNil(parts?.selectedText)
        XCTAssertNil(parts?.frontmostApp)
    }

    // MARK: - June-era / gpt-5.5 tolerant seams (R1)

    /// Operator screenshot class: instruction heredoc close (`>`) missing
    /// between spoken text and ZAZNACZONY_TEKST, with a huge selection body.
    private func juneEraWireMissingClose(
        instruction: String,
        selection: String,
        app: String? = "Xcode"
    ) -> String {
        var out = "INSTRUKCJA_UŻYTKOWNIKA:\n<<<\n\(instruction)\n\n"
        out += "ZAZNACZONY_TEKST:\n<<<\n\(selection)\n>\n"
        if let app {
            out += "\nKONTEKST:\n- frontmost_app: \(app)\n"
        }
        return out
    }

    func testParsesJuneEraWireMissingHeredocClose() {
        let selection = String(repeating: "long unbreakable_token_fragment)\" ", count: 200)
        let raw = juneEraWireMissingClose(
            instruction: "No wiesz co, spróbujesz jeszcze raz?",
            selection: selection
        )
        let parts = AssistivePromptParser.parse(raw)
        XCTAssertEqual(parts?.instruction, "No wiesz co, spróbujesz jeszcze raz?")
        XCTAssertEqual(parts?.selectedText, selection.trimmingCharacters(in: .whitespacesAndNewlines))
        XCTAssertEqual(parts?.frontmostApp, "Xcode")
    }

    func testPresentedJuneEraWireNeverShowsSkeletonInBubble() {
        let selection = """
        vibecrafted workflow claude --prompt "$(cat <<'PROMPT'
        Masz do zrobienia audit...
        PROMPT)"
        """
        let raw = juneEraWireMissingClose(
            instruction: "Spróbujesz jeszcze raz?",
            selection: selection
        )
        let presented = AssistivePromptParser.presented(
            ChatMessage(role: .you, timestamp: "13:03", text: raw)
        )
        XCTAssertEqual(presented.text, "Spróbujesz jeszcze raz?")
        XCTAssertEqual(presented.wireText, raw)
        XCTAssertFalse(presented.text.contains("INSTRUKCJA_UŻYTKOWNIKA"))
        XCTAssertFalse(presented.text.contains("ZAZNACZONY_TEKST"))
        XCTAssertTrue(presented.contextSelection?.contains("vibecrafted") == true)
        // Display text stays a short spoken instruction — not the wall of wire.
        XCTAssertLessThan(presented.text.utf8.count, 200)
    }

    func testParsesEnglishWireMissingHeredocClose() {
        let raw = "USER_INSTRUCTION:\n<<<\ntry again\n\nSELECTED_TEXT:\n<<<\npasted body\n>\n\nCONTEXT:\n- frontmost_app: Safari\n"
        let parts = AssistivePromptParser.parse(raw)
        XCTAssertEqual(parts?.instruction, "try again")
        XCTAssertEqual(parts?.selectedText, "pasted body")
        XCTAssertEqual(parts?.frontmostApp, "Safari")
    }

    func testOpenEndedSelectionStillSalvagesInstruction() {
        // Selection heredoc never closed — still must not dump the skeleton.
        let raw = "INSTRUKCJA_UŻYTKOWNIKA:\n<<<\nkontynuuj\n>\n\nZAZNACZONY_TEKST:\n<<<\nunclosed selection body without close"
        let parts = AssistivePromptParser.parse(raw)
        XCTAssertEqual(parts?.instruction, "kontynuuj")
        XCTAssertEqual(parts?.selectedText, "unclosed selection body without close")
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

    /// V4: hydrating a June-era legacy session and then re-selecting a live
    /// thread must not leave skeleton dump text in the live view.
    @MainActor
    func testSelectingLegacyThenLiveDoesNotPoisonLiveMessages() {
        let legacyWire = juneEraWireMissingClose(
            instruction: "Cześć. Kim jesteś?",
            selection: String(repeating: "legacy_prompt_body)\" ", count: 400)
        )
        let liveYou = ChatMessage(role: .you, timestamp: "14:00", text: "live plain message")
        let liveAssistant = ChatMessage(
            role: .assistant, timestamp: "14:01", text: "live assistant reply"
        )
        let legacyYou = ChatMessage(role: .you, timestamp: "13:00", text: legacyWire)
        let legacyAssistant = ChatMessage(
            role: .assistant, timestamp: "13:01", text: "legacy assistant reply"
        )

        var liveThread = ChatThread(title: "live", meta: "now")
        liveThread.backendId = "t_live"
        var legacyThread = ChatThread(title: "legacy", meta: "Jun 15")
        legacyThread.backendId = "t_legacy"

        let provider = MultiThreadStubProvider(messagesByBackendId: [
            "t_live": [liveYou, liveAssistant],
            "t_legacy": [legacyYou, legacyAssistant],
        ], threads: [liveThread, legacyThread])

        let store = AgentChatStore(threadsProvider: provider)
        // Prefer live first if the store auto-selects the first row.
        let liveID = store.threads.first { $0.backendId == "t_live" }!.id
        let legacyID = store.threads.first { $0.backendId == "t_legacy" }!.id

        store.select(legacyID)
        let legacyMessages = store.threads.first { $0.id == legacyID }?.messages ?? []
        XCTAssertEqual(legacyMessages.first?.text, "Cześć. Kim jesteś?")
        XCTAssertFalse(legacyMessages.first?.text.contains("INSTRUKCJA") == true)
        XCTAssertNotNil(legacyMessages.first?.wireText)

        store.select(liveID)
        let liveMessages = store.threads.first { $0.id == liveID }?.messages ?? []
        XCTAssertEqual(liveMessages.count, 2)
        XCTAssertEqual(liveMessages.first?.text, "live plain message")
        XCTAssertNil(liveMessages.first?.wireText)
        XCTAssertEqual(liveMessages.last?.text, "live assistant reply")
        // Live view must not pick up the legacy wire body.
        XCTAssertFalse(liveMessages.contains { $0.text.contains("INSTRUKCJA") })
        XCTAssertFalse(liveMessages.contains { $0.text.contains("legacy_prompt_body") })
    }
}

/// Provider with per-backend message tables so select(legacy) → select(live)
/// exercises the real lazy-load path without sharing one message array.
private final class MultiThreadStubProvider: ChatThreadsProviding {
    private let messagesByBackendId: [String: [ChatMessage]]
    private let threads: [ChatThread]

    init(messagesByBackendId: [String: [ChatMessage]], threads: [ChatThread]) {
        self.messagesByBackendId = messagesByBackendId
        self.threads = threads
    }

    func listThreads() -> [ChatThread] { threads }
    func searchThreads(query: String) -> [ChatThread] { threads }
    func loadMessages(backendId: String) -> [ChatMessage] {
        messagesByBackendId[backendId] ?? []
    }
    func deleteThread(backendId: String) -> Bool { true }
    func setThreadFavorite(backendId: String, isFavorite: Bool) -> Bool { true }
    func renameThread(backendId: String, title: String) -> Bool { true }
    func setGeneratedTitle(backendId: String, title: String) -> Bool { true }
    func exportThreadMarkdown(backendId: String, assistantOnly: Bool) -> String? { nil }
    func generateThreadId() -> String { "t_generated" }
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
