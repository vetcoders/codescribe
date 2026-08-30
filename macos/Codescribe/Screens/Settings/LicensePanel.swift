import AppKit
import SwiftUI

struct LicensePanel: View {
  @ObservedObject var model: SettingsViewModel
  @State private var key = ""

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      EyebrowLabel(text: "Settings · License")
      Text("Basic stays free.")
        .font(CSFont.ui(26, .bold))
        .tracking(-0.5)
        .foregroundStyle(CSColor.textHigh)
        .padding(.top, 6)
      Text(
        "A signed CSK1 key unlocks the Agentic lane. Validation is local and the key stays in the macOS Keychain."
      )
      .font(CSFont.ui(12.5))
      .lineSpacing(2)
      .foregroundStyle(CSColor.textMutedAlt)
      .padding(.top, 8)

      SettingsSectionLabel("License status")
        .padding(.top, CSSpace.section)
      VStack(spacing: 0) {
        RuntimeRow(
          key: "State", value: stateLabel, tint: model.licenseStatus.agenticEntitled,
          trailing: .none)
        divider
        RuntimeRow(
          key: "SKU", value: model.licenseStatus.sku ?? "Basic", tint: false, mono: true,
          trailing: .none)
        divider
        RuntimeRow(
          key: "Updates through", value: model.licenseStatus.updatesUntil ?? "—", tint: false,
          mono: true, trailing: .none)
      }
      .padding(.top, CSSpace.control)
      .clipShape(RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous))
      .overlay(
        RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous)
          .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
      )

      SettingsSectionLabel("Enter or restore key")
        .padding(.top, CSSpace.section)
      SecureField("CSK1.…", text: $key)
        .font(CSFont.mono(11.5, .regular))
        .textFieldStyle(.plain)
        .padding(CSSpace.md)
        .background(CSColor.surfaceRaised(0.04))
        .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
        .overlay(
          RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
            .strokeBorder(CSColor.hairline(0.10), lineWidth: 1)
        )
        .padding(.top, CSSpace.control)
        .accessibilityLabel("Codescribe license key")

      HStack(spacing: 12) {
        Button("Activate / Restore") {
          if model.activateLicense(key) { key = "" }
        }
        .buttonStyle(.borderedProminent)
        .tint(CSColor.chromeAccent)
        .disabled(key.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)

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
            model.removeLicense()
          }
          .csFocusRing()
          .foregroundStyle(CSColor.dangerLight)
        }
      }
      .padding(.top, 12)

      if let error = model.licenseError {
        Text(error)
          .font(CSFont.mono(10.5, .medium))
          .foregroundStyle(CSColor.dangerLight)
          .padding(.top, 10)
          .textSelection(.enabled)
      }

      Text(
        "Codescribe does not phone home while you work. A future fulfillment service may refresh the validation timestamp explicitly; offline grace is 30 days."
      )
      .font(CSFont.ui(11.5))
      .lineSpacing(2)
      .foregroundStyle(CSColor.textFaintAlt)
      .padding(.top, 18)
    }
    .padding(.horizontal, CSSpace.xl)
    .padding(.vertical, CSSpace.section)
  }

  private var stateLabel: String {
    switch model.licenseStatus.state {
    case .unlicensed: return "Unlicensed · Basic"
    case .active: return "Active · Agentic unlocked"
    case .graceOffline:
      return "Offline grace · \(model.licenseStatus.daysLeft ?? 0) days left"
    case .expiredUpdates: return "Updates expired · installed app remains active"
    }
  }

  private var divider: some View {
    Rectangle().fill(CSColor.hairline(0.05)).frame(height: 1)
  }
}

#if DEBUG
  #Preview("License panel") {
    ScrollView { LicensePanel(model: .preview(.license)) }
      .frame(width: 720, height: 760)
      .background(CSColor.windowWash)
      .preferredColorScheme(.dark)
  }
#endif
