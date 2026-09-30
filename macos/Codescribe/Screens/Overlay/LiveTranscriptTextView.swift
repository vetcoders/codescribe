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

struct LiveTranscriptScrollFollowState: Equatable {
  enum Mode: Equatable {
    case followingTail
    case manualScroll
    case detached
  }

  private(set) var mode: Mode = .followingTail
  private(set) var revealRevision = 0

  var followsTail: Bool { mode == .followingTail }
  var isManualScrollActive: Bool { mode == .manualScroll }

  mutating func userScrollBegan() {
    transition(to: .manualScroll)
  }

  mutating func userScrollMoved(isAtLiveEdge: Bool, hasSelection: Bool) {
    guard !isManualScrollActive else { return }
    settle(isAtLiveEdge: isAtLiveEdge, hasSelection: hasSelection)
  }

  mutating func userScrollEnded(isAtLiveEdge: Bool, hasSelection: Bool) {
    settle(isAtLiveEdge: isAtLiveEdge, hasSelection: hasSelection)
  }

  mutating func selectionChanged(
    _ selection: NSRange,
    textLength: Int,
    isAtLiveEdge: Bool
  ) {
    guard !isManualScrollActive else { return }
    guard LiveTranscriptSelectionPolicy.followsTail(selection: selection, textLength: textLength)
    else {
      transition(to: .detached)
      return
    }
    guard isAtLiveEdge else { return }
    transition(to: .followingTail)
  }

  func allowsTailReveal(revision: Int) -> Bool {
    revision == revealRevision && followsTail
  }

  private mutating func settle(isAtLiveEdge: Bool, hasSelection: Bool) {
    transition(to: isAtLiveEdge && !hasSelection ? .followingTail : .detached)
  }

