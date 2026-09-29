import SwiftUI

/// Popover editor for a vendor's OAuth client id override (settings.json). An
/// empty save restores the shipped default.
struct OAuthClientIdEditor: View {
  let accountBrand: String
  let placeholder: String
  let savedClientId: String
  @Binding var draft: String
  let onSave: () -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: CSSpace.sm) {
      Text("\(accountBrand) OAuth client id")
        .font(CSFont.ui(12.5, .semibold))
        .foregroundStyle(CSColor.textBody)
      Text("Optional override (settings.json) — empty restores the shipped default.")
        .font(CSFont.ui(11.5))
        .foregroundStyle(CSColor.textMutedAlt)
        .fixedSize(horizontal: false, vertical: true)
      HStack(spacing: 8) {
        TextField(placeholder, text: $draft)
          .settingsInputChrome()
          .onSubmit(onSave)
          .accessibilityLabel("\(accountBrand) OAuth client id")
        SettingsChipButton(
          "Save", tint: CSColor.oliveLight,
          enabled: draft != savedClientId,
          action: onSave
        )
      }
    }
    .padding(CSSpace.card)
    .frame(minWidth: 360)
  }
}
