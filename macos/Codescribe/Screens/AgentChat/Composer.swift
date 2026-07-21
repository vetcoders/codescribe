import AppKit
import OSLog
import SwiftUI
import UniformTypeIdentifiers

/// Diagnostic breadcrumbs for the attachment staging path. Filter with:
///   log show --predicate 'subsystem == "com.vetcoders.codescribe"' --info
private let attachLog = Logger(
    subsystem: Bundle.main.bundleIdentifier ?? "com.vetcoders.codescribe",
    category: "attachments"
)

/// Bottom composer: the 📎 attach button (image picker), staged-attachment chips,
/// the message field, the ripple mic (shares the dictation core later), and the
/// terracotta send ↑ button. Below: the affordance row mirroring the mock's
/// capability hints. Images stage through three converging paths — picker,
/// drag & drop, and ⌘V paste — all landing in `store.addAttachments`.
struct Composer: View {
    @ObservedObject var store: AgentChatStore
    @State private var fieldFocused = false
    /// Chat text scale (⌘+/-/0) — applied to the message field + placeholder so the
    /// composer input tracks the message bodies. Chrome (chips, affordance hints,
    /// icons) keeps its intrinsic size.
    @Environment(\.csTextScale) private var textScale
    @State private var fieldHeight = ComposerTextLayout.minimumHeight(fontSize: 13.5)

    // ⌘V interception. The native text editor consumes `paste:` before any
    // SwiftUI `.onPasteCommand` gets a look-in, so pasting an image needs a local
    // key monitor (same pattern as the ⌘+/-/0 monitor in App.swift). Scoped hard:
    // only fires when the composer field itself is focused in this view's own
    // window, so ⌘V in the thread-rail search, Settings, or any other field is
    // untouched.
    @State private var pasteMonitor: Any?
    /// Window hosting this composer — resolved via `hostWindowReader` so the
    /// monitor can ignore key events belonging to other windows.
    @State private var hostWindow: NSWindow?

    // Drag-over is tracked by two OR'd targets so it stays stable as the pointer
    // crosses from the composer padding onto the text field. The outer target
    // (whole strip) bootstraps the drag; the inner clear overlay (above the
    // text view) actually intercepts a drop that lands *on* the field, so the
    // field editor never eats it as pasted path text. See `dropCatcher`.
    @State private var overOuter = false
    @State private var overInner = false
    /// True while an image is being dragged anywhere over the composer.
    private var isDragging: Bool { overOuter || overInner }

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            if !store.pendingAttachments.isEmpty {
                attachmentChips
            }

            HStack(alignment: .bottom, spacing: 10) {
                // Attach images (NSOpenPanel → staged chips → vision FFI on send).
                Button(action: pickAttachments) {
                    CSIconView(
                        icon: .attach,
                        size: 15,
                        color: store.pendingAttachments.isEmpty ? CSColor.textFaint : CSColor.terracottaLight
                    )
                }
                .buttonStyle(.plain)
                .help("Attach an image (PNG, JPEG, GIF, WebP)")

                ComposerTextView(
                    text: $store.draft,
                    height: $fieldHeight,
                    textScale: textScale,
                    isFocused: $fieldFocused,
                    onSend: { store.send() }
                )
                .frame(height: fieldHeight)
                .accessibilityIdentifier(ComposerAccessibility.textViewIdentifier)

                micButton

                Button(action: performPrimaryAction) {
                    ZStack {
                        CSIconView(
                            icon: primaryAction.icon,
                            size: 15,
                            weight: primaryAction.iconWeight,
                            color: ChatPalette.sendGlyph
                        )
                        if primaryAction == .stopping {
                            ProgressView()
                                .controlSize(.small)
                                .scaleEffect(0.5)
                                .tint(ChatPalette.sendGlyph)
                        }
                    }
                        .frame(width: 32, height: 32)
                        .background(CSColor.terracotta)
                        .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
                }
                .buttonStyle(.plain)
                .disabled(!primaryAction.isEnabled)
                .opacity(primaryAction == .stopping ? 0.72 : 1)
                .help(primaryAction.accessibilityLabel)
                .accessibilityIdentifier(ComposerActionAccessibility.identifier)
                .accessibilityLabel(Text(primaryAction.accessibilityLabel))
            }
            .padding(.leading, 13)
            .padding(.trailing, 11)
            .padding(.vertical, 9)
            .background(CSColor.surfaceRaised(isDragging ? 0.07 : 0.04))
            .overlay(
                RoundedRectangle(cornerRadius: 13, style: .continuous)
                    .strokeBorder(
                        isDragging ? CSColor.terracotta : CSColor.hairline(0.09),
                        lineWidth: isDragging ? 1.5 : 1
                    )
            )
            .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
            // Sits above the NSTextField and swallows a drop that lands *on* the
            // field, so the field editor never pastes the path as text. Only
            // hit-testable mid-drag (isDragging) so typing/clicks pass through
            // the rest of the time.
            .overlay(dropCatcher)
            .animation(.easeOut(duration: 0.12), value: isDragging)

