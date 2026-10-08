import XCTest

@testable import Codescribe

/// The Shortcuts tab carries two save contracts: the three per-mode gestures
/// are a draft that needs the explicit Save button, everything else writes on
/// change. These tests pin the draft half — what a save actually confirms, and
/// how a validation entry is presented — because the bridge can refuse one
/// mode/gesture pair while accepting another in the same save, and the core's
/// `save_if_changed` only warns when a write fails. Neither a returned `Ok` nor
/// the absence of a thrown error proves a gesture landed.
@MainActor
final class ShortcutsSaveReceiptTests: XCTestCase {

  /// Records writes and persists only what it accepts, mirroring the bridge:
  /// `set_mode_binding` rejects Assistive + any Hold gesture outright
  /// (bridge/src/hotkeys.rs), so a save spanning several modes can land
  /// partially.
  private final class RecordingHotkeysEngine: HotkeysEngine {
    struct Rejected: Error { let reason: String }

    private(set) var writes: [(mode: CsWorkMode, binding: CsShortcutBinding)] = []
    private(set) var resets = 0
    private var persisted: [CsModeBinding]
    private let rejects: (CsWorkMode, CsShortcutBinding) -> Bool
    private let conflicts: ([CsModeBinding]) -> [CsHotkeyConflict]

    init(
      persisted: [CsModeBinding] = CsModeBinding.sampleBindings,
      rejects: @escaping (CsWorkMode, CsShortcutBinding) -> Bool = { _, _ in false },
      conflicts: @escaping ([CsModeBinding]) -> [CsHotkeyConflict] = { _ in [] }
    ) {
      self.persisted = persisted
      self.rejects = rejects
      self.conflicts = conflicts
    }

    func modeBindings() -> [CsModeBinding] { persisted }
    func availableBindings() -> [CsBindingOption] { CsBindingOption.sampleOptions }

    func setModeBinding(mode: CsWorkMode, binding: CsShortcutBinding) throws {
      writes.append((mode, binding))
      if rejects(mode, binding) {
        throw Rejected(reason: "\(mode) refuses \(binding)")
      }
      guard let index = persisted.firstIndex(where: { $0.mode == mode }) else { return }
      persisted[index] = CsModeBinding(
        mode: mode,
        modeLabel: persisted[index].modeLabel,
        modeDescription: persisted[index].modeDescription,
        binding: binding,
        bindingLabel: CsBindingOption.sampleOptions.first { $0.binding == binding }?.label ?? ""
      )
    }

    func resetToDefaults() throws {
      resets += 1
      persisted = CsModeBinding.sampleBindings
    }

    func validate(candidate: [CsModeBinding]) -> [CsHotkeyConflict] { conflicts(candidate) }
    func rearmAfterPermissionGrant() {}
    func channelModifier() -> String { "ctrl" }
    func setChannelModifier(_ value: String) throws {}
    func fnTapTogglesDictation() -> Bool { false }
    func setFnTapTogglesDictation(_ enabled: Bool) throws {}
    func middleMouseActsAsFn() -> Bool { false }
    func setMiddleMouseActsAsFn(_ enabled: Bool) throws {}
  }

  private func model(_ hotkeys: HotkeysEngine) -> SettingsViewModel {
    let model = SettingsViewModel(
      engine: MockSettingsEngine(),
      permissionProbe: MockPermissionProbe(),
      hotkeys: hotkeys
    )
    model.loadHotkeys()
    return model
  }

  /// A save that lands everywhere names every requested mode as saved, clears
  /// the pending state, and reports no failure.
  func testSaveConfirmsEveryModeThatReachedDisk() throws {
    let engine = RecordingHotkeysEngine()
    let model = model(engine)

    model.editDraftBinding(mode: .dictation, binding: .doubleCtrl)
    model.editDraftBinding(mode: .formatting, binding: .holdCtrlShift)
    XCTAssertTrue(model.hasPendingBindingChanges)
    XCTAssertTrue(model.canSaveBindings)

    model.saveBindings()

    let receipt = try XCTUnwrap(model.bindingSaveReceipt)
    XCTAssertEqual(receipt.saved, [.dictation, .formatting])
    XCTAssertEqual(receipt.rejected, [])
    XCTAssertNil(receipt.failureDetail)
    XCTAssertFalse(model.hasPendingBindingChanges)
    XCTAssertNotNil(receipt.sentence)
  }

