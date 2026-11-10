import AppKit
import Foundation
import UniformTypeIdentifiers

/// Clipboard bytes stay on disk. The draft contains editable local pointers;
/// publication adds verified metadata through x9bus, never inline file bytes.
enum ConversationClipboardAttachment {
  static let maximumBytes = 50 * 1024 * 1024
  /// Uncompressed Retina TIFFs written by clipboard managers like Paste.app can
  /// exceed 50 MB on disk before lossless PNG compression reduces them to a few MB.
  static let maximumRawTIFFBytes = 256 * 1024 * 1024
  static let inlineTextBytes = 4096

  private static let filenamesPasteboardType = NSPasteboard.PasteboardType("NSFilenamesPboardType")

  @MainActor
  static func text(
    from clipboard: NSPasteboard,
    saveImage: (Data) throws -> URL,
    saveFile: (Data, String) throws -> URL
  ) throws -> String? {
    // Finder also publishes a text representation; actual file objects win.
    if clipboard.availableType(from: [.fileURL, filenamesPasteboardType]) != nil {
      guard
        let files = clipboard.readObjects(
          forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL],
        !files.isEmpty
      else { throw CocoaError(.fileReadCorruptFile) }
      return try stageFiles(files, saveImage: saveImage, saveFile: saveFile)
    }
    if let image = try ConversationClipboardImage.png(from: clipboard) {
      try validate(image)
      return "\n" + ConversationAttachmentPointer.line(try saveImage(image)) + "\n"
    }
    if let text = clipboard.string(forType: .string) {
      if let imageFiles = resolvedExistingFileURLs(from: text, imagesOnly: true) {
        return try stageFiles(imageFiles, saveImage: saveImage, saveFile: saveFile)
      }
      let data = Data(text.utf8)
      guard data.count > inlineTextBytes else { return text }
      try validate(data)
      return "\n" + ConversationAttachmentPointer.line(try saveFile(data, "Pasted text.txt")) + "\n"
    }
    // PDF and other data objects can be copied without a file URL.
    var pointers: [String] = []
    var total = 0
    for item in clipboard.pasteboardItems ?? [] {
      if let type = item.types.first(where: {
        UTType($0.rawValue)?.conforms(to: .data) == true
      }), let data = item.data(forType: type) {
        try validate(data)
        total += data.count
        guard total <= maximumBytes else { throw CocoaError(.fileReadTooLarge) }
        let ext = UTType(type.rawValue)?.preferredFilenameExtension ?? "bin"
        pointers.append(ConversationAttachmentPointer.line(try saveFile(data, "Pasted.\(ext)")))
      }
    }
    return pointers.isEmpty ? nil : "\n" + pointers.joined(separator: "\n") + "\n"
  }

  /// Drag-and-drop entry point: promotes dragged plain-text file paths (such as
  /// path cards dragged from Paste.app) to staged file attachments so the
  /// composer never inserts raw file paths into the prose editor.
  @MainActor
  static func textForDrop(
    from draggingPasteboard: NSPasteboard,
    saveImage: (Data) throws -> URL,
    saveFile: (Data, String) throws -> URL
  ) throws -> String? {
    if draggingPasteboard.availableType(from: [.fileURL, filenamesPasteboardType, .png, .tiff])
      == nil,
      let rawText = draggingPasteboard.string(forType: .string),
      let files = resolvedExistingFileURLs(from: rawText, imagesOnly: false)
    {
      return try stageFiles(files, saveImage: saveImage, saveFile: saveFile)
    }
    return try text(from: draggingPasteboard, saveImage: saveImage, saveFile: saveFile)
  }

  private static func stageFiles(
    _ files: [URL],
    saveImage: (Data) throws -> URL,
    saveFile: (Data, String) throws -> URL
  ) throws -> String {
    var total = 0
    let pointers = try files.map { file in
      let values = try file.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
      if isTIFFFile(file) {
        guard values.isRegularFile == true,
          let size = values.fileSize, size > 0, size <= maximumRawTIFFBytes
        else { throw CocoaError(.fileReadTooLarge) }
        let handle = try FileHandle(forReadingFrom: file)
        defer { try? handle.close() }
        let raw = try handle.read(upToCount: maximumRawTIFFBytes + 1) ?? Data()
        guard !raw.isEmpty, raw.count <= maximumRawTIFFBytes else {
          throw CocoaError(.fileReadTooLarge)
        }
        let png = try pngData(fromTIFF: raw)
        guard !png.isEmpty, png.count <= maximumBytes - total else {
          throw CocoaError(.fileReadTooLarge)
        }
        total += png.count
        return ConversationAttachmentPointer.line(try saveImage(png))
      }
      guard values.isRegularFile == true,
        let size = values.fileSize, size > 0, size <= maximumBytes - total
      else { throw CocoaError(.fileReadTooLarge) }
      let handle = try FileHandle(forReadingFrom: file)
      defer { try? handle.close() }
      let data = try handle.read(upToCount: maximumBytes - total + 1) ?? Data()
      try validate(data)
      total += data.count
      guard total <= maximumBytes else { throw CocoaError(.fileReadTooLarge) }
      return ConversationAttachmentPointer.line(try saveFile(data, file.lastPathComponent))
    }
    return "\n" + pointers.joined(separator: "\n") + "\n"
  }

