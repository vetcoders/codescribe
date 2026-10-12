import Foundation
import Observation

/// Sorting order for saved conversations.
enum OverlayConversationSortOrder: String, CaseIterable, Sendable {
  case newest
  case oldest
  case name

  var title: String {
    switch self {
    case .newest:
      return String(localized: "Newest first")
    case .oldest:
      return String(localized: "Oldest first")
    case .name:
      return String(localized: "By name")
    }
  }
}

/// A matched conversation returned by the bus conversation search.
struct OverlayConversationSearchResult: Equatable, Sendable {
  let conversationID: String
  let lastActivity: String

  init(conversationID: String, lastActivity: String = "") {
    self.conversationID = conversationID
    self.lastActivity = lastActivity
  }
}

/// Response returned by the bus conversation search.
struct OverlayConversationSearchResponse: Equatable, Sendable {
  let results: [OverlayConversationSearchResult]
  let complete: Bool
  let errorCount: UInt32

  init(
    results: [OverlayConversationSearchResult] = [],
    complete: Bool = true,
    errorCount: UInt32 = 0
  ) {
    self.results = results
    self.complete = complete
    self.errorCount = errorCount
  }
}

/// Protocol abstracting the bridge bus conversation search for dependency injection in tests.
protocol OverlayConversationSearchClient: Sendable {
  func search(rootPath: String, query: String) async throws -> OverlayConversationSearchResponse
}

/// Live bridge client calling `CodescribeThreads().searchBusConversations(rootPath:query:)`.
struct LiveOverlayConversationSearchClient: OverlayConversationSearchClient {
  func search(rootPath: String, query: String) async throws -> OverlayConversationSearchResponse {
    let bridgeResponse = try await Task.detached {
      try CodescribeThreads().searchBusConversations(rootPath: rootPath, query: query)
    }.value
    return OverlayConversationSearchResponse(
      results: bridgeResponse.results.map {
        OverlayConversationSearchResult(
          conversationID: $0.conversationId,
          lastActivity: $0.lastActivity
        )
      },
      complete: bridgeResponse.complete,
      errorCount: bridgeResponse.errorCount
    )
  }
}

/// Independent query and sort state manager for saved conversations in the overlay.
@MainActor @Observable
final class OverlayConversationSearchModel {
  var query: String = "" {
    didSet {
      if query != oldValue {
        queryDidChange()
      }
    }
  }

  var sortOrder: OverlayConversationSortOrder = .newest

  private(set) var isLoading: Bool = false
  private(set) var searchError: String? = nil
  private(set) var isPartial: Bool = false
  private(set) var errorCount: UInt32 = 0

  /// Matched conversation IDs returned by the latest completed backend search.
  /// `nil` indicates that no query filter is active (i.e. empty query, showing all saved conversations).
  private(set) var matchedConversationIDs: Set<String>? = nil

  /// Last activity dates reported by backend, keyed by conversation ID.
  private(set) var backendLastActivityDates: [String: Date] = [:]

  /// State for empty-query archive metadata background loading.
  private(set) var isMetadataLoading: Bool = false
  private(set) var metadataError: String? = nil
  private(set) var isMetadataPartial: Bool = false
  private(set) var metadataErrorCount: UInt32 = 0

  let client: any OverlayConversationSearchClient
  let rootPath: String
  var debounceDuration: Duration

  private var searchGeneration: UInt64 = 0
  private var searchTask: Task<Void, Never>? = nil
  private var metadataGeneration: UInt64 = 0
  private var nextSnapshotSequence: UInt64 = 0
  private var backendLastActivityRevisions: [String: UInt64] = [:]

  init(
    client: any OverlayConversationSearchClient = LiveOverlayConversationSearchClient(),
    rootPath: String = OverlayChannelDeliveryReader.productionRoot().path,
    debounceDuration: Duration = .milliseconds(150)
  ) {
    self.client = client
    self.rootPath = rootPath
    self.debounceDuration = debounceDuration
  }

  /// Effective search error: surfaces active query search error when a query is present,
  /// or background archive metadata error when viewing all saved conversations.
  var effectiveSearchError: String? {
    let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
    if !trimmed.isEmpty {
      return searchError
    }
    return metadataError
  }

  /// Effective partial flag: surfaces query partial results flag when a query is present,
  /// or archive metadata partial status when viewing all saved conversations.
  var isEffectivePartial: Bool {
    let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
    if !trimmed.isEmpty {
      return isPartial
    }
    return isMetadataPartial
  }

  /// Whether the latest search completed successfully with a complete, non-partial, zero-result match from the backend.
  var isExhaustiveZeroMatch: Bool {
    guard !isLoading, searchError == nil, !isPartial, let matched = matchedConversationIDs else {
      return false
    }
    let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty else { return false }
    return matched.isEmpty
  }

  /// Whether the active search query completed successfully with an exhaustive (complete, non-partial, non-error) zero-hit result.
  /// Never true while loading, on error, or when results are partial/incomplete.
  func hasExhaustiveNoMatch(in conversations: [OverlayConversation]) -> Bool {
    guard !isLoading, searchError == nil, !isPartial, matchedConversationIDs != nil else {
      return false
    }
    let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty else { return false }
    return conversations.isEmpty
  }

  /// Immediately clears the search query and restores the full list without changing selection.
  func clear() {
    searchTask?.cancel()
    searchTask = nil
    searchGeneration &+= 1
    query = ""
    isLoading = false
    searchError = nil
    isPartial = false
    errorCount = 0
    matchedConversationIDs = nil
  }

