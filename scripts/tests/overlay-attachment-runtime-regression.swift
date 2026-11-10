import AppKit
import Foundation

struct CodescribeConfig {
  func configDir() -> String { FileManager.default.temporaryDirectory.path }
}

enum ConversationClipboardImage {
  @MainActor
  static func png(from _: NSPasteboard) throws -> Data? { nil }
}

@main
struct OverlayAttachmentRuntimeRegression {
  @MainActor
  static func main() throws {
    let maximumBytes = ConversationClipboardAttachment.maximumBytes
    let width = 4096
    let height = 4096
    guard let bitmap = NSBitmapImageRep(
      bitmapDataPlanes: nil, pixelsWide: width, pixelsHigh: height,
      bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
      colorSpaceName: .deviceRGB, bytesPerRow: width * 4, bitsPerPixel: 32),
      let pixels = bitmap.bitmapData
    else { fatalError("could not allocate Retina-sized bitmap") }

    for y in 0..<height {
      let row = y * bitmap.bytesPerRow
      for x in 0..<width {
        let offset = row + x * 4
        pixels[offset] = UInt8(x & 0xff)
        pixels[offset + 1] = UInt8(y & 0xff)
        pixels[offset + 2] = UInt8((x + y) & 0xff)
        pixels[offset + 3] = 0xff
      }
    }
    guard let tiff = bitmap.representation(using: .tiff, properties: [:]) else {
      fatalError("could not encode Retina TIFF")
    }
    guard tiff.count > maximumBytes else {
      fatalError("fixture TIFF is only \(tiff.count) bytes; expected over the 50 MB cap")
    }

    let file = FileManager.default.temporaryDirectory
      .appendingPathComponent(UUID().uuidString).appendingPathExtension("tiff")
    defer { try? FileManager.default.removeItem(at: file) }
    try tiff.write(to: file)
    let pasteboard = NSPasteboard.withUniqueName()
    defer { pasteboard.releaseGlobally() }
    guard pasteboard.writeObjects([file as NSURL]) else {
      fatalError("could not publish TIFF file URL to the test pasteboard")
    }

    var savedImage: Data?
    do {
      let draft = try ConversationClipboardAttachment.text(
        from: pasteboard,
        saveImage: { data in
          savedImage = data
          let assets = URL(fileURLWithPath: CodescribeConfig().configDir())
            .appendingPathComponent("assets", isDirectory: true)
          try FileManager.default.createDirectory(at: assets, withIntermediateDirectories: true)
          return assets.appendingPathComponent("inline.png")
        },
        saveFile: { _, _ in
          fatalError("TIFF screenshot was stored as an opaque file")
        })
      guard let draft, let png = savedImage else {
        fatalError("large TIFF did not become an image attachment")
      }
      guard Array(png.prefix(8)) == [137, 80, 78, 71, 13, 10, 26, 10] else {
        fatalError("transcoded payload is not PNG")
      }
      guard png.count <= maximumBytes else {
        fatalError("normalized PNG still exceeds the 50 MB cap")
      }
      guard NSBitmapImageRep(data: png)?.pixelsWide == width else {
        fatalError("transcoded PNG lost the source dimensions")
      }
      guard ConversationDraft.split(draft).attachments.count == 1 else {
        fatalError("transcoded image did not become an attachment pointer")
      }
      print("PASS: \(tiff.count) byte TIFF became \(png.count) byte PNG before the 50 MB check")
    } catch {
      fputs("FAIL: raw TIFF file URL was rejected before image normalization: \(error)\n", stderr)
      exit(1)
    }
  }
}
