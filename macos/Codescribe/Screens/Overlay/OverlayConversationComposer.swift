import AppKit
import SwiftUI

/// One input surface; the conversation owner retains the draft and publishes it.
struct OverlayConversationComposer: View {
  @Environment(\.csTextScale) private var textScale
  let palette: OverlayAppearancePalette
  @Binding var draft: String
  let sending: Bool
  let onSubmit: () -> Void
  var onEditorActive: (Bool) -> Void = { _ in }
  var onTypingActivity: () -> Void = {}
  var onFocusChange: ((Bool) -> Void)? = nil
  var placeholder = String(localized: "Message the Agent")
  var clipboard: NSPasteboard = .general
  var saveImage: (Data) throws -> URL = { data in
    URL(fileURLWithPath: try savePastedImage(data: data))
  }
  var saveFile: (Data, String) throws -> URL = { data, name in
    URL(fileURLWithPath: try savePastedAttachment(data: data, name: name))
  }
  @State private var pasteError: String?
  @State private var editorHeight: CGFloat = 28

  var body: some View {
    VStack(alignment: .leading, spacing: 4) {
      composerContent
        .modifier(OverlayControlGlass())
      if let pasteError {
        Text(verbatim: pasteError)
          .font(.system(size: 10 * textScale))
          .foregroundStyle(palette.errorStatus.color)
          .accessibilityIdentifier("overlay-conversation-paste-error")
      }
    }
  }

  /// The editor holds the prose; pasted attachments ride as pills.
  private var prose: Binding<String> {
    Binding(
      get: { ConversationDraft.split(draft).text },
      set: { draft = ConversationDraft.compose(text: $0, attachments: attachments) })
  }

  private var attachments: [String] { ConversationDraft.split(draft).attachments }

  private func attach(_ paths: [String]) {
    let parts = ConversationDraft.split(draft)
    let added = paths.filter { !parts.attachments.contains($0) }
    guard !added.isEmpty else { return }
    draft = ConversationDraft.compose(text: parts.text, attachments: parts.attachments + added)
  }

  private func detach(_ path: String) {
    let parts = ConversationDraft.split(draft)
    draft = ConversationDraft.compose(
      text: parts.text, attachments: parts.attachments.filter { $0 != path })
  }

  private var composerContent: some View {
    VStack(alignment: .leading, spacing: 6) {
      let attached = attachments
      if !attached.isEmpty {
        ScrollView(.horizontal, showsIndicators: false) {
          HStack(spacing: 6) {
            ForEach(attached, id: \.self) { path in
              OverlayComposerAttachmentPill(path: path, palette: palette) { detach(path) }
            }
          }
        }
        .frame(maxHeight: 30)
        .accessibilityIdentifier("overlay-conversation-attachments")
      }
      editorRow
    }
    .padding(.horizontal, ComposerControlMetrics.horizontalPadding)
    .padding(.vertical, ComposerControlMetrics.verticalPadding)
  }

  private var editorRow: some View {
    HStack(alignment: .bottom, spacing: ComposerControlMetrics.spacing) {
      ConversationMessageField(
        text: prose, height: $editorHeight,
        textColor: palette.primaryText.nsColor,
        fontSize: ComposerControlMetrics.textSize * textScale,
        sending: sending, placeholder: placeholder, onSubmit: onSubmit,
        onEditorActive: onEditorActive,
        onTypingActivity: onTypingActivity,
        onFocusChange: { onFocusChange?($0) },
        clipboard: clipboard, saveImage: saveImage, saveFile: saveFile,
        onPasteError: { pasteError = $0 },
        hasAttachments: { !attachments.isEmpty }, onAttach: attach
      )
      .frame(height: editorHeight)
      .transaction { $0.animation = nil }
      .accessibilityIdentifier("overlay-conversation-composer")
      .help("Paste files, images, or long text to attach a local copy.")
      .overlay(alignment: .topLeading) {
        if prose.wrappedValue.isEmpty {
          Text(verbatim: placeholder)
            .font(.system(size: ComposerControlMetrics.textSize * textScale))
            .foregroundStyle(palette.mutedText.color)
            .padding(.top, ComposerControlMetrics.textVerticalInset)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
        }
      }
      Button("Send", systemImage: "paperplane.fill", action: onSubmit)
        .labelStyle(.iconOnly)
        .font(.system(size: ComposerControlMetrics.glyphSize, weight: .semibold))
        .foregroundStyle(CSColor.terracotta)
        .frame(
          width: ComposerControlMetrics.hitTargetSize, height: ComposerControlMetrics.hitTargetSize
        )
        .contentShape(Circle())
        .buttonStyle(.plain)
        .disabled(sending || draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        .accessibilityIdentifier("overlay-conversation-send")
    }
  }
}

/// A pasted attachment in the composer: the bubble's pill plus a remove cross.
struct OverlayComposerAttachmentPill: View {
  let path: String
  let palette: OverlayAppearancePalette
  let onRemove: () -> Void
  @State private var thumbnail: NSImage?

