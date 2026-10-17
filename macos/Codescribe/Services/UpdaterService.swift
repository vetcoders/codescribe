import AppKit
import Combine
import OSLog
import Sparkle

// Sparkle 2 update channel, wrapped in the app's service idiom.
//
// Owns the single SPUStandardUpdaterController for the process. The controller
// starts the updater immediately (scheduled background checks per
// SUEnableAutomaticChecks / user consent), and the tray's "Check for Updates…"
// row routes through `checkForUpdates()`.
//
// Feed + key contract (see macos/project.yml):
//   SUFeedURL      → https://codescribe.vetcoders.io/appcast.xml
//   SUPublicEDKey  → injected at build time via SPARKLE_ED_PUBLIC_KEY; an empty
//                    key makes Sparkle reject every update (fail-closed), never
//                    accept an unsigned one.
@MainActor
final class UpdaterService: ObservableObject {
  private static let log = Logger(
    subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
    category: "updater"
  )

  /// False while a check is already in flight or the updater has not started.
  @Published private(set) var canCheckForUpdates = false

  private let controller: SPUStandardUpdaterController
  private let releasePolicy: ReleaseUpdatePolicy
  private var cancellable: AnyCancellable?

  init() {
    let policy = ReleaseUpdatePolicy(
      currentVersion: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString")
        as? String ?? ""
    )
    releasePolicy = policy
    controller = SPUStandardUpdaterController(
      startingUpdater: true,
      updaterDelegate: policy,
      userDriverDelegate: nil
    )
    cancellable = controller.updater.publisher(for: \.canCheckForUpdates)
      .receive(on: DispatchQueue.main)
      .sink { [weak self] in self?.canCheckForUpdates = $0 }

    // E2E verifier hook: a scripted upgrade test can't click the tray, so an
    // env flag triggers the user-initiated check shortly after launch. Not a
    // user-facing surface; release builds without the flag are unaffected.
    if ProcessInfo.processInfo.environment["CODESCRIBE_SPARKLE_CHECK_AT_LAUNCH"] == "1" {
      Self.log.info("sparkle check-at-launch hook armed")
      DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in
        self?.checkForUpdates()
      }
    }
    // Headless variant for the automated N-1→N verifier: a background check
    // (paired with the SUAutomaticallyUpdate defaults override) downloads,
    // verifies the EdDSA signature, and stages the install without any UI.
    if ProcessInfo.processInfo.environment["CODESCRIBE_SPARKLE_BACKGROUND_CHECK"] == "1" {
      Self.log.info("sparkle background-check hook armed")
      DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in
        self?.controller.updater.checkForUpdatesInBackground()
      }
    }
  }

  /// User-initiated check ("Check for Updates…" in the tray). The app is an
  /// LSUIElement accessory, so activate first — otherwise Sparkle's update
  /// window can appear behind the frontmost app.
  func checkForUpdates() {
    NSApp.activate(ignoringOtherApps: true)
    controller.checkForUpdates(nil)
  }
}

/// Sparkle applies OS, channel, rollout and skipped-update filters first.
/// This final selection accepts only canonical release identities: counters
/// from older feeds cannot masquerade as newer product versions.
@MainActor
final class ReleaseUpdatePolicy: NSObject, SPUUpdaterDelegate {
  let currentVersion: String

  init(currentVersion: String) {
    self.currentVersion = currentVersion
  }

  static func accepts(version: String, displayVersion: String, currentVersion: String) -> Bool {
    guard version == displayVersion,
      isReleaseVersion(version), isReleaseVersion(currentVersion)
    else { return false }
    return SUStandardVersionComparator.default.compareVersion(version, toVersion: currentVersion)
      == .orderedDescending
  }

  private static func isReleaseVersion(_ value: String) -> Bool {
    let parts = value.split(separator: ".", omittingEmptySubsequences: false)
    return parts.count == 3 && parts.allSatisfy { part in
      !part.isEmpty && (part.count == 1 || part.first != "0")
        && part.utf8.allSatisfy { (48...57).contains($0) }
    }
  }

  func bestValidUpdate(in appcast: SUAppcast, for updater: SPUUpdater) -> SUAppcastItem? {
    let candidates = appcast.items.filter {
      Self.accepts(
        version: $0.versionString, displayVersion: $0.displayVersionString,
        currentVersion: currentVersion
      )
    }
    return candidates.max {
      SUStandardVersionComparator.default.compareVersion($0.versionString, toVersion: $1.versionString)
        == .orderedAscending
    } ?? SUAppcastItem.empty()
  }
}
