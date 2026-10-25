import AppKit
import SwiftUI

/// One input surface; the conversation owner retains the draft and publishes it.
struct OverlayConversationComposer: View {
  @Environment(\.csTextScale) private var textScale
  let palette: OverlayAppearancePalette
  @Binding var draft: String
  var maximumEditorHeight: CGFloat? = nil
  let sending: Bool
  let onSubmit: () -> Void
  var onEditorActive: (Bool) -> Void = { _ in }
  var onTypingActivity: () -> Void = {}
  var onCancel: (() -> Void)? = nil
  var focusRevision: UInt64 = 0
  /// Out of sight under the agents list or a folded bar.
  var isCovered = false
  var onFocusChange: ((Bool) -> Void)? = nil
  var placeholder = String(localized: "Message the Agent")
  var clipboard: NSPasteboard = .general
  var saveImage: (Data) throws -> URL = { data in
    URL(fileURLWithPath: try savePastedImage(data: data))
  }
  var saveFile: (Data, String) throws -> URL = { data, name in
    URL(fileURLWithPath: try savePastedAttachment(data: data, name: name))
  }
  var replySelection: OverlayUserReplyReference? = nil
  var onClearReply: () -> Void = {}
  var draftMicrophoneRecording = false
  var draftMicrophoneBusy = false
  /// The header's exact-owner live take projection; this changes only the glyph.
  var liveSpeechActive = false
  var onToggleDraftMicrophone: (() -> Void)? = nil
  var onStageCaptureAttachments: (([String]) -> Bool)? = nil
  var pendingCaptureAttachments: [String] = []
  var onRemoveCaptureAttachment: ((String) -> Void)? = nil
  var captureAttachmentError: String? = nil
  var onRetryCaptureAttachments: (() -> Void)? = nil
  var onNavigateReply: (() -> Void)? = nil
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
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
    if onStageCaptureAttachments?(paths) == true { return }
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
      if let replySelection {
        HStack(alignment: .top, spacing: 8) {
          Button {
            onNavigateReply?()
          } label: {
            VStack(alignment: .leading, spacing: 2) {
              Text(verbatim: replySelection.author).fontWeight(.semibold)
              Text(verbatim: replySelection.text).lineLimit(2)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
          }
          .buttonStyle(.plain)
          .disabled(onNavigateReply == nil)
          Button("Clear reply", systemImage: "xmark", action: onClearReply)
            .labelStyle(.iconOnly).buttonStyle(.plain)
            .accessibilityIdentifier("overlay-conversation-clear-reply")
        }
        .font(.system(size: 11 * textScale))
        .foregroundStyle(palette.mutedText.color)
        .accessibilityIdentifier("overlay-conversation-reply-context")
      }
      if !pendingCaptureAttachments.isEmpty {
        Text("Attachments for active recording")
          .font(.system(size: 10 * textScale)).foregroundStyle(palette.mutedText.color)
        WrapLayout(spacing: 6) {
          ForEach(pendingCaptureAttachments, id: \.self) { path in
            OverlayComposerAttachmentPill(path: path, palette: palette) {
              onRemoveCaptureAttachment?(path)
            }
          }
        }
        .accessibilityIdentifier("overlay-conversation-capture-attachments")
        if let captureAttachmentError {
          HStack {
            Text(verbatim: captureAttachmentError)
              .foregroundStyle(palette.errorStatus.color)
            if let onRetryCaptureAttachments {
              Button("Retry attachments", action: onRetryCaptureAttachments)
                .buttonStyle(.plain)
            }
          }
          .font(.system(size: 10 * textScale))
        } else {
          Text("Attaching to this recording…")
            .font(.system(size: 10 * textScale))
            .foregroundStyle(palette.mutedText.color)
        }
      }
      let attached = attachments
      if !attached.isEmpty {
        ScrollView(.horizontal, showsIndicators: false) {
          HStack(spacing: 6) {
            ForEach(attached, id: \.self) { path in
              OverlayComposerAttachmentPill(path: path, palette: palette) { detach(path) }
            }
          }
          // The chips keep their own sideways scrolling; the pin only makes
          // sure a scroller the system forces back is the overlay one.
          .csAutohidingScrollers(.includesHorizontal)
        }
        .frame(maxHeight: 30)
        .accessibilityIdentifier("overlay-conversation-attachments")
        // Maciej, 2026-10-17 01:15Z: the chips sit above the editor whose
        // tracking set the I-beam. The row claims the arrow; the editable
        // field keeps its text cursor, since editable text never inherits
        // menu ownership.
        .background { OverlayMenuCursorRegion() }
      }
      editorRow
    }
    .padding(.horizontal, ComposerControlMetrics.horizontalPadding)
    .padding(.vertical, ComposerControlMetrics.verticalPadding)
  }

  var draftMicrophoneSymbol: String {
    liveSpeechActive
      ? OverlayAgentAudioControls.liveSpeechSymbol
      : draftMicrophoneRecording ? "mic.fill" : "mic"
  }

  var draftMicrophoneAccessibilityLabel: String {
    if liveSpeechActive { return String(localized: "Live speech active") }
    return draftMicrophoneRecording
      ? String(localized: "Stop dictation") : String(localized: "Dictate message")
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
        onCancel: onCancel, focusRevision: focusRevision, isCovered: isCovered,
        onFocusChange: { onFocusChange?($0) },
        clipboard: clipboard, saveImage: saveImage, saveFile: saveFile,
        onPasteError: { pasteError = $0 },
        hasAttachments: { !attachments.isEmpty }, onAttach: attach
      )
      .frame(height: min(editorHeight, maximumEditorHeight ?? editorHeight))
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
      if let onToggleDraftMicrophone {
        // The same live take addressed to this conversation lights the
        // header and gives this draft control its live-speech glyph.
        Button(action: onToggleDraftMicrophone) {
          Image(systemName: draftMicrophoneSymbol)
            .symbolEffect(
              .pulse, options: .repeating,
              isActive: draftMicrophoneRecording && !reduceMotion)
        }
        .font(.system(size: ComposerControlMetrics.glyphSize, weight: .semibold))
        .frame(
          width: ComposerControlMetrics.hitTargetSize, height: ComposerControlMetrics.hitTargetSize
        )
        .contentShape(Circle()).buttonStyle(.plain)
        // A pastel at rest (Maciej, 2026-10-23: "daj bardziej pastelowy");
        // hover and recording bring the whole violet back.
        .conversationActionHover(
          restingColor: palette.agentAccent.color.opacity(draftMicrophoneRecording ? 1 : 0.5),
          hoverColor: palette.agentAccent.color
        )
        .disabled(draftMicrophoneBusy)
        .accessibilityLabel(Text(verbatim: draftMicrophoneAccessibilityLabel))
        .accessibilityIdentifier("overlay-conversation-draft-microphone")
      }
      Button("Send", systemImage: "paperplane.fill", action: onSubmit)
        .labelStyle(.iconOnly)
        .font(.system(size: ComposerControlMetrics.glyphSize, weight: .semibold))
        .frame(
          width: ComposerControlMetrics.hitTargetSize, height: ComposerControlMetrics.hitTargetSize
        )
        .contentShape(Circle())
        .buttonStyle(.plain)
        .conversationActionHover(
          restingColor: palette.listeningStatus.color.opacity(0.8),
          hoverColor: palette.listeningStatus.color
        )
        .disabled(sending || draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        .accessibilityIdentifier("overlay-conversation-send")
    }
  }
}