  /// A partial save keeps going past the first refusal, names the refused mode,
  /// and snaps its picker back to the gesture that is actually in effect — the
  /// draft must never claim a gesture disk does not hold.
  func testPartialSaveNamesTheRefusedModeAndRestoresPersistedTruth() throws {
    let engine = RecordingHotkeysEngine(rejects: { mode, binding in
      mode == .assistive && binding == .holdCtrl
    })
    let model = model(engine)
    let assistiveBefore = model.modeBindings.first { $0.mode == .assistive }?.binding

    model.editDraftBinding(mode: .assistive, binding: .holdCtrl)
    model.editDraftBinding(mode: .dictation, binding: .doubleCtrl)
    model.saveBindings()

    // Both modes were attempted; one refusal does not abandon the other write.
    XCTAssertEqual(engine.writes.count, 2)
    let receipt = try XCTUnwrap(model.bindingSaveReceipt)
    XCTAssertEqual(receipt.saved, [.dictation])
    XCTAssertEqual(receipt.rejected, [.assistive])
    XCTAssertTrue(receipt.hasRejection)
    XCTAssertNotNil(receipt.failureDetail)
    XCTAssertEqual(
      model.draftBindings.first { $0.mode == .assistive }?.binding, assistiveBefore,
      "a refused gesture must not linger in the picker")
    XCTAssertEqual(model.draftBindings.first { $0.mode == .dictation }?.binding, .doubleCtrl)
    XCTAssertFalse(model.hasPendingBindingChanges)
  }

  /// A partial save can leave the snapped-back draft in a blocking conflict:
  /// Dictation=Double Ctrl lands, Agent=Hold Ctrl is refused and snaps back to
  /// Double Right Option, which Double Ctrl disables
  /// (app/os/shortcut_registry.rs). The conflict must not swallow the receipt:
  /// both the completed write and the refusal stay reportable.
  func testPartialSaveKeepsItsReceiptWhenThePersistedSetConflicts() throws {
    let engine = RecordingHotkeysEngine(
      rejects: { mode, binding in mode == .assistive && binding == .holdCtrl },
      conflicts: { candidate in
        let dictation = candidate.first { $0.mode == .dictation }?.binding
        let assistive = candidate.first { $0.mode == .assistive }?.binding
        guard dictation == .doubleCtrl, assistive == .doubleRightOption else { return [] }
        return [
          CsHotkeyConflict(
            gestureLabel: "Double-tap Right Option",
            message: "Dictation is set to Double Ctrl, so Right Option toggle is disabled.",
            blocking: true)
        ]
      })
    let model = model(engine)
    XCTAssertEqual(model.draftBindings.first { $0.mode == .assistive }?.binding, .doubleRightOption)

    model.editDraftBinding(mode: .dictation, binding: .doubleCtrl)
    model.editDraftBinding(mode: .assistive, binding: .holdCtrl)
    XCTAssertTrue(model.canSaveBindings, "the draft itself is conflict-free")

    model.saveBindings()

    XCTAssertTrue(model.hasBlockingBindingConflicts, "the snapped-back set conflicts")
    let receipt = try XCTUnwrap(model.bindingSaveReceipt, "a conflict must not hide the receipt")
    XCTAssertEqual(receipt.saved, [.dictation])
    XCTAssertEqual(receipt.rejected, [.assistive])
    XCTAssertNotNil(receipt.failureDetail)
  }

  /// The receipt confirms persistence only. The detector routes a subset of
  /// the pairs the picker offers (docs/HOTKEYS_CONTRACT.md), so the sentence
  /// must not claim the gesture is in effect.
  func testSavedSentenceClaimsPersistenceNotEffect() throws {
    let receipt = HotkeyBindingSaveReceipt(
      saved: [.formatting], rejected: [], failureDetail: nil, locale: Locale(identifier: "en"))
    let sentence = try XCTUnwrap(receipt.sentence)
    XCTAssertTrue(sentence.hasPrefix("Saved: "), sentence)
    XCTAssertFalse(sentence.localizedCaseInsensitiveContains("in effect"), sentence)
  }

  /// Mode names are joined in the interface language, not the regional
  /// locale: a Polish UI on an English-region Mac still reads "i", not "and".
  func testModeListFollowsTheInterfaceLocale() throws {
    let english = HotkeyBindingSaveReceipt(
      saved: [.dictation, .assistive], rejected: [], failureDetail: nil,
      locale: Locale(identifier: "en"))
    let polish = HotkeyBindingSaveReceipt(
      saved: [.dictation, .assistive], rejected: [], failureDetail: nil,
      locale: Locale(identifier: "pl"))

    XCTAssertTrue(try XCTUnwrap(english.sentence).contains(" and "))
    XCTAssertTrue(try XCTUnwrap(polish.sentence).contains(" i "))
    XCTAssertFalse(try XCTUnwrap(polish.sentence).contains(" and "))
  }

  /// The receipt describes one finished save. Editing again, or resetting,
  /// makes it stale, so it goes away instead of confirming a state the screen
  /// no longer shows.
  func testReceiptIsClearedByTheNextEditAndByReset() {
    let engine = RecordingHotkeysEngine()
    let model = model(engine)

    model.editDraftBinding(mode: .dictation, binding: .doubleCtrl)
    model.saveBindings()
    XCTAssertNotNil(model.bindingSaveReceipt)

    model.editDraftBinding(mode: .dictation, binding: .holdFn)
    XCTAssertNil(model.bindingSaveReceipt)

    model.saveBindings()
    XCTAssertNotNil(model.bindingSaveReceipt)
    model.resetBindingsToDefaults()
    XCTAssertNil(model.bindingSaveReceipt)
    XCTAssertEqual(engine.resets, 1)
  }

