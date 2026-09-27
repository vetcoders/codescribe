import SwiftUI

/// The transcript keeps its native view identity while status yields to a
/// scrollable region in a small window. Neither can paint over the action row.
struct OverlayTranscriptLayout<Transcript: View, Status: View>: View {
  @State private var statusHeight: CGFloat = 0
  @ViewBuilder var transcript: Transcript
  @ViewBuilder var status: Status

  var body: some View {
    GeometryReader { geometry in
      VStack(alignment: .leading, spacing: CSSpace.sm) {
        transcript
          .frame(minHeight: 0, maxHeight: .infinity)
          .clipped()
        ScrollView(.vertical) {
          VStack(alignment: .leading, spacing: CSSpace.sm) {
            status
          }
          .frame(maxWidth: .infinity, alignment: .leading)
          .fixedSize(horizontal: false, vertical: true)
          .onGeometryChange(for: CGFloat.self) {
            $0.size.height
          } action: {
            statusHeight = $0
          }
        }
        .scrollBounceBehavior(.basedOnSize)
        .frame(height: min(statusHeight, max(0, geometry.size.height / 2)))
      }
    }
  }
}