/// The asset store mints its own file names: a pasted image lands as
/// `inline_<sha256>.png` and a pasted file as `clipboard_<sha256>_<name>`.
/// Neither is a label — the Founder's chip read `inline_5af8…f0108a4f.png`
/// (2026-10-18). A pasted picture is named for what it is, and a file the user
/// chose gives back the name they chose; anything else keeps its own name.
enum ConversationAttachmentLabel {
  private static let inlineImagePrefix = "inline_"
  private static let clipboardPrefix = "clipboard_"
  private static let hashLength = 64

  /// `bytes` is the pointer's own size, or 0 when the caller has none yet.
  static func display(fileName: String, bytes: Int = 0) -> String {
    if fileName.hasPrefix(inlineImagePrefix) {
      let image = String(
        localized: "Image", comment: "Attachment chip label for an image pasted into a message")
      guard bytes > 0 else { return image }
      let size = ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
      return "\(image) · \(size)"
    }
    return chosenName(fileName) ?? fileName
  }

  /// Peels the content hash the clipboard store prefixes to a chosen name.
  private static func chosenName(_ fileName: String) -> String? {
    guard fileName.hasPrefix(clipboardPrefix) else { return nil }
    let stored = fileName.dropFirst(clipboardPrefix.count)
    let hash = stored.prefix(hashLength)
    guard hash.count == hashLength, hash.allSatisfy(\.isHexDigit),
      stored.dropFirst(hashLength).first == "_"
    else { return nil }
    let chosen = stored.dropFirst(hashLength + 1)
    return chosen.isEmpty ? nil : String(chosen)
  }
}

