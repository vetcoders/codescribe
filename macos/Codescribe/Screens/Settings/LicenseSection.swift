import AppKit
import SwiftUI

/// Settings › User › License, laid out per the Founder-approved card
/// (2026-10-20): a header, one sentence on what a key unlocks, then one calm
/// card. The card's head names what runs — "Codescribe Agent" or Basic mode —
/// with the purchase kind under it and one status on the right. With a key,
/// a key row carries the offline countdown and the Keychain note, and
/// "Manage key" opens Change key / Get license key / Remove key inside the
/// same card. Without a key the activation field is open at once. Errors,
/// details and Try again stay under the card.
struct LicenseSection: View {
  @ObservedObject var model: SettingsViewModel
  /// Sample key shape, not copy: identical in every language.
  private static let keyPlaceholder = "CSK1.…"
  /// The key is one control on the activation row, not a full-width well
  /// (Founder brief, round 16, 2026-10-10).
  private static let keyFieldWidth: CGFloat = 260
  /// Product name, not copy: identical in every language.
  private static let agentProductName = "Codescribe Agent"
  /// The one SKU whose purchase kind has an on-screen name.
  private static let lifetimeSku = "agentic-lifetime"
  private static let licensePageURL = "https://codescribe.vetcoders.io/license/"

  /// Draft of a key being typed; never the stored key.
  @State private var key = ""
  @FocusState private var keyFocused: Bool
  /// "Manage key" is expanded: the management band shows inside the card.
  @State private var managingKey = false
  /// "Change key" is open: the replacement-key editor shows in the band.
  @State private var changingKey = false
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