  private var name: String { URL(fileURLWithPath: path).lastPathComponent }

  var body: some View {
    HStack(spacing: 6) {
      if let thumbnail {
        Image(nsImage: thumbnail)
          .resizable()
          .aspectRatio(contentMode: .fill)
          .frame(width: 18, height: 18)
          .clipShape(RoundedRectangle(cornerRadius: 4, style: .continuous))
      } else {
        Image(systemName: "doc")
          .font(.system(size: 10, weight: .medium))
          .foregroundStyle(palette.mutedText.color)
      }
      Text(verbatim: name)
        .font(CSFont.mono(10.5, .medium))
        .foregroundStyle(palette.primaryText.color)
        .lineLimit(1)
        .truncationMode(.middle)
        .frame(maxWidth: 160)
      Button(action: onRemove) {
        Image(systemName: "xmark")
          .font(.system(size: 8, weight: .bold))
          .foregroundStyle(palette.mutedText.color)
          .frame(width: 14, height: 14)
          .contentShape(Circle())
      }
      .buttonStyle(.plain)
      .help("Remove attachment")
      .accessibilityLabel("Remove attachment")
      .accessibilityIdentifier("overlay-conversation-attachment-remove")
    }
    .padding(.horizontal, 9)
    .padding(.vertical, 5)
    .background(palette.primaryText.color.opacity(0.05))
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.pill, style: .continuous)
        .strokeBorder(palette.primaryText.color.opacity(0.10), lineWidth: 1)
    )
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.pill, style: .continuous))
    .help(path)
    .accessibilityElement(children: .contain)
    .accessibilityLabel(Text(verbatim: name))
    .task(id: path) { thumbnail = NSImage(contentsOf: URL(fileURLWithPath: path)) }
  }
}

/// SwiftUI owns the draft; AppKit owns text editing and the responder chain.
private struct ConversationMessageField: NSViewRepresentable {
  @Binding var text: String
  @Binding var height: CGFloat
  let textColor: NSColor
  let fontSize: CGFloat
  let sending: Bool
  let placeholder: String
  let onSubmit: () -> Void
  var onEditorActive: (Bool) -> Void = { _ in }
  var onTypingActivity: () -> Void = {}
  var onFocusChange: (Bool) -> Void = { _ in }
  let clipboard: NSPasteboard
  let saveImage: (Data) throws -> URL
  let saveFile: (Data, String) throws -> URL
  let onPasteError: (String?) -> Void
  var hasAttachments: () -> Bool = { false }
  var onAttach: ([String]) -> Void = { _ in }

  func makeCoordinator() -> Coordinator { Coordinator(self) }

