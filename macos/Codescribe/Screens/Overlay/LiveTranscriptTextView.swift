import AppKit
import SwiftUI

/// Selection rules for the live transcript's native text view.
///
/// The stream may append or replace its open tail while the user has an older
/// phrase selected. AppKit resets selection when its storage is replaced, so the
/// representable snapshots and restores a clamped UTF-16 range on every update.
/// Keeping this policy pure makes the P0 behavior testable without a recording.
enum LiveTranscriptSelectionPolicy {
  static func preservedRange(_ selection: NSRange, updatedLength: Int) -> NSRange {
    let safeLength = max(0, updatedLength)
    let location = min(max(0, selection.location), safeLength)
    let length = min(max(0, selection.length), safeLength - location)
    return NSRange(location: location, length: length)
  }

  static func followsTail(selection: NSRange, textLength: Int) -> Bool {
    selection.length == 0 && selection.location >= max(0, textLength)
  }
}

/// The one AppKit transcript surface: read-only while recording, an editor for
/// the formatted take.
///
/// `Text` plus SwiftUI's selection overlay loses its selection whenever the
/// rapidly-changing value is rebuilt. A real `NSTextView` owns the responder
/// chain instead: drag selection, Cmd-C, Select All and the standard context
/// menu keep working while the recording and transcript updates continue.
///
/// In the editable phase the same view carries the local revision draft:
/// keystrokes flow out through `onTextChange`, focus transitions through
/// `onEditingChanged` (the panel becomes key only inside that window), and
/// Escape through `onCancelEdit`. Bytes still arrive from the caller — the
/// view never invents transcript truth.
struct LiveTranscriptTextView: NSViewRepresentable {
  let text: String
  /// SwiftUI diffs `String` by canonical equivalence, so "é" and "e\u{301}"
  /// look like no change and `updateNSView` is skipped. The engine owns its
  /// bytes; this identity makes every byte-level revision reach the canvas.
  private let utf8Identity: [UInt8]
  let isEditable: Bool
  let appearance: OverlayAppearance
  let onEditingChanged: ((Bool) -> Void)?
  let onTextChange: ((String) -> Void)?
  let onCancelEdit: (() -> Void)?
  @Environment(\.csTextScale) private var textScale

  init(
    text: String,
    isEditable: Bool = false,
    appearance: OverlayAppearance,
    onEditingChanged: ((Bool) -> Void)? = nil,
    onTextChange: ((String) -> Void)? = nil,
    onCancelEdit: (() -> Void)? = nil
  ) {
    self.text = text
    self.utf8Identity = Array(text.utf8)
    self.isEditable = isEditable
    self.appearance = appearance
    self.onEditingChanged = onEditingChanged
    self.onTextChange = onTextChange
    self.onCancelEdit = onCancelEdit
  }

  func makeCoordinator() -> Coordinator { Coordinator() }

  func makeNSView(context: Context) -> NSScrollView {
    let textView = Self.makeTextView()
    textView.delegate = context.coordinator

    let scrollView = NSScrollView()
    scrollView.borderType = .noBorder
    scrollView.drawsBackground = false
    scrollView.hasHorizontalScroller = false
    scrollView.hasVerticalScroller = true
    scrollView.autohidesScrollers = true
    scrollView.horizontalScrollElasticity = .none
    scrollView.documentView = textView

    update(textView, coordinator: context.coordinator)
    return scrollView
  }

  func updateNSView(_ scrollView: NSScrollView, context: Context) {
    guard let textView = scrollView.documentView as? LiveTranscriptNativeTextView else { return }
    update(textView, coordinator: context.coordinator)
  }

  static func makeTextView() -> LiveTranscriptNativeTextView {
    let textView = LiveTranscriptNativeTextView(usingTextLayoutManager: true)
    textView.isEditable = false
    textView.isSelectable = true
    textView.isRichText = true
    textView.importsGraphics = false
    textView.allowsUndo = false
    textView.drawsBackground = false
    textView.isHorizontallyResizable = false
    textView.isVerticallyResizable = true
    textView.autoresizingMask = [.width]
    textView.textContainerInset = NSSize(width: 0, height: 0)
    textView.textContainer?.lineFragmentPadding = 0
    textView.textContainer?.widthTracksTextView = true
    textView.textContainer?.containerSize = NSSize(
      width: 0,
      height: CGFloat.greatestFiniteMagnitude
    )
    textView.setAccessibilityIdentifier("overlay-transcript-live")
    textView.setAccessibilityLabel("Live transcript")
    return textView
  }

