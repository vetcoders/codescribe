import AppKit
import SwiftUI

/// Settings › User › License: a header, one sentence on what a key unlocks,
/// then one card with one layout (Founder spec and mockups, 2026-10-29). The
/// head names what runs — "Codescribe Agent" or Basic mode — with the purchase
/// kind under it and one status on the right. What the card offers depends on
/// the license state: with a saved key, Change key and Remove key; without a
/// key, the activation field and Get license key; when the saved license
/// cannot be read, the problem with Try again and its details. Every message
/// sits where its operation happened, inside the card.
struct LicenseSection: View {
  @ObservedObject var model: SettingsViewModel
  /// Product name, not copy: identical in every language.
  private static let agentProductName = "Codescribe Agent"
  /// The one SKU whose purchase kind has an on-screen name.
  private static let lifetimeSku = "agentic-lifetime"
  private static let licensePageURL = "https://codescribe.vetcoders.io/license/"

  /// What the card shows, projected from the service's state.
  private enum Phase {
    /// No verified key yet and the saved license is still being read.
    case checking
    /// The saved license could not be read or verified. Not "no key".
    case unknown
    /// Read successfully, and no key is saved.
    case noKey
    /// A key is saved, in any verified state, including lapsed ones.
    case saved
  }

  /// The operation the user started from this card, so its failure is shown
  /// at its own place; a read failure has no operation here.
  private enum Operation { case activate, remove }

  /// Draft of a key being typed; never the stored key.
  @State private var key = ""
  @FocusState private var keyFocused: Bool
  /// "Change key" is open: the replacement-key field shows in the card.
  @State private var changingKey = false
  /// An activation started here is running.
  @State private var activating = false
  @State private var lastOperation: Operation?
  /// "Remove key" asked first: the stored key is never shown, so a removal
  /// the user did not mean cannot be undone from the app.
  @State private var confirmingRemove = false

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsSectionHeaderRow(String(localized: "License"))

      Text(
        String(
          localized: "A license unlocks Agent mode.",
          comment: "User panel, License section: what a license unlocks")
      )
      .font(SettingsScale.helper)
      .foregroundStyle(CSColor.secondaryInk)
      .fixedSize(horizontal: false, vertical: true)
      .padding(.top, CSSpace.xxs)

