import SwiftUI

/// Word-reveal caret: 8×18 terracotta block, soft-pulsing on a 1s cycle.
struct BlinkingCaret: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  let animating: Bool

  var body: some View {
    Group {
      if reduceMotion || !animating {
        caret.opacity(1)
      } else {
        // Remove the stateful view to tear down repeatForever, as StatusPill
        // does. A nil animation transaction alone cannot cancel the loop.
        AnimatedOverlayCaret()
      }
    }
    .transaction { transaction in
      if reduceMotion || !animating {
        transaction.animation = nil
        transaction.disablesAnimations = true
      }
    }
  }

  private var caret: some View {
    RoundedRectangle(cornerRadius: 1, style: .continuous)
      .fill(CSColor.terracotta)
      .frame(width: 7, height: 15)
      .padding(.bottom, 3)
  }
}
