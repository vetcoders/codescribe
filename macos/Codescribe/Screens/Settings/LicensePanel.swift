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
        String(localized: "Basic stays free."),
        blurb: String(
          localized:
            "A signed CSK1 key unlocks the Agentic lane. Validation is local and the key stays in the macOS Keychain."
        )
      )

      SettingsSectionLabel(String(localized: "License status"))
        .padding(.top, CSSpace.section)
      VStack(spacing: 0) {
        RuntimeRow(
          key: String(localized: "State"), value: stateLabel,
          tint: model.licenseStatus.agenticEntitled,
          trailing: .none)
        divider
        RuntimeRow(
          key: String(localized: "SKU"), value: model.licenseStatus.sku ?? "Basic", tint: false,
          mono: true,
          trailing: .none)
        divider
        RuntimeRow(
          key: String(localized: "Updates through"),
          value: model.licenseStatus.updatesUntil ?? "—", tint: false,
          mono: true, trailing: .none)
      }
      .padding(.top, CSSpace.control)
      .clipShape(RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous))
      .overlay(
        RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous)
          .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
      )

      SettingsSectionLabel(String(localized: "Enter or restore key"))
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
        Button("Activate / Restore") {
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
        Button("Get license") {
          if let url = URL(string: "https://codescribe.vetcoders.io/license/") {
            NSWorkspace.shared.open(url)
          }
        }
        .buttonStyle(.bordered)
        .help("Open codescribe.vetcoders.io/license — enter your email, paste the key back here")
        .accessibilityIdentifier("settings-license-get")

        if model.licenseStatus.state != .unlicensed {
          Button("Remove license", role: .destructive) {
            Task { @MainActor in await model.removeLicense() }
          }
          .csFocusRing()
          .foregroundStyle(CSColor.danger)
          .disabled(model.licenseBusy)
        }
      }
      .padding(.top, 12)

      if model.licenseReadState != .available {
        Text(model.licenseReadState == .loading
          ? String(localized: "Checking license…")
          : String(localized: "License access is unavailable. The last verified license still follows its original expiry."))
          .font(CSFont.ui(11.5))
          .padding(.top, 10)
      }
      Button("Retry license access") { model.refreshLicense() }
        .disabled(model.licenseBusy)
        .padding(.top, 10)

      if let error = model.licenseError {
        Text(error)
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(CSColor.danger)
          .padding(.top, 10)
          .textSelection(.enabled)
      }

      Text(
        "Codescribe does not phone home while you work. A future fulfillment service may refresh the validation timestamp explicitly; offline grace is 30 days."
      )
      .font(CSFont.ui(11.5))
      .lineSpacing(2)
      .foregroundStyle(Color.secondary)
      .padding(.top, 18)
    }
    .padding(.horizontal, CSSpace.xl)
    .padding(.vertical, CSSpace.section)
  }

  private var stateLabel: String {
    if model.licenseStatus.state == .unlicensed, model.licenseReadState != .available {
      return model.licenseReadState == .loading
        ? String(localized: "Checking license…")
        : String(localized: "License access unavailable")
    }
    switch model.licenseStatus.state {
    case .unlicensed: return String(localized: "Unlicensed · Basic")
    case .active: return String(localized: "Active · Agentic unlocked")
    case .graceOffline:
      let daysLeft = Int(model.licenseStatus.daysLeft ?? 0)
      return String(localized: "Offline grace · \(daysLeft) days left")
    case .expiredUpdates:
      return String(localized: "Updates expired · installed app remains active")
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
