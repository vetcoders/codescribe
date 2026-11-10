import Foundation

/// Copies app-scoped preferences once when the renamed product is first launched.
@MainActor
enum AppPreferencesMigration {
  static let sourceBundleIdentifier = "com.vetcoders.codescribe"
  static let targetBundleIdentifier = "com.vetcoders.x9bus"
  static let migrationMarker = "x9bus.preferences.migrated-from-codescribe.v1"

  static func migrateIfNeeded(bundleIdentifier: String?, defaults: UserDefaults = .standard) {
    guard bundleIdentifier == targetBundleIdentifier else { return }
    let source = defaults.persistentDomain(forName: sourceBundleIdentifier) ?? [:]
    let destination = defaults.persistentDomain(forName: targetBundleIdentifier) ?? [:]
    guard let merged = mergedPreferences(source: source, destination: destination) else { return }
    defaults.setPersistentDomain(merged, forName: targetBundleIdentifier)
  }

  static func mergedPreferences(
    source: [String: Any], destination: [String: Any]
  ) -> [String: Any]? {
    guard destination[migrationMarker] == nil else { return nil }
    var merged = destination
    for (key, value) in source where key != migrationMarker && merged[key] == nil {
      merged[key] = value
    }
    merged[migrationMarker] = true
    return merged
  }
}
