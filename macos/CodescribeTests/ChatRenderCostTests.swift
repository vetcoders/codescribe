import SwiftUI
import XCTest
@testable import Codescribe

/// Render-cost measurement for the chat stream hot path (bolączka #3: beachball
/// on a >20k stream / 100k paste). NOT a pass/fail perf gate — it measures the
/// real code paths and prints per-tick numbers for the implementation report:
///
///   [render-cost] <case>: <µs>/tick (over N ticks)
///
/// "Legacy" reproduces the pre-U5 per-tick work verbatim: the 5-message scroll
/// signature (grapheme `count` + tool-line string concat) and the full markdown
/// re-parse of the growing bubble that `MarkdownText.body` used to pay on every
/// delta. "Current" is what ships after U5: `MessageList.tailSignature` (O(1)
/// utf8 counts, last two turns) and the raw-mode plain `Text` (no parse at all;
/// markdown is paid once, only when a bubble is toggled rich).
///
/// Numbers come from a Debug test host — treat them as relative (legacy vs
/// current on identical fixtures), not absolute release timings.
@MainActor
final class ChatRenderCostTests: XCTestCase {
    private let ticks = 200

    // MARK: Fixtures

    /// Markdown-shaped assistant text grown to ~`chars` bytes — the 20k stream.
    private func streamedMarkdown(chars: Int) -> String {
        let unit = """
        ## Analysis pass

        The `EventBus` re-emits on retry, and the store subscribes twice on \
        remount. A minimal patch gates the emit on a settled flag and de-dupes \
        the listener registration on remount.

        - check `events/bus.ts` for the retry loop
        - check `ui/store.ts` for the double subscribe
        - add a regression test for the double fire

        ```ts
        bus.on("retry", () => emitOnce(event))
        ```


        """
        var out = ""
        out.reserveCapacity(chars + unit.utf8.count)
        while out.utf8.count < chars { out += unit }
        return out
    }

    /// Plain prose grown to ~`chars` bytes — the 100k paste into a You turn.
    private func pastedProse(chars: Int) -> String {
        let unit = "Pasted log line with some detail about the failing request and its retry budget. "
        var out = ""
        out.reserveCapacity(chars + unit.utf8.count)
        while out.utf8.count < chars { out += unit }
        return out
    }

    /// A realistic tail during a stream: small You turn, a tool row, and the
    /// growing assistant bubble. `pasted` adds a 100k You turn into the window.
    private func streamingThread(streamChars: Int, pastedChars: Int? = nil) -> [ChatMessage] {
        var messages: [ChatMessage] = []
        if let pastedChars {
            messages.append(ChatMessage(role: .you, timestamp: "now", text: pastedProse(chars: pastedChars)))
        }
        messages.append(ChatMessage(role: .you, timestamp: "now", text: "where do we double-dispatch events?"))
        var tool = ChatMessage(role: .tool, timestamp: "now", text: "")
        tool.toolLines = [
            ToolLine(verb: "grep", detail: "events/bus.ts · ui/store.ts"),
            ToolLine(verb: "read", detail: "2 files · 318 lines"),
            ToolLine(verb: "tool", detail: "regression-test", state: .running),
        ]
        messages.append(tool)
        var assistant = ChatMessage(role: .assistant, timestamp: "now",
                                    text: streamedMarkdown(chars: streamChars))
        assistant.isStreaming = true
        messages.append(assistant)
        return messages
    }

    // MARK: Legacy per-tick work (pre-U5, reproduced verbatim for the baseline)

    /// The old `MessageList.lastSignature`: suffix(5), grapheme `count` on every
    /// text (O(n) walk — 100k steps per tick on a big paste), tool-line string
    /// concat with `detail` and `reason` per line.
    private func legacySignature(_ messages: [ChatMessage]) -> String {
        messages.suffix(5).map { message in
            let tools = message.toolLines.map { line in
                "\(line.id)-\(line.state)-\(line.detail)-\(line.reason?.count ?? 0)"
            }.joined(separator: ",")
            return "\(message.id)-\(message.text.count)-\(message.reasoning.count)-\(tools)"
        }.joined(separator: "|")
    }

    /// The old streaming-bubble body eval: `MarkdownText.body` re-runs
    /// `MDBlock.parse` on the whole grown text every delta, then materializes an
    /// AttributedString per prose block.
    private func legacyMarkdownTick(_ text: String) -> Int {
        let blocks = MDBlock.parse(text)
        var materialized = 0
        for block in blocks {
            if case let .paragraph(body) = block {
                _ = MarkdownText.inlineAttributed(
                    body, fontSize: 14, baseFont: CSFont.ui(14),
                    baseColor: CSColor.textBodyAlt
                )
                materialized += 1
            }
        }
        return blocks.count + materialized
    }

    // MARK: Timing harness

    private func perTickMicros(_ label: String, body: () -> Int) -> Double {
        var sink = 0
        let start = CFAbsoluteTimeGetCurrent()
        for _ in 0..<ticks { sink &+= body() }
        let elapsed = CFAbsoluteTimeGetCurrent() - start
        let micros = elapsed / Double(ticks) * 1_000_000
        print("[render-cost] \(label): \(String(format: "%.1f", micros)) µs/tick (over \(ticks) ticks, sink \(sink))")
        XCTAssertGreaterThan(sink, 0, "measured body must do real work")
        return micros
    }

    // MARK: Measurements

    func testSignatureCost20kStream() {
        let thread = streamingThread(streamChars: 20_000)
        let legacy = perTickMicros("legacy signature · 20k stream") {
            legacySignature(thread).utf8.count
        }
        let current = perTickMicros("tailSignature · 20k stream") {
            MessageList.tailSignature(thread).utf8.count
        }
        // Regression guard, loose on purpose (CI machines vary): the new
        // signature must not be slower than the legacy one it replaces.
        XCTAssertLessThanOrEqual(current, legacy * 2)
    }

    func testSignatureCost100kPasteInWindow() {
        let thread = streamingThread(streamChars: 20_000, pastedChars: 100_000)
        let legacy = perTickMicros("legacy signature · 20k stream + 100k paste") {
            legacySignature(thread).utf8.count
        }
        let current = perTickMicros("tailSignature · 20k stream + 100k paste") {
            MessageList.tailSignature(thread).utf8.count
        }
        XCTAssertLessThanOrEqual(current, legacy * 2)
    }

    func testBubbleBodyCost20kStream() {
        let text = streamedMarkdown(chars: 20_000)
        _ = perTickMicros("legacy markdown re-parse · 20k streaming bubble") {
            legacyMarkdownTick(text)
        }
        // Raw mode (the new default) hands the string straight to `Text` — the
        // only per-tick string work left in the bubble body.
        _ = perTickMicros("raw-mode body · 20k streaming bubble") {
            _ = Text(text)
            return text.utf8.count
        }
    }

    func testPasteParseCost100k() {
        // A 100k paste renders its You bubble through MarkdownText once (not per
        // tick) — this is that one-shot cost, for the report's paste column.
        let text = pastedProse(chars: 100_000)
        _ = perTickMicros("markdown parse · 100k pasted turn (one-shot cost)") {
            MDBlock.parse(text).count
        }
    }
}
