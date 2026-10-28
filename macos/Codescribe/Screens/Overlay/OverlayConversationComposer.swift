import AppKit
import QuartzCore
import SwiftUI

/// One input surface; the conversation owner retains the draft and publishes it.
struct OverlayConversationComposer: View {
  /// Restart admission needs the current keyboard recipient, not the editor
  /// ownership retained across window blur for automatic-collapse protection.
  @MainActor
  static func hasFocusedEditor(in window: NSWindow? = NSApp?.keyWindow) -> Bool {
    ConversationMessageField.hasFocusedEditor(in: window)
  }

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
  var draftSyncChoices: [OverlayComposerSyncChoice] = []
  var draftSyncNames: [String] = []
  var draftSyncArmed: Bool = false
  var draftSyncEnabled: Bool = false
  var onActivateDraftSync: (() -> Void)? = nil
  var onProseEdit: ((OverlayComposerProseEdit) -> Void)? = nil
  var onToggleDraftSync: ((String) -> Void)? = nil
  var onStopDraftSync: (() -> Void)? = nil
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
  @State private var pasteError: String?
  /// A refused long Fn points here: hint line plus a lit microphone.
  @State private var microphoneHint = ComposerMicrophoneHintState()
  @State private var accessoryHeight: CGFloat = 30
  @State private var editorHeight: CGFloat = 28

