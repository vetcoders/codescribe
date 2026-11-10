#!/usr/bin/env python3
"""Source contract for image normalization and file drops in the overlay composer."""

from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parents[2]
ATTACHMENT = ROOT / "macos/Codescribe/Screens/Overlay/ConversationClipboardAttachment.swift"
COMPOSER = ROOT / "macos/Codescribe/Screens/Overlay/OverlayConversationComposer.swift"


def main() -> int:
    attachment = ATTACHMENT.read_text()
    composer = COMPOSER.read_text()
    failures: list[str] = []

    for label, fragment in [
        ("TIFF file URLs must be identified by type", "UTType(filenameExtension: file.pathExtension)"),
        ("TIFF bytes must be decoded before the payload cap", "NSBitmapImageRep(data: data)"),
        ("TIFF bytes must be encoded as PNG", "representation(using: .png, properties: [:])"),
        ("normalized images must use the image asset store", "saveImage(normalized)"),
        ("normalized images must be checked against the 50 MB cap", "normalized.count <= maximumBytes - total"),
    ]:
        if fragment not in attachment:
            failures.append(label)

    if "registerForDraggedTypes([.string, .fileURL])" not in composer:
        failures.append("the overlay editor must register file URL drops while retaining text drops")
    for fragment, label in [
        ("override func draggingEntered(_ sender: NSDraggingInfo)", "the editor must accept file URL drags"),
        ("override func prepareForDragOperation(_ sender: NSDraggingInfo)", "the editor must own file URL drop preparation"),
        ("override func performDragOperation(_ sender: NSDraggingInfo)", "the editor must consume file URL drops"),
        ("droppedText?(sender.draggingPasteboard)", "drop handling must read the dragging pasteboard"),
        ("onAttach(parts.attachments)", "drop handling must route pointers to attachment pills"),
        ("insertText(prose, replacementRange: selectedRange())", "drop handling must insert prose without raw paths"),
    ]:
        if fragment not in composer:
            failures.append(label)

    perform_start = composer.find("override func performDragOperation(_ sender: NSDraggingInfo)")
    if perform_start >= 0:
        next_override = composer.find("\n    override func ", perform_start + 1)
        perform_body = composer[perform_start:next_override if next_override >= 0 else None]
        for fragment, label in [
            ("insertAttachmentAwareText(text)", "drop handling must pass attachment pointers into the pill pipeline"),
            ("droppedText?(sender.draggingPasteboard)", "drop handling must read the dragging pasteboard"),
            ("return true", "drop handling must consume file URLs rather than insert raw paths"),
        ]:
            if fragment not in perform_body:
                failures.append(label)
    else:
        failures.append("drop handling must pass attachment pointers into the pill pipeline")

    if "private func insertAttachmentAwareText(_ text: String)" not in composer:
        failures.append("paste and drop must share the attachment-pill insertion path")
    for fragment, label in [
        ("onAttach(parts.attachments)", "attachment pointers must route to pills"),
        ("insertText(prose, replacementRange: selectedRange())", "only prose may enter the editor"),
    ]:
        if fragment not in composer:
            failures.append(label)

    if failures:
        for failure in failures:
            print(f"FAIL: {failure}", file=sys.stderr)
        return 1
    print("PASS: TIFF normalization and file URL drop source contract")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
