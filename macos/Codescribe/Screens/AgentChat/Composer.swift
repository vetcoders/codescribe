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
/// capability hints.
struct Composer: View {
    @ObservedObject var store: AgentChatStore
    @FocusState private var fieldFocused: Bool
    /// Highlights the composer while an image file is dragged over it.
    @State private var isDropTargeted = false

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            if !store.pendingAttachments.isEmpty {
                attachmentChips
            }

            HStack(spacing: 10) {
                // Attach images (NSOpenPanel → staged chips → vision FFI on send).
                Button(action: pickAttachments) {
                    Text("📎")
                        .font(.system(size: 15))
                        .foregroundStyle(store.pendingAttachments.isEmpty ? CSColor.textFaint : CSColor.terracottaLight)
                }
                .buttonStyle(.plain)
                .help("Attach an image (PNG, JPEG, GIF, WebP)")

                TextField("", text: $store.draft, prompt:
                    Text("Type a message, or hold Fn to speak…")
                        .font(CSFont.ui(13.5))
                        .foregroundColor(CSColor.textFaint)
                )
                .textFieldStyle(.plain)
                .font(CSFont.ui(13.5))
                .foregroundStyle(CSColor.textBody)
                .focused($fieldFocused)
                .onSubmit { store.send() }

                RippleMic()

                Button(action: { store.send() }) {
                    Text("↑")
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundStyle(ChatPalette.sendGlyph)
                        .frame(width: 32, height: 32)
                        .background(CSColor.terracotta)
                        .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
                }
                .buttonStyle(.plain)
                .disabled(!store.canSend)
            }
            .padding(.leading, 13)
            .padding(.trailing, 11)
            .padding(.vertical, 9)
            .background(CSColor.surfaceRaised(isDropTargeted ? 0.07 : 0.04))
            .overlay(
                RoundedRectangle(cornerRadius: 13, style: .continuous)
                    .strokeBorder(
                        isDropTargeted ? CSColor.terracotta : CSColor.hairline(0.09),
                        lineWidth: isDropTargeted ? 1.5 : 1
                    )
            )
            .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))

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
        // the 📎 picker). The whole bottom strip is the drop area; the input box
        // border lights up terracotta while a file is dragged over it.
        .onDrop(of: [.fileURL], isTargeted: $isDropTargeted.animation(.easeOut(duration: 0.12))) { providers in
            handleDrop(providers)
        }
    }

    // MARK: Attachment chips

    private var attachmentChips: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(store.pendingAttachments) { attachment in
                    HStack(spacing: 6) {
                        Image(systemName: "photo")
                            .font(.system(size: 11))
                            .foregroundStyle(CSColor.terracottaLight)
                        Text(attachment.name)
                            .font(CSFont.mono(10.5, .medium))
                            .foregroundStyle(CSColor.textBodyAlt)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .frame(maxWidth: 160)
                        Button(action: { store.removeAttachment(attachment.id) }) {
                            Image(systemName: "xmark")
                                .font(.system(size: 9, weight: .bold))
                                .foregroundStyle(CSColor.textFaint)
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
                Task { @MainActor in ingestDroppedURL(url) }
            }
        }
        return true
    }

    /// Stage a single dropped file if it is an accepted image type, otherwise log
    /// the rejection. Called on the main actor per resolved URL.
    @MainActor
    private func ingestDroppedURL(_ url: URL) {
        let type = UTType(filenameExtension: url.pathExtension)
        let isImage = type.map { candidate in
            Self.acceptedImageTypes.contains { candidate.conforms(to: $0) }
        } ?? false
        guard isImage else {
            attachLog.info(
                "onDrop: rejected non-image name=\(url.lastPathComponent, privacy: .public) ext=\(url.pathExtension, privacy: .public)"
            )
            return
        }
        store.addAttachments([url])
    }

    private let affordances = [
        "· streaming",
        "· thread memory",
        "· attach file / image",
        "· context: selection · clipboard · frontmost app",
    ]
}