  func makeNSView(context: Context) -> NSScrollView {
    let scroll = MessageScrollView()
    scroll.drawsBackground = false
    scroll.hasVerticalScroller = true
    scroll.scrollerStyle = .overlay
    scroll.autohidesScrollers = true
    scroll.borderType = .noBorder
    let editor = MessageTextView()
    editor.isRichText = false
    editor.isEditable = true
    editor.isSelectable = true
    editor.drawsBackground = false
    let font = NSFont.systemFont(ofSize: fontSize)
    editor.font = font
    editor.typingAttributes[.font] = font
    editor.textContainerInset = NSSize(width: 0, height: ComposerControlMetrics.textVerticalInset)
    editor.textContainer?.lineFragmentPadding = 0
    editor.textContainer?.widthTracksTextView = true
    editor.isHorizontallyResizable = false
    editor.isVerticallyResizable = true
    editor.minSize = .zero
    editor.maxSize = NSSize(
      width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
    editor.autoresizingMask = [.width]
    editor.delegate = context.coordinator
    editor.setAccessibilityLabel(placeholder)
    editor.onFocusChange = { [weak coordinator = context.coordinator] in
      coordinator?.noteFocusChange($0)
    }
    scroll.onHeightChange = { context.coordinator.updateHeight($0) }
    scroll.documentView = editor
    return scroll
  }

  func updateNSView(_ scroll: NSScrollView, context: Context) {
    context.coordinator.parent = self
    (scroll as? MessageScrollView)?.onHeightChange = { context.coordinator.updateHeight($0) }
    guard let editor = scroll.documentView as? MessageTextView else { return }
    let font = NSFont.systemFont(ofSize: fontSize)
    if editor.font != font {
      let selection = editor.selectedRange()
      editor.font = font
      editor.setSelectedRange(selection)
    }
    if (editor.typingAttributes[.font] as? NSFont) != font {
      editor.typingAttributes[.font] = font
    }
    editor.setAccessibilityLabel(placeholder)
    editor.textColor = textColor
    editor.insertionPointColor = textColor
    editor.sending = sending
    editor.submit = { context.coordinator.submit($0) }
    editor.onEditorActive = { context.coordinator.noteEditorActive($0) }
    editor.onTypingActivity = { context.coordinator.noteTyping() }
    editor.onFocusChange = { [weak coordinator = context.coordinator] in
      coordinator?.noteFocusChange($0)
    }
    editor.pastedText = {
      try ConversationClipboardAttachment.text(
        from: clipboard, saveImage: saveImage, saveFile: saveFile)
    }
    editor.onPasteError = onPasteError
    editor.hasAttachments = hasAttachments
    editor.onAttach = onAttach
    if editor.string != text {
      context.coordinator.applyingExternalText = true
      editor.string = text
      editor.setSelectedRange(NSRange(location: text.utf16.count, length: 0))
      editor.revealCaretAfterLayout()
      context.coordinator.applyingExternalText = false
    }
    scroll.needsLayout = true
  }

  @MainActor
  final class Coordinator: NSObject, NSTextViewDelegate {
    var parent: ConversationMessageField
    var applyingExternalText = false
    private var pendingHeight: CGFloat?
    private var heightUpdateScheduled = false
    init(_ parent: ConversationMessageField) { self.parent = parent }

    func updateHeight(_ height: CGFloat) {
      pendingHeight = height
      guard !heightUpdateScheduled, parent.height != height else { return }
      heightUpdateScheduled = true
      // AppKit reports the height at its real viewport width. Publish after
      // this layout pass, rather than changing SwiftUI state during layout.
      DispatchQueue.main.async { [weak self] in
        guard let self else { return }
        heightUpdateScheduled = false
        if let pendingHeight, parent.height != pendingHeight {
          parent.height = pendingHeight
        }
      }
    }

    func textDidChange(_ notification: Notification) {
      guard !applyingExternalText else { return }
      if let editor = notification.object as? MessageTextView {
        parent.text = editor.string
        editor.revealCaretAfterLayout()
      }
      parent.onTypingActivity()
    }

    func textViewDidChangeSelection(_ notification: Notification) {
      (notification.object as? MessageTextView)?.revealCaretAfterLayout()
    }

    func noteFocusChange(_ active: Bool) {
      parent.onFocusChange(active)
    }

    func noteEditorActive(_ active: Bool) {
      parent.onEditorActive(active)
    }

    func noteTyping() {
      parent.onTypingActivity()
    }

    func submit(_ text: String) {
      parent.text = text
      parent.onSubmit()
    }
  }

  private final class MessageScrollView: NSScrollView {
    var onHeightChange: (CGFloat) -> Void = { _ in }
    var caretRevealPending = false
    private var previousViewport = NSSize.zero

    override func layout() {
      super.layout()
      guard let editor = documentView as? NSTextView,
        let container = editor.textContainer, let textLayout = editor.layoutManager,
        contentSize.width > 0
      else { return }
      let viewport = contentSize
      if previousViewport != viewport, window?.firstResponder === editor {
        caretRevealPending = true
      }
      previousViewport = viewport
      if editor.frame.width != viewport.width {
        editor.setFrameSize(NSSize(width: viewport.width, height: editor.frame.height))
      }
      container.containerSize = NSSize(width: viewport.width, height: .greatestFiniteMagnitude)
      textLayout.ensureLayout(for: container)
      let textHeight = max(
        textLayout.usedRect(for: container).maxY,
        textLayout.extraLineFragmentRect.maxY)
      let documentHeight = ceil(textHeight + editor.textContainerInset.height * 2)
      let documentSize = NSSize(width: viewport.width, height: max(viewport.height, documentHeight))
      editor.minSize = NSSize(width: 0, height: viewport.height)
      if editor.frame.size != documentSize { editor.setFrameSize(documentSize) }
      onHeightChange(min(112, max(28, documentHeight)))
      if documentHeight <= viewport.height {
        contentView.scroll(to: .zero)
        reflectScrolledClipView(contentView)
      } else if caretRevealPending {
        editor.scrollRangeToVisible(editor.selectedRange())
      }
      caretRevealPending = false
    }
  }

  private final class MessageTextView: NSTextView {
    var sending = false
    var submit: ((String) -> Void)?
    var onEditorActive: (Bool) -> Void = { _ in }
    var onTypingActivity: () -> Void = {}
    var pastedText: (() throws -> String?)?
    var onPasteError: (String?) -> Void = { _ in }
    /// Read at Return: a paste can add a pill before SwiftUI updates the view.
    var hasAttachments: () -> Bool = { false }
    var onAttach: ([String]) -> Void = { _ in }
    var onFocusChange: ((Bool) -> Void)?
    private var isFirstResponderActive = false
    private var isWindowKey = false
    private var windowObservers: [NSObjectProtocol] = []

    func revealCaretAfterLayout() {
      guard let scroll = enclosingScrollView as? MessageScrollView else { return }
      scroll.caretRevealPending = true
      scroll.needsLayout = true
    }

    override func viewDidMoveToWindow() {
      super.viewDidMoveToWindow()
      rebuildWindowObservers()
      notifyFocusChange()
    }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
      super.viewWillMove(toWindow: newWindow)
      if newWindow == nil {
        isFirstResponderActive = false
        isWindowKey = false
        clearWindowObservers()
        onFocusChange?(false)
      }
    }

    isolated deinit {
      clearWindowObservers()
    }

    private func clearWindowObservers() {
      for observer in windowObservers {
        NotificationCenter.default.removeObserver(observer)
      }
      windowObservers.removeAll()
    }

    private func rebuildWindowObservers() {
      clearWindowObservers()
      guard let window else {
        isWindowKey = false
        return
      }
      isWindowKey = window.isKeyWindow
      let center = NotificationCenter.default
      let becomeKey = center.addObserver(
        forName: NSWindow.didBecomeKeyNotification,
        object: window,
        queue: .main
      ) { [weak self] _ in
        MainActor.assumeIsolated {
          guard let self else { return }
          self.isWindowKey = true
          self.notifyFocusChange()
        }
      }
      let resignKey = center.addObserver(
        forName: NSWindow.didResignKeyNotification,
        object: window,
        queue: .main
      ) { [weak self] _ in
        MainActor.assumeIsolated {
          guard let self else { return }
          self.isWindowKey = false
          self.notifyFocusChange()
        }
      }
      let willClose = center.addObserver(
        forName: NSWindow.willCloseNotification,
        object: window,
        queue: .main
      ) { [weak self] _ in
        MainActor.assumeIsolated {
          guard let self else { return }
          self.isWindowKey = false
          self.notifyFocusChange()
        }
      }
      windowObservers = [becomeKey, resignKey, willClose]
    }

    func notifyFocusChange() {
      onFocusChange?(isFocusedInKeyWindow)
    }

    var isFocusedInKeyWindow: Bool {
      guard isWindowKey, window != nil else { return false }
      return isFirstResponderActive
    }

    override func paste(_ sender: Any?) {
      do {
        if let text = try pastedText?() {
          // Store pointers become pills; only the prose enters the editor.
          let parts = ConversationDraft.split(text)
          if !parts.attachments.isEmpty { onAttach(parts.attachments) }
          let prose =
            parts.attachments.isEmpty ? text : parts.text.trimmingCharacters(in: .newlines)
          if !prose.isEmpty || !parts.attachments.isEmpty {
            insertText(prose, replacementRange: selectedRange())
          }
          onPasteError(nil)
          onTypingActivity()
          return
        }
        onPasteError(nil)
        super.paste(sender)
      } catch {
        onPasteError(
          String(localized: "Could not paste this attachment. Use files up to 50 MB and try again.")
        )
      }
    }

    /// A plain-text editor's standard Paste validation rejects image-only
    /// boards before our attachment-aware paste method can receive the action.
    override func validateUserInterfaceItem(_ item: NSValidatedUserInterfaceItem) -> Bool {
      if item.action == #selector(NSText.paste(_:)) {
        return isEditable && pastedText != nil
      }
      return super.validateUserInterfaceItem(item)
    }

