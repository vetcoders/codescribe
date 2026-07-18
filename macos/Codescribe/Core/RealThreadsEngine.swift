import Foundation

// Backs the thread rail / drawer with REAL persisted threads from the codescribe
// ThreadStore via the UniFFI bridge (CodescribeThreads). Lists/searches thread
// summaries for the rail, loads messages on demand, and forwards lightweight
// thread mutations that already exist in the core.
final class RealThreadsEngine: ChatThreadsProviding {
    private let threads = CodescribeThreads()

    func listThreads() -> [ChatThread] {
        guard let list = try? threads.listThreads(filter: nil) else { return [] }
        return list.map(Self.thread)
    }

    func searchThreads(query: String) -> [ChatThread] {
        guard let list = try? threads.searchThreads(query: query) else { return [] }
        return list.map(Self.thread)
    }

    func generateThreadId() -> String {
        threads.generateThreadId()
    }

    func loadMessages(backendId: String) -> [ChatMessage] {
        guard let thread = try? threads.loadThread(id: backendId) else { return [] }
        var toolNamesById: [String: String] = [:]
        return thread.messages.compactMap { message -> ChatMessage? in
            let content = StoredMessageContent(rawJson: message.rawJson)
            content.toolUses.forEach { toolNamesById[$0.id] = $0.name }

            if !content.toolResults.isEmpty {
                return Self.toolActivityMessage(
                    from: content.toolResults,
                    toolNamesById: toolNamesById,
                    timestampMs: message.timestampMs
                )
            }
            if content.hasToolUseOnly {
                return nil
            }

            let text = message.text.trimmingCharacters(in: .whitespacesAndNewlines)
            switch message.role {
            case "user":
                guard !text.isEmpty || content.hasDisplayableNonTextBlock else { return nil }
                return ChatMessage(role: .you, timestamp: Self.timeString(timestampMs: message.timestampMs), text: text)
            case "assistant":
                guard !text.isEmpty else { return nil }
                return ChatMessage(role: .assistant, timestamp: Self.timeString(timestampMs: message.timestampMs), text: text)
            default: return nil  // skip system/tool turns in the transcript view
            }
        }
    }

    func deleteThread(backendId: String) -> Bool {
        do {
            try threads.deleteThread(id: backendId)
            return true
        } catch {
            return false
        }
    }

    func setThreadFavorite(backendId: String, isFavorite: Bool) -> Bool {
        (try? threads.setThreadFavorite(id: backendId, isFavorite: isFavorite)) ?? false
    }

    func renameThread(backendId: String, title: String) -> Bool {
        (try? threads.renameThread(id: backendId, title: title)) ?? false
    }

    func exportThreadMarkdown(backendId: String, assistantOnly: Bool) -> String? {
        try? threads.exportThreadMarkdown(id: backendId, assistantOnly: assistantOnly)
    }

    private static func thread(from summary: CsThreadSummary) -> ChatThread {
        let updatedAt = Date(timeIntervalSince1970: Double(summary.updatedAtMs) / 1000.0)
        var thread = ChatThread(
            title: summary.title.isEmpty ? "Untitled" : summary.title,
            meta: ThreadRailMeta.drawerSubtitle(
                model: summary.model,
                tokens: summary.totalTokens,
                updatedAt: updatedAt
            ),
            isFavorite: summary.isFavorite
        )
        thread.backendId = summary.id
        thread.updatedAt = updatedAt
        thread.model = summary.model
        thread.totalTokens = summary.totalTokens
        return thread
    }

    private static func toolActivityMessage(
        from results: [StoredToolResult],
        toolNamesById: [String: String],
        timestampMs: Int64
    ) -> ChatMessage {
        let lines = results.map { result in
            ToolLine(
                verb: result.isError ? "failed" : "ran",
                detail: toolNamesById[result.toolUseId] ?? "tool result"
            )
        }
        var message = ChatMessage(role: .tool, timestamp: timeString(timestampMs: timestampMs), text: "")
        let n = lines.count
        message.toolTitle = "What I checked · \(n) tool\(n == 1 ? "" : "s")"
        message.toolLines = lines
        return message
    }

    private static func timeString(timestampMs: Int64) -> String {
        guard timestampMs > 0 else { return "" }
        let date = Date(timeIntervalSince1970: Double(timestampMs) / 1000.0)
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm"
        return formatter.string(from: date)
    }
}

private struct StoredMessageContent {
    var toolUses: [StoredToolUse] = []
    var toolResults: [StoredToolResult] = []
    var hasDisplayableNonTextBlock = false

    var hasToolUseOnly: Bool {
        !toolUses.isEmpty && toolResults.isEmpty
    }

    init(rawJson: String) {
        guard let data = rawJson.data(using: .utf8),
              let blocks = try? JSONDecoder().decode([StoredContentBlock].self, from: data) else {
            return
        }
        hasDisplayableNonTextBlock = blocks.contains { block in
            guard let type = block.type else { return false }
            return !["text", "input_text", "output_text", "tool_use", "tool_result"].contains(type)
        }
        toolUses = blocks.compactMap { block in
            guard block.type == "tool_use",
                  let id = block.id,
                  let name = block.name else { return nil }
            return StoredToolUse(id: id, name: name)
        }
        toolResults = blocks.compactMap { block in
            guard block.type == "tool_result",
                  let toolUseId = block.toolUseId else { return nil }
            return StoredToolResult(toolUseId: toolUseId, isError: block.isError ?? false)
        }
    }
}

private struct StoredContentBlock: Decodable {
    let type: String?
    let id: String?
    let name: String?
    let toolUseId: String?
    let isError: Bool?

    enum CodingKeys: String, CodingKey {
        case type
        case id
        case name
        case toolUseId = "tool_use_id"
        case isError = "is_error"
    }
}

private struct StoredToolUse {
    let id: String
    let name: String
}

private struct StoredToolResult {
    let toolUseId: String
    let isError: Bool
}
