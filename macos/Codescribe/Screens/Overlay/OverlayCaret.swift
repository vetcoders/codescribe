import AppKit
import SwiftUI

/// A growing capture bubble uses the same native text and insertion geometry as
/// the transcript canvas. It sizes to its contents inside the conversation's
/// scroll view; a second nested scroll view would compete for the live edge.
struct TranscriptActivityText: NSViewRepresentable {
  let text: String
  let fontSize: CGFloat
  let color: Color
  private let utf8Identity: [UInt8]

  init(text: String, fontSize: CGFloat, color: Color) {
    self.text = text
    self.fontSize = fontSize
    self.color = color
    self.utf8Identity = Array(text.utf8)
  }

  func makeNSView(context: Context) -> LiveTranscriptNativeTextView {
    let view = LiveTranscriptTextView.makeTextView()
    view.textContainer?.widthTracksTextView = false
    view.setAccessibilityIdentifier("conversation-transcript-live")
    updateNSView(view, context: context)
    return view
  }

  func updateNSView(_ view: LiveTranscriptNativeTextView, context: Context) {
    let paragraph = NSMutableParagraphStyle()
    paragraph.lineSpacing = 5
    let attributes: [NSAttributedString.Key: Any] = [
      .font: NSFont.systemFont(ofSize: fontSize),
      .foregroundColor: NSColor(color),
      .paragraphStyle: paragraph,
    ]
    let incoming = NSAttributedString(string: text, attributes: attributes)
    if !view.string.utf8.elementsEqual(text.utf8) || view.textStorage?.isEqual(to: incoming) != true
    {
      let selection = view.selectedRange()
      view.textStorage?.setAttributedString(incoming)
      view.setSelectedRange(
        LiveTranscriptSelectionPolicy.preservedRange(selection, updatedLength: incoming.length))
    }
    view.typingAttributes = attributes
    view.setTranscriptActivity(true)
    view.needsLayout = true
  }

  func sizeThatFits(
    _ proposal: ProposedViewSize, nsView view: LiveTranscriptNativeTextView, context: Context
  ) -> CGSize? {
    // Reserve the decorative marker's width without adding a character or
    // changing selection/copy. TextKit still owns all line wrapping and glyphs.
    let markerSpace: CGFloat = 12
    let idealWidth = min(660, (view.textStorage?.size().width ?? 0) + markerSpace)
    let width = max(markerSpace + 1, proposal.width ?? idealWidth)
    view.setFrameSize(NSSize(width: width, height: max(1, view.frame.height)))
    view.textContainer?.containerSize = NSSize(
      width: width - markerSpace, height: .greatestFiniteMagnitude)
    if let manager = view.textLayoutManager, let range = manager.textContentManager?.documentRange {
      manager.ensureLayout(for: range)
    }
    view.sizeToFit()
    view.needsLayout = true
    // The marker hosts SwiftUI. Let AppKit lay it out after this SwiftUI size
    // query finishes; re-entering the hosting graph here is not permitted.
    return NSSize(width: width, height: max(ceil(fontSize * 1.3), view.frame.height))
  }
}

/// Paint-only activity marker; never inserted into the transcript or clipboard.
struct TranscriptActivityUnderline: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  let subdued: Bool

  init(subdued: Bool = false) {
    self.subdued = subdued
  }

  var body: some View {
    TimelineView(.animation(minimumInterval: 0.6, paused: reduceMotion)) { context in
      let bright = reduceMotion || Int(context.date.timeIntervalSinceReferenceDate / 0.6) % 2 == 0
      RoundedRectangle(cornerRadius: 1)
        .fill(CSColor.terracotta)
        .frame(width: 10, height: 2)
        .shadow(color: CSColor.terracotta.opacity(bright ? (subdued ? 0.1 : 0.3) : 0), radius: 2)
        .opacity(bright ? 1 : (subdued ? 0.75 : 0.35))
    }
    .frame(width: 10, height: 4, alignment: .bottom)
    .allowsHitTesting(false)
    .accessibilityHidden(true)
  }
}
