import Foundation

/// Slash-command palette for the composer.
///
/// The whole decision surface is pure value logic so it can be tested without a
/// window: what the draft text means (`ComposerPaletteQuery`), which entries
/// survive the filter, and what the draft becomes after a pick. The SwiftUI
/// layer only draws the rows this file returns.
///
/// Design rule: the palette NEVER swallows ordinary typing. It engages only for
/// a slash at the very start of the draft, so "path/to/file" and "and/or" stay
/// plain text, and a draft that already carries prose is never reinterpreted as
/// a command halfway through.

/// One selectable palette entry.
struct ComposerPaletteEntry: Identifiable, Equatable {
  /// Stable identity within its command (a model id, a grant key, …).
  let id: String
  let title: String
  let subtitle: String?
  /// True for the entry matching the current setting, so the UI can mark it.
  var isCurrent: Bool = false
}

/// The commands the palette exposes. Adding one means adding a case plus its
/// row in `allCases` — the parser and the filter need no changes.
enum ComposerPaletteCommand: String, CaseIterable, Equatable {
  case model
  case grants

  var keyword: String { rawValue }

  var summary: String {
    switch self {
    case .model: "Wybierz model asystenta"
    case .grants: "Narzędzia z „zawsze zezwalaj”"
    }
  }

  /// Longest keyword prefix shared with `text`, used for ranking.
  static func matching(prefix: String) -> [ComposerPaletteCommand] {
    let needle = prefix.lowercased()
    guard !needle.isEmpty else { return allCases }
    return allCases.filter { $0.keyword.hasPrefix(needle) }
  }
}

/// What the current draft means to the palette.
enum ComposerPaletteQuery: Equatable {
  /// Not a palette draft — send as a normal message.
  case inactive
  /// `/` or a partial keyword: offer commands.
  case commands(prefix: String)
  /// A complete keyword plus optional argument: offer that command's entries.
  case entries(command: ComposerPaletteCommand, filter: String)

  /// Parse a draft. Only a leading `/` (after whitespace) activates the
  /// palette, and only while the draft holds a single line — a multi-line
  /// draft is prose the user is composing, never a command.
  static func parse(_ draft: String) -> ComposerPaletteQuery {
    let trimmed = draft.trimmingCharacters(in: .whitespaces)
    guard trimmed.hasPrefix("/"), !trimmed.contains("\n") else { return .inactive }

    let body = String(trimmed.dropFirst())
    guard let separator = body.firstIndex(of: " ") else {
      // Still typing the keyword. An exact keyword with no space yet also
      // shows its entries, so "/model" lists models before the space.
      if let command = ComposerPaletteCommand(rawValue: body.lowercased()) {
        return .entries(command: command, filter: "")
      }
      return .commands(prefix: body)
    }
    let keyword = String(body[body.startIndex..<separator]).lowercased()
    guard let command = ComposerPaletteCommand(rawValue: keyword) else {
      // "/notacommand foo" is not a command — let it send as text rather
      // than trapping the user in an empty palette.
      return .inactive
    }
    let filter = String(body[body.index(after: separator)...])
      .trimmingCharacters(in: .whitespaces)
    return .entries(command: command, filter: filter)
  }
}

/// Filtering + draft rewriting. Free functions on purpose: no state to get
/// stale, and every one of them is directly testable.
enum ComposerPalette {
  /// Case-insensitive substring match over title and id, preserving the
  /// caller's ordering (providers already rank their catalogs).
  static func filter(_ entries: [ComposerPaletteEntry], by filter: String)
    -> [ComposerPaletteEntry]
  {
    let needle = filter.trimmingCharacters(in: .whitespaces).lowercased()
    guard !needle.isEmpty else { return entries }
    return entries.filter {
      $0.title.lowercased().contains(needle) || $0.id.lowercased().contains(needle)
    }
  }

  /// Draft text after picking a command from the command list: the keyword
  /// plus a trailing space, so the entry list opens and typing filters it.
  static func draft(afterPicking command: ComposerPaletteCommand) -> String {
    "/\(command.keyword) "
  }

  /// The palette consumes the draft when an entry is applied — the command was
  /// an instruction to the app, not a message to the agent, so leaving its
  /// text behind would send "/model gpt-5" to the model on the next Enter.
  static let draftAfterApplyingEntry = ""
}

/// Data behind the palette. One protocol so the store stays testable and
/// previews render without a live bridge.
protocol ComposerPaletteSourcing {
  func entries(for command: ComposerPaletteCommand) -> [ComposerPaletteEntry]
  func apply(_ entry: ComposerPaletteEntry, for command: ComposerPaletteCommand) throws
}

/// Live palette source over the settings + MCP-admin bridges.
///
/// Model discovery hits the provider API with the operator's key, so its result
/// is cached for the window's lifetime and refreshed only when the palette is
/// reopened after a change — a per-keystroke network call while filtering would
/// be both slow and rude to the provider.
final class RealComposerPaletteSource: ComposerPaletteSourcing {
  private let settings: SettingsEngine
  private let mcpAdmin: MCPAdminEngine
  private var cachedModels: [ComposerPaletteEntry]?

  init(settings: SettingsEngine, mcpAdmin: MCPAdminEngine) {
    self.settings = settings
    self.mcpAdmin = mcpAdmin
  }

  func entries(for command: ComposerPaletteCommand) -> [ComposerPaletteEntry] {
    switch command {
    case .model: models()
    case .grants: grants()
    }
  }

  func apply(_ entry: ComposerPaletteEntry, for command: ComposerPaletteCommand) throws {
    switch command {
    case .model:
      try settings.updateConfig(key: "LLM_ASSISTIVE_MODEL", value: entry.id)
      cachedModels = nil
    case .grants:
      try mcpAdmin.revokeToolGrant(key: entry.id)
    }
  }

  private func models() -> [ComposerPaletteEntry] {
    if let cachedModels { return cachedModels }
    let snapshot = settings.loadSettings()
    let current = snapshot.llmAssistiveModel
    let providerID = snapshot.llmAssistiveProvider ?? "openai-responses"
    let discovery = settings.discoverModels(providerId: providerID)
    let entries: [ComposerPaletteEntry] = discovery.models.map { model in
      let subtitle: String? = model.id == model.displayName ? nil : model.id
      return ComposerPaletteEntry(
        id: model.id,
        title: model.displayName,
        subtitle: subtitle,
        isCurrent: model.id == current
      )
    }
    cachedModels = entries
    return entries
  }

  private func grants() -> [ComposerPaletteEntry] {
    // A revoke list that silently swallowed its error would tell the
    // operator "nothing is granted" while grants keep letting tools run.
    guard let grants = try? mcpAdmin.listToolGrants() else {
      return [
        ComposerPaletteEntry(
          id: "",
          title: "Nie udało się odczytać uprawnień",
          subtitle: "sprawdź ~/.codescribe/tool_grants.json"
        )
      ]
    }
    return grants.map { grant in
      ComposerPaletteEntry(
        id: grant.key,
        title: grant.key,
        subtitle: "nadane \(grant.grantedAt) · wybierz, aby cofnąć"
      )
    }
  }
}