  private mutating func transition(to next: Mode) {
    guard mode != next else { return }
    mode = next
    revealRevision &+= 1
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
  let contentInsets: NSEdgeInsets
  let onEditingChanged: ((Bool) -> Void)?
  let onTextChange: ((String) -> Void)?
  let onCancelEdit: (() -> Void)?
  @Environment(\.csTextScale) private var textScale

  init(
    text: String,
    isEditable: Bool = false,
    appearance: OverlayAppearance,
    contentInsets: NSEdgeInsets = NSEdgeInsetsZero,
    onEditingChanged: ((Bool) -> Void)? = nil,
    onTextChange: ((String) -> Void)? = nil,
    onCancelEdit: (() -> Void)? = nil
  ) {
    self.text = text
    self.utf8Identity = Array(text.utf8)
    self.isEditable = isEditable
    self.appearance = appearance
    self.contentInsets = contentInsets
    self.onEditingChanged = onEditingChanged
    self.onTextChange = onTextChange
    self.onCancelEdit = onCancelEdit
  }

  func makeCoordinator() -> Coordinator { Coordinator() }

  func makeNSView(context: Context) -> NSScrollView {
    let textView = Self.makeTextView()
    textView.delegate = context.coordinator

    let scrollView = LiveTranscriptScrollView()
    scrollView.followCoordinator = context.coordinator
    scrollView.automaticallyAdjustsContentInsets = false
    scrollView.contentInsets = NSEdgeInsetsZero
    scrollView.contentView.automaticallyAdjustsContentInsets = false
    scrollView.contentView.contentInsets = contentInsets
    scrollView.borderType = .noBorder
    scrollView.drawsBackground = false
    scrollView.hasHorizontalScroller = false
    scrollView.hasVerticalScroller = true
    scrollView.autohidesScrollers = true
    scrollView.horizontalScrollElasticity = .none
    scrollView.documentView = textView
    context.coordinator.attach(to: scrollView)

    update(textView, coordinator: context.coordinator)
    return scrollView
  }

  static func dismantleNSView(_ scrollView: NSScrollView, coordinator: Coordinator) {
    coordinator.detach()
  }

  func updateNSView(_ scrollView: NSScrollView, context: Context) {
    context.coordinator.attach(to: scrollView)
    let oldInsets = scrollView.contentView.contentInsets
    let insetsChanged =
      oldInsets.top != contentInsets.top || oldInsets.bottom != contentInsets.bottom
      || oldInsets.left != contentInsets.left || oldInsets.right != contentInsets.right
    scrollView.contentView.contentInsets = contentInsets
    guard let textView = scrollView.documentView as? LiveTranscriptNativeTextView else { return }
    update(textView, coordinator: context.coordinator)
    if insetsChanged {
      context.coordinator.scheduleTailReveal(for: textView)
    }
  }

  static func makeTextView() -> LiveTranscriptNativeTextView {
    let textView = LiveTranscriptNativeTextView(usingTextLayoutManager: true)
    // Build 1487 crashed in NSWritingToolsEditTracker when proofreading raced
    // a live transcript revision; the reducer alone owns these text changes.
    if #available(macOS 15.0, *) {
      textView.writingToolsBehavior = .none
    }
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

  func update(
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

    let sameBytes = textView.string.utf8.elementsEqual(text.utf8)
    let sameAttributes =
      coordinator.renderedAttributes.map {
        NSDictionary(dictionary: $0).isEqual(to: attributes)
      } ?? false
    // While the caret is in the canvas the bytes we are handed are the bytes
    // the user just typed; repainting storage would throw the caret away.
    if sameBytes, coordinator.isEditing { return }
    guard !sameBytes || !sameAttributes else { return }

    let previousSelection = textView.selectedRange()
    let wasFollowingTail = coordinator.followsTail
    let previousOrigin = textView.enclosingScrollView?.contentView.bounds.origin
    coordinator.applyingUpdate = true
    let previous = textView.string as NSString
    let incoming = text as NSString
    if sameAttributes {
      // Preserve the recorded prefix in TextKit. Replacing the whole storage
      // invalidates every paragraph on each live append or open-tail revision.
      // Compare UTF-16 units, not Characters: canonical Unicode equivalence
      // must never hide a byte-level revision from the engine.
      var prefix = 0
      let sharedLength = min(previous.length, incoming.length)
      while prefix < sharedLength, previous.character(at: prefix) == incoming.character(at: prefix)
      {
        prefix += 1
      }
      if prefix > 0, prefix < incoming.length,
        (0xDC00...0xDFFF).contains(incoming.character(at: prefix))
      {
        prefix -= 1  // Do not split a surrogate pair at the changed boundary.
      }
      let tail = NSAttributedString(
        string: incoming.substring(from: prefix), attributes: attributes)
      textView.textStorage?.replaceCharacters(
        in: NSRange(location: prefix, length: previous.length - prefix), with: tail)
    } else {
      textView.textStorage?.setAttributedString(
        NSAttributedString(string: text, attributes: attributes))
    }
    coordinator.renderedAttributes = attributes

    let updatedLength = incoming.length
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
      coordinator.scheduleTailReveal(for: textView, expectedLength: tail.location)
    }
    if !wasFollowingTail, !coordinator.isEditing, let previousOrigin,
      let scroll = textView.enclosingScrollView
    {
      textView.layoutSubtreeIfNeeded()
      scroll.contentView.scroll(to: previousOrigin)
      scroll.reflectScrolledClipView(scroll.contentView)
    }
    coordinator.applyingUpdate = false
  }

