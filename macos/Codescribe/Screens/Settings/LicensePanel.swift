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
            Text("A signed CSK1 key unlocks the Agentic lane. Validation is local and the key stays in the macOS Keychain.")
                .font(CSFont.ui(12.5))
                .lineSpacing(2)
                .foregroundStyle(CSColor.textMutedAlt)
                .padding(.top, 8)

            SettingsSectionLabel("License status")
                .padding(.top, 24)
            VStack(spacing: 0) {
                RuntimeRow(key: "State", value: stateLabel, tint: model.licenseStatus.agenticEntitled, trailing: .none)
                divider
                RuntimeRow(key: "SKU", value: model.licenseStatus.sku ?? "Basic", tint: false, mono: true, trailing: .none)
                divider
                RuntimeRow(key: "Updates through", value: model.licenseStatus.updatesUntil ?? "—", tint: false, mono: true, trailing: .none)
            }
            .padding(.top, 11)
            .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 13, style: .continuous)
                    .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
            )

            SettingsSectionLabel("Enter or restore key")
                .padding(.top, 24)
            SecureField("CSK1.…", text: $key)
                .font(CSFont.mono(11.5, .regular))
                .textFieldStyle(.plain)
                .padding(12)
                .background(CSColor.surfaceRaised(0.04))
                .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
                .overlay(
                    RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                        .strokeBorder(CSColor.hairline(0.10), lineWidth: 1)
                )
                .padding(.top, 11)
                .accessibilityLabel("Codescribe license key")

            HStack(spacing: 12) {
                Button("Activate / Restore") {
                    if model.activateLicense(key) { key = "" }
                }
                .buttonStyle(.borderedProminent)
                .tint(CSColor.chromeAccent)
                .disabled(key.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)

                if model.licenseStatus.state != .unlicensed {
                    Button("Remove license", role: .destructive) {
                        model.removeLicense()
                    }
                    .buttonStyle(.plain)
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

            Text("Codescribe does not phone home while you work. A future fulfillment service may refresh the validation timestamp explicitly; offline grace is 30 days.")
                .font(CSFont.ui(11.5))
                .lineSpacing(2)
                .foregroundStyle(CSColor.textFaintAlt)
                .padding(.top, 18)
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 24)
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
        .background(SettingsView.windowGradient)
        .preferredColorScheme(.dark)
}
#endif
