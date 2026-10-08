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

      bindingRows.padding(.top, CSSpace.lg)
      inputSurfaceSection.padding(.top, 12)
      pasteModeSection.padding(.top, 12)
      deferredInsertSection.padding(.top, 12)
      badgeLegend.padding(.top, 12)

      if !model.bindingConflicts.isEmpty {
        conflictList.padding(.top, 16)
      }

      actions.padding(.top, CSSpace.section)
      hint.padding(.top, 14)
    }
    .padding(.horizontal, CSSpace.xl)
    .padding(.vertical, CSSpace.section)
  }

  // MARK: Header

  private var header: some View {
    SettingsPageHeader(
      String(localized: "Trigger keys."),
      blurb: String(localized: "One gesture per mode. Changes apply immediately — no restart.")
    )
  }

  // MARK: Per-mode binding rows

  private var bindingRows: some View {
    VStack(spacing: 0) {
      ForEach(Array(model.draftBindings.enumerated()), id: \.element.mode) { index, row in
        if index > 0 { divider }
        bindingRow(row)
      }
    }
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous))
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous)
        .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
    )
  }

  private func bindingRow(_ row: CsModeBinding) -> some View {
    VStack(alignment: .leading, spacing: 11) {
      HStack(spacing: 12) {
        VStack(alignment: .leading, spacing: 3) {
          Text(row.mode.visibleName)
            .font(CSFont.ui(13.5, .semibold))
            .foregroundStyle(Color.primary)
          Text(row.mode.blurb)
            .font(CSFont.ui(11.5, .medium))
            .foregroundStyle(Color.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)

        bindingPicker(row)
      }

      if row.mode == .assistive {
        assistiveModeSplit()
      }
    }
    .padding(.horizontal, 16)
    .padding(.vertical, 14)
    .background(Color.primary.opacity(0.04))
  }

  private func bindingPicker(_ row: CsModeBinding) -> some View {
    Menu {
      ForEach(model.bindingOptions, id: \.binding) { option in
        Button {
          model.editDraftBinding(mode: row.mode, binding: option.binding)
        } label: {
          if option.binding == row.binding {
            Label(option.binding.visibleName, systemImage: "checkmark")
          } else {
            Text(option.binding.visibleName)
          }
        }
      }
    } label: {
      HStack(spacing: 8) {
        Text(row.binding.visibleName)
          .font(CSFont.mono(12, .semibold))
          .foregroundStyle(CSColor.terracotta)
        CSIconView(icon: .chevronUpDown, size: 9, weight: .semibold, color: Color.secondary)
      }
      .padding(.horizontal, 12)
      .padding(.vertical, 8)
      .background(
        RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
          .fill(Color.primary.opacity(0.08))
      )
      .overlay(
        RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
          .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
      )
    }
    .menuStyle(.borderlessButton)
    .menuIndicator(.hidden)
    .fixedSize()
  }

  private func assistiveModeSplit() -> some View {
    VStack(alignment: .leading, spacing: 7) {
      assistiveModeVariant(
        title: "Attach selection",
        gesture: armGestureLabel,
        description:
          "Shift or Command during an already-started Fn hold attaches {selection_N}. It does not start voice chat, hide the overlay, or stop the take. Fn+Shift from idle is dictation, not Assistive."
      )
      // Arm modifier is attach-only (default Shift; Cmd alternative).
      HStack(spacing: 8) {
        Text("Arm with")
          .font(CSFont.ui(11, .medium))
          .foregroundStyle(Color.secondary)
        Picker("Arm modifier", selection: armModifierBinding) {
          Text(verbatim: "Shift").tag("shift")
          Text(verbatim: "Command").tag("cmd")
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .frame(maxWidth: 180)
      }
      .padding(.top, 2)
    }
    .padding(.horizontal, 12)
    .padding(.vertical, 10)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .fill(CSColor.assistive.opacity(0.08))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(CSColor.assistive.opacity(0.18), lineWidth: 1)
    )
  }

  private func assistiveModeVariant(
    title: LocalizedStringKey, gesture: String, description: LocalizedStringKey
  ) -> some View {
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
          .foregroundStyle(Color.secondary)
          .fixedSize(horizontal: false, vertical: true)
      }
      Spacer(minLength: 8)
      Text(gesture)
        .font(CSFont.mono(10.5, .semibold))
        .foregroundStyle(Color.primary)
        .multilineTextAlignment(.trailing)
        .fixedSize(horizontal: false, vertical: true)
    }
  }

  /// Derived from the configured arm modifier — never hardcode Fn+Command.
  private var armGestureLabel: String {
    ArmGestureCopy.label(for: model.holdArmModifier)
  }

  private var armModifierBinding: Binding<String> {
    Binding(
      get: { model.holdArmModifier },
      set: { model.setHoldArmModifier($0) }
    )
  }

  private var badgeLegend: some View {
    VStack(alignment: .leading, spacing: 8) {
      SettingsSectionLabel(String(localized: "Dot colors"))
      HStack(spacing: 12) {
        legendItem(color: CSColor.terracotta, text: "Red — dictation or formatting is recording")
        legendItem(color: CSColor.assistive, text: "Purple — voice goes to the Agent")
        legendItem(color: CSColor.amber, text: "Orange — processing after recording")
      }
      HStack(spacing: 12) {
        VStack(alignment: .leading, spacing: 2) {
          Text("Pointer indicator")
            .font(CSFont.ui(12.5, .semibold))
            .foregroundStyle(Color.primary)
          Text("Base size; Agent mode stays proportionally larger")
            .font(CSFont.ui(10.5, .medium))
            .foregroundStyle(Color.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)

        Picker("Pointer indicator", selection: holdBadgeBinding) {
          ForEach(HoldBadgeOption.allCases) { option in
            Text(option.visibleName).tag(option)
          }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .frame(width: 230)
      }
      .padding(.top, 4)
    }
    .padding(.horizontal, 14)
    .padding(.vertical, 11)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .fill(Color.primary.opacity(0.05))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
    )
  }

  private var holdBadgeBinding: Binding<HoldBadgeOption> {
    Binding(
      get: { model.holdBadgeOption },
      set: { model.setHoldBadgeOption($0) }
    )
  }

  // MARK: Channel, Fn tap, middle mouse

  /// Three input surfaces on the same hotkey config as the mode rows.
  /// Command is absent from the channel picker. Both toggles default off.
  private var inputSurfaceSection: some View {
    VStack(alignment: .leading, spacing: 0) {
      inputSurfaceRow(
        title: "Agent channel",
        detail:
          "Ctrl+digit switches an agent channel. Choose Fn if you want the globe key instead. Command is not offered — it collides with tab switching."
      ) {
        Picker("Agent channel modifier", selection: channelModifierBinding) {
          Text(verbatim: "Ctrl").tag("ctrl")
          Text(verbatim: "Fn").tag("fn")
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .frame(maxWidth: 160)
      }
      divider
      inputSurfaceRow(
        title: "Tap Fn to dictate",
        detail:
          "A quick Fn press starts dictation and the next tap stops it. Holding past the hold delay stays hold-to-talk. For best results set the macOS Fn key action to Do Nothing — Codescribe reacts to a single tap, and macOS can claim a double-press for its own dictation."
      ) {
        Toggle("Tap Fn to dictate", isOn: fnTapBinding)
          .labelsHidden()
          .toggleStyle(.switch)
      }
      divider
      inputSurfaceRow(
        title: "Middle mouse acts as Fn",
        detail:
          "The middle mouse button follows the same press, hold, and tap rules as Fn. The click still reaches the frontmost app."
      ) {
        Toggle("Middle mouse acts as Fn", isOn: middleMouseBinding)
          .labelsHidden()
          .toggleStyle(.switch)
      }
    }
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous))
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous)
        .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
    )
  }

  private func inputSurfaceRow<Control: View>(
    title: LocalizedStringKey, detail: LocalizedStringKey,
    @ViewBuilder control: () -> Control
  ) -> some View {
    HStack(alignment: .center, spacing: 12) {
      VStack(alignment: .leading, spacing: 3) {
        Text(title)
          .font(CSFont.ui(13.5, .semibold))
          .foregroundStyle(Color.primary)
        Text(detail)
          .font(CSFont.ui(11.5, .medium))
          .foregroundStyle(Color.secondary)
          .fixedSize(horizontal: false, vertical: true)
      }
      .frame(maxWidth: .infinity, alignment: .leading)
      control()
    }
    .padding(.horizontal, 16)
    .padding(.vertical, 14)
    .background(Color.primary.opacity(0.04))
  }

  private var channelModifierBinding: Binding<String> {
    Binding(get: { model.channelModifier }, set: { model.setChannelModifier($0) })
  }

  private var fnTapBinding: Binding<Bool> {
    Binding(get: { model.fnTapTogglesDictation }, set: { model.setFnTapTogglesDictation($0) })
  }

  private var middleMouseBinding: Binding<Bool> {
    Binding(get: { model.middleMouseActsAsFn }, set: { model.setMiddleMouseActsAsFn($0) })
  }

  // MARK: Automatic paste mode

  /// Safe / Comfort / Off — one persisted `PASTE_MODE` shared with the tray
  /// Quick settings row. Each mode carries its one-sentence contract so the
  /// choice is explained where it is made.
  private var pasteModeSection: some View {
    VStack(alignment: .leading, spacing: 8) {
      SettingsSectionLabel(String(localized: "Automatic paste"))
      HStack(spacing: 12) {
        VStack(alignment: .leading, spacing: 2) {
          Text("Paste after dictation")
            .font(CSFont.ui(12.5, .semibold))
            .foregroundStyle(Color.primary)
          Text("Where the transcript goes when a Hold or toggle take ends.")
            .font(CSFont.ui(10.5, .medium))
            .foregroundStyle(Color.secondary)
            .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)

        Picker("Paste mode", selection: pasteModeBinding) {
          ForEach(CsPasteMode.allModes, id: \.self) { mode in
            Text(mode.visibleName).tag(mode)
          }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .fixedSize()
        .accessibilityIdentifier("settings.pasteMode")
      }
      Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 6, verticalSpacing: 3) {
        ForEach(CsPasteMode.allModes, id: \.self) { mode in
          GridRow {
            Text(mode.visibleName)
              .font(CSFont.ui(10.5, .semibold))
              .foregroundStyle(mode == model.pasteMode ? Color.primary : Color.secondary)
              .fixedSize()
            Text(mode.blurb)
              .font(CSFont.ui(10.5, .medium))
              .foregroundStyle(Color.secondary)
              .fixedSize(horizontal: false, vertical: true)
          }
        }
      }
    }
    .padding(.horizontal, 14)
    .padding(.vertical, 11)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .fill(Color.primary.opacity(0.05))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
    )
  }

  private var pasteModeBinding: Binding<CsPasteMode> {
    Binding(
      get: { model.pasteMode },
      set: { model.setPasteMode($0) }
    )
  }

  // MARK: Deferred insert chord

  /// Command chord delivering an armed transcript at the caret. A closed
  /// four-option set mirroring core `DeferredInsertShortcut`; writes go
  /// through the same `update_config` brain as every other setting. Off by
  /// default — the tap is listen-only, so a host app bound to the same chord
  /// would also react (core/config/types.rs).
  private var deferredInsertSection: some View {
    VStack(alignment: .leading, spacing: 8) {
      SettingsSectionLabel(String(localized: "Deferred insert"))
      HStack(spacing: 12) {
        VStack(alignment: .leading, spacing: 2) {
          Text("Insert armed transcript")
            .font(CSFont.ui(12.5, .semibold))
            .foregroundStyle(Color.primary)
          Text(
            "Global chord pastes the armed transcript at the caret. Apps bound to the same chord will also react."
          )
          .font(CSFont.ui(10.5, .medium))
          .foregroundStyle(Color.secondary)
          .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)

        Picker("Deferred insert shortcut", selection: deferredInsertBinding) {
          ForEach(DeferredInsertShortcutOption.allCases) { option in
            Text(option.visibleName).tag(option)
          }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .fixedSize()
      }
    }
    .padding(.horizontal, 14)
    .padding(.vertical, 11)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .fill(Color.primary.opacity(0.05))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(Color.primary.opacity(0.12), lineWidth: 1)
    )
  }

  private var deferredInsertBinding: Binding<DeferredInsertShortcutOption> {
    Binding(
      get: { model.deferredInsertShortcut },
      set: { model.setDeferredInsertShortcut($0) }
    )
  }

  private func legendItem(color: Color, text: LocalizedStringKey) -> some View {
    HStack(spacing: 6) {
      Circle().fill(color).frame(width: 7, height: 7)
      Text(text)
        .font(CSFont.ui(11.5, .medium))
        .foregroundStyle(Color.secondary)
        .lineLimit(2)
        .fixedSize(horizontal: false, vertical: true)
    }
    .frame(maxWidth: .infinity, alignment: .leading)
  }

  // MARK: Conflicts (inline validation)

  private var conflictList: some View {
    VStack(alignment: .leading, spacing: 8) {
      SettingsSectionLabel(String(localized: "Conflicts"))
      ForEach(Array(model.bindingConflicts.enumerated()), id: \.offset) { _, conflict in
        conflictRow(conflict)
      }
    }
  }

  private func conflictRow(_ conflict: CsHotkeyConflict) -> some View {
    let accent = conflict.blocking ? CSColor.terracotta : CSColor.amber
    let accentLight = conflict.blocking ? CSColor.terracotta : CSColor.amber
    return HStack(alignment: .top, spacing: 9) {
      Text(verbatim: conflict.blocking ? "!" : "i")
        .font(CSFont.ui(11, .bold))
        .foregroundStyle(accentLight)
        .frame(width: 14)
      VStack(alignment: .leading, spacing: 2) {
        Text(conflict.visibleGesture(options: model.bindingOptions))
          .font(CSFont.mono(11, .semibold))
          .foregroundStyle(accentLight)
        Text(conflict.message)
          .font(CSFont.ui(12, .medium))
          .foregroundStyle(Color.primary)
          .fixedSize(horizontal: false, vertical: true)
      }
    }
    .padding(.horizontal, 14)
    .padding(.vertical, 10)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous).fill(accent.opacity(0.08))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(accent.opacity(0.2), lineWidth: 1)
    )
  }

  // MARK: Permission degradation

  private var permissionNote: some View {
    HStack(alignment: .top, spacing: 9) {
      Text(verbatim: "!")
        .font(CSFont.ui(11, .bold))
        .foregroundStyle(CSColor.amber)
        .frame(width: 14)
      VStack(alignment: .leading, spacing: 2) {
        Text("Shortcuts need Input Monitoring + Accessibility")
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(CSColor.amber)
        Text(
          "You can edit bindings here, but they won't fire until both are granted. Click to open System Settings."
        )
        .font(CSFont.ui(12, .medium))
        .foregroundStyle(Color.primary)
        .fixedSize(horizontal: false, vertical: true)
      }
    }
    .padding(.horizontal, 14)
    .padding(.vertical, 11)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .fill(CSColor.amber.opacity(0.08))
    )
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
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
      Button {
        model.resetBindingsToDefaults()
      } label: {
        Text("Reset to defaults")
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(Color.secondary)
      }
      .csFocusRing()

      Spacer(minLength: 0)

      Button {
        model.saveBindings()
      } label: {
        Text("Save")
          .font(CSFont.ui(12.5, .semibold))
          .padding(.horizontal, 18)
          .padding(.vertical, 8)
          .foregroundStyle(model.canSaveBindings ? Color.primary : Color.secondary)
          .background(
            RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous)
              .fill(
                model.canSaveBindings
                  ? CSColor.terracotta.opacity(0.9)
                  : Color.primary.opacity(0.06))
          )
      }
      .csFocusRing()
      .disabled(!model.canSaveBindings)
    }
  }

  private var hint: some View {
    HStack(spacing: 8) {
      Text(verbatim: "●")
        .font(CSFont.mono(11, .medium))
        .foregroundStyle(model.hasBlockingBindingConflicts ? CSColor.terracotta : CSColor.olive)
      Text(
        model.hasBlockingBindingConflicts
          ? "Resolve the conflict above before saving"
          : "Bindings persist to settings.json and reload the detector live"
      )
      .font(CSFont.mono(11, .medium))
      .foregroundStyle(Color.secondary)
    }
  }

  private var divider: some View {
    Rectangle().fill(Color.primary.opacity(0.12)).frame(height: 1)
  }
}

/// Single production owner for attach-arm gesture copy in Settings.
enum ArmGestureCopy {
  static func label(for modifier: String) -> String {
    modifier == "cmd"
      ? String(localized: "Command during Fn hold")
      : String(localized: "Shift during Fn hold")
  }
}

#if DEBUG
  #Preview("Shortcuts panel") {
    ScrollView { ShortcutsPanel(model: .preview(.shortcuts)) }
      .frame(width: 720, height: 620)
  }
#endif