  private func transcriptAttributes() -> [NSAttributedString.Key: Any] {
    let size = 15 * textScale
    let font = NSFont.systemFont(ofSize: size, weight: .regular)
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
    private(set) var scrollFollowState = LiveTranscriptScrollFollowState()
    var followsTail: Bool { scrollFollowState.followsTail }
    var applyingUpdate = false
    var isEditing = false
    var renderedAttributes: [NSAttributedString.Key: Any]?
    var onEditingChanged: ((Bool) -> Void)?
    var onTextChange: ((String) -> Void)?
    var onCancelEdit: (() -> Void)?
    private weak var observedScrollView: NSScrollView?
    private var scrollObservers: [NSObjectProtocol] = []

    func attach(to scrollView: NSScrollView) {
      guard observedScrollView !== scrollView else { return }
      detach()
      observedScrollView = scrollView
      let center = NotificationCenter.default
      scrollObservers = [
        center.addObserver(
          forName: NSScrollView.willStartLiveScrollNotification,
          object: scrollView,
          queue: nil
        ) { [weak self] _ in
          MainActor.assumeIsolated {
            self?.userScrollBegan()
          }
        },
        center.addObserver(
          forName: NSScrollView.didLiveScrollNotification,
          object: scrollView,
          queue: nil
        ) { [weak self] _ in
          MainActor.assumeIsolated {
            self?.reportScrollMovement()
          }
        },
        center.addObserver(
          forName: NSScrollView.didEndLiveScrollNotification,
          object: scrollView,
          queue: nil
        ) { [weak self] _ in
          MainActor.assumeIsolated {
            self?.reportScrollEnd()
          }
        },
      ]
    }

    func detach() {
      for observer in scrollObservers { NotificationCenter.default.removeObserver(observer) }
      scrollObservers.removeAll()
      observedScrollView = nil
    }

    func userScrollBegan() {
      scrollFollowState.userScrollBegan()
    }

    func userScrollMoved(isAtLiveEdge: Bool, hasSelection: Bool) {
      scrollFollowState.userScrollMoved(
        isAtLiveEdge: isAtLiveEdge,
        hasSelection: hasSelection
      )
    }

    func userScrollEnded(isAtLiveEdge: Bool, hasSelection: Bool) {
      scrollFollowState.userScrollEnded(
        isAtLiveEdge: isAtLiveEdge,
        hasSelection: hasSelection
      )
    }

    func reportScrollMovement(in scrollView: NSScrollView? = nil) {
      guard let scrollView = scrollView ?? observedScrollView,
        let documentView = scrollView.documentView,
        let textView = documentView as? NSTextView
      else { return }
      userScrollMoved(
        isAtLiveEdge: isAtLiveEdge(documentView: documentView, in: scrollView),
        hasSelection: textView.selectedRange().length > 0
      )
    }

    func reportScrollEnd(in scrollView: NSScrollView? = nil) {
      guard let scrollView = scrollView ?? observedScrollView,
        let documentView = scrollView.documentView,
        let textView = documentView as? NSTextView
      else { return }
      userScrollEnded(
        isAtLiveEdge: isAtLiveEdge(documentView: documentView, in: scrollView),
        hasSelection: textView.selectedRange().length > 0
      )
    }

    private func isAtLiveEdge(documentView: NSView, in scrollView: NSScrollView) -> Bool {
      let clip = scrollView.contentView
      let liveBottomOrigin = LiveTranscriptNativeTextView.liveBottomScrollOrigin(
        documentMaxY: documentView.bounds.maxY,
        clipHeight: clip.bounds.height,
        contentInsets: clip.contentInsets
      )
      return clip.bounds.origin.y >= liveBottomOrigin - 2
    }

    func scheduleTailReveal(
      for textView: LiveTranscriptNativeTextView,
      expectedLength: Int? = nil
    ) {
      guard scrollFollowState.followsTail else { return }
      let revision = scrollFollowState.revealRevision
      DispatchQueue.main.async { [weak self, weak textView] in
        guard let self, let textView,
          self.scrollFollowState.allowsTailReveal(revision: revision),
          !self.isEditing,
          textView.selectedRange().length == 0,
          expectedLength == nil || (textView.string as NSString).length == expectedLength
        else { return }
        textView.revealTranscriptTail()
      }
    }

    func textViewDidChangeSelection(_ notification: Notification) {
      guard !applyingUpdate,
        let textView = notification.object as? NSTextView
      else { return }
      let scrollView = textView.enclosingScrollView
      let viewportAtLiveEdge = scrollView.flatMap { scrollView in
        scrollView.documentView.map { isAtLiveEdge(documentView: $0, in: scrollView) }
      } ?? false
      scrollFollowState.selectionChanged(
        textView.selectedRange(),
        textLength: (textView.string as NSString).length,
        isAtLiveEdge: viewportAtLiveEdge
      )
    }

    func textDidChange(_ notification: Notification) {
      guard !applyingUpdate, let textView = notification.object as? NSTextView else { return }
      // Native edits (including rich paste) can change attributes independently
      // of the projection. Reapply the transcript style when editing finishes.
      renderedAttributes = nil
      onTextChange?(textView.string)
    }

    func textView(
      _ textView: NSTextView, doCommandBy commandSelector: Selector
    ) -> Bool {
      let isScrollCommand =
        commandSelector == #selector(NSResponder.scrollPageUp(_:))
        || commandSelector == #selector(NSResponder.scrollPageDown(_:))
        || commandSelector == #selector(NSResponder.scrollLineUp(_:))
        || commandSelector == #selector(NSResponder.scrollLineDown(_:))
        || commandSelector == #selector(NSResponder.scrollToBeginningOfDocument(_:))
        || commandSelector == #selector(NSResponder.scrollToEndOfDocument(_:))
      if isScrollCommand, let scrollView = textView.enclosingScrollView {
        userScrollBegan()
        DispatchQueue.main.async { [weak self, weak scrollView] in
          guard let self, let scrollView else { return }
          self.reportScrollEnd(in: scrollView)
        }
        return false
      }
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

/// Wheel input owns the viewport before AppKit delivers any bounds changes.
final class LiveTranscriptScrollView: NSScrollView {
  weak var followCoordinator: LiveTranscriptTextView.Coordinator?

  override func scrollWheel(with event: NSEvent) {
    followCoordinator?.userScrollBegan()
    super.scrollWheel(with: event)
    let phaseEnded = event.phase.contains(.ended) || event.phase.contains(.cancelled)
    let momentumEnded =
      event.momentumPhase.contains(.ended) || event.momentumPhase.contains(.cancelled)
    if (event.phase.isEmpty && event.momentumPhase.isEmpty)
      || momentumEnded || (phaseEnded && event.momentumPhase.isEmpty)
    {
      followCoordinator?.reportScrollEnd(in: self)
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
  static func liveBottomScrollOrigin(
    documentMaxY: CGFloat,
    clipHeight: CGFloat,
    contentInsets: NSEdgeInsets
  ) -> CGFloat {
    max(-contentInsets.top, documentMaxY + contentInsets.bottom - clipHeight)
  }

  func revealTranscriptTail() {
    let length = (string as NSString).length
    let range = NSRange(location: max(0, length - 1), length: min(1, length))
    scrollRangeToVisible(range)
    guard let scroll = enclosingScrollView else { return }
    // A transcript ending in a newline has an empty insertion line after its
    // final glyph. Revealing only that glyph can leave the viewport one line
    // short of the document's live edge, so place the document extent at the
    // viewport bottom after TextKit has laid out the requested tail range.
    // TextKit 2 lays that trailing line out only once the viewport reaches it,
    // so the document grows after the scroll that uncovered it. Lay the new
    // viewport out and re-place the bottom while the extent still moves; the
    // growth is the uncovered tail, so the second pass settles.
    for _ in 0..<3 {
      let documentMaxY = bounds.maxY
      var origin = scroll.contentView.bounds.origin
      origin.y = Self.liveBottomScrollOrigin(
        documentMaxY: documentMaxY,
        clipHeight: scroll.contentView.bounds.height,
        contentInsets: scroll.contentView.contentInsets
      )
      scroll.contentView.scroll(to: origin)
      scroll.reflectScrolledClipView(scroll.contentView)
      textLayoutManager?.textViewportLayoutController.layoutViewport()
      if bounds.maxY == documentMaxY { return }
    }
  }

  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

  private var editCoordinator: LiveTranscriptTextView.Coordinator? {
    delegate as? LiveTranscriptTextView.Coordinator
  }

  func beginEditingIfNeeded() {
    guard isEditable, let coordinator = editCoordinator, !coordinator.isEditing else { return }
    (window as? FloatingOverlayPanel)?.takeKeyForEdit()
    coordinator.isEditing = true
    coordinator.onEditingChanged?(true)
  }

  override func becomeFirstResponder() -> Bool {
    guard super.becomeFirstResponder() else { return false }
    beginEditingIfNeeded()
    return true
  }

  override func mouseDown(with event: NSEvent) {
    super.mouseDown(with: event)
    // AppKit can preselect this view while the take is still read-only. An
    // explicit later click must open the edit gate even if responder identity
    // does not change and becomeFirstResponder is therefore not called again.
    if window?.firstResponder === self { beginEditingIfNeeded() }
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
