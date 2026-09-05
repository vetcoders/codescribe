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

/// Read-only AppKit transcript surface used while recording.
///
/// `Text` plus SwiftUI's selection overlay loses its selection whenever the
/// rapidly-changing value is rebuilt. A real `NSTextView` owns the responder
/// chain instead: drag selection, Cmd-C, Select All and the standard context
/// menu keep working while the recording and transcript updates continue.
struct LiveTranscriptTextView: NSViewRepresentable {
  let text: String
  let appearance: OverlayAppearance
  @Environment(\.csTextScale) private var textScale

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
    let rendered = attributedTranscript()
    guard textView.attributedString() != rendered else { return }

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

  private func attributedTranscript() -> NSAttributedString {
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
    let result = NSMutableAttributedString()

    let attributes: [NSAttributedString.Key: Any] = [
      .font: font,
      .foregroundColor: OverlayAppearancePalette.resolve(appearance).bodyText.nsColor,
      .paragraphStyle: paragraph,
    ]
    result.append(NSAttributedString(string: text, attributes: attributes))
    return result
  }

  @MainActor
  final class Coordinator: NSObject, NSTextViewDelegate {
    var followsTail = true
    var applyingUpdate = false

    func textViewDidChangeSelection(_ notification: Notification) {
      guard !applyingUpdate,
        let textView = notification.object as? NSTextView
      else { return }
      followsTail = LiveTranscriptSelectionPolicy.followsTail(
        selection: textView.selectedRange(),
        textLength: (textView.string as NSString).length
      )
    }
  }
}

/// First-click selection is important because the overlay is deliberately a
/// non-activating panel: it must not steal focus merely by appearing, but an
/// explicit click in the transcript must immediately begin a drag selection.
final class LiveTranscriptNativeTextView: NSTextView {
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

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