            dictationPreview
            dictationFeedback

            // Affordance row
            HStack(spacing: 16) {
                ForEach(affordances, id: \.self) { item in
                    Text(item)
                        .font(CSFont.mono(10, .medium))
                        .foregroundStyle(CSColor.textFaintAlt)
                }
            }
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 14)
        .overlay(alignment: .top) {
            Rectangle().fill(CSColor.hairline(0.06)).frame(height: 1)
        }
        // Drag an image from Finder onto the composer to stage it (same path as
        // the 📎 picker). The whole bottom strip is the outer drop area — it
        // bootstraps the drag-over state and catches drops that miss the field;
        // `dropCatcher` handles drops on the field itself.
        .onDrop(of: [.fileURL], isTargeted: $overOuter) { providers in
            handleDrop(providers)
        }
        .background(hostWindowReader)
        .onAppear(perform: installPasteMonitor)
        .onDisappear(perform: removePasteMonitor)
        .task(id: store.composerFocusRequest) {
            guard store.composerFocusRequest > 0 else { return }
            // The first summon can create the window and request focus in the
            // same run-loop turn. Yield once so the native field editor exists.
            await Task.yield()
            focusNativeComposer()
        }
    }

    private var primaryAction: ComposerActionVisualState {
        ComposerActionVisualState.resolve(
            canSend: store.canSend,
            activePhase: store.selectedComposerTurnPhase
        )
    }

    private func performPrimaryAction() {
        switch primaryAction {
        case .send:
            store.send()
        case .stop:
            store.stopActiveTurn()
        case .stopping:
            break
        }
    }

    /// W1-B moved the composer onto a native `NSTextView`. Keep the SwiftUI
    /// focus binding in sync, then explicitly make that view first responder so
    /// a newly-created Agent window and an already-visible window behave alike.
    @MainActor
    private func focusNativeComposer() {
        fieldFocused = true
        let window = hostWindow
        DispatchQueue.main.async {
            guard let textView = Self.nativeComposer(in: window?.contentView) else { return }
            window?.makeFirstResponder(textView)
        }
    }

    @MainActor
    private static func nativeComposer(in view: NSView?) -> NSTextView? {
        guard let view else { return nil }
        if let textView = view as? NSTextView,
           textView.accessibilityIdentifier() == "agent-composer-text" {
            return textView
        }
        for child in view.subviews {
            if let textView = nativeComposer(in: child) { return textView }
        }
        return nil
    }

    // MARK: Voice-note mic

    /// The composer mic: click to start a voice note, click again to stop and
    /// insert the transcript into the draft. The ripple lives only while
    /// recording; a spinner shows the preparing transition. Disabled (and dimmed)
    /// while a hotkey/overlay dictation session owns the microphone.
    private var micButton: some View {
        Button(action: { store.toggleDictation() }) {
            micVisual
                .frame(width: 22, height: 22)
                .contentShape(Rectangle())
                .opacity(micState == .blocked ? 0.35 : micState == .preparing ? 0.68 : 1)
        }
        .buttonStyle(.plain)
        .disabled(!micState.isEnabled)
        .help(micState.accessibilityLabel)
        .accessibilityIdentifier(ComposerAccessibility.micIdentifier)
        .accessibilityLabel(Text(micState.accessibilityLabel))
        .animation(.easeOut(duration: 0.15), value: store.dictationPhase)
    }

    private var micState: ComposerMicVisualState {
        if store.dictationBlocked { return .blocked }
        switch store.dictationPhase {
        case .preparing: return .preparing
        case .recording: return .recording
        case .idle, .failed: return .idle
        }
    }

    private var micVisual: some View {
        ZStack(alignment: .bottomTrailing) {
            RippleMic(state: micState)
            if micState == .preparing {
                Circle()
                    .fill(CSColor.glassUnder)
                    .frame(width: 10, height: 10)
                    .overlay {
                        ProgressView()
                            .controlSize(.small)
                            .scaleEffect(0.42)
                    }
                    .offset(x: 2, y: 2)
            }
        }
    }

    /// Small non-modal error line under the input box (permission off, no speech,
    /// STT failure). Self-clears via the store after a few seconds.
    @ViewBuilder
    private var dictationFeedback: some View {
        if case let .failed(message) = store.dictationPhase {
            HStack(spacing: 6) {
                CSIconView(icon: .error, size: 10.5)
                Text(message)
                    .font(CSFont.mono(10.5, .medium))
            }
            .foregroundStyle(CSColor.amber)
            .padding(.leading, 2)
            .transition(.opacity)
        }
    }

    @ViewBuilder
    private var dictationPreview: some View {
        if !store.dictationPreview.isEmpty {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                CSIconView(icon: .mic, size: 10.5, color: CSColor.terracottaLight)
                Text(store.dictationPreview)
                    .font(CSFont.ui(12.5 * textScale))
                    .foregroundStyle(CSColor.textFaint)
                    .lineLimit(2)
                    .truncationMode(.tail)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.leading, 2)
            .transition(.opacity.combined(with: .move(edge: .top)))
        }
    }

    /// Transparent drop target layered over the input box. Hit-testable only
    /// while a drag is in progress, so it intercepts a field drop (beating the
    /// native text editor) without blocking clicks/typing at rest. Its own
    /// `isTargeted` binding keeps `isDragging` true while the pointer is over the
    /// field even as the outer target reports false — no highlight flicker.
    private var dropCatcher: some View {
        Color.clear
            .contentShape(Rectangle())
            .onDrop(of: [.fileURL], isTargeted: $overInner) { providers in
                handleDrop(providers)
            }
            .allowsHitTesting(isDragging)
    }

    // MARK: Attachment chips

    private var attachmentChips: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(store.pendingAttachments) { attachment in
                    HStack(spacing: 6) {
                        CSIconView(icon: .photo, size: 11, color: CSColor.terracottaLight)
                        Text(attachment.name)
                            .font(CSFont.mono(10.5, .medium))
                            .foregroundStyle(CSColor.textBodyAlt)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .frame(maxWidth: 160)
                        Button(action: { store.removeAttachment(attachment.id) }) {
                            CSIconView(icon: .close, size: 9, weight: .bold, color: CSColor.textFaint)
                        }
                        .buttonStyle(.plain)
                        .help("Remove attachment")
                    }
                    .padding(.horizontal, 9)
                    .padding(.vertical, 5)
                    .background(CSColor.surfaceRaised(0.05))
                    .overlay(
                        RoundedRectangle(cornerRadius: CSRadius.pill, style: .continuous)
                            .strokeBorder(CSColor.hairline(0.10), lineWidth: 1)
                    )
                    .clipShape(RoundedRectangle(cornerRadius: CSRadius.pill, style: .continuous))
                }
            }
            .padding(.horizontal, 2)
        }
        .frame(maxHeight: 30)
    }

    // MARK: Image picker

    private func pickAttachments() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        panel.canChooseFiles = true
        panel.prompt = "Attach"
        panel.message = "Attach images to send to the agent"
        // Restrict to the vision-supported image types the bridge actually loads.
        panel.allowedContentTypes = [.png, .jpeg, .gif, .webP, .bmp, .tiff]
        attachLog.info("pickAttachments: presenting NSOpenPanel (modeless begin)")
        panel.begin { response in
            let ok = response == .OK
            let urls = ok ? panel.urls : []
            let names = urls.map { $0.lastPathComponent }.joined(separator: ", ")
            attachLog.info(
                "pickAttachments completion: response=\(ok ? "OK" : "cancel", privacy: .public) urls=\(urls.count, privacy: .public) files=[\(names, privacy: .public)]"
            )
            guard ok else { return }
            Task { @MainActor in store.addAttachments(urls) }
        }
    }

    // MARK: Drag & drop

    /// Image types accepted by the drop area — mirrors the NSOpenPanel picker so
    /// the two staging paths agree on what counts as an image.
    private static let acceptedImageTypes: [UTType] = [.png, .jpeg, .gif, .webP, .bmp, .tiff]

    /// Handle a batch of dropped file providers. Loads each file URL off-main,
    /// then stages the image ones via the existing `store.addAttachments` path
    /// (chips + send parity). Non-image files are rejected with a breadcrumb.
    /// Returns true when at least one file-URL provider was accepted for loading.
    private func handleDrop(_ providers: [NSItemProvider]) -> Bool {
        let fileProviders = providers.filter {
            $0.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier)
        }
        attachLog.info(
            "onDrop: providers=\(providers.count, privacy: .public) fileURL=\(fileProviders.count, privacy: .public)"
        )
        guard !fileProviders.isEmpty else { return false }

        for provider in fileProviders {
            _ = provider.loadObject(ofClass: URL.self) { url, error in
                guard let url else {
                    attachLog.error(
                        "onDrop: failed to load dropped URL: \(error?.localizedDescription ?? "nil", privacy: .public)"
                    )
                    return
                }
                Task { @MainActor in ingestFileURL(url, source: "onDrop") }
            }
        }
        return true
    }

    /// Stage a single dropped/pasted file if it is an accepted image type,
    /// otherwise log the rejection. Called on the main actor per resolved URL —
    /// the shared convergence point for the drag & drop and ⌘V staging paths.
    @MainActor
    private func ingestFileURL(_ url: URL, source: String) {
        let type = UTType(filenameExtension: url.pathExtension)
        let isImage = type.map { candidate in
            Self.acceptedImageTypes.contains { candidate.conforms(to: $0) }
        } ?? false
        guard isImage else {
            attachLog.info(
                "\(source, privacy: .public): rejected non-image name=\(url.lastPathComponent, privacy: .public) ext=\(url.pathExtension, privacy: .public)"
            )
            return
        }
        store.addAttachments([url])
    }

    // MARK: ⌘V paste

    /// Where a ⌘V in the composer should route, decided from what the pasteboard
    /// holds. Pure so the 8-combination matrix is unit-testable.
    enum PasteDisposition: Equatable {
        /// File URLs on the pasteboard (Finder copy) → stage the image ones.
        case stageFiles
        /// A bare image with no text (screenshot ⌘⇧⌃4) → save to a temp file, stage.
        case stageImage
        /// Text present (or nothing usable) → let the field editor paste normally.
        case passthroughText
    }

    /// Decision table for a composer paste. File URLs win outright; a pasteboard
    /// image only stages when there is no text alongside it (copying from a
    /// browser puts image + text on the pasteboard, and the user expects TEXT).
    static func pasteDisposition(hasFileURLs: Bool, hasImage: Bool, hasText: Bool) -> PasteDisposition {
        if hasFileURLs { return .stageFiles }
        if hasImage, !hasText { return .stageImage }
        return .passthroughText
    }

    /// One local key monitor for ⌘V. Installed while the composer is on screen;
    /// events for other windows or without composer-field focus pass through
    /// untouched, so search fields and Settings keep native paste behaviour.
    private func installPasteMonitor() {
        guard pasteMonitor == nil else { return }
        pasteMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            // Local key monitors always deliver on the main thread; assume the
            // main actor statically so the store calls stay isolation-checked.
            MainActor.assumeIsolated {
                guard isComposerPasteEvent(event) else { return event }
                return handlePaste(NSPasteboard.general) ? nil : event
            }
        }
    }

    private func removePasteMonitor() {
        if let pasteMonitor { NSEvent.removeMonitor(pasteMonitor) }
        pasteMonitor = nil
    }

    /// True only for a plain ⌘V aimed at this composer: field focused, event in
    /// our own window, no other modifiers (⌘⇧V paste-and-match-style passes on).
    @MainActor
    private func isComposerPasteEvent(_ event: NSEvent) -> Bool {
        guard fieldFocused, let hostWindow, event.window === hostWindow else { return false }
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        // ⌘ alone — shift/option/control bail (⌘⇧V stays native), but stray
        // state flags like capsLock must not defeat the match.
        guard flags.contains(.command),
              flags.isDisjoint(with: [.shift, .option, .control]) else { return false }
        return event.charactersIgnoringModifiers?.lowercased() == "v"
    }

    /// Route a composer ⌘V. Returns true when the event was consumed by staging
    /// (the field editor must not also paste), false to pass it through as text.
    @MainActor
    private func handlePaste(_ pasteboard: NSPasteboard) -> Bool {
        let fileURLs = (pasteboard.readObjects(
            forClasses: [NSURL.self],
            options: [.urlReadingFileURLsOnly: true]
        ) as? [URL]) ?? []
        let hasImage = pasteboard.availableType(from: [.png, .tiff]) != nil
        let hasText = pasteboard.availableType(from: [.string]) != nil
        let disposition = Self.pasteDisposition(
            hasFileURLs: !fileURLs.isEmpty, hasImage: hasImage, hasText: hasText
        )
        attachLog.info(
            "paste: fileURLs=\(fileURLs.count, privacy: .public) hasImage=\(hasImage, privacy: .public) hasText=\(hasText, privacy: .public) disposition=\(String(describing: disposition), privacy: .public)"
        )
        switch disposition {
        case .stageFiles:
            for url in fileURLs { ingestFileURL(url, source: "paste") }
            return true
        case .stageImage:
            stagePastedImage(pasteboard)
            return true
        case .passthroughText:
            return false
        }
    }

    /// Persist a bare pasteboard image (screenshot-style TIFF/PNG) to a readable
    /// temp file and stage it through the shared attachments path.
    @MainActor
    private func stagePastedImage(_ pasteboard: NSPasteboard) {
        let pngData: Data?
        if let png = pasteboard.data(forType: .png) {
            pngData = png
        } else if let tiff = pasteboard.data(forType: .tiff),
                  let bitmap = NSBitmapImageRep(data: tiff) {
            pngData = bitmap.representation(using: .png, properties: [:])
        } else {
            pngData = nil
        }
        guard let pngData else {
            attachLog.error("paste: pasteboard image had no decodable PNG/TIFF payload")
            return
        }
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("pasted-\(Self.pastedNameFormatter.string(from: Date())).png")
        do {
            try pngData.write(to: url)
            attachLog.info(
                "paste: staged clipboard image bytes=\(pngData.count, privacy: .public) file=\(url.lastPathComponent, privacy: .public)"
            )
            store.addAttachments([url])
        } catch {
            attachLog.error(
                "paste: failed to write clipboard image: \(error.localizedDescription, privacy: .public)"
            )
        }
    }

    /// Readable, collision-safe temp-file stamp (`pasted-20260715-140233-421.png`).
    private static let pastedNameFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyyMMdd-HHmmss-SSS"
        return formatter
    }()

    /// Invisible probe resolving the NSWindow this composer lives in, so the
    /// paste monitor can discriminate our window from Settings / the overlay.
    /// Identity-guarded so re-reporting the same window can't loop view updates.
    private var hostWindowReader: some View {
        WindowReader { window in
            if hostWindow !== window { hostWindow = window }
        }
    }

    private let affordances = [
        "· streaming",
        "· attach file / image",
    ]
}

