import Foundation

/// User content crosses the existing Rust copy authority, which owns tagging
/// and invalidates pending clipboard restores. Never write it via NSPasteboard.
/// The existing bridge applies transcript-tag settings and trims the payload.
@MainActor
enum UserTextClipboard {
  private enum CopyError: LocalizedError {
    case emptyText
    case boundaryWhitespace

    nonisolated var errorDescription: String? {
      switch self {
      case .emptyText: return String(localized: "There is no text to copy.")
      case .boundaryWhitespace:
        return String(localized: "This text cannot be copied without changing its spacing.")
      }
    }
  }

  private static var pendingCopy: Task<Result<Void, Error>, Never>?

  static func copy(_ text: String, preservingWhitespace: Bool = false) async throws {
    // The bridge treats trimmed-empty input as a successful no-op. It must not
    // acknowledge recovery or show Copied when no clipboard write happened.
    guard text.contains(where: { !$0.isWhitespace }) else { throw CopyError.emptyText }
    // Retained drafts may be consumed after success. Refuse a lossy bridge copy
    // rather than acknowledge it and discard the user's original spacing.
    if preservingWhitespace,
      !text.utf8.elementsEqual(text.trimmingCharacters(in: .whitespacesAndNewlines).utf8)
    {
      throw CopyError.boundaryWhitespace
    }
    let preceding = pendingCopy
    let write = Task<Result<Void, Error>, Never> { @MainActor in
      // Await earlier copies even if they failed; each caller receives its own
      // result, and a slower earlier bridge call cannot overwrite a later copy.
      if let preceding { _ = await preceding.value }
      do {
        try await CodescribeHotkeys().copyTextTagged(text: text)
        return .success(())
      } catch {
        return .failure(error)
      }
    }
    pendingCopy = write
    try await write.value.get()
  }
}
