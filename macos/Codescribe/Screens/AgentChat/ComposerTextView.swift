import AppKit
import SwiftUI

enum ComposerAccessibility {
    static let textViewIdentifier = "agent-composer-text"
    static let micIdentifier = "agent-composer-mic"
}

enum ComposerTextKeyDisposition: Equatable {
    case send
    case insertNewline
    case native

    /// Return and keypad-enter share the same contract. IME confirmation and
    /// modified shortcuts stay native so composing text is never sent early.
    static func resolve(
        keyCode: UInt16,
        modifiers: NSEvent.ModifierFlags,
        hasMarkedText: Bool
    ) -> ComposerTextKeyDisposition {
        guard keyCode == 36 || keyCode == 76, !hasMarkedText else { return .native }
        let flags = modifiers.intersection(.deviceIndependentFlagsMask)
        guard flags.isDisjoint(with: [.command, .control, .option]) else { return .native }
        return flags.contains(.shift) ? .insertNewline : .send
    }
}

struct ComposerTextLayout: Equatable {
    static let minimumLines = 1
    static let maximumLines = 8
    static let verticalPadding: CGFloat = 6

    let height: CGFloat
    let isVerticallyScrollable: Bool

    static func resolve(
        contentHeight: CGFloat,
        lineHeight: CGFloat,
        minimumLines: Int = ComposerTextLayout.minimumLines,
        maximumLines: Int = ComposerTextLayout.maximumLines,
        verticalPadding: CGFloat = ComposerTextLayout.verticalPadding
    ) -> ComposerTextLayout {
        let safeLineHeight = max(1, lineHeight)
        let safeMinimum = max(1, minimumLines)
        let safeMaximum = max(safeMinimum, maximumLines)
        let minimumHeight = safeLineHeight * CGFloat(safeMinimum) + verticalPadding
        let maximumHeight = safeLineHeight * CGFloat(safeMaximum) + verticalPadding
        let measured = max(contentHeight, minimumHeight)
        return ComposerTextLayout(
            height: min(measured, maximumHeight),
            isVerticallyScrollable: measured > maximumHeight
        )
    }

    static func minimumHeight(fontSize: CGFloat) -> CGFloat {
        resolve(contentHeight: 0, lineHeight: lineHeight(fontSize: fontSize)).height
    }

    static func lineHeight(fontSize: CGFloat) -> CGFloat {
        ceil(NSLayoutManager().defaultLineHeight(for: composerFont(size: fontSize)))
    }

    /// AppKit-backed measurement used by tests and by the representable's live
    /// text system. Newlines and visual wrapping both participate.
    static func contentHeight(text: String, width: CGFloat, fontSize: CGFloat) -> CGFloat {
        let storage = NSTextStorage(string: text.isEmpty ? " " : text)
        let layoutManager = NSLayoutManager()
        let container = NSTextContainer(
            containerSize: NSSize(
                width: max(1, width),
                height: CGFloat.greatestFiniteMagnitude
            )
        )
        container.lineFragmentPadding = 5
        container.widthTracksTextView = false
        storage.addAttribute(
            .font,
            value: composerFont(size: fontSize),
            range: NSRange(location: 0, length: storage.length)
        )
        layoutManager.addTextContainer(container)
        storage.addLayoutManager(layoutManager)
        layoutManager.ensureLayout(for: container)
        return ceil(layoutManager.usedRect(for: container).height + verticalPadding)
    }

    fileprivate static func composerFont(size: CGFloat) -> NSFont {
        NSFont(name: FontLoader.spaceGrotesk, size: size) ?? .systemFont(ofSize: size)
    }
}

/// Native multiline composer. NSTextView owns text input, selection, undo,
/// marked-text/IME behavior and vertical scrolling; SwiftUI owns only the
/// measured one-to-eight-line frame.
struct ComposerTextView: NSViewRepresentable {
    @Binding var text: String
    @Binding var height: CGFloat
    let textScale: CGFloat
    @Binding var isFocused: Bool
    let onSend: () -> Void

    func makeCoordinator() -> Coordinator { Coordinator(parent: self) }

    func makeNSView(context: Context) -> NSScrollView {
        let scrollView = ComposerScrollView()
        scrollView.borderType = .noBorder
        scrollView.drawsBackground = false
        scrollView.hasHorizontalScroller = false
        scrollView.hasVerticalScroller = false
        scrollView.autohidesScrollers = true
        scrollView.horizontalScrollElasticity = .none
        scrollView.verticalScrollElasticity = .automatic

        let textView = ComposerNativeTextView()
        textView.delegate = context.coordinator
        textView.isRichText = false
        textView.importsGraphics = false
        textView.allowsUndo = true
        textView.drawsBackground = false
        textView.isHorizontallyResizable = false
        textView.isVerticallyResizable = true
        textView.autoresizingMask = [.width]
        textView.textContainerInset = NSSize(width: 0, height: 3)
        textView.textContainer?.lineFragmentPadding = 5
        textView.textContainer?.widthTracksTextView = true
        textView.textContainer?.containerSize = NSSize(
            width: 0,
            height: CGFloat.greatestFiniteMagnitude
        )
        textView.string = text
        textView.placeholder = "Type a message…"
        textView.font = ComposerTextLayout.composerFont(size: 13.5 * textScale)
        textView.textColor = NSColor(srgbRed: 0xE9 / 255, green: 0xE7 / 255, blue: 0xE0 / 255, alpha: 1)
        textView.insertionPointColor = NSColor(srgbRed: 0xD9 / 255, green: 0x77 / 255, blue: 0x57 / 255, alpha: 1)
        textView.setAccessibilityIdentifier(ComposerAccessibility.textViewIdentifier)
        textView.setAccessibilityLabel("Message")
        textView.onKeyDown = { [weak coordinator = context.coordinator, weak textView] event in
            guard let coordinator, let textView else { return false }
            return coordinator.handleKeyDown(event, in: textView)
        }

        scrollView.documentView = textView
        scrollView.onContentWidthChange = { [weak coordinator = context.coordinator, weak textView, weak scrollView] in
            guard let coordinator, let textView, let scrollView else { return }
            coordinator.refreshLayout(textView, in: scrollView)
        }
        context.coordinator.refreshLayout(textView, in: scrollView)
        return scrollView
    }