/// A pasted attachment in the composer: the bubble's pill plus a remove cross.
/// The pill body is the same preview button as a bubble's `AttachmentChip`;
/// the cross stays a sibling control, so removing never opens the preview.
struct OverlayComposerAttachmentPill: View {
  let path: String
  let palette: OverlayAppearancePalette
  let onRemove: () -> Void
  @State private var thumbnail: NSImage?
  @State private var bytes = 0
  @State private var showPreview = false

  private var name: String {
    ConversationAttachmentLabel.display(
      fileName: URL(fileURLWithPath: path).lastPathComponent, bytes: bytes)
  }

  private var attachment: MessageAttachment {
    MessageAttachment(name: name, url: URL(fileURLWithPath: path))
  }

  var body: some View {
    HStack(spacing: 6) {
      Button {
        showPreview = true
      } label: {
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
          // A label, not code: the system voice, per the mono policy.
          Text(verbatim: name)
            .font(CSFont.ui(10.5, .medium))
            .foregroundStyle(palette.primaryText.color)
            .lineLimit(1)
            .truncationMode(.middle)
            .frame(maxWidth: 160)
        }
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .help("Preview attachment")
      .accessibilityIdentifier("overlay-conversation-attachment-preview")
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
    .accessibilityElement(children: .contain)
    .accessibilityLabel(Text(verbatim: name))
    .task(id: path) {
      let url = URL(fileURLWithPath: path)
      thumbnail = NSImage(contentsOf: url)
      bytes = (try? url.resourceValues(forKeys: [.fileSizeKey]))?.fileSize ?? 0
    }
    .sheet(isPresented: $showPreview) {
      AttachmentPreviewSheet(attachment: attachment) {
        showPreview = false
        onRemove()
      }
    }
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
  var onCancel: (() -> Void)? = nil
  var focusRevision: UInt64 = 0
  var isCovered = false
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
    // This editor mutates TextKit geometry during main-thread layout. Give it
    // explicit backing layers so an ancestor's flattened layer cannot draw it
    // on a Core Animation worker while layout holds the view-hierarchy lock.
    // The reverse order (TextKit draw -> insertion-indicator reparent) deadlocks.
    scroll.wantsLayer = true
    scroll.canDrawConcurrently = false
    scroll.layer?.drawsAsynchronously = false
    scroll.contentView.wantsLayer = true
    scroll.contentView.canDrawConcurrently = false
    scroll.contentView.layer?.drawsAsynchronously = false
    scroll.drawsBackground = false
    scroll.hasVerticalScroller = true
    // The scroller policy lives in `CSScrollerPin` (Founder, 2026-10-18); the
    // coordinator keeps it pinned across "Show scroll bars" changes.
    context.coordinator.scrollerPin.attach(to: scroll)
    scroll.borderType = .noBorder
    let editor = MessageTextView()
    editor.wantsLayer = true
    editor.canDrawConcurrently = false
    editor.layer?.drawsAsynchronously = false
    editor.isRichText = false
    editor.isEditable = true
    editor.isSelectable = true
    // Without this, Cmd+Z has nothing recorded to undo in the overlay's
    // composer (the chat window's ComposerTextView already allows undo).
    editor.allowsUndo = true
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

  static func dismantleNSView(_ scroll: NSScrollView, coordinator: Coordinator) {
    coordinator.scrollerPin.detach()
    coordinator.undoHistory.removeAllActions()
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
    editor.onCancel = onCancel
    if editor.string != text {
      context.coordinator.applyingExternalText = true
      editor.string = text
      editor.setSelectedRange(NSRange(location: text.utf16.count, length: 0))
      editor.revealCaretAfterLayout()
      context.coordinator.applyingExternalText = false
    }
    if context.coordinator.focusRevision != focusRevision {
      context.coordinator.focusRevision = focusRevision
      DispatchQueue.main.async { [weak editor] in
        guard let editor else { return }
        if let panel = editor.window as? FloatingOverlayPanel {
          panel.takeKeyForTranscript()
        } else if let panel = editor.window as? OverlayIntercomPanel {
          guard panel.isVisible, panel.canBecomeKey else { return }
          panel.makeKeyAndOrderFront(nil)
        }
        editor.window?.makeFirstResponder(editor)
      }
    }
    // Typing under the agents list went into this field, out of sight. Covered,
    // it gives the keyboard up; uncovered, it takes it back only from a panel
    // that still holds the keyboard, so nothing is taken from another app.
    if context.coordinator.isCovered != isCovered {
      context.coordinator.isCovered = isCovered
      if isCovered {
        if editor.window?.firstResponder === editor { editor.window?.makeFirstResponder(nil) }
      } else {
        DispatchQueue.main.async { [weak editor, weak coordinator = context.coordinator] in
          guard let editor, let window = editor.window, window.isKeyWindow,
            coordinator?.isCovered == false
          else { return }
          window.makeFirstResponder(editor)
        }
      }
    }
    scroll.needsLayout = true
  }

  @MainActor
  final class Coordinator: NSObject, NSTextViewDelegate {
    var parent: ConversationMessageField
    var applyingExternalText = false
    var focusRevision: UInt64 = 0
    var isCovered = false
    let scrollerPin = CSScrollerPin()
    /// The overlay panel outlives every composer it shows, so typing undo must
    /// not land on the panel's undo manager (see `ComposerTextView.Coordinator`).
    let undoHistory = UndoManager()
    private var pendingHeight: CGFloat?
    private var heightUpdateScheduled = false
    init(_ parent: ConversationMessageField) { self.parent = parent }

    func undoManager(for view: NSTextView) -> UndoManager? { undoHistory }

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
    var onCancel: (() -> Void)?
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

    /// Every standard Cmd edit shortcut lives here: a nonactivating
    /// borderless panel has no menu-bar routing, so only shortcuts handled
    /// in the view work at all. Paste alone was handled, and Maciej found
    /// the gap by hand (P1!, 2026-10-22): Cmd+V worked while Cmd+C, Cmd+X,
    /// Cmd+A, Cmd+Z and Cmd+Shift+Z did nothing.
    private func performComposerEditCommand(with event: NSEvent) -> Bool {
      guard event.type == .keyDown, window?.firstResponder === self,
        let key = event.charactersIgnoringModifiers?.lowercased()
      else { return false }
      let modifiers = event.modifierFlags.intersection([.command, .control, .option, .shift])
      switch (modifiers, key) {
      case (.command, "v"):
        paste(nil)
      case (.command, "c"):
        copy(nil)
      case (.command, "x"):
        cut(nil)
        onTypingActivity()
      case (.command, "a"):
        selectAll(nil)
      case (.command, "z"):
        undoManager?.undo()
        onTypingActivity()
      case ([.command, .shift], "z"):
        undoManager?.redo()
        onTypingActivity()
      default:
        return false
      }
      return true
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
      if performComposerEditCommand(with: event) { return true }
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
      if performComposerEditCommand(with: event) { return }
      let alternate = event.modifierFlags.intersection([.shift, .option, .control, .command])
      if event.keyCode == 53, alternate.isEmpty, !hasMarkedText(), let onCancel {
        onCancel()
        return
      }
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
