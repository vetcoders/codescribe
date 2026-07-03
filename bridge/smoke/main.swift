import Foundation

// Live smoke: drive the REAL codescribe agent engine through the generated Swift
// bindings. Proves the keystone end-to-end (engine -> UniFFI -> Swift streaming).
final class Printer: CsAgentListener, @unchecked Sendable {
    func onTextDelta(delta: String) {
        FileHandle.standardOutput.write(delta.data(using: .utf8)!)
    }
    func onTextDone(text: String) {}
    func onReasoningDelta(delta: String) {}
    func onToolExecuting(name: String, id: String) { print("\n[tool: \(name)]") }
    func onToolResult(name: String, id: String, summary: String, isError: Bool) {
        print("\n[tool result: \(name) error=\(isError)] \(summary.prefix(80))")
    }
    func onDone() { print("\n[DONE]") }
    func onError(message: String) { print("\n[ERROR] \(message)") }
}

let agent = CodescribeAgent()
print("is_available:", agent.isAvailable())
guard agent.isAvailable() else {
    print("provider not available — no LLM_ASSISTIVE_* in env / keychain not readable by this binary")
    exit(0)
}

let sem = DispatchSemaphore(value: 0)
Task {
    do {
        print("--- streaming reply ---")
        let final = try await agent.streamReply(
            text: "Say hello in exactly three words.",
            listener: Printer()
        )
        print("\n--- FINAL ---\n\(final)")
    } catch {
        print("\nstream error: \(error)")
    }
    sem.signal()
}
sem.wait()
