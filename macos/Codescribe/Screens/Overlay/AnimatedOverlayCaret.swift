import SwiftUI

struct AnimatedOverlayCaret: View {
  @State private var on = false

  var body: some View {
    RoundedRectangle(cornerRadius: 1, style: .continuous)
      .fill(CSColor.terracotta)
      .frame(width: 7, height: 15)
      .padding(.bottom, 3)
      .opacity(on ? 1 : 0.7)
      .onAppear {
        withAnimation(.easeInOut(duration: 1).repeatForever(autoreverses: true)) {
          on = true
        }
      }
  }
}