  static func isTIFFFile(_ url: URL) -> Bool {
    UTType(filenameExtension: url.pathExtension)?.conforms(to: .tiff) == true
  }

  static func pngData(fromTIFF data: Data) throws -> Data {
    guard let bitmap = NSBitmapImageRep(data: data),
      let png = bitmap.representation(using: .png, properties: [:])
    else { throw CocoaError(.fileReadCorruptFile) }
    return png
  }

  static func resolvedExistingFileURLs(from text: String, imagesOnly: Bool) -> [URL]? {
    let lines =
      text
      .components(separatedBy: .newlines)
      .map { $0.trimmingCharacters(in: .whitespaces) }
      .filter { !$0.isEmpty }
    guard !lines.isEmpty else { return nil }
    var urls: [URL] = []
    for line in lines {
      let candidate: URL
      if line.hasPrefix("file://"), let url = URL(string: line), url.isFileURL {
        candidate = url
      } else if line.hasPrefix("/") {
        candidate = URL(fileURLWithPath: line)
      } else {
        return nil
      }
      guard (try? candidate.resourceValues(forKeys: [.isRegularFileKey]))?.isRegularFile == true
      else { return nil }
      if imagesOnly {
        guard UTType(filenameExtension: candidate.pathExtension)?.conforms(to: .image) == true
        else { return nil }
      }
      urls.append(candidate)
    }
    return urls
  }

  private static func validate(_ data: Data) throws {
    guard !data.isEmpty, data.count <= maximumBytes else {
      throw CocoaError(.fileReadTooLarge)
    }
  }
}

/// Markers are part of the single draft, so deleting one removes the attachment.
/// Only pointers into Codescribe's stores become --attach-file arguments.
enum ConversationAttachmentPointer {
  static let prefix = "[attachment] "

  static func line(_ url: URL) -> String { prefix + url.path }

  /// The data directory is fixed for the process: `CODESCRIBE_DATA_DIR` is
  /// read at launch. The composer splits its draft on every body pass, and
  /// each fresh `CodescribeConfig()` reads the run-monitor store and logs on
  /// the main thread — four times per painted frame of the 2026-10-19 unroll.
  static let configDirectory = URL(fileURLWithPath: CodescribeConfig().configDir())

  static func extract(
    from draft: String,
    configDirectory: URL = ConversationAttachmentPointer.configDirectory
  ) -> (text: String, paths: [String]) {
    let root = configDirectory.resolvingSymlinksInPath()
    let stores = ["assets", "attachments"].map {
      root.appendingPathComponent($0, isDirectory: true).path + "/"
    }
    var paths: [String] = []
    let lines = draft.components(separatedBy: "\n").filter { line in
      guard line.hasPrefix(prefix) else { return true }
      let raw = String(line.dropFirst(prefix.count))
      guard raw.hasPrefix("/") else { return true }
      let path = URL(fileURLWithPath: raw).resolvingSymlinksInPath().path
      guard stores.contains(where: { path.hasPrefix($0) }) else { return true }
      if !paths.contains(path) { paths.append(path) }
      return false
    }
    return (lines.joined(separator: "\n"), paths)
  }
}

/// The composer shows the prose and a pill per attachment. Founder, 2026-10-15:
/// a pasted image "powinien się zwijać" to a pill, as in the Agent window's
/// input, instead of a raw path. The draft stays one string; the pills are its
/// pointer lines into Codescribe's stores, kept after the prose.
enum ConversationDraft {
  static func split(
    _ draft: String,
    configDirectory: URL = ConversationAttachmentPointer.configDirectory
  ) -> (text: String, attachments: [String]) {
    let parts = ConversationAttachmentPointer.extract(from: draft, configDirectory: configDirectory)
    return (parts.text, parts.paths)
  }

  static func compose(text: String, attachments: [String]) -> String {
    guard !attachments.isEmpty else { return text }
    let lines = attachments.map { ConversationAttachmentPointer.line(URL(fileURLWithPath: $0)) }
    return text + "\n" + lines.joined(separator: "\n")
  }
}
