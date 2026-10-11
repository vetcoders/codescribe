import AppKit
import SwiftUI

struct LicensePanel: View {
  @ObservedObject var model: SettingsViewModel
  /// Sample key shape, not copy: identical in every language.
  private static let keyPlaceholder = "CSK1.…"
  /// The key is one control on the activation row, not a full-width well
  /// (Founder brief, round 16, 2026-10-10).
  private static let keyFieldWidth: CGFloat = 260
  /// Label column of the status card; the longest Polish label ("Licencja")
  /// sets it once for every row.
  private static let statusLabelWidth: CGFloat = 96

  @State private var key = ""
  @FocusState private var keyFocused: Bool

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      SettingsPageHeader(
        String(localized: "License"),
        blurb: String(
          localized: "Basic mode stays free. A license unlocks Agent mode.",
          comment: "License panel: what is available without a key and what a license unlocks")
      )

      statusCard
        .padding(.top, CSSpace.section)

      ProvidersSectionHeader(String(localized: "License key"))
        .padding(.top, CSSpace.section)

      activationRow
        .padding(.top, CSSpace.control)

      secondaryActions
        .padding(.top, CSSpace.md)

      if let error = model.licenseError {
        Text(error)
          .font(CSFont.ui(11.5))
          .foregroundStyle(CSColor.danger)
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
            .font(CSFont.mono(10.5, .medium))
            .textSelection(.enabled)
        }
        .font(CSFont.ui(11.5))
        .foregroundStyle(Color.secondary)
        .padding(.top, CSSpace.md)
      }

      Text(
        String(
          localized: "The key is stored in the macOS Keychain.",
          comment: "License panel footnote: where the key lives")
      )
      .font(CSFont.ui(11.5))
      .foregroundStyle(Color.secondary)
      .fixedSize(horizontal: false, vertical: true)
      .padding(.top, CSSpace.lg)
    }
    .frame(maxWidth: 560, alignment: .leading)
    .padding(.horizontal, CSSpace.xl)
    .padding(.vertical, CSSpace.section)
    .frame(maxWidth: .infinity, alignment: .leading)
  }

  // MARK: - Status

  /// One card for the whole license truth: the state — carrying the remaining
  /// period while the key runs on offline grace — then the operating mode and,
  /// for the one-time offer, what was purchased. Rows appear under the same
  /// conditions as before, so no state gains or loses a fact here.
  private var statusCard: some View {
    VStack(alignment: .leading, spacing: CSSpace.xs) {
      HStack(alignment: .center, spacing: CSSpace.md) {
        Text(stateRowLabel)
          .font(.subheadline)
          .foregroundStyle(Color.secondary)
          .frame(width: Self.statusLabelWidth, alignment: .leading)
        HStack(alignment: .center, spacing: CSSpace.sm) {
          Circle()
            .fill(model.licenseAllowsAgentMode ? CSColor.oliveLight : Color.secondary.opacity(0.4))
            .frame(width: 7, height: 7)
            .accessibilityHidden(true)
          Text(stateLabel)
            .font(.body.weight(.semibold))
            .foregroundStyle(Color.primary)
            .lineLimit(2)
            .fixedSize(horizontal: false, vertical: true)
        }
        Spacer(minLength: 0)
      }
      .accessibilityElement(children: .ignore)
      .accessibilityLabel(stateRowLabel)
      .accessibilityValue(stateLabel)

      if showsModeRow {
        statusRow(
          String(localized: "Mode", comment: "License panel: operating mode"),
          modeLabel(agentMode: model.licenseAllowsAgentMode))
      }
      if showsLicenseOfferRow {
        statusRow(String(localized: "License"), licenseOfferLabel)
      }
    }
    .settingsGroupedInset()
  }

  private func statusRow(_ label: String, _ value: String) -> some View {
    HStack(alignment: .firstTextBaseline, spacing: CSSpace.md) {
      Text(label)
        .font(.subheadline)
        .foregroundStyle(Color.secondary)
        .frame(width: Self.statusLabelWidth, alignment: .leading)
      Text(value)
        .font(.callout)
        .foregroundStyle(Color.primary)
        .fixedSize(horizontal: false, vertical: true)
      Spacer(minLength: 0)
    }
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(label)
    .accessibilityValue(value)
  }

  // MARK: - Activation

  private var activationRow: some View {
    HStack(alignment: .center, spacing: CSSpace.sm) {
      SecureField(Self.keyPlaceholder, text: $key)
        .font(CSFont.mono(11.5, .regular))
        .textFieldStyle(.plain)
        .focused($keyFocused)
        .padding(.horizontal, CSSpace.md)
        .padding(.vertical, CSSpace.sm)
        .background(Color.primary.opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
        .overlay(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
        )
        .overlay {
          CSFocusOutline(isFocused: keyFocused, cornerRadius: CSRadius.input)
        }
        .frame(maxWidth: Self.keyFieldWidth)
        .accessibilityLabel("Codescribe license key")

      Button("Activate") {
        let submitted = key
        Task { @MainActor in
          if await model.activateLicense(submitted), key == submitted { key = "" }
        }
      }
      .buttonStyle(.borderedProminent)
      .tint(CSColor.chromeAccent)
      .disabled(model.licenseBusy || key.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)

      Spacer(minLength: 0)
    }
  }

  /// The two actions that are not activation keep their own row below it.
  private var secondaryActions: some View {
    HStack(spacing: CSSpace.md) {
      // Self-service issuance: codescribe.vetcoders.io/license/ mints a
      // signed key for an email on the spot (open beta). Without this
      // button the panel demanded a key and never said where one comes
      // from (operator, 2026-08-09).
      Button("Get license key") {
        if let url = URL(string: "https://codescribe.vetcoders.io/license/") {
          NSWorkspace.shared.open(url)
        }
      }
      .buttonStyle(.bordered)
      .help("Open the license-key page")
      .accessibilityIdentifier("settings-license-get")

      if model.licenseStatus.state != .unlicensed {
        Button("Remove key", role: .destructive) {
          Task { @MainActor in await model.removeLicense() }
        }
        .csFocusRing()
        .foregroundStyle(CSColor.danger)
        .disabled(model.licenseBusy)
      }

      Spacer(minLength: 0)
    }
  }

  // MARK: - Derived labels

  /// Effective operating mode, derived from whether the current license
  /// allows Agent mode — never the raw SKU, which never renders on screen.
  private func modeLabel(agentMode: Bool) -> String {
    agentMode
      ? String(
        localized: "license.mode.agent", defaultValue: "Agent",
        comment: "License panel, Mode row value: agent operating mode")
      : String(
        localized: "license.mode.basic", defaultValue: "Basic",
        comment: "License panel, Mode row value: basic operating mode")
  }

  /// Name of the purchased offer; shown only for the lifetime agent SKU, so
  /// no other identifier ever needs an on-screen name invented for it.
  private var licenseOfferLabel: String {
    String(
      localized: "license.offer.agentLifetime", defaultValue: "Agent · one-time purchase",
      comment: "License panel, License row value: name of the purchased offer")
  }

  /// No key yet, but the license state is readable: the Status row is
  /// replaced by a single combined Mode row (always Basic in that state).
  private var isCombinedModeRow: Bool {
    model.licenseReadState == .available && model.licenseStatus.state == .unlicensed
  }

  /// Separate Mode row, shown once a license state beyond "no key" is known.
  /// Loading/unavailable show only the Status row; the no-key state shows
  /// only the combined row above — never both.
  private var showsModeRow: Bool {
    model.licenseReadState == .available && !isCombinedModeRow
  }

  private var showsLicenseOfferRow: Bool {
    showsModeRow && model.licenseStatus.sku == "agentic-lifetime"
  }

  private var stateRowLabel: String {
    if isCombinedModeRow {
      return String(localized: "Mode", comment: "License panel: operating mode")
    }
    return String(localized: "Status", comment: "License panel: current license status")
  }

  private var stateLabel: String {
    if model.licenseReadState == .loading {
      return String(localized: "Checking license…")
    }
    if model.licenseStatus.state == .unlicensed, model.licenseReadState == .unavailable {
      return String(localized: "Unknown")
    }
    switch model.licenseStatus.state {
    case .unlicensed:
      return modeLabel(agentMode: false)
    case .active:
      return String(localized: "Active", comment: "License status: active")
    case .graceOffline:
      let daysLeft = Int(model.licenseStatus.daysLeft ?? 0)
      return String(
        localized: "Active · \(daysLeft) days left",
        comment:
          "License status; the count is days left in the local activation period, not days without internet")
    case .expiredUpdates:
      return String(localized: "Inactive", comment: "License status: a time limit has elapsed")
    }
  }
}

#if DEBUG
  #Preview("License panel") {
    ScrollView { LicensePanel(model: .preview(.license)) }
      .frame(width: 720, height: 760)
  }
#endif
