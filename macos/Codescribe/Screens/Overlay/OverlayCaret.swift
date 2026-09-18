import SwiftUI

/// Word-reveal caret: 8×18 terracotta block, soft-pulsing on a 1s cycle.
struct BlinkingCaret: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion

  var body: some View {
    if reduceMotion {
      caret.opacity(1)
    } else {
      AnimatedOverlayCaret()
    }
  }

  private var caret: some View {
    RoundedRectangle(cornerRadius: 1, style: .continuous)
      .fill(CSColor.terracotta)
      .frame(width: 7, height: 15)
      .padding(.bottom, 3)
  }
}