  /// Directly executes a search with no debounce, useful for tests and deterministic operations.
  func searchImmediately(query: String) async {
    self.query = query
    searchTask?.cancel()
    searchTask = nil
    searchGeneration &+= 1
    let generation = searchGeneration
    await executeSearch(query: query, generation: generation)
  }

  /// Records a verified activity date for a conversation owner if this snapshot's monotonic sequence
  /// is greater than or equal to any previously recorded revision. Authoritative newer revisions
  /// may move an activity date earlier or later; older snapshots never overwrite newer ones.
  private func recordActivityDate(
    _ dateString: String,
    for conversationID: String,
    sequence: UInt64
  ) {
    guard !dateString.isEmpty, let date = MessageTimestamp.date(fromISO8601: dateString) else {
      return
    }
    let currentRevision = backendLastActivityRevisions[conversationID] ?? 0
    if sequence >= currentRevision {
      backendLastActivityRevisions[conversationID] = sequence
      backendLastActivityDates[conversationID] = date
    }
  }

  /// Background empty-query load to populate true last activity dates for all verified archives
  /// without clobbering active query filters or racing user search.
  func loadArchiveMetadata() async {
    metadataGeneration &+= 1
    let generation = metadataGeneration
    nextSnapshotSequence &+= 1
    let sequence = nextSnapshotSequence
    isMetadataLoading = true
    metadataError = nil
    isMetadataPartial = false
    metadataErrorCount = 0

    do {
      let response = try await client.search(rootPath: rootPath, query: "")
      guard !Task.isCancelled, generation == metadataGeneration else {
        return
      }
      isMetadataLoading = false
      metadataError = nil
      isMetadataPartial = !response.complete || response.errorCount > 0
      metadataErrorCount = response.errorCount

      for res in response.results {
        recordActivityDate(res.lastActivity, for: res.conversationID, sequence: sequence)
      }
    } catch {
      guard !Task.isCancelled, generation == metadataGeneration else {
        return
      }
      isMetadataLoading = false
      if !(error is CancellationError) {
        metadataError = error.localizedDescription
      }
      isMetadataPartial = false
      metadataErrorCount = 0
    }
  }

  private func queryDidChange() {
    searchTask?.cancel()
    searchTask = nil
    let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty else {
      searchGeneration &+= 1
      isLoading = false
      searchError = nil
      isPartial = false
      errorCount = 0
      matchedConversationIDs = nil
      return
    }

    isLoading = true
    searchError = nil
    searchGeneration &+= 1
    let generation = searchGeneration

    searchTask = Task { [weak self] in
      guard let self else { return }
      if self.debounceDuration > .zero {
        try? await Task.sleep(for: self.debounceDuration)
      }
      guard !Task.isCancelled else { return }
      await self.executeSearch(query: trimmed, generation: generation)
    }
  }

  private func executeSearch(query: String, generation: UInt64) async {
    guard generation == searchGeneration else { return }
    nextSnapshotSequence &+= 1
    let sequence = nextSnapshotSequence
    do {
      let response = try await client.search(rootPath: rootPath, query: query)
      guard generation == searchGeneration else {
        // Discard stale completion!
        return
      }
      isLoading = false
      searchError = nil
      isPartial = !response.complete || response.errorCount > 0
      errorCount = response.errorCount
      matchedConversationIDs = Set(response.results.map(\.conversationID))
      for res in response.results {
        recordActivityDate(res.lastActivity, for: res.conversationID, sequence: sequence)
      }
    } catch {
      guard generation == searchGeneration else { return }
      isLoading = false
      if !(error is CancellationError) {
        searchError = error.localizedDescription
      }
      isPartial = false
      errorCount = 0
      matchedConversationIDs = []
    }
  }

  /// Filters and sorts raw saved conversations according to active query, matched IDs,
  /// backend activity dates, and chosen sort order. Precomputes dates before sorting.
  func filterAndSort(
    _ conversations: [OverlayConversation],
    dateResolver: (OverlayConversation) -> Date?
  ) -> [OverlayConversation] {
    let filtered: [OverlayConversation]
    if let matched = matchedConversationIDs {
      filtered = conversations.filter { matched.contains($0.id) }
    } else {
      filtered = conversations
    }

    func resolvedDate(for conv: OverlayConversation) -> Date? {
      dateResolver(conv) ?? backendLastActivityDates[conv.id]
    }

    switch sortOrder {
    case .newest:
      return
        filtered
        .map { (conversation: $0, date: resolvedDate(for: $0)) }
        .sorted {
          if let left = $0.date, let right = $1.date {
            if left != right { return left > right }
          } else if $0.date != nil {
            return true
          } else if $1.date != nil {
            return false
          }
          return $0.conversation.id < $1.conversation.id
        }
        .map(\.conversation)
    case .oldest:
      return
        filtered
        .map { (conversation: $0, date: resolvedDate(for: $0)) }
        .sorted {
          if let left = $0.date, let right = $1.date {
            if left != right { return left < right }
          } else if $0.date != nil {
            return true
          } else if $1.date != nil {
            return false
          }
          return $0.conversation.id < $1.conversation.id
        }
        .map(\.conversation)
    case .name:
      return filtered.sorted { a, b in
        let comp = a.name.localizedStandardCompare(b.name)
        if comp != .orderedSame {
          return comp == .orderedAscending
        }
        return a.id < b.id
      }
    }
  }
}