  /// The height budget caps the editor viewport, never the draft's line count.
  private var isCompact: Bool { maximumEditorHeight != nil }
  private var showsDraftSync: Bool { !draftSyncChoices.isEmpty || draftSyncActive }
  private func cornerLayout(width: CGFloat = .greatestFiniteMagnitude)
    -> ConversationComposerCornerLayout
  {
    ConversationComposerCornerLayout(
      width: width, fontSize: ComposerControlMetrics.textSize * textScale,
      compact: isCompact, showsSync: showsDraftSync,
      showsMicrophone: onToggleDraftMicrophone != nil)
  }
  private var controlGlyphSize: CGFloat {
    isCompact ? ComposerControlMetrics.intercomGlyphSize : ComposerControlMetrics.glyphSize
  }
  private var restingActionColor: Color {
    palette.mutedText.color.opacity(ComposerControlMetrics.restingActionOpacity)
  }
  private var editorViewportHeight: CGFloat {
    max(
      cornerLayout().actionHeight,
      ComposerTextLayout.resolve(
        contentHeight: editorHeight,
        lineHeight: ComposerTextLayout.lineHeight(
          fontSize: ComposerControlMetrics.textSize * textScale),
        maximumHeight: maximumEditorHeight
          ?? ComposerControlMetrics.conversationEditorMaximumHeight * textScale
      ).height)
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 4) {
      composerContent
        .background {
          if reduceTransparency {
            RoundedRectangle(cornerRadius: CSRadius.composer, style: .continuous)
              .fill(palette.desktopBackground.color)
          }
        }
        .modifier(
          OverlayControlGlass(cornerRadius: CSRadius.composer, isEnabled: !reduceTransparency))
      if microphoneHint.visible {
        Text(verbatim: ComposerCaretOwnership.microphoneHint)
          .font(.system(size: 10 * textScale, weight: .medium))
          .foregroundStyle(palette.agentAccent.color)
          .transition(.opacity)
          .accessibilityIdentifier("overlay-conversation-microphone-hint")
      } else if let pasteError, !isCompact {
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

  private func pickAttachments() {
    let picker = NSOpenPanel()
    picker.canChooseFiles = true
    picker.canChooseDirectories = false
    picker.allowsMultipleSelection = true
    picker.prompt = String(localized: "Attach")
    picker.begin { response in
      guard response == .OK else { return }
      let urls = picker.urls
      Task { @MainActor in
        // Use the very same bounded file admission and asset store as native
        // paste. The private board never changes the user's clipboard.
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        do {
          guard board.writeObjects(urls as [NSURL]) else {
            throw CocoaError(.fileReadCorruptFile)
          }
          if let pointers = try ConversationClipboardAttachment.text(
            from: board, saveImage: saveImage, saveFile: saveFile)
          {
            attach(ConversationDraft.split(pointers).attachments)
          }
          pasteError = nil
        } catch {
          pasteError = String(
            localized: "Could not paste this attachment. Use files up to 50 MB and try again.")
        }
      }
    }
  }

  private var composerContent: some View {
    VStack(alignment: .leading, spacing: isCompact ? ComposerControlMetrics.intercomSpacing : 6) {
      if replySelection != nil || !pendingCaptureAttachments.isEmpty || !attachments.isEmpty
        || pasteError != nil
      {
        if isCompact {
          ScrollView(.vertical, showsIndicators: false) {
            accessoryContent
              .fixedSize(horizontal: false, vertical: true)
              .onGeometryChange(for: CGFloat.self) {
                $0.size.height
              } action: { height in
                if abs(accessoryHeight - height) > 0.5 { accessoryHeight = height }
              }
          }
          .frame(
            height: min(
              accessoryHeight, ComposerControlMetrics.intercomSupplementaryHeight(scale: textScale))
          )
          .clipped()
          .layoutPriority(1)
          .csAutohidingScrollers()
        } else {
          accessoryContent
            .fixedSize(horizontal: false, vertical: true)
        }
      }
      editorRow
    }
    .padding(
      .horizontal,
      isCompact
        ? ComposerControlMetrics.intercomHorizontalPadding
        : ComposerControlMetrics.horizontalPadding
    )
    .padding(.vertical, cornerLayout().verticalPadding)
  }

  private var accessoryContent: some View {
    VStack(alignment: .leading, spacing: isCompact ? ComposerControlMetrics.intercomSpacing : 6) {
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
        // A horizontal ScrollView has no intrinsic row height. Reserve the
        // pill's actual space so compression cannot put it over the first line.
        .frame(height: 30)
        .accessibilityIdentifier("overlay-conversation-attachments")
        // Maciej, 2026-10-17 01:15Z: the chips sit above the editor whose
        // tracking set the I-beam. The row claims the arrow; the editable
        // field keeps its text cursor, since editable text never inherits
        // menu ownership.
        .background { OverlayMenuCursorRegion() }
      }
      if let pasteError, isCompact {
        Text(verbatim: pasteError)
          .font(.system(size: 10 * textScale))
          .foregroundStyle(palette.errorStatus.color)
          .accessibilityIdentifier("overlay-conversation-paste-error")
      }
    }
  }

  var draftMicrophoneSymbol: String {
    liveSpeechActive
      ? OverlayAgentAudioControls.liveSpeechSymbol
      : draftMicrophoneRecording ? "mic.fill" : "mic"
  }

  /// One "+" opens the composer actions; the former paperclip and link
  /// buttons live inside it.
  private func composerPlusMenu(_ corners: ConversationComposerCornerLayout) -> some View {
    Menu {
      Button("Attach files…", systemImage: "paperclip", action: pickAttachments)
        .disabled(sending)
        .accessibilityIdentifier("overlay-conversation-attach")
      if showsDraftSync {
        Divider()
        Button("Sync draft", systemImage: "link") { onActivateDraftSync?() }
          .disabled(sending || !draftSyncEnabled || onActivateDraftSync == nil)
          .accessibilityIdentifier("overlay-conversation-draft-sync")
        ForEach(draftSyncChoices) { choice in
          Toggle(
            isOn: Binding(
              get: { choice.selected },
              set: { _ in onToggleDraftSync?(choice.id) })
          ) {
            Text(verbatim: choice.title)
          }
          .disabled(sending || !choice.enabled)
          .help(
            choice.enabled
              ? String(localized: "Share future typing and pasted attachments with this composer")
              : String(localized: "This composer has a pending delivery or recording attachments."))
        }
        if draftSyncNames.count > 1 {
          Button("Stop syncing") { onStopDraftSync?() }
            .disabled(sending)
        }
      }
    } label: {
      Image(systemName: "plus")
        .font(.system(size: min(controlGlyphSize, corners.glyphSize), weight: .semibold))
        .frame(width: corners.actionWidth, height: corners.actionHeight)
        .contentShape(Rectangle())
    }
    .menuStyle(.button)
    .buttonStyle(.plain)
    .menuIndicator(.hidden)
    .frame(width: corners.actionWidth, height: corners.actionHeight)
    .conversationActionHover(
      restingColor: draftSyncActive ? palette.agentAccent.color : restingActionColor,
      hoverColor: draftSyncActive ? palette.agentAccent.color : palette.primaryText.color
    )
    .disabled(sending)
    .overlayFastHint(composerPlusHint, palette: palette)
    .accessibilityLabel(Text("Composer actions"))
    .accessibilityValue(Text(verbatim: draftSyncAccessibilityValue))
    .accessibilityIdentifier("overlay-conversation-plus")
  }

  private var draftSyncActive: Bool { draftSyncArmed || draftSyncNames.count > 1 }

  private var draftSyncAccessibilityValue: String {
    let status =
      draftSyncArmed
      ? String(localized: "Sync armed")
      : draftSyncNames.count > 1 ? String(localized: "Sync linked") : String(localized: "Sync off")
    guard !draftSyncNames.isEmpty else { return status }
    return status + ": " + draftSyncNames.joined(separator: " · ")
  }

  /// The hint names the menu, not one of its entries; sync state rides along
  /// so an armed or linked composer explains its tinted plus.
  private var composerPlusHint: String {
    let base = String(localized: "Composer actions")
    if draftSyncArmed {
      return base + " · " + String(localized: "Sync armed")
    } else if draftSyncNames.count > 1 {
      return base + " · " + String(localized: "Sync linked")
    }
    return base
  }

  var draftMicrophoneAccessibilityLabel: String {
    if liveSpeechActive { return String(localized: "Live speech active") }
    return draftMicrophoneRecording
      ? String(localized: "Stop dictation") : String(localized: "Dictate message")
  }

  private var editorRow: some View {
    GeometryReader { geometry in
      let corners = cornerLayout(width: geometry.size.width)
      ConversationMessageField(
        text: prose, height: $editorHeight,
        textColor: palette.primaryText.nsColor,
        fontSize: ComposerControlMetrics.textSize * textScale,
        maximumEditorHeight: maximumEditorHeight,
        corners: corners,
        sending: sending, placeholder: placeholder, onSubmit: onSubmit,
        onEditorActive: onEditorActive,
        onTypingActivity: onTypingActivity,
        onCancel: onCancel, focusRevision: focusRevision, isCovered: isCovered,
        onFocusChange: { onFocusChange?($0) },
        clipboard: clipboard, saveImage: saveImage, saveFile: saveFile,
        onPasteError: { pasteError = $0 },
        hasAttachments: { !attachments.isEmpty }, onAttach: attach,
        onProseEdit: onProseEdit,
        onMicrophoneRedirect: onToggleDraftMicrophone == nil
          ? nil : { microphoneHint.show(reduceMotion: reduceMotion) }
      )
      .frame(maxWidth: .infinity)
      .frame(height: geometry.size.height)
      .transaction { $0.animation = nil }
      .accessibilityIdentifier("overlay-conversation-composer")
      .overlay(alignment: .topLeading) {
        if prose.wrappedValue.isEmpty {
          Text(verbatim: placeholder)
            .font(.system(size: ComposerControlMetrics.textSize * textScale))
            .lineLimit(1)
            .foregroundStyle(palette.mutedText.color)
            .padding(.top, ComposerControlMetrics.textVerticalInset)
            .padding(.leading, corners.placeholderInset(viewportHeight: geometry.size.height))
            .padding(.trailing, corners.trailingWidth + corners.clearance)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
        }
      }
      .overlay(alignment: .bottomLeading) {
        composerPlusMenu(corners)
      }
      .overlay(alignment: .bottomTrailing) {
        HStack(spacing: corners.spacing) {
          if let onToggleDraftMicrophone {
            // The same live take addressed to this conversation lights the
            // header and gives this draft control its live-speech glyph.
            Button(action: onToggleDraftMicrophone) {
              Image(systemName: draftMicrophoneSymbol)
                .symbolEffect(
                  .pulse, options: .repeating,
                  isActive: draftMicrophoneRecording && !reduceMotion)
            }
            .font(.system(size: min(controlGlyphSize, corners.glyphSize), weight: .semibold))
            .frame(width: corners.actionWidth, height: corners.actionHeight)
            .contentShape(Circle()).buttonStyle(.plain)
            .conversationActionHover(
              restingColor: draftMicrophoneRecording || liveSpeechActive || microphoneHint.visible
                ? palette.agentAccent.color : restingActionColor,
              hoverColor: draftMicrophoneRecording || liveSpeechActive || microphoneHint.visible
                ? palette.agentAccent.color : palette.primaryText.color
            )
            .scaleEffect(microphoneHint.visible && !reduceMotion ? 1.18 : 1)
            .disabled(draftMicrophoneBusy)
            .overlayFastHint(draftMicrophoneAccessibilityLabel, palette: palette)
            .accessibilityLabel(Text(verbatim: draftMicrophoneAccessibilityLabel))
            .accessibilityIdentifier("overlay-conversation-draft-microphone")
          }
          Button("Send", systemImage: "paperplane.fill", action: onSubmit)
            .labelStyle(.iconOnly)
            .font(.system(size: min(controlGlyphSize, corners.glyphSize), weight: .semibold))
            .frame(width: corners.actionWidth, height: corners.actionHeight)
            .contentShape(Circle())
            .buttonStyle(.plain)
            .conversationActionHover(
              restingColor: restingActionColor,
              hoverColor: palette.primaryText.color
            )
            .disabled(sending || draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            .accessibilityIdentifier("overlay-conversation-send")
            .overlayFastHint(
              draftSyncNames.count > 1
                ? String(localized: "Send to all linked agents") : String(localized: "Send"),
              palette: palette)
        }
      }
    }
    .frame(height: editorViewportHeight)
  }
}

/// Shared coordinates for the visible controls and TextKit's exclusion paths.
/// Only the bottom corners are reserved; the middle remains native editable text.
private struct ConversationComposerCornerLayout: Equatable {
  let lineHeight: CGFloat
  let actionWidth: CGFloat
  let actionHeight: CGFloat
  let spacing: CGFloat
  let clearance: CGFloat
  let verticalPadding: CGFloat
  let leadingWidth: CGFloat
  let trailingWidth: CGFloat

  init(width: CGFloat, fontSize: CGFloat, compact: Bool, showsSync: Bool, showsMicrophone: Bool) {
    lineHeight = ComposerTextLayout.lineHeight(fontSize: fontSize)
    actionHeight = min(ComposerControlMetrics.hitTargetSize, 2 * lineHeight)
    let padding =
      compact
      ? ComposerControlMetrics.intercomVerticalPadding : ComposerControlMetrics.verticalPadding
    // Send plus BOTH outer margins fits inside two actual scaled text rows.
    verticalPadding = min(padding, max(0, (2 * lineHeight - actionHeight) / 2))
    // The "+" menu folds attach and sync into one control; sync no longer
    // widens the leading corner.
    let leadingCount: CGFloat = 1
    _ = showsSync
    let trailingCount: CGFloat = showsMicrophone ? 2 : 1
    let count = leadingCount + trailingCount
    clearance = min(4, max(0, width / 24))
    spacing = min(
      compact ? ComposerControlMetrics.intercomSpacing : ComposerControlMetrics.spacing,
      max(0, width / 32))
    let proseWidth = min(3 * fontSize, max(0, width / 3))
    actionWidth = min(
      ComposerControlMetrics.hitTargetSize,
      max(1, (width - proseWidth - 2 * clearance - (count - 2) * spacing) / count))
    leadingWidth = leadingCount * actionWidth + (leadingCount - 1) * spacing
    trailingWidth = trailingCount * actionWidth + (trailingCount - 1) * spacing
  }

  var glyphSize: CGFloat { min(actionWidth, actionHeight) * 0.65 }

  func placeholderInset(viewportHeight: CGFloat) -> CGFloat {
    viewportHeight - actionHeight < ComposerControlMetrics.textVerticalInset + lineHeight
      ? leadingWidth + clearance : 0
  }

  func exclusions(viewport: NSRect, textOrigin: NSPoint) -> [NSRect] {
    let y = viewport.maxY - actionHeight - textOrigin.y
    return [
      NSRect(x: -textOrigin.x, y: y, width: leadingWidth + clearance, height: actionHeight),
      NSRect(
        x: viewport.width - trailingWidth - clearance - textOrigin.x, y: y,
        width: trailingWidth + clearance, height: actionHeight),
    ]
  }

  /// Once a draft transfers into the multiline body, the control row is
  /// released for the rest of that draft: the WHOLE bottom row is excluded,
  /// so shrinking text never drops back between the controls.
  func releasedRowExclusions(viewport: NSRect, textOrigin: NSPoint) -> [NSRect] {
    let y = viewport.maxY - actionHeight - textOrigin.y
    return [NSRect(x: -textOrigin.x, y: y, width: viewport.width, height: actionHeight)]
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
              .clipShape(RoundedRectangle(cornerRadius: CSRadius.chip, style: .continuous))
          } else {
            Image(systemName: "doc")
              .font(.system(size: 10, weight: .medium))
              .foregroundStyle(palette.mutedText.color)
          }
          // A label, not code: the system voice, per the mono policy.
          Text(verbatim: name)
            .font(CSFont.ui(11, .medium))
            .foregroundStyle(palette.primaryText.color)
            .lineLimit(1)
            .truncationMode(.middle)
            .frame(maxWidth: 160)
        }
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .overlayFastHint(String(localized: "Preview attachment"), palette: palette)
      .accessibilityIdentifier("overlay-conversation-attachment-preview")
      Button(action: onRemove) {
        Image(systemName: "xmark")
          .font(.system(size: 8, weight: .bold))
          .foregroundStyle(palette.mutedText.color)
          .frame(width: 14, height: 14)
          .contentShape(Circle())
      }
      .buttonStyle(.plain)
      .overlayFastHint(String(localized: "Remove attachment"), palette: palette)
      .accessibilityLabel("Remove attachment")
      .accessibilityIdentifier("overlay-conversation-attachment-remove")
    }
    .padding(.horizontal, 9)
    .padding(.vertical, 5)
    .background(palette.primaryText.color.opacity(0.05))
    .overlay(
      Capsule(style: .continuous)
        .strokeBorder(palette.primaryText.color.opacity(0.10), lineWidth: 1)
    )
    .clipShape(Capsule(style: .continuous))
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
  @MainActor
  static func hasFocusedEditor(in window: NSWindow?) -> Bool {
    guard let window, window.isKeyWindow, window.isVisible,
      let editor = window.firstResponder as? MessageTextView
    else { return false }
    return editor.window === window && !editor.isHiddenOrHasHiddenAncestor
  }

