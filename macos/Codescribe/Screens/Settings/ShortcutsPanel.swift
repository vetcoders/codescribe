import SwiftUI

// Shortcuts panel: edit the per-mode trigger gestures (Dictation / Formatting /
// Assistive). Picker-based on purpose — the binding space is a CLOSED set
// (docs/HOTKEYS_CONTRACT.md: Hold Fn/Ctrl/… + Double-tap Ctrl/Option), so a
// free-form "press keys" recorder would be both harder (hold vs double-tap timing)
// and wrong (it can't map arbitrary keystrokes into this fixed enum). Conflicts
// validate inline via the revived shortcut_registry; a save is gated on a clean
// draft. The hotkey engine seeds at launch and live-reloads on write, so a saved
// change takes effect on the running CGEventTap without a restart.

struct ShortcutsPanel: View {
    @ObservedObject var model: SettingsViewModel

    private var permissionDegraded: Bool {
        !model.permissions.inputMonitoring.isGranted
            || !model.permissions.accessibility.isGranted
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header

            if permissionDegraded {
                permissionNote.padding(.top, 18)
            }

            bindingRows.padding(.top, 20)
            badgeLegend.padding(.top, 12)

            if !model.bindingConflicts.isEmpty {
                conflictList.padding(.top, 16)
            }

            actions.padding(.top, 22)
            hint.padding(.top, 14)
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 24)
    }

    // MARK: Header

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            EyebrowLabel(text: "Settings · Shortcuts")
            Text("Trigger keys.")
                .font(CSFont.ui(26, .bold))
                .tracking(-0.5)
                .foregroundStyle(CSColor.textHigh)
            Text("One gesture per mode. Changes apply immediately — no restart.")
                .font(CSFont.ui(13, .medium))
                .foregroundStyle(CSColor.textMuted)
        }
    }

    // MARK: Per-mode binding rows

    private var bindingRows: some View {
        VStack(spacing: 0) {
            ForEach(Array(model.draftBindings.enumerated()), id: \.element.modeLabel) { index, row in
                if index > 0 { divider }
                bindingRow(row)
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 13, style: .continuous)
                .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
        )
    }

    private func bindingRow(_ row: CsModeBinding) -> some View {
        VStack(alignment: .leading, spacing: 11) {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(row.modeLabel)
                        .font(CSFont.ui(13.5, .semibold))
                        .foregroundStyle(CSColor.textHigh)
                    Text(row.modeDescription)
                        .font(CSFont.ui(11.5, .medium))
                        .foregroundStyle(CSColor.textMuted)
                }
                .frame(maxWidth: .infinity, alignment: .leading)

                bindingPicker(row)
            }

            if row.mode == .assistive {
                assistiveModeSplit(row)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 14)
        .background(CSColor.surfaceRaised(0.02))
    }

    private func bindingPicker(_ row: CsModeBinding) -> some View {
        Menu {
            ForEach(model.bindingOptions, id: \.label) { option in
                Button {
                    model.editDraftBinding(mode: row.mode, binding: option.binding)
                } label: {
                    if option.binding == row.binding {
                        Label(option.label, systemImage: "checkmark")
                    } else {
                        Text(option.label)
                    }
                }
            }
        } label: {
            HStack(spacing: 8) {
                Text(row.bindingLabel)
                    .font(CSFont.mono(12, .semibold))
                    .foregroundStyle(CSColor.terracottaLight)
                CSIconView(icon: .chevronUpDown, size: 9, weight: .semibold, color: CSColor.textMuted)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(
                RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                    .fill(CSColor.surfaceRaised(0.04))
            )
            .overlay(
                RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                    .strokeBorder(CSColor.hairline(0.09), lineWidth: 1)
            )
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
    }

    private func assistiveModeSplit(_ row: CsModeBinding) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            assistiveModeVariant(
                title: "Voice chat",
                gesture: "Hold Fn+Shift",
                description: "Talk to the agent."
            )
            assistiveModeVariant(
                title: "Act on selection",
                gesture: selectionAssistiveGesture(row),
                description: "Select text, then speak an instruction."
            )
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(CSColor.assistive.opacity(0.08))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(CSColor.assistive.opacity(0.18), lineWidth: 1)
        )
    }

    private func assistiveModeVariant(title: String, gesture: String, description: String) -> some View {
        HStack(alignment: .top, spacing: 9) {
            Circle()
                .fill(CSColor.assistive)
                .frame(width: 6, height: 6)
                .padding(.top, 5)
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                    .font(CSFont.ui(11.5, .semibold))
                    .foregroundStyle(CSColor.assistiveLight)
                Text(description)
                    .font(CSFont.ui(11, .medium))
                    .foregroundStyle(CSColor.textMuted)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            Text(gesture)
                .font(CSFont.mono(10.5, .semibold))
                .foregroundStyle(CSColor.textBodyAlt)
                .multilineTextAlignment(.trailing)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func selectionAssistiveGesture(_ row: CsModeBinding) -> String {
        row.binding == .disabled ? "Hold Fn+Command" : "\(row.bindingLabel) or Hold Fn+Command"
    }

    private var badgeLegend: some View {
        VStack(alignment: .leading, spacing: 8) {
            SettingsSectionLabel("Dot colors")
            HStack(spacing: 12) {
                legendItem(color: CSColor.terracotta, text: "Red — dictation or formatting is recording")
                legendItem(color: CSColor.assistive, text: "Purple — voice goes to the agent")
                legendItem(color: CSColor.amber, text: "Orange — processing after recording")
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(CSColor.surfaceRaised(0.025))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
        )
    }

    private func legendItem(color: Color, text: String) -> some View {
        HStack(spacing: 6) {
            Circle().fill(color).frame(width: 7, height: 7)
            Text(text)
                .font(CSFont.ui(11.5, .medium))
                .foregroundStyle(CSColor.textMuted)
                .lineLimit(2)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    // MARK: Conflicts (inline validation)

    private var conflictList: some View {
        VStack(alignment: .leading, spacing: 8) {
            SettingsSectionLabel("Conflicts")
            ForEach(Array(model.bindingConflicts.enumerated()), id: \.offset) { _, conflict in
                conflictRow(conflict)
            }
        }
    }

    private func conflictRow(_ conflict: CsHotkeyConflict) -> some View {
        let accent = conflict.blocking ? CSColor.terracotta : CSColor.amber
        let accentLight = conflict.blocking ? CSColor.terracottaLight : CSColor.amber
        return HStack(alignment: .top, spacing: 9) {
            Text(conflict.blocking ? "!" : "i")
                .font(CSFont.ui(11, .bold))
                .foregroundStyle(accentLight)
                .frame(width: 14)
            VStack(alignment: .leading, spacing: 2) {
                Text(conflict.gestureLabel)
                    .font(CSFont.mono(11, .semibold))
                    .foregroundStyle(accentLight)
                Text(conflict.message)
                    .font(CSFont.ui(12, .medium))
                    .foregroundStyle(CSColor.textBodyAlt)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous).fill(accent.opacity(0.08))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(accent.opacity(0.2), lineWidth: 1)
        )
    }

    // MARK: Permission degradation

    private var permissionNote: some View {
        HStack(alignment: .top, spacing: 9) {
            Text("!")
                .font(CSFont.ui(11, .bold))
                .foregroundStyle(CSColor.amber)
                .frame(width: 14)
            VStack(alignment: .leading, spacing: 2) {
                Text("Shortcuts need Input Monitoring + Accessibility")
                    .font(CSFont.ui(12.5, .semibold))
                    .foregroundStyle(CSColor.amber)
                Text("You can edit bindings here, but they won't fire until both are granted. Click to open System Settings.")
                    .font(CSFont.ui(12, .medium))
                    .foregroundStyle(CSColor.textBodyAlt)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous).fill(CSColor.amber.opacity(0.08))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(CSColor.amber.opacity(0.2), lineWidth: 1)
        )
        .contentShape(Rectangle())
        .onTapGesture {
            if !model.permissions.inputMonitoring.isGranted {
                PermissionKind.inputMonitoring.openSystemSettings()
            } else {
                PermissionKind.accessibility.openSystemSettings()
            }
        }
    }

    // MARK: Actions

    private var actions: some View {
        HStack(spacing: 12) {
            Button { model.resetBindingsToDefaults() } label: {
                Text("Reset to defaults")
                    .font(CSFont.ui(12.5, .semibold))
                    .foregroundStyle(CSColor.textMuted)
            }
            .buttonStyle(.plain)

            Spacer(minLength: 0)

            Button { model.saveBindings() } label: {
                Text("Save")
                    .font(CSFont.ui(12.5, .semibold))
                    .padding(.horizontal, 18)
                    .padding(.vertical, 8)
                    .foregroundStyle(model.canSaveBindings ? CSColor.textHigh : CSColor.textFaint)
                    .background(
                        RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
                            .fill(model.canSaveBindings
                                  ? CSColor.terracotta.opacity(0.9)
                                  : CSColor.surfaceRaised(0.03))
                    )
            }
            .buttonStyle(.plain)
            .disabled(!model.canSaveBindings)
        }
    }

    private var hint: some View {
        HStack(spacing: 8) {
            Text("●")
                .font(CSFont.mono(11, .medium))
                .foregroundStyle(model.hasBlockingBindingConflicts ? CSColor.terracotta : CSColor.olive)
            Text(model.hasBlockingBindingConflicts
                 ? "Resolve the conflict above before saving"
                 : "Bindings persist to settings.json and reload the detector live")
                .font(CSFont.mono(11, .medium))
                .foregroundStyle(CSColor.textFaint)
        }
    }

    private var divider: some View {
        Rectangle().fill(CSColor.hairline(0.05)).frame(height: 1)
    }
}

#if DEBUG
#Preview("Shortcuts panel") {
    ScrollView { ShortcutsPanel(model: .preview(.shortcuts)) }
        .frame(width: 720, height: 620)
        .background(SettingsView.windowGradient)
        .preferredColorScheme(.dark)
}
#endif
