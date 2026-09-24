/// One entry of a closed settings option list: the persisted id and its label.
struct SettingsMenuOption: Identifiable, Hashable {
  let id: String
  let label: String
}