  @Binding var text: String
  @Binding var height: CGFloat
  let textColor: NSColor
  let fontSize: CGFloat
  var maximumEditorHeight: CGFloat? = nil
  let corners: ConversationComposerCornerLayout
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
  var onProseEdit: ((OverlayComposerProseEdit) -> Void)? = nil
  /// Present when this field has its own microphone (see ComposerCaretOwnership).
  var onMicrophoneRedirect: (() -> Void)? = nil

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
    scroll.maximumEditorHeight =
      maximumEditorHeight
      ?? ComposerControlMetrics.conversationEditorMaximumHeight * fontSize
      / ComposerControlMetrics.textSize
    scroll.corners = corners
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
    editor.textContainer?.widthTracksTextView = false
    editor.isHorizontallyResizable = false
    // MessageScrollView owns the measured document frame, including the fixed
    // space that lets the last line scroll clear of the corner controls.
    editor.isVerticallyResizable = false
    editor.minSize = .zero
    editor.maxSize = NSSize(
      width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
    editor.autoresizingMask = [.width]
    editor.delegate = context.coordinator
    editor.setAccessibilityLabel(placeholder)
    editor.onFocusChange = { [weak coordinator = context.coordinator] in
      coordinator?.noteFocusChange($0)
    }
    editor.onMicrophoneRedirect = onMicrophoneRedirect
    scroll.onHeightChange = { context.coordinator.updateHeight($0) }
    scroll.documentView = editor
    return scroll
  }