    func updateNSView(_ scrollView: NSScrollView, context: Context) {
        context.coordinator.parent = self
        guard let textView = scrollView.documentView as? ComposerNativeTextView else { return }

        let font = ComposerTextLayout.composerFont(size: 13.5 * textScale)
        if textView.font != font {
            textView.font = font
            textView.typingAttributes[.font] = font
        }
        if textView.string != text {
            let selection = textView.selectedRange()
            textView.string = text
            let utf16Count = (text as NSString).length
            let location = min(selection.location, utf16Count)
            let length = min(selection.length, utf16Count - location)
            textView.setSelectedRange(NSRange(location: location, length: length))
        }
        textView.needsDisplay = true
        context.coordinator.refreshLayout(textView, in: scrollView)

        DispatchQueue.main.async { [weak textView] in
            guard let textView, let window = textView.window else { return }
            if isFocused, window.firstResponder !== textView {
                window.makeFirstResponder(textView)
            } else if !isFocused, window.firstResponder === textView {
                window.makeFirstResponder(nil)
            }
        }
    }

    @MainActor
    final class Coordinator: NSObject, NSTextViewDelegate {
        var parent: ComposerTextView

        init(parent: ComposerTextView) {
            self.parent = parent
        }

        func textDidChange(_ notification: Notification) {
            guard let textView = notification.object as? ComposerNativeTextView,
                  let scrollView = textView.enclosingScrollView else { return }
            if parent.text != textView.string { parent.text = textView.string }
            textView.needsDisplay = true
            refreshLayout(textView, in: scrollView)
        }

        func textDidBeginEditing(_ notification: Notification) {
            parent.isFocused = true
        }

        func textDidEndEditing(_ notification: Notification) {
            parent.isFocused = false
        }

        fileprivate func handleKeyDown(
            _ event: NSEvent,
            in textView: ComposerNativeTextView
        ) -> Bool {
            switch ComposerTextKeyDisposition.resolve(
                keyCode: event.keyCode,
                modifiers: event.modifierFlags,
                hasMarkedText: textView.hasMarkedText()
            ) {
            case .send:
                parent.onSend()
                return true
            case .insertNewline:
                textView.insertNewline(nil)
                return true
            case .native:
                return false
            }
        }

        func refreshLayout(_ textView: NSTextView, in scrollView: NSScrollView) {
            guard scrollView.contentSize.width > 0,
                  let layoutManager = textView.layoutManager,
                  let textContainer = textView.textContainer,
                  let font = textView.font else { return }
            layoutManager.ensureLayout(for: textContainer)
            let contentHeight = ceil(
                layoutManager.usedRect(for: textContainer).height
                    + textView.textContainerInset.height * 2
            )
            let lineHeight = ceil(layoutManager.defaultLineHeight(for: font))
            let layout = ComposerTextLayout.resolve(
                contentHeight: contentHeight,
                lineHeight: lineHeight
            )
            if scrollView.hasVerticalScroller != layout.isVerticallyScrollable {
                scrollView.hasVerticalScroller = layout.isVerticallyScrollable
            }
            if abs(parent.height - layout.height) > 0.5 {
                DispatchQueue.main.async { [weak self] in
                    guard let self, abs(self.parent.height - layout.height) > 0.5 else { return }
                    self.parent.height = layout.height
                }
            }
            textView.scrollRangeToVisible(textView.selectedRange())
        }
    }
}

private final class ComposerScrollView: NSScrollView {
    var onContentWidthChange: (() -> Void)?
    private var measuredContentWidth: CGFloat = -1

    override func layout() {
        super.layout()
        let width = contentSize.width
        guard abs(width - measuredContentWidth) > 0.5 else { return }
        measuredContentWidth = width
        onContentWidthChange?()
    }
}

private final class ComposerNativeTextView: NSTextView {
    var placeholder = ""
    var onKeyDown: ((NSEvent) -> Bool)?

    override func keyDown(with event: NSEvent) {
        if onKeyDown?(event) == true { return }
        super.keyDown(with: event)
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard string.isEmpty, !placeholder.isEmpty, let font else { return }
        let attributes: [NSAttributedString.Key: Any] = [
            .font: font,
            .foregroundColor: NSColor(
                srgbRed: 0x6F / 255,
                green: 0x72 / 255,
                blue: 0x68 / 255,
                alpha: 1
            ),
        ]
        let origin = NSPoint(
            x: textContainerInset.width + (textContainer?.lineFragmentPadding ?? 0),
            y: textContainerInset.height
        )
        placeholder.draw(at: origin, withAttributes: attributes)
    }
}
