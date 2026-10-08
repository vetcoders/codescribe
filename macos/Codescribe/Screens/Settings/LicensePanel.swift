import AppKit
import SwiftUI

struct LicensePanel: View {
  @ObservedObject var model: SettingsViewModel
  /// Sample key shape, not copy: identical in every language.
  private static let keyPlaceholder = "CSK1.…"

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
      VStack(spacing: 0) {
        RuntimeRow(
          key: stateRowLabel,
          value: stateLabel,
          tint: model.licenseAllowsAgentMode,
          trailing: .none)
        if showsModeRow {
          divider
          RuntimeRow(
            key: String(localized: "Mode", comment: "License panel: operating mode"),
            value: modeLabel(agentMode: model.licenseAllowsAgentMode), tint: false,
            trailing: .none)
        }
        if showsLicenseOfferRow {
          divider
          RuntimeRow(
            key: String(localized: "License"),
            value: licenseOfferLabel, tint: false,
            trailing: .none)
        }
      }
      .clipShape(RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous))
      .overlay(
        RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous)
          .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
      )
      .padding(.top, CSSpace.section)

      SettingsSectionLabel(String(localized: "License key"))
        .padding(.top, CSSpace.section)
      SecureField(Self.keyPlaceholder, text: $key)
        .font(CSFont.mono(11.5, .regular))
        .textFieldStyle(.plain)
        .focused($keyFocused)
        .padding(CSSpace.md)
        .background(Color.primary.opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
        .overlay(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
        )
        .overlay {
          CSFocusOutline(isFocused: keyFocused, cornerRadius: CSRadius.input)
        }
        .padding(.top, CSSpace.control)
        .accessibilityLabel("Codescribe license key")

      HStack(spacing: CSSpace.md) {
        Button("Activate") {
          let submitted = key
          Task { @MainActor in
            if await model.activateLicense(submitted), key == submitted { key = "" }
          }
        }
        .buttonStyle(.borderedProminent)
        .tint(CSColor.chromeAccent)
        .disabled(model.licenseBusy || key.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)

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
      }
      .padding(.top, CSSpace.md)

      if let error = model.licenseError {
        Text(error)
          .font(CSFont.ui(11.5))
          .foregroundStyle(CSColor.danger)
          .padding(.top, 10)
      }
      if model.licenseReadState == .unavailable {
        Button("Try again") { model.refreshLicense() }
          .disabled(model.licenseBusy)
          .padding(.top, 10)
      }
      if let details = model.licenseErrorDetails {
        DisclosureGroup("Details") {
          Text(details)
            .font(CSFont.mono(10.5, .medium))
            .textSelection(.enabled)
        }
        .font(CSFont.ui(11.5))
        .foregroundStyle(Color.secondary)
        .padding(.top, 10)
      }

      Text(
        String(
          localized: "The key is verified locally and stored in the macOS Keychain.",
          comment: "License panel footnote: how the key is checked and where it lives")
      )
      .font(CSFont.ui(11.5))
      .foregroundStyle(Color.secondary)
      .fixedSize(horizontal: false, vertical: true)
      .padding(.top, CSSpace.section)
    }
    .frame(maxWidth: 560, alignment: .leading)
    .padding(.horizontal, CSSpace.xl)
    .padding(.vertical, CSSpace.section)
    .frame(maxWidth: .infinity, alignment: .leading)
  }

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

  private var divider: some View {
    Rectangle().fill(Color.primary.opacity(0.12)).frame(height: 1)
  }
}

#if DEBUG
  #Preview("License panel") {
    ScrollView { LicensePanel(model: .preview(.license)) }
      .frame(width: 720, height: 760)
  }
#endif