  static func dismantleNSView(_ scroll: NSScrollView, coordinator: Coordinator) {
    coordinator.applyingExternalText = true
    coordinator.pendingProseEdit = nil
    (scroll.documentView as? NSTextView)?.delegate = nil
    coordinator.scrollerPin.detach()
    coordinator.undoHistory.removeAllActions()
  }

  func updateNSView(_ scroll: NSScrollView, context: Context) {
    context.coordinator.parent = self
    (scroll as? MessageScrollView)?.maximumEditorHeight =
      maximumEditorHeight
      ?? ComposerControlMetrics.conversationEditorMaximumHeight * fontSize
      / ComposerControlMetrics.textSize
    (scroll as? MessageScrollView)?.onHeightChange = { context.coordinator.updateHeight($0) }
    (scroll as? MessageScrollView)?.corners = corners
    guard let editor = scroll.documentView as? MessageTextView else { return }
    let font = NSFont.systemFont(ofSize: fontSize)
    if editor.font != font {
      let selection = editor.selectedRange()
      editor.font = font
      editor.setSelectedRange(selection)
      (scroll as? MessageScrollView)?.invalidateContentMeasurement()
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
    let hadMicrophone = editor.onMicrophoneRedirect != nil
    editor.onMicrophoneRedirect = onMicrophoneRedirect
    if hadMicrophone != (onMicrophoneRedirect != nil) { editor.notifyFocusChange() }
    editor.pastedText = {
      try ConversationClipboardAttachment.text(
        from: clipboard, saveImage: saveImage, saveFile: saveFile)
    }
    editor.onPasteError = onPasteError
    editor.hasAttachments = hasAttachments
    editor.onAttach = onAttach
    editor.onCancel = onCancel
    if !editor.string.utf16.elementsEqual(text.utf16) {
      context.coordinator.applyingExternalText = true
      context.coordinator.pendingProseEdit = nil
      // Another linked editor replaced the document. Local undo operations
      // describe its previous contents and must not overwrite that newer edit.
      context.coordinator.undoHistory.removeAllActions()
      editor.string = text
      editor.setSelectedRange(NSRange(location: text.utf16.count, length: 0))
      (scroll as? MessageScrollView)?.invalidateContentMeasurement()
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
    /// One native proposal, consumed by the corresponding textDidChange only.
    /// This is provenance, not another draft or an inferred text difference.
    var pendingProseEdit: OverlayComposerProseEdit?
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

    func textView(
      _ textView: NSTextView, shouldChangeTextIn affectedCharRange: NSRange,
      replacementString: String?
    ) -> Bool {
      recordProseEdit(
        in: textView, ranges: [affectedCharRange],
        replacementStrings: replacementString.map { [$0] })
      return true
    }

    func textView(
      _ textView: NSTextView, shouldChangeTextInRanges affectedRanges: [NSValue],
      replacementStrings: [String]?
    ) -> Bool {
      recordProseEdit(
        in: textView, ranges: affectedRanges.map(\.rangeValue),
        replacementStrings: replacementStrings)
      return true
    }

    private func recordProseEdit(
      in editor: NSTextView, ranges: [NSRange], replacementStrings: [String]?
    ) {
      pendingProseEdit = nil
      // A nil replacement is an attribute-only proposal. Unknown edits still
      // reach the canonical setter; they never acquire guessed shared ranges.
      guard !applyingExternalText, parent.onProseEdit != nil,
        let replacementStrings, !ranges.isEmpty,
        ranges.count == replacementStrings.count
      else { return }
      let before = editor.string
      guard before.utf16.elementsEqual(parent.text.utf16) else { return }
      let length = before.utf16.count
      var previousEnd: Int?
      for range in ranges {
        guard range.location != NSNotFound, range.location >= 0, range.length >= 0,
          range.location <= length, range.length <= length - range.location,
          Range(range, in: before) != nil
        else { return }
        if let previousEnd, range.location <= previousEnd { return }
        if ranges.count > 1, range.length == 0 { return }
        previousEnd = range.location + range.length
      }
      let replacements = zip(ranges, replacementStrings).map {
        OverlayComposerTextReplacement(range: $0.0, replacement: $0.1)
      }
      let proposed = NSMutableString(string: before)
      for replacement in replacements.reversed() {
        proposed.replaceCharacters(in: replacement.range, with: replacement.replacement)
      }
      pendingProseEdit = OverlayComposerProseEdit(
        before: before, after: proposed as String, replacements: replacements)
    }

    func textDidChange(_ notification: Notification) {
      let edit = pendingProseEdit
      pendingProseEdit = nil
      guard !applyingExternalText else { return }
      if let editor = notification.object as? MessageTextView {
        (editor.enclosingScrollView as? MessageScrollView)?.invalidateContentMeasurement()
        if let edit, let onProseEdit = parent.onProseEdit,
          edit.before.utf16.elementsEqual(parent.text.utf16),
          edit.after.utf16.elementsEqual(editor.string.utf16)
        {
          onProseEdit(edit)
        } else {
          // Preserve native input even if AppKit supplied no usable proposal
          // (including undo/IME variants); the model resets its sharing epoch.
          parent.text = editor.string
        }
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
      guard !applyingExternalText else { return }
      pendingProseEdit = nil
      parent.text = text
      parent.onSubmit()
    }
  }

  private final class MessageScrollView: NSScrollView {
    var onHeightChange: (CGFloat) -> Void = { _ in }
    var maximumEditorHeight = ComposerControlMetrics.conversationEditorMaximumHeight {
      didSet {
        if oldValue != maximumEditorHeight { invalidateContentMeasurement() }
      }
    }
    var corners: ConversationComposerCornerLayout? {
      didSet {
        if oldValue != corners { invalidateContentMeasurement() }
      }
    }
    var caretRevealPending = false
    private var previousViewport = NSSize.zero
    private var observedOrigin = NSPoint.zero
    private var exclusionRects: [NSRect] = []
    private var needsContentMeasurement = true
    private var usesViewportCorners = false
    /// One-way per-draft latch: trips when the draft first needs a second
    /// row, resets only when the draft empties (send/cancel).
    private var rowReleasedForDraft = false
    private let cornerClip = CAShapeLayer()
    private var arranging = false

    func invalidateContentMeasurement() {
      needsContentMeasurement = true
      needsLayout = true
    }

    override func reflectScrolledClipView(_ clipView: NSClipView) {
      super.reflectScrolledClipView(clipView)
      guard clipView === contentView else { return }
      if clipView.bounds.origin != observedOrigin {
        // A later scroll wins over an older selection/edit reveal. Geometry
        // changes inside layout consume their reveal once, without rearming it.
        if !arranging { caretRevealPending = false }
        observedOrigin = clipView.bounds.origin
      }
      // Scrolling changes only the viewport clip. It never invalidates TextKit,
      // measures the document, resizes it, or requests another layout pass.
      updateCornerClip()
    }

    private func updateCornerClip() {
      guard let layer = contentView.layer else { return }
      CATransaction.begin()
      CATransaction.setDisableActions(true)
      defer { CATransaction.commit() }
      guard usesViewportCorners, let corners else {
        layer.mask = nil
        return
      }
      let viewport = contentView.bounds
      let upper = NSRect(
        x: viewport.minX, y: viewport.minY, width: viewport.width,
        height: max(0, viewport.height - corners.actionHeight))
      let path = CGMutablePath()
      // AppKit owns the backing layer's coordinate system (including flipping).
      path.addRect(contentView.convertToLayer(upper))
      if !rowReleasedForDraft {
        // A released row shows no text between the controls, so the writable
        // middle gap exists only before the latch trips.
        let middle = NSRect(
          x: viewport.minX + corners.leadingWidth + corners.clearance, y: upper.maxY,
          width: max(
            0,
            viewport.width - corners.leadingWidth - corners.trailingWidth - 2 * corners.clearance),
          height: corners.actionHeight)
        path.addRect(contentView.convertToLayer(middle))
      }
      cornerClip.frame = layer.bounds
      cornerClip.bounds = layer.bounds
      cornerClip.path = path
      layer.mask = cornerClip
    }

    func prepareForCaretReveal() {
      caretRevealPending = false
      if !arranging { layoutSubtreeIfNeeded() }
    }

    private func caretRect(_ range: NSRange, in editor: NSTextView) -> NSRect? {
      guard let window = editor.window, range.location != NSNotFound else { return nil }
      let index = min(range.location, editor.textStorage?.length ?? 0)
      let screenRect = editor.firstRect(
        forCharacterRange: NSRange(location: index, length: 0), actualRange: nil)
      // An insertion rectangle may have zero width and still have a visible
      // line height. Do not discard it as an empty NSRect.
      guard screenRect.height > 0, screenRect.origin.x.isFinite, screenRect.origin.y.isFinite,
        screenRect.height.isFinite
      else { return nil }
      return editor.convert(window.convertFromScreen(screenRect), from: nil)
    }

    func revealUnobscuredRange(_ range: NSRange, in editor: NSTextView) {
      guard usesViewportCorners, let corners, range.location != NSNotFound else { return }
      let viewport = contentView.bounds
      let length = editor.textStorage?.length ?? 0
      let start = min(range.location, length)
      let end = start + min(range.length, length - start)
      let blockedRects = effectiveExclusions(corners, viewport: viewport, textOrigin: .zero)
      // Native scrolling chooses the visible end of a selection. Correct only
      // an endpoint actually under a control, never drag an offscreen endpoint
      // back into view or replace the native selected range.
      for index in [end, start] {
        guard let rect = caretRect(NSRange(location: index, length: 0), in: editor),
          blockedRects.contains(where: { $0.intersects(rect.insetBy(dx: -1, dy: 0)) })
        else { continue }
        var target = viewport
        target.origin.y = rect.maxY - (viewport.height - corners.actionHeight)
        contentView.scroll(to: contentView.constrainBoundsRect(target).origin)
        reflectScrolledClipView(contentView)
        break
      }
    }

    private func effectiveExclusions(
      _ corners: ConversationComposerCornerLayout, viewport: NSRect, textOrigin: NSPoint
    ) -> [NSRect] {
      rowReleasedForDraft
        ? corners.releasedRowExclusions(viewport: viewport, textOrigin: textOrigin)
        : corners.exclusions(viewport: viewport, textOrigin: textOrigin)
    }

    private func reserveCorners(
      _ corners: ConversationComposerCornerLayout, viewport: NSRect, editor: NSTextView,
      container: NSTextContainer
    ) {
      let rects = effectiveExclusions(
        corners, viewport: viewport, textOrigin: editor.textContainerOrigin)
      guard rects != exclusionRects else { return }
      exclusionRects = rects
      container.exclusionPaths = rects.map { NSBezierPath(rect: $0) }
      editor.needsDisplay = true
    }

    override func layout() {
      guard !arranging else { return }
      arranging = true
      defer { arranging = false }
      super.layout()
      guard let corners, let editor = documentView as? MessageTextView,
        let container = editor.textContainer, let textLayout = editor.layoutManager,
        contentSize.width > 0
      else { return }
      let viewport = contentSize
      if previousViewport != viewport {
        // Preserve a visible caret on resize, but never jump from a manually
        // scrolled passage to an offscreen selection.
        let oldViewport = NSRect(origin: observedOrigin, size: previousViewport)
        if window?.firstResponder === editor,
          let rect = caretRect(editor.selectedRange(), in: editor), oldViewport.intersects(rect)
        {
          caretRevealPending = true
        }
        needsContentMeasurement = true
      }
      previousViewport = viewport
      func documentHeight() -> CGFloat {
        textLayout.ensureLayout(for: container)
        return ceil(
          max(textLayout.usedRect(for: container).maxY, textLayout.extraLineFragmentRect.maxY)
            + editor.textContainerOrigin.y + editor.textContainerInset.height)
      }
      if needsContentMeasurement {
        // Full-width text is a lower bound for the fitting height. Start there
        // each time, then admit native line increments for the bottom corners.
        // Reusing yesterday's exclusions makes growth/shrink oscillate; starting
        // at one row would relayout a long document at every candidate height.
        let minimum = max(
          corners.actionHeight,
          ComposerTextLayout.resolve(contentHeight: 0, lineHeight: corners.lineHeight).height)
        let limit = max(
          minimum,
          ComposerTextLayout.resolve(
            contentHeight: .greatestFiniteMagnitude, lineHeight: corners.lineHeight,
            maximumHeight: maximumEditorHeight
          ).height)
        editor.leadingTextInset = 0
        exclusionRects = []
        container.exclusionPaths = []
        container.containerSize = NSSize(
          width: max(1, viewport.width), height: .greatestFiniteMagnitude)
        if (editor.textStorage?.length ?? 0) == 0 { rowReleasedForDraft = false }
        var measuredHeight = documentHeight()
        if measuredHeight > minimum { rowReleasedForDraft = true }
        var candidate = min(limit, max(minimum, measuredHeight))
        if measuredHeight <= limit {
          // A fitting draft has stationary native corner wrapping. No scroll
          // origin participates in these document coordinates.
          var latchRetried = false
          while true {
            reserveCorners(
              corners,
              viewport: NSRect(
                origin: .zero, size: NSSize(width: viewport.width, height: candidate)),
              editor: editor, container: container)
            measuredHeight = documentHeight()
            if !rowReleasedForDraft, measuredHeight > minimum, !latchRetried {
              // First transfer into the multiline body releases the whole
              // control row for the rest of this draft; remeasure once with
              // the full-row exclusion so text leaves the row immediately.
              rowReleasedForDraft = true
              latchRetried = true
              exclusionRects = []
              continue
            }
            if measuredHeight <= candidate || candidate >= limit { break }
            candidate = min(limit, candidate + corners.lineHeight)
          }
        }
        usesViewportCorners = measuredHeight > candidate
        if usesViewportCorners {
          exclusionRects = []
          container.exclusionPaths = []
          if viewport.height - corners.actionHeight
            < corners.lineHeight + 2 * editor.textContainerInset.height
          {
            // In a one-row viewport the controls occupy every visible line.
            // Lay out in its writable middle until resizing admits a full-width
            // row above them; clipping must never make edge characters unreachable.
            editor.leadingTextInset = corners.leadingWidth + corners.clearance
            container.containerSize = NSSize(
              width: max(
                1,
                viewport.width - corners.leadingWidth - corners.trailingWidth
                  - 2 * corners.clearance), height: .greatestFiniteMagnitude)
            usesViewportCorners = false
          }
          measuredHeight = documentHeight()
        }
        // Overflow has stable full-width layout plus one fixed corner-height
        // tail. Every character can scroll above the cutouts. This extent is
        // derived only from text/size/configuration, never the scroll origin or
        // yesterday's frame, so roundtrips cannot accumulate a blank tail.
        let documentSize = NSSize(
          width: viewport.width,
          height: max(
            viewport.height, measuredHeight + (usesViewportCorners ? corners.actionHeight : 0)))
        editor.minSize = NSSize(width: 0, height: viewport.height)
        if editor.frame.size != documentSize { editor.setFrameSize(documentSize) }
        needsContentMeasurement = false
        onHeightChange(candidate)
      }
      updateCornerClip()
      let revealCaret = caretRevealPending
      caretRevealPending = false
      if editor.frame.height <= viewport.height {
        contentView.scroll(to: .zero)
        reflectScrolledClipView(contentView)
      } else if revealCaret, window?.firstResponder === editor {
        editor.scrollRangeToVisible(editor.selectedRange())
      }
    }
  }

  private final class MessageTextView: NSTextView, ComposerMicrophoneRedirecting {
    var onMicrophoneRedirect: (() -> Void)?
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
    var leadingTextInset: CGFloat = 0 {
      didSet {
        if oldValue != leadingTextInset { invalidateTextContainerOrigin() }
      }
    }

    override var textContainerOrigin: NSPoint {
      let origin = super.textContainerOrigin
      return NSPoint(x: leadingTextInset > 0 ? leadingTextInset : origin.x, y: origin.y)
    }

    override func scrollRangeToVisible(_ range: NSRange) {
      let scroll = enclosingScrollView as? MessageScrollView
      scroll?.prepareForCaretReveal()
      super.scrollRangeToVisible(range)
      scroll?.revealUnobscuredRange(range, in: self)
    }

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
        ComposerCaretOwnership.shared.report(self, focused: false)
      }
    }

    isolated deinit {
      clearWindowObservers()
      ComposerCaretOwnership.shared.release(ObjectIdentifier(self))
    }

    func showMicrophoneRedirect() { onMicrophoneRedirect?() }

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
      ComposerCaretOwnership.shared.report(
        self, focused: isFocusedInKeyWindow && onMicrophoneRedirect != nil)
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