      card
        .padding(.top, SettingsScale.headerGap)
    }
    // A removed key returns the card to its empty state; no replacement
    // draft survives into it.
    .onChange(of: phase) { _, current in
      if current == .noKey || current == .checking { closeEditor() }
    }
    // A new read owns whatever it reports; an older operation's place for
    // messages no longer applies.
    .onChange(of: model.licenseReadState) { _, state in
      if state == .loading { lastOperation = nil }
    }
  }

  // MARK: - Card

  private var card: some View {
    VStack(alignment: .leading, spacing: 0) {
      VStack(alignment: .leading, spacing: CSSpace.md) {
        summary
        if phase == .saved {
          keyInfo
        }
      }
      .padding(SettingsScale.cardPadding)

      switch phase {
      case .checking:
        EmptyView()
      case .unknown:
        problemBlock
        divider
        keyArea
          .padding(SettingsScale.cardPadding)
      case .noKey:
        divider
        keyEditor(
          label: String(
            localized: "License key",
            comment: "License card, empty state: label above the activation field"),
          cancellable: false
        )
        .padding(SettingsScale.cardPadding)
        divider
        getKeyRow
          .padding(SettingsScale.cardPadding)
      case .saved:
        divider
        keyArea
          .padding(SettingsScale.cardPadding)
        problemBlock
      }
    }
    .clipShape(RoundedRectangle(cornerRadius: SettingsScale.cardRadius, style: .continuous))
    .settingsGroupedInset(padding: 0)
    .confirmationDialog(
      String(
        localized: "Remove the license key?",
        comment: "License card: title of the confirmation before removing the key"),
      isPresented: $confirmingRemove,
      titleVisibility: .visible
    ) {
      Button("Remove key", role: .destructive) {
        lastOperation = .remove
        Task { @MainActor in await model.removeLicense() }
      }
      Button("Cancel", role: .cancel) {}
    } message: {
      Text(
        String(
          localized:
            "Codescribe goes back to Basic mode and Agent mode stops working. The key is deleted from this Mac's Keychain. To activate again you will need the key itself, which Codescribe cannot show, or a new key from the license page.",
          comment: "License card: what removing the key does, in the confirmation"))
    }
  }

  /// Product name and purchase kind on the left, one status on the right.
  private var summary: some View {
    HStack(alignment: .firstTextBaseline, spacing: CSSpace.md) {
      VStack(alignment: .leading, spacing: CSSpace.xxs) {
        Text(productName)
          .font(SettingsScale.sectionTitle)
          .foregroundStyle(Color.primary)
          .fixedSize(horizontal: false, vertical: true)
        if let purchase = purchaseLine {
          Text(purchase)
            .font(SettingsScale.helper)
            .foregroundStyle(CSColor.secondaryInk)
            .fixedSize(horizontal: false, vertical: true)
        }
      }
      Spacer(minLength: CSSpace.md)
      statusBadge
    }
  }

  /// Green dot and "Active" in olive on a soft capsule while the license
  /// works; every other state is plain secondary text.
  private var statusBadge: some View {
    HStack(alignment: .center, spacing: CSSpace.xs) {
      if isActive {
        Circle()
          .fill(CSColor.oliveLight)
          .frame(width: 7, height: 7)
      }
      Text(statusLabel)
        .font(SettingsScale.controlLabel)
        .foregroundStyle(isActive ? CSColor.oliveLight : CSColor.secondaryInk)
        .lineLimit(1)
    }
    .padding(.horizontal, isActive ? SettingsScale.controlInsetH : 0)
    .padding(.vertical, isActive ? SettingsScale.controlInsetV : 0)
    .background {
      if isActive {
        Capsule().fill(CSColor.oliveLight.opacity(0.12))
      }
    }
    .fixedSize()
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(
      String(localized: "License status", comment: "License card: accessibility label of the status"))
    .accessibilityValue(statusLabel)
  }

  /// The saved key: the offline countdown when it runs, then the Keychain note.
  private var keyInfo: some View {
    VStack(alignment: .leading, spacing: CSSpace.xxs) {
      if let remaining = remainingLine {
        Text(remaining)
          .font(SettingsScale.settingName)
          .foregroundStyle(Color.primary)
          .fixedSize(horizontal: false, vertical: true)
      }
      Text(
        String(
          localized: "Key stored in the macOS Keychain",
          comment: "License card, key row: where the saved key lives")
      )
      .font(SettingsScale.helper)
      .foregroundStyle(CSColor.secondaryInk)
      .fixedSize(horizontal: false, vertical: true)
    }
  }

  /// With a saved key: Change key and Remove key, or the replacement-key
  /// field while Change key is open.
  @ViewBuilder
  private var keyArea: some View {
    if changingKey {
      keyEditor(
        label: String(
          localized: "New license key",
          comment: "License card, Change key: label above the replacement-key field"),
        cancellable: true
      )
    } else {
      keyActionsRow
    }
  }

  /// Change key on the left; Remove key, in red, set apart on the right.
  private var keyActionsRow: some View {
    HStack(alignment: .center, spacing: CSSpace.md) {
      let title = String(
        localized: "Change key", comment: "License card: opens the field for a new key")
      SettingsChipButton(enabled: !model.licenseBusy, action: openEditor) {
        Text(title).font(SettingsScale.controlLabel)
      }
      .accessibilityLabel(title)

      Spacer(minLength: CSSpace.md)

      SettingsChipButton {
        confirmingRemove = true
      } label: {
        Text("Remove key")
          .font(SettingsScale.controlLabel)
          .foregroundStyle(CSColor.dangerLight)
      }
      .disabled(model.licenseBusy)
    }
  }

  /// Without a key: where a key comes from.
  private var getKeyRow: some View {
    HStack(alignment: .center, spacing: CSSpace.md) {
      // Self-service issuance: codescribe.vetcoders.io/license/ mints a
      // signed key for an email on the spot (open beta). Without this
      // button the section demanded a key and never said where one comes
      // from (operator, 2026-08-09).
      SettingsChipButton(action: openLicensePage) {
        Text("Get license key").font(SettingsScale.controlLabel)
      }
      .help("Open the license-key page")
      .accessibilityIdentifier("settings-license-get")

      Spacer(minLength: 0)
    }
  }

  // MARK: - Activation

  /// Label, key field and Activate; Cancel only when replacing a saved key.
  /// Under the field: the running activation, or what went wrong with it.
  private func keyEditor(label: String, cancellable: Bool) -> some View {
    VStack(alignment: .leading, spacing: CSSpace.xs) {
      Text(label)
        .font(SettingsScale.helper)
        .foregroundStyle(CSColor.secondaryInk)
        .accessibilityHidden(true)
      HStack(alignment: .center, spacing: CSSpace.sm) {
        SecureField(
          String(
            localized: "Paste license key",
            comment: "License card: placeholder of the license-key field"),
          text: $key
        )
        .font(CSFont.mono(12, .regular))
        .textFieldStyle(.plain)
        .focused($keyFocused)
        .padding(.horizontal, CSSpace.md)
        .padding(.vertical, CSSpace.sm)
        .background(Color.primary.opacity(CSAlpha.line))
        .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
        .overlay(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .strokeBorder(
              fieldError == nil ? Color.primary.opacity(CSAlpha.stroke) : CSColor.dangerLight,
              lineWidth: 1)
        )
        .overlay {
          CSFocusOutline(isFocused: keyFocused, cornerRadius: CSRadius.input)
        }
        .frame(maxWidth: .infinity)
        .accessibilityLabel("Codescribe license key")

        Button("Activate", action: activate)
          .buttonStyle(.borderedProminent)
          .tint(CSColor.chromeAccent)
          .disabled(
            model.licenseBusy || key.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)

        if cancellable {
          let title = String(
            localized: "Cancel", comment: "License card: closes the new-key field, the saved key stays")
          SettingsChipButton(action: closeEditor) {
            Text(title).font(SettingsScale.controlLabel)
          }
          .accessibilityLabel(title)
        }
      }

      if activating {
        HStack(alignment: .center, spacing: CSSpace.xs) {
          ProgressView().controlSize(.small)
          Text(
            String(
              localized: "Activating the key…",
              comment: "License card: an activation is running; no result yet"))
        }
        .font(SettingsScale.helper)
        .foregroundStyle(CSColor.secondaryInk)
        .padding(.top, CSSpace.xxs)
      } else if let message = fieldError {
        VStack(alignment: .leading, spacing: CSSpace.xs) {
          Text(message)
            .foregroundStyle(CSColor.dangerLight)
            .fixedSize(horizontal: false, vertical: true)
          if cancellable {
            Text(
              String(
                localized: "Your current key is still saved.",
                comment: "License card, Change key: a failed new key left the saved key in place")
            )
            .foregroundStyle(CSColor.secondaryInk)
            .fixedSize(horizontal: false, vertical: true)
          }
          details
        }
        .font(SettingsScale.helper)
        .padding(.top, CSSpace.xxs)
      }
    }
    // Editing the key starts a correction; the old verdict no longer applies.
    .onChange(of: key) { _, _ in
      if lastOperation == .activate, !activating { lastOperation = nil }
    }
  }

  /// The failed removal (Try again asks again), else the failed read of the
  /// saved license, each under a divider; nothing when neither failed.
  @ViewBuilder
  private var problemBlock: some View {
    if let message = removeError {
      divider
      problem(message, retry: { confirmingRemove = true })
        .padding(SettingsScale.cardPadding)
    } else if let message = readError {
      divider
      problem(message, retry: retryRead)
        .padding(SettingsScale.cardPadding)
    }
  }

  /// A failed read or removal: what failed, Try again, and the details.
  private func problem(_ message: String, retry: @escaping () -> Void) -> some View {
    VStack(alignment: .leading, spacing: CSSpace.sm) {
      Text(message)
        .font(SettingsScale.helper)
        .foregroundStyle(CSColor.dangerLight)
        .fixedSize(horizontal: false, vertical: true)
      SettingsChipButton(enabled: !model.licenseBusy, action: retry) {
        Text("Try again").font(SettingsScale.controlLabel)
      }
      details
    }
  }

  @ViewBuilder
  private var details: some View {
    if let details = model.licenseErrorDetails {
      DisclosureGroup("Details") {
        Text(details)
          .font(CSFont.mono(11, .medium))
          .textSelection(.enabled)
      }
      .font(SettingsScale.helper)
      .foregroundStyle(CSColor.secondaryInk)
    }
  }

  private var divider: some View {
    Rectangle().fill(Color.primary.opacity(CSAlpha.stroke)).frame(height: 1)
  }

  // MARK: - Actions

  private func activate() {
    let submitted = key
    lastOperation = .activate
    activating = true
    Task { @MainActor in
      let activated = await model.activateLicense(submitted)
      activating = false
      if activated, key == submitted {
        closeEditor()
      }
    }
  }

  private func retryRead() {
    lastOperation = nil
    model.refreshLicense()
  }

  private func openLicensePage() {
    if let url = URL(string: Self.licensePageURL) {
      NSWorkspace.shared.open(url)
    }
  }

  private func openEditor() {
    lastOperation = nil
    changingKey = true
  }

  /// Closes the editor and drops the draft; the saved key is untouched.
  private func closeEditor() {
    key = ""
    changingKey = false
    lastOperation = nil
  }

  // MARK: - Derived state

  private var phase: Phase {
    guard model.licenseStatus.state == .unlicensed else { return .saved }
    switch model.licenseReadState {
    case .loading: return .checking
    case .unavailable: return .unknown
    case .available: return .noKey
    }
  }

  /// The failed activation's message, shown under the key field.
  private var fieldError: String? {
    lastOperation == .activate ? model.licenseError : nil
  }

  /// The failed removal's message, shown under Change key / Remove key.
  private var removeError: String? {
    lastOperation == .remove ? model.licenseError : nil
  }

  /// The failed read or verification of the saved license.
  private var readError: String? {
    guard lastOperation == nil, model.licenseReadState == .unavailable else { return nil }
    return model.licenseError
  }

  /// The license works right now: active, or active on offline grace.
  private var isActive: Bool {
    guard model.licenseReadState == .available else { return false }
    switch model.licenseStatus.state {
    case .active, .graceOffline: return true
    case .unlicensed, .expiredUpdates: return false
    }
  }

  /// What runs: the Agent product when the license allows it, Basic mode
  /// otherwise — never the raw SKU.
  private var productName: String {
    model.licenseAllowsAgentMode
      ? Self.agentProductName
      : String(
        localized: "Basic mode",
        comment: "License card: product name when no license allows Agent mode")
  }

  /// Under the product name: the purchase kind, or why Agent mode is off.
  /// A key that works but whose SKU does not include Agent mode keeps that
  /// one distinct fact visible; no other SKU gets an invented name.
  private var purchaseLine: String? {
    if model.licenseAllowsAgentMode {
      guard model.licenseStatus.sku == Self.lifetimeSku else { return nil }
      return String(
        localized: "One-time purchase", comment: "License card: purchase kind of the lifetime license")
    }
    guard model.licenseReadState == .available else { return nil }
    switch model.licenseStatus.state {
    case .active, .graceOffline:
      return String(
        localized: "This license does not include Agent mode",
        comment: "License card: the key works, but its SKU does not unlock Agent mode")
    case .unlicensed, .expiredUpdates:
      return String(
        localized: "No active license", comment: "License card: no key, or the key is inactive")
    }
  }

  /// Offline grace only: days left of the 30-day window since the last
  /// successful validation, after which the license reads Inactive. Not
  /// license expiry, not an updates window, not a trial.
  private var remainingLine: String? {
    guard model.licenseStatus.state == .graceOffline else { return nil }
    let daysLeft = Int(model.licenseStatus.daysLeft ?? 0)
    return String(
      localized: "Works offline for \(daysLeft) more days",
      comment:
        "License card, key row: days left of offline grace since the last successful validation; not license expiry")
  }

  private var statusLabel: String {
    if model.licenseReadState == .loading {
      return String(localized: "Checking license…")
    }
    if model.licenseStatus.state == .unlicensed, model.licenseReadState == .unavailable {
      return String(localized: "Unknown")
    }
    switch model.licenseStatus.state {
    case .active, .graceOffline:
      return String(localized: "Active", comment: "License status: active")
    case .unlicensed, .expiredUpdates:
      return String(
        localized: "Inactive", comment: "License status: no key, or a time limit has elapsed")
    }
  }
}