enum ComposerActionAccessibility {
    static let identifier = "agent-composer-primary-action"
}

/// Pure Send/Stop/Stopping projection used by the view and focused tests.
enum ComposerActionVisualState: Equatable {
    case send(enabled: Bool)
    case stop
    case stopping

    static func resolve(canSend: Bool, activePhase: ComposerTurnPhase?) -> ComposerActionVisualState {
        switch activePhase {
        case .thinking?, .streaming?: return .stop
        case .cancelling?: return .stopping
        case nil: return .send(enabled: canSend)
        }
    }

    var isEnabled: Bool {
        switch self {
        case .send(let enabled): return enabled
        case .stop: return true
        case .stopping: return false
        }
    }

    var accessibilityLabel: String {
        switch self {
        case .send: return "Send message"
        case .stop: return "Stop response"
        case .stopping: return "Stopping response"
        }
    }

    var icon: CSIcon {
        switch self {
        case .send: return .send
        case .stop, .stopping: return .stop
        }
    }

    var iconWeight: CSIconWeight {
        switch self {
        case .send: return .semibold
        case .stop, .stopping: return .fill
        }
    }
}

/// Minimal probe reporting the `NSWindow` that hosts a SwiftUI hierarchy. The
/// composer's ⌘V monitor uses it to scope key events to its own window; the
/// resolve callback fires async because `window` is nil until the view lands
/// in a window, and mutating view state mid-update is illegal.
private struct WindowReader: NSViewRepresentable {
    var onResolve: (NSWindow?) -> Void

    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async { [weak view] in onResolve(view?.window) }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        DispatchQueue.main.async { [weak nsView] in onResolve(nsView?.window) }
    }
}
