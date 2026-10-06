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
      SettingsPageHeader(String(localized: "License"))
      VStack(spacing: 0) {
        RuntimeRow(
          key: stateRowLabel,
          value: stateLabel,
          tint: model.licenseAllowsAgentMode,
          trailing: .none)
        if model.licenseStatus.sku != nil {
          divider
          RuntimeRow(
            key: String(localized: "Plan"), value: planLabel, tint: false,
            mono: planLabel == model.licenseStatus.sku,
            trailing: .none)
        }
        if let updatesUntil = model.licenseStatus.updatesUntil {
          divider
          RuntimeRow(
            key: String(localized: "Updates through"),
            value: updatesUntil, tint: false,
            mono: true, trailing: .none)
        }
      }
      .padding(.top, CSSpace.section)
      .clipShape(RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous))
      .overlay(
        RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous)
          .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
      )

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

      HStack(spacing: 12) {
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
          Button("Remove key from this Mac", role: .destructive) {
            Task { @MainActor in await model.removeLicense() }
          }
          .csFocusRing()
          .foregroundStyle(CSColor.danger)
          .disabled(model.licenseBusy)
        }
      }
      .padding(.top, 12)

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
    }
    .padding(.horizontal, CSSpace.xl)
    .padding(.vertical, CSSpace.section)
  }

  /// Known SKUs display the operating mode. Unknown identifiers stay visible
  /// so support can identify them without inventing an offer name.
  private var planLabel: String {
    switch model.licenseStatus.sku {
    case nil: String(localized: "Basic mode", comment: "Operating mode: basic transcription")
    case "agentic-lifetime": String(localized: "Agent mode", comment: "Operating mode: agent features")
    case let sku?: sku
    }
  }

  private var stateRowLabel: String {
    if model.licenseReadState == .available, model.licenseStatus.state == .unlicensed {
      return String(localized: "Mode", comment: "License panel: operating mode when no key is stored")
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
      return String(localized: "Basic mode", comment: "Operating mode: basic transcription")
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