    private func performComposerPaste(with event: NSEvent) -> Bool {
      guard event.type == .keyDown, window?.firstResponder === self,
        event.modifierFlags.intersection([.command, .control, .option, .shift]) == .command,
        event.charactersIgnoringModifiers?.lowercased() == "v"
      else { return false }
      paste(nil)
      return true
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
      if performComposerPaste(with: event) { return true }
      return super.performKeyEquivalent(with: event)
    }

    override func becomeFirstResponder() -> Bool {
      let accepted = super.becomeFirstResponder()
      if accepted {
        isFirstResponderActive = true
        onEditorActive(true)
        notifyFocusChange()
      }
      return accepted
    }

    override func resignFirstResponder() -> Bool {
      let resigned = super.resignFirstResponder()
      if resigned {
        isFirstResponderActive = false
        onEditorActive(false)
        notifyFocusChange()
      }
      return resigned
    }

    override func keyDown(with event: NSEvent) {
      if performComposerPaste(with: event) { return }
      let alternate = event.modifierFlags.intersection([.shift, .option, .control, .command])
      if [36, 76].contains(event.keyCode), alternate.isEmpty, !hasMarkedText() {
        if !sending
          && (hasAttachments() || !string.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        {
          submit?(string)
        }
        return
      }
      super.keyDown(with: event)
      if event.modifierFlags.intersection([.command, .control]).isEmpty {
        onTypingActivity()
      }
    }

    override func mouseDown(with event: NSEvent) {
      (window as? FloatingOverlayPanel)?.takeKeyForTranscript()
      onEditorActive(true)
      super.mouseDown(with: event)
      // Resigning the old transcript closes its gate during mouseDown. Only
      // an accepted user click reclaims it; programmatic preselection stays passive.
      if window?.firstResponder === self {
        (window as? FloatingOverlayPanel)?.takeKeyForTranscript()
      }
      notifyFocusChange()
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override var needsPanelToBecomeKey: Bool { true }
  }
}

/// An actual image payload wins over its optional filename/URL text representation.
enum ConversationClipboardImage {
  @MainActor
  static func png(from pasteboard: NSPasteboard) throws -> Data? {
    guard pasteboard.availableType(from: [.fileURL]) == nil,
      let type = pasteboard.availableType(from: [.png, .tiff])
    else { return nil }
    guard let bytes = pasteboard.data(forType: type),
      let bitmap = NSBitmapImageRep(data: bytes),
      let png = bitmap.representation(using: .png, properties: [:])
    else { throw CocoaError(.fileReadCorruptFile) }
    return png
  }
}