  /// Every sentence names the modes it is talking about, and an empty receipt
  /// has nothing to say.
  func testReceiptSentenceNamesTheModesItReportsOn() throws {
    let saved = HotkeyBindingSaveReceipt(saved: [.dictation], rejected: [], failureDetail: nil)
    let partial = HotkeyBindingSaveReceipt(
      saved: [.dictation], rejected: [.assistive], failureDetail: nil)
    let nothing = HotkeyBindingSaveReceipt(saved: [], rejected: [], failureDetail: nil)

    let savedSentence = try XCTUnwrap(saved.sentence)
    XCTAssertTrue(savedSentence.contains(CsWorkMode.dictation.visibleName))
    let partialSentence = try XCTUnwrap(partial.sentence)
    XCTAssertTrue(partialSentence.contains(CsWorkMode.dictation.visibleName))
    XCTAssertTrue(partialSentence.contains(CsWorkMode.assistive.visibleName))
    XCTAssertNil(nothing.sentence)
    XCTAssertTrue(nothing.isEmpty)
    XCTAssertFalse(saved.hasRejection)
    XCTAssertTrue(partial.hasRejection)
  }

  /// The macOS Fn-tap note is deliberately NOT a conflict in the core
  /// (app/os/shortcut_registry.rs) and must not be presented as one: it stays
  /// non-blocking and gets its own sentence.
  func testFnTapNoteStaysNonBlockingAndLocalized() {
    let note = CsHotkeyConflict(
      gestureLabel: "Hold Fn/Globe",
      message:
        "Fn/Globe tap is configured by macOS. Codescribe Hold Fn may intercept that tap while dictation is active; this is informational, not a shortcut conflict.",
      blocking: false
    )

    let presented = note.presentation(options: CsBindingOption.sampleOptions)

    XCTAssertFalse(presented.blocking)
    XCTAssertNotEqual(presented.message, note.message, "the note must carry our own sentence")
    XCTAssertEqual(presented.technical, "\(note.gestureLabel) · \(note.message)")
    XCTAssertEqual(presented.gesture, CsShortcutBinding.holdFn.visibleName)
  }

  /// A blocking conflict keeps its blocking flag, swaps the wire sentence for a
  /// localized one and keeps the wire strings recoverable underneath.
  func testBlockingConflictIsLocalizedWithoutLosingTheWireStrings() {
    let conflict = CsHotkeyConflict(
      gestureLabel: "Double-tap Left Option",
      message: "Dictation is set to Double Ctrl, so Left Option toggle is disabled.",
      blocking: true
    )

    let presented = conflict.presentation(options: CsBindingOption.sampleOptions)

    XCTAssertTrue(presented.blocking)
    XCTAssertNotEqual(presented.message, conflict.message)
    XCTAssertEqual(presented.technical, "\(conflict.gestureLabel) · \(conflict.message)")
    XCTAssertEqual(presented.gesture, CsShortcutBinding.doubleLeftOption.visibleName)
  }

  /// An unmapped wire sentence is shown verbatim rather than mistranslated, and
  /// then there is no separate technical line to repeat it.
  func testUnmappedConflictFallsThroughVerbatim() {
    let conflict = CsHotkeyConflict(
      gestureLabel: "Hold Ctrl",
      message: "Some future rule the Swift side has never heard of.",
      blocking: true
    )

    let presented = conflict.presentation(options: CsBindingOption.sampleOptions)

    XCTAssertEqual(presented.message, conflict.message)
    XCTAssertNil(presented.technical)
  }

  /// Every gesture reads differently for a screen reader, and the spoken form
  /// never reuses the terse key-cap label — left and right Option stay apart.
  func testSpokenGestureNamesAreDistinctAndNotTheKeyCapLabels() {
    let bindings: [CsShortcutBinding] = [
      .disabled, .holdFn, .holdCtrl, .holdCtrlAlt, .holdCtrlShift, .holdCtrlCmd,
      .doubleCtrl, .doubleLeftOption, .doubleRightOption,
    ]
    let spoken = bindings.map(\.spokenName)

    XCTAssertEqual(Set(spoken).count, bindings.count)
    for binding in bindings {
      XCTAssertNotEqual(binding.spokenName, binding.visibleName, "\(binding)")
      XCTAssertFalse(binding.spokenName.contains("⌥"), "\(binding)")
      XCTAssertFalse(binding.spokenName.contains("⌃"), "\(binding)")
    }
  }
}
