import SwiftUI

/// Borderless pop-up menu over a closed option list: a checkmark
/// on the selected id, the current label as the trigger. The Dictation engine
/// pickers share this grammar.
struct SettingsOptionMenu: View {
  let options: [SettingsMenuOption]
  let selectedId: String
  let currentLabel: String
  let onSelect: @MainActor (String) -> Void

  var body: some View {
    Menu {
      ForEach(options) { option in
        Button {
          onSelect(option.id)
        } label: {
          if option.id == selectedId {
            Label(option.label, systemImage: "checkmark")
          } else {
            Text(option.label)
          }
        }
      }
    } label: {
      SettingsMenuLabel(text: currentLabel)
    }
    .menuStyle(.borderlessButton)
    .menuIndicator(.hidden)
    .fixedSize()
  }
}
