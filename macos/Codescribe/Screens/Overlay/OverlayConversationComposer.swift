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
  var clipboard: NSPasteboard = .general
  var saveImage: (Data) throws -> URL = { data in
    URL(fileURLWithPath: try savePastedImage(data: data))
  }
  @State private var pasteError: String?

  var body: some View {
    VStack(alignment: .leading, spacing: 4) {
      composerContent.modifier(OverlayControlGlass())
      if let pasteError {
        Text(verbatim: pasteError)
          .font(.system(size: 10 * textScale))
          .foregroundStyle(palette.errorStatus.color)
          .accessibilityIdentifier("overlay-conversation-paste-error")
      }
    }
  }

  private var composerContent: some View {
    HStack(alignment: .bottom, spacing: 8) {
      ConversationMessageField(
        text: $draft, textColor: palette.primaryText.nsColor, fontSize: 14 * textScale,
        sending: sending, onSubmit: onSubmit, onEditorActive: onEditorActive,
        onTypingActivity: onTypingActivity,
        clipboard: clipboard, saveImage: saveImage,
        onPasteError: { pasteError = $0 }
      )
      .fixedSize(horizontal: false, vertical: true)
      .accessibilityIdentifier("overlay-conversation-composer")
      .overlay(alignment: .topLeading) {
        if draft.isEmpty {
          Text("Message the Agent")
            .font(.system(size: 14 * textScale))
            .foregroundStyle(palette.mutedText.color)
            .padding(.top, 5)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
        }
      }
      Button("Send", systemImage: "paperplane.fill", action: onSubmit)
        .labelStyle(.iconOnly)
        .font(.system(size: 16, weight: .semibold))
        .foregroundStyle(CSColor.terracotta)
        .frame(width: 28, height: 28)
        .contentShape(Circle())
        .buttonStyle(.plain)
        .disabled(sending || draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        .accessibilityIdentifier("overlay-conversation-send")
    }
    .padding(.horizontal, 12)
    .padding(.vertical, 8)
  }
}

/// SwiftUI owns the draft; AppKit owns text editing and the responder chain.
private struct ConversationMessageField: NSViewRepresentable {
  @Binding var text: String
  let textColor: NSColor
  let fontSize: CGFloat
  let sending: Bool
  let onSubmit: () -> Void
  var onEditorActive: (Bool) -> Void = { _ in }
  var onTypingActivity: () -> Void = {}
  let clipboard: NSPasteboard
  let saveImage: (Data) throws -> URL
  let onPasteError: (String?) -> Void

  func makeCoordinator() -> Coordinator { Coordinator(self) }

  func makeNSView(context: Context) -> NSScrollView {
    let scroll = NSScrollView()
    scroll.drawsBackground = false
    scroll.hasVerticalScroller = true
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
    editor.textContainerInset = NSSize(width: 0, height: 5)
    editor.textContainer?.lineFragmentPadding = 0
    editor.textContainer?.widthTracksTextView = true
    editor.isHorizontallyResizable = false
    editor.isVerticallyResizable = true
    editor.autoresizingMask = [.width]
    editor.delegate = context.coordinator
    editor.setAccessibilityLabel(String(localized: "Message the Agent"))
    scroll.documentView = editor
    return scroll
  }

  func updateNSView(_ scroll: NSScrollView, context: Context) {
    context.coordinator.parent = self
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
    editor.textColor = textColor
    editor.insertionPointColor = textColor
    editor.sending = sending
    editor.submit = { context.coordinator.submit($0) }
    editor.onEditorActive = { context.coordinator.noteEditorActive($0) }
    editor.onTypingActivity = { context.coordinator.noteTyping() }
    editor.pasteImage = {
      guard let data = try ConversationClipboardImage.png(from: clipboard) else { return nil }
      return try saveImage(data)
    }
    editor.onPasteError = onPasteError
    if editor.string != text {
      context.coordinator.applyingExternalText = true
      editor.string = text
      editor.setSelectedRange(NSRange(location: text.utf16.count, length: 0))
      context.coordinator.applyingExternalText = false
    }
  }

  func sizeThatFits(_ proposal: ProposedViewSize, nsView: NSScrollView, context: Context) -> CGSize?
  {
    let width = max(1, proposal.width ?? 300)
    guard let editor = nsView.documentView as? NSTextView,
      let container = editor.textContainer, let layout = editor.layoutManager
    else { return nil }
    container.containerSize = NSSize(width: width, height: .greatestFiniteMagnitude)
    layout.ensureLayout(for: container)
    let height = layout.usedRect(for: container).height + editor.textContainerInset.height * 2
    return CGSize(width: width, height: min(112, max(28, height)))
  }

  @MainActor
  final class Coordinator: NSObject, NSTextViewDelegate {
    var parent: ConversationMessageField
    var applyingExternalText = false
    init(_ parent: ConversationMessageField) { self.parent = parent }

    func textDidChange(_ notification: Notification) {
      guard !applyingExternalText else { return }
      if let editor = notification.object as? NSTextView { parent.text = editor.string }
      parent.onTypingActivity()
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

  private final class MessageTextView: NSTextView {
    var sending = false
    var submit: ((String) -> Void)?
    var onEditorActive: (Bool) -> Void = { _ in }
    var onTypingActivity: () -> Void = {}
    var pasteImage: (() throws -> URL?)?
    var onPasteError: (String?) -> Void = { _ in }

    override func paste(_ sender: Any?) {
      do {
        if let url = try pasteImage?() {
          insertText("\n\(url.path)\n", replacementRange: selectedRange())
          onPasteError(nil)
          onTypingActivity()
          return
        }
        onPasteError(nil)
        super.paste(sender)
      } catch {
        onPasteError(String(localized: "Could not save pasted image. Try pasting again."))
      }
    }

    override func becomeFirstResponder() -> Bool {
      let accepted = super.becomeFirstResponder()
      if accepted { onEditorActive(true) }
      return accepted
    }

    override func resignFirstResponder() -> Bool {
      let resigned = super.resignFirstResponder()
      if resigned { onEditorActive(false) }
      return resigned
    }

    override func keyDown(with event: NSEvent) {
      let alternate = event.modifierFlags.intersection([.shift, .option, .control, .command])
      if [36, 76].contains(event.keyCode), alternate.isEmpty, !hasMarkedText() {
        if !sending && !string.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
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
    }
  }
}

/// Only a bare clipboard image is converted; ordinary text keeps native paste behavior.
enum ConversationClipboardImage {
  @MainActor
  static func png(from pasteboard: NSPasteboard) throws -> Data? {
    guard pasteboard.availableType(from: [.fileURL, .string]) == nil,
      let type = pasteboard.availableType(from: [.png, .tiff])
    else { return nil }
    guard let bytes = pasteboard.data(forType: type),
      let bitmap = NSBitmapImageRep(data: bytes),
      let png = bitmap.representation(using: .png, properties: [:])
    else { throw CocoaError(.fileReadCorruptFile) }
    return png
  }
}
