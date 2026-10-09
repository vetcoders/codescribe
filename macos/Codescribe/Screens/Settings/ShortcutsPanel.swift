import SwiftUI

// Shortcuts panel: edit the per-mode trigger gestures (Dictation / Formatting /
// Assistive) plus the input surfaces and paste policy around them.
//
// Picker-based on purpose — the binding space is a CLOSED set
// (docs/HOTKEYS_CONTRACT.md: Hold Fn/Ctrl/… + Double-tap Ctrl/Option), so a
// free-form "press keys" recorder would be both harder (hold vs double-tap
// timing) and wrong (it can't map arbitrary keystrokes into this fixed enum).
//
// Two different save contracts live on this one screen, and the copy says so:
// the three mode gestures are a DRAFT that needs the explicit Save button,
// while every other control here writes on change. Conflicts validate inline
// via the revived shortcut_registry; a save is gated on a clean draft and then
// confirmed against persisted truth, never against the draft. The hotkey engine
// seeds at launch and live-reloads on write, so a saved change takes effect on
// the running CGEventTap without a restart.

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

      modeSection.padding(.top, CSSpace.lg)
      dictationContextSection.padding(.top, 12)
      inputSurfaceSection.padding(.top, 12)
      pasteModeSection.padding(.top, 12)
      pasteOnDemandSection.padding(.top, 12)
      badgeLegend.padding(.top, 12)
    }
    .padding(.horizontal, CSSpace.xl)
    .padding(.vertical, CSSpace.section)
  }

  // MARK: Header

  /// The blurb separates the two save contracts instead of claiming that
  /// everything on the screen persists by itself.
  private var header: some View {
    SettingsPageHeader(
      String(localized: "Keyboard shortcuts"),
      blurb: String(
        localized:
          "The three mode gestures are saved with the button below. Every other setting here applies as soon as you change it."
      )
    )
  }

  // MARK: Per-mode gestures, their conflicts and their save

  /// The one draft-and-save island on this screen: three gestures, the blocking
  /// conflicts that refuse the save, the save itself, and the notes that do not
  /// block it.
  private var modeSection: some View {
    VStack(alignment: .leading, spacing: 12) {
      bindingRows
      if !blockingConflicts.isEmpty { conflictList }
      saveRow
      if !informationalNotices.isEmpty { noticeList }
    }
  }

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
    HStack(spacing: 12) {
      VStack(alignment: .leading, spacing: 3) {
        Text(row.mode.visibleName)
          .font(CSFont.ui(13.5, .semibold))
          .foregroundStyle(Color.primary)
        Text(row.mode.blurb)
          .font(CSFont.ui(11.5, .medium))
          .foregroundStyle(Color.secondary)
          .fixedSize(horizontal: false, vertical: true)
      }
      .frame(maxWidth: .infinity, alignment: .leading)

      bindingPicker(row)
    }
    .padding(.horizontal, 16)
    .padding(.vertical, 14)
    .background(Color.primary.opacity(0.04))
  }

  /// The pill shows the terse gesture (`2× Left ⌥ (Option)`); VoiceOver reads
  /// the spelled-out form, so left and right Option stay distinguishable for a
  /// screen reader that skips key caps.
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
        .accessibilityLabel(Text(option.binding.spokenName))
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
    .accessibilityLabel(Text(row.mode.visibleName))
    .accessibilityValue(Text(row.binding.spokenName))
  }

  // MARK: Save the mode gestures

  /// Directly under the three gestures, because these two buttons govern only
  /// those three.
  private var saveRow: some View {
    VStack(alignment: .leading, spacing: 8) {
      HStack(spacing: 12) {
        Button {
          model.saveBindings()
        } label: {
          Text("Save mode shortcuts")
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

        Button {
          model.resetBindingsToDefaults()
        } label: {
          Text("Restore default mode shortcuts")
            .font(CSFont.ui(12.5, .semibold))
            .foregroundStyle(Color.secondary)
        }
        .csFocusRing()

        Spacer(minLength: 0)
      }

      saveStatus
    }
  }

  /// Pending edits or the reason a save is refused, and, independently, the
  /// persisted outcome of the last save — never a guess built from the draft.
  ///
  /// The receipt is not an `else` branch. A partial save can leave the
  /// snapped-back draft in a blocking conflict (Dictation=Double Ctrl lands,
  /// Agent=Hold Ctrl is refused and snaps back to Double Right Option, which
  /// Double Ctrl disables); the conflict line and the receipt then both apply,
  /// and hiding the receipt would hide completed writes and the refusal.
  @ViewBuilder private var saveStatus: some View {
    VStack(alignment: .leading, spacing: 6) {
      if model.hasBlockingBindingConflicts {
        statusLine(
          color: CSColor.terracotta,
          text: String(localized: "Resolve the conflict above to save the mode shortcuts.")
        )
      } else if model.hasPendingBindingChanges {
        statusLine(
          color: CSColor.amber,
          text: String(localized: "Unsaved changes to the mode gestures.")
        )
      }
      if let receipt = model.bindingSaveReceipt, let sentence = receipt.sentence {
        VStack(alignment: .leading, spacing: 3) {
          statusLine(
            color: receipt.hasRejection ? CSColor.terracotta : CSColor.olive,
            text: sentence
          )
          if let detail = receipt.failureDetail {
            Text(verbatim: detail)
              .font(CSFont.mono(10, .medium))
              .foregroundStyle(Color.secondary)
              .fixedSize(horizontal: false, vertical: true)
          }
        }
      }
    }
  }

  private func statusLine(color: Color, text: String) -> some View {
    HStack(alignment: .top, spacing: 8) {
      Text(verbatim: "●")
        .font(CSFont.mono(11, .medium))
        .foregroundStyle(color)
      Text(text)
        .font(CSFont.ui(11.5, .medium))
        .foregroundStyle(Color.secondary)
        .fixedSize(horizontal: false, vertical: true)
    }
  }

  // MARK: Dictation context (attach selection)

  /// Its own section, not a sub-row of the Agent gesture: attaching a selection
  /// belongs to the running dictation hold and never switches the take to the
  /// Agent.
  private var dictationContextSection: some View {
    VStack(alignment: .leading, spacing: 8) {
      SettingsSectionLabel(String(localized: "Dictation context"))
      VStack(alignment: .leading, spacing: 7) {
        HStack(alignment: .top, spacing: 9) {
          Circle()
            .fill(CSColor.assistive)
            .frame(width: 6, height: 6)
            .padding(.top, 5)
          VStack(alignment: .leading, spacing: 1) {
            Text("Attach selection")
              .font(CSFont.ui(11.5, .semibold))
              .foregroundStyle(CSColor.assistiveLight)
            Text(
              "Shift or Command during an already-started Fn hold attaches the selected text. It does not switch to the Agent."
            )
            .font(CSFont.ui(11, .medium))
            .foregroundStyle(Color.secondary)
            .fixedSize(horizontal: false, vertical: true)
          }
          Spacer(minLength: 8)
          Text(armGestureLabel)
            .font(CSFont.mono(10.5, .semibold))
            .foregroundStyle(Color.primary)
            .multilineTextAlignment(.trailing)
            .fixedSize(horizontal: false, vertical: true)
        }

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

  // MARK: Indicator states

  /// The dot is the colour; the label is the state it stands for. Full words
  /// and no line limit, so a longer translation wraps instead of ending in an
  /// ellipsis.
  private var badgeLegend: some View {
    VStack(alignment: .leading, spacing: 8) {
      SettingsSectionLabel(String(localized: "Indicator states"))
      HStack(alignment: .top, spacing: 12) {
        legendItem(color: CSColor.terracotta, text: "Recording")
        legendItem(color: CSColor.assistive, text: "Agent")
        legendItem(color: CSColor.amber, text: "Processing")
      }
      HStack(spacing: 12) {
        VStack(alignment: .leading, spacing: 2) {
          Text("Pointer indicator")
            .font(CSFont.ui(12.5, .semibold))
            .foregroundStyle(Color.primary)
          Text("Base size; Agent mode stays proportionally larger")
            .font(CSFont.ui(10.5, .medium))
            .foregroundStyle(Color.secondary)
            .fixedSize(horizontal: false, vertical: true)
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

  // MARK: Extra gestures — channel, Fn tap, middle mouse

  /// Three input surfaces on the same hotkey config as the mode rows, all
  /// writing on change. Command is absent from the channel picker. Both
  /// toggles default off.
  private var inputSurfaceSection: some View {
    VStack(alignment: .leading, spacing: 8) {
      SettingsSectionLabel(String(localized: "Extra gestures"))
      VStack(alignment: .leading, spacing: 0) {
        inputSurfaceRow(
          title: "Agent channel",
          detail: "Ctrl + digit switches an Agent channel. Choose Fn to use Fn + digit instead."
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
            "One tap starts dictation and the next tap stops it. Holding records only while you hold."
        ) {
          Toggle("Tap Fn to dictate", isOn: fnTapBinding)
            .labelsHidden()
            .toggleStyle(.switch)
        }
        divider
        inputSurfaceRow(
          title: "Middle mouse acts as Fn",
          detail:
            "The middle mouse button follows the same press, hold and tap rules as Fn. Its ordinary click can still reach the app in front."
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
  }

  /// Two sentences on screen and no tooltip: macOS caveats live in the user
  /// guide, and a native tooltip only covers the control (Founder, 2026-10-08).
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
  /// Quick settings row. The picker owns a full-width row of its own, and only
  /// the SELECTED mode explains itself: three permanent paragraphs made the
  /// choice harder to read, and squeezing the segmented control next to the
  /// title wrapped the label after two words in a narrow window.
  private var pasteModeSection: some View {
    VStack(alignment: .leading, spacing: 8) {
      SettingsSectionLabel(String(localized: "Automatic paste"))
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
      .frame(maxWidth: .infinity, alignment: .leading)
      .accessibilityIdentifier("settings.pasteMode")

      Text(model.pasteMode.blurb)
        .font(CSFont.ui(10.5, .medium))
        .foregroundStyle(Color.secondary)
        .fixedSize(horizontal: false, vertical: true)
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

  // MARK: Paste on demand

  /// Command chord delivering a transcript that is waiting to be inserted. A
  /// closed four-option set mirroring core `DeferredInsertShortcut`; writes go
  /// through the same `update_config` brain as every other setting. Off by
  /// default — the tap is listen-only, so a host app bound to the same chord
  /// would also react (core/config/types.rs). The picker sits on its own
  /// full-width row so neither the name nor the warning wraps after two words.
  private var pasteOnDemandSection: some View {
    VStack(alignment: .leading, spacing: 8) {
      SettingsSectionLabel(String(localized: "Deferred insert"))
      VStack(alignment: .leading, spacing: 2) {
        Text("Paste transcript")
          .font(CSFont.ui(12.5, .semibold))
          .foregroundStyle(Color.primary)
        Text("Choose the shortcut that pastes the transcript waiting to be inserted.")
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
      .frame(maxWidth: .infinity, alignment: .leading)

      Text("The app you are pasting into may handle this shortcut as well.")
        .font(CSFont.ui(10.5, .medium))
        .foregroundStyle(Color.secondary)
        .fixedSize(horizontal: false, vertical: true)
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
    HStack(alignment: .top, spacing: 6) {
      Circle().fill(color).frame(width: 7, height: 7).padding(.top, 4)
      Text(text)
        .font(CSFont.ui(11.5, .medium))
        .foregroundStyle(Color.secondary)
        .fixedSize(horizontal: false, vertical: true)
    }
    .frame(maxWidth: .infinity, alignment: .leading)
  }

  // MARK: Conflicts and notes (inline validation)

  /// Blocking entries refuse the save. Non-blocking ones are facts about the
  /// machine: `fn_tap_intercept_note` is deliberately not a conflict in the
  /// core, and must not be painted as one here either.
  private var blockingConflicts: [HotkeyConflictPresentation] {
    model.bindingConflicts
      .map { $0.presentation(options: model.bindingOptions) }
      .filter(\.blocking)
  }

  private var informationalNotices: [HotkeyConflictPresentation] {
    model.bindingConflicts
      .map { $0.presentation(options: model.bindingOptions) }
      .filter { !$0.blocking }
  }

  private var conflictList: some View {
    VStack(alignment: .leading, spacing: 8) {
      SettingsSectionLabel(String(localized: "Conflicts"))
      ForEach(Array(blockingConflicts.enumerated()), id: \.offset) { _, conflict in
        validationRow(conflict, accent: CSColor.terracotta, marker: "!")
      }
    }
  }

  /// Informational notes read as a quiet field under the save row: the faint
  /// row background, a monochrome globe (the Fn key's own symbol) and
  /// secondary text, so amber stays for conflicts that need a decision
  /// (Founder, 2026-10-08).
  private var noticeList: some View {
    VStack(alignment: .leading, spacing: 4) {
      ForEach(Array(informationalNotices.enumerated()), id: \.offset) { _, notice in
        noticeRow(notice)
      }
    }
  }

  private func noticeRow(_ entry: HotkeyConflictPresentation) -> some View {
    HStack(alignment: .top, spacing: 9) {
      Image(systemName: "globe")
        .font(CSFont.ui(12, .semibold))
        .foregroundStyle(Color.secondary)
        .frame(width: 14)
        .accessibilityHidden(true)
      Text(entry.message)
        .font(CSFont.ui(11.5, .medium))
        .foregroundStyle(Color.secondary)
        .fixedSize(horizontal: false, vertical: true)
    }
    .padding(.horizontal, 14)
    .padding(.vertical, 10)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .fill(Color.primary.opacity(0.04))
    )
  }

  private func validationRow(
    _ entry: HotkeyConflictPresentation, accent: Color, marker: String
  ) -> some View {
    HStack(alignment: .top, spacing: 9) {
      Text(verbatim: marker)
        .font(CSFont.ui(11, .bold))
        .foregroundStyle(accent)
        .frame(width: 14)
      VStack(alignment: .leading, spacing: 2) {
        Text(entry.gesture)
          .font(CSFont.mono(11, .semibold))
          .foregroundStyle(accent)
        Text(entry.message)
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
