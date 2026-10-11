import AppKit
import Foundation
import UniformTypeIdentifiers

/// Clipboard bytes stay on disk. The draft contains editable local pointers;
/// publication adds verified metadata through cs-bus, never inline file bytes.
enum ConversationClipboardAttachment {
  static let maximumBytes = 50 * 1024 * 1024
  static let inlineTextBytes = 4096

  @MainActor
  static func text(
    from clipboard: NSPasteboard,
    saveImage: (Data) throws -> URL,
    saveFile: (Data, String) throws -> URL
  ) throws -> String? {
    // Finder also publishes a text representation; actual file objects win.
    if clipboard.availableType(from: [.fileURL]) != nil {
      guard
        let files = clipboard.readObjects(
          forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL],
        !files.isEmpty
      else { throw CocoaError(.fileReadCorruptFile) }
      var total = 0
      let pointers = try files.map { file in
        let values = try file.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
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
    if let image = try ConversationClipboardImage.png(from: clipboard) {
      try validate(image)
      return "\n" + ConversationAttachmentPointer.line(try saveImage(image)) + "\n"
    }
    if let text = clipboard.string(forType: .string) {
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

  static func extract(
    from draft: String,
    configDirectory: URL = URL(fileURLWithPath: CodescribeConfig().configDir())
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