      if let error = model.licenseError {
        Text(error)
          .font(SettingsScale.helper)
          .foregroundStyle(CSColor.dangerLight)
          .fixedSize(horizontal: false, vertical: true)
          .padding(.top, CSSpace.md)
      }
      if model.licenseReadState == .unavailable {
        Button("Try again") { model.refreshLicense() }
          .disabled(model.licenseBusy)
          .padding(.top, CSSpace.md)
      }
      if let details = model.licenseErrorDetails {
        DisclosureGroup("Details") {
          Text(details)
            .font(CSFont.mono(11, .medium))
            .textSelection(.enabled)
        }
        .font(SettingsScale.helper)
        .foregroundStyle(CSColor.secondaryInk)
        .padding(.top, CSSpace.md)
      }
    }
    // A removed key returns the card to its empty state; nothing of the
    // management band survives into it.
    .onChange(of: hasKey) { _, present in
      if !present { closeManagement() }
    }
  }

  // MARK: - Card

  private var card: some View {
    VStack(alignment: .leading, spacing: 0) {
      summary
        .padding(SettingsScale.cardPadding)

      if hasKey {
        divider.padding(.horizontal, SettingsScale.cardPadding)
        keyRow
          .padding(SettingsScale.cardPadding)
        if managingKey {
          managementBand
        }
      } else {
        divider
        keyEditor(
          label: String(
            localized: "License key",
            comment: "License card, empty state: label above the activation field"),
          cancellable: false
        )
        .padding(SettingsScale.cardPadding)
        divider
        actionsRow
          .padding(SettingsScale.cardPadding)
      }
    }
    .clipShape(RoundedRectangle(cornerRadius: SettingsScale.cardRadius, style: .continuous))
    .settingsGroupedInset(padding: 0)
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

  /// The saved key: the offline countdown when it runs, the Keychain note,
  /// and the one entry into key management.
  private var keyRow: some View {
    HStack(alignment: .center, spacing: CSSpace.md) {
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
      Spacer(minLength: CSSpace.md)
      SettingsChipButton(action: toggleManagement) {
        HStack(spacing: CSSpace.xs) {
          Text(manageKeyTitle).font(SettingsScale.controlLabel)
          CSIconView(icon: .chevronRight, size: 9, weight: .semibold)
            .rotationEffect(.degrees(managingKey ? 90 : 0))
            .accessibilityHidden(true)
        }
      }
      .accessibilityLabel(manageKeyTitle)
      .accessibilityValue(
        managingKey
          ? String(localized: "Expanded", comment: "License card: key management is open")
          : String(localized: "Collapsed", comment: "License card: key management is closed"))
    }
  }

  /// Key management inside the card: Change key, then the editor when it is
  /// open, then Get license key and, set apart on the right, Remove key.
  private var managementBand: some View {
    VStack(alignment: .leading, spacing: 0) {
      divider
      HStack(alignment: .center, spacing: CSSpace.md) {
        Text(
          String(
            localized: "Key management",
            comment: "License card: heading of the expanded key management")
        )
        .font(SettingsScale.settingName)
        .foregroundStyle(Color.primary)
        Spacer(minLength: CSSpace.md)
        if !changingKey {
          let title = String(
            localized: "Change key", comment: "License card: opens the field for a new key")
          SettingsChipButton(enabled: !model.licenseBusy, action: { changingKey = true }) {
            Text(title).font(SettingsScale.controlLabel)
          }
          .accessibilityLabel(title)
        }
      }
      .padding(SettingsScale.cardPadding)

      if changingKey {
        keyEditor(
          label: String(
            localized: "New license key",
            comment: "License card, Change key: label above the replacement-key field"),
          cancellable: true
        )
        .padding(.horizontal, SettingsScale.cardPadding)
        .padding(.bottom, SettingsScale.cardPadding)
      }

      divider
      actionsRow
        .padding(SettingsScale.cardPadding)
    }
    .background(Color.primary.opacity(CSAlpha.subtle))
  }

  // MARK: - Activation

  /// Label, key field and Activate; Cancel only when replacing a working key.
  private func keyEditor(label: String, cancellable: Bool) -> some View {
    VStack(alignment: .leading, spacing: CSSpace.xs) {
      Text(label)
        .font(SettingsScale.helper)
        .foregroundStyle(CSColor.secondaryInk)
        .accessibilityHidden(true)
      HStack(alignment: .center, spacing: CSSpace.sm) {
        SecureField(Self.keyPlaceholder, text: $key)
          .font(CSFont.mono(12, .regular))
          .textFieldStyle(.plain)
          .focused($keyFocused)
          .padding(.horizontal, CSSpace.md)
          .padding(.vertical, CSSpace.sm)
          .background(Color.primary.opacity(CSAlpha.line))
          .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
          .overlay(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
              .strokeBorder(Color.primary.opacity(CSAlpha.stroke), lineWidth: 1)
          )
          .overlay {
            CSFocusOutline(isFocused: keyFocused, cornerRadius: CSRadius.input)
          }
          .frame(maxWidth: Self.keyFieldWidth)
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

        Spacer(minLength: 0)
      }
    }
  }

  /// Get license key on the left; Remove key, only with a saved key, set
  /// apart on the right.
  private var actionsRow: some View {
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

      Spacer(minLength: CSSpace.md)

      if hasKey {
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
    .confirmationDialog(
      String(
        localized: "Remove the license key?",
        comment: "License card: title of the confirmation before removing the key"),
      isPresented: $confirmingRemove,
      titleVisibility: .visible
    ) {
      Button("Remove key", role: .destructive) {
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

  private var divider: some View {
    Rectangle().fill(Color.primary.opacity(CSAlpha.stroke)).frame(height: 1)
  }

  // MARK: - Actions

  private func activate() {
    let submitted = key
    Task { @MainActor in
      if await model.activateLicense(submitted), key == submitted {
        key = ""
        closeManagement()
      }
    }
  }

  private func openLicensePage() {
    if let url = URL(string: Self.licensePageURL) {
      NSWorkspace.shared.open(url)
    }
  }

  private func toggleManagement() {
    if managingKey {
      closeManagement()
    } else {
      managingKey = true
    }
  }

  /// Closes the editor and drops the draft; the saved key is untouched.
  private func closeEditor() {
    key = ""
    changingKey = false
  }

  private func closeManagement() {
    closeEditor()
    managingKey = false
  }

  // MARK: - Derived labels

  /// A saved key exists (in any verified state, including lapsed ones).
  private var hasKey: Bool { model.licenseStatus.state != .unlicensed }

  /// The license works right now: active, or active on offline grace.
  private var isActive: Bool {
    guard model.licenseReadState == .available else { return false }
    switch model.licenseStatus.state {
    case .active, .graceOffline: return true
    case .unlicensed, .expiredUpdates: return false
    }
  }

  private var manageKeyTitle: String {
    String(localized: "Manage key", comment: "License card: opens key management inside the card")
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