  private func update(
    _ textView: LiveTranscriptNativeTextView,
    coordinator: Coordinator
  ) {
    coordinator.onEditingChanged = onEditingChanged
    coordinator.onTextChange = onTextChange
    coordinator.onCancelEdit = onCancelEdit
    let attributes = transcriptAttributes()
    textView.isEditable = isEditable
    textView.allowsUndo = isEditable
    textView.typingAttributes = attributes
    textView.insertionPointColor = OverlayAppearancePalette.resolve(appearance).bodyText.nsColor

    let rendered = NSAttributedString(string: text, attributes: attributes)
    let sameBytes = textView.string.utf8.elementsEqual(rendered.string.utf8)
    // While the caret is in the canvas the bytes we are handed are the bytes
    // the user just typed; repainting storage would throw the caret away.
    if sameBytes, coordinator.isEditing { return }
    guard !sameBytes || textView.attributedString() != rendered else { return }

    let previousSelection = textView.selectedRange()
    let wasFollowingTail = coordinator.followsTail
    coordinator.applyingUpdate = true
    textView.textStorage?.setAttributedString(rendered)

    let updatedLength = rendered.length
    if previousSelection.length > 0 || !wasFollowingTail {
      textView.setSelectedRange(
        LiveTranscriptSelectionPolicy.preservedRange(
          previousSelection,
          updatedLength: updatedLength
        )
      )
    } else {
      let tail = NSRange(location: updatedLength, length: 0)
      textView.setSelectedRange(tail)
      DispatchQueue.main.async { [weak textView, weak coordinator] in
        guard let textView, coordinator?.followsTail == true else { return }
        textView.scrollRangeToVisible(tail)
      }
    }
    coordinator.applyingUpdate = false
  }

  private func transcriptAttributes() -> [NSAttributedString.Key: Any] {
    let size = 15 * textScale
    let descriptor = NSFontDescriptor(fontAttributes: [
      .family: FontLoader.spaceGrotesk,
      .traits: [NSFontDescriptor.TraitKey.weight: NSFont.Weight.medium.rawValue],
    ])
    let font =
      NSFont(descriptor: descriptor, size: size)
      ?? .systemFont(ofSize: size, weight: .medium)
    let paragraph = NSMutableParagraphStyle()
    paragraph.lineSpacing = 5
    return [
      .font: font,
      .foregroundColor: OverlayAppearancePalette.resolve(appearance).bodyText.nsColor,
      .paragraphStyle: paragraph,
    ]
  }

  @MainActor
  final class Coordinator: NSObject, NSTextViewDelegate {
    var followsTail = true
    var applyingUpdate = false
    var isEditing = false
    var onEditingChanged: ((Bool) -> Void)?
    var onTextChange: ((String) -> Void)?
    var onCancelEdit: (() -> Void)?

    func textViewDidChangeSelection(_ notification: Notification) {
      guard !applyingUpdate,
        let textView = notification.object as? NSTextView
      else { return }
      followsTail = LiveTranscriptSelectionPolicy.followsTail(
        selection: textView.selectedRange(),
        textLength: (textView.string as NSString).length
      )
    }

    func textDidChange(_ notification: Notification) {
      guard !applyingUpdate, let textView = notification.object as? NSTextView else { return }
      onTextChange?(textView.string)
    }

    func textView(
      _ textView: NSTextView, doCommandBy commandSelector: Selector
    ) -> Bool {
      // Escape: NSTextView routes it as `cancelOperation:` first and falls
      // back to `complete:` (word completion) — both mean "drop the draft".
      let isEscape =
        commandSelector == #selector(NSResponder.cancelOperation(_:))
        || commandSelector == #selector(NSTextView.complete(_:))
      guard isEscape else { return false }
      onCancelEdit?()
      textView.window?.makeFirstResponder(nil)
      return true
    }
  }
}

/// First-click selection is important because the overlay is deliberately a
/// non-activating panel: it must not steal focus merely by appearing, but an
/// explicit click in the transcript must immediately begin a drag selection.
///
/// When editable, gaining first responder is what makes the hosting
/// `FloatingOverlayPanel` key; resigning gives the keyboard back.
final class LiveTranscriptNativeTextView: NSTextView {
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

  private var editCoordinator: LiveTranscriptTextView.Coordinator? {
    delegate as? LiveTranscriptTextView.Coordinator
  }

  override func becomeFirstResponder() -> Bool {
    guard super.becomeFirstResponder() else { return false }
    if isEditable, let coordinator = editCoordinator, !coordinator.isEditing {
      (window as? FloatingOverlayPanel)?.takeKeyForEdit()
      coordinator.isEditing = true
      coordinator.onEditingChanged?(true)
    }
    return true
  }

  override func resignFirstResponder() -> Bool {
    guard super.resignFirstResponder() else { return false }
    if let coordinator = editCoordinator, coordinator.isEditing {
      coordinator.isEditing = false
      coordinator.onEditingChanged?(false)
      (window as? FloatingOverlayPanel)?.releaseKeyAfterEdit()
    }
    return true
  }

  @discardableResult
  func copySelection(to pasteboard: NSPasteboard) -> Bool {
    let selection = selectedRange()
    let source = string as NSString
    guard selection.length > 0, NSMaxRange(selection) <= source.length else { return false }

    pasteboard.clearContents()
    return pasteboard.setString(source.substring(with: selection), forType: .string)
  }

  override func copy(_ sender: Any?) {
    _ = copySelection(to: .general)
  }
}
