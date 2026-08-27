import SwiftUI

extension DictationOverlayView {
  /// Split chrome: title runs the primary act; chevron is a separate menu.
  /// One capsule, two hit targets. Kept out of `DictationOverlayView.swift`
  /// so Loctree `body performPrimaryAction` stays a single hop body.
  func splitPrimaryAction(kind: OverlayPrimaryActionKind) -> some View {
    let shape = RoundedRectangle(cornerRadius: buttonRadius, style: .continuous)
    return HStack(spacing: 0) {
      Button {
        performPrimaryAction(kind)
      } label: {
        Text(state.primaryActionTitle)
          .font(CSFont.ui(12, .semibold))
          .lineLimit(1)
          .padding(.leading, 10)
          .padding(.trailing, 8)
          .frame(height: primaryActionHeight)
          .contentShape(Rectangle())
      }
      .csFocusRing(cornerRadius: buttonRadius)
      .help(state.primaryActionHelp)
      .accessibilityLabel(state.primaryActionTitle)
      .accessibilityHint(state.primaryActionHelp)
      .accessibilityIdentifier("overlay-primary-action")

      Rectangle()
        .fill(CSColor.hairline(0.14))
        .frame(width: 1, height: 14)

      Menu {
        secondaryActionButtons(for: kind)
        Divider()
        Button("Close", role: .destructive) { state.close() }
      } label: {
        Image(systemName: "chevron.down")
          .font(.system(size: 9, weight: .semibold))
          .frame(width: 22, height: primaryActionHeight)
          .contentShape(Rectangle())
      }
      .menuStyle(.borderlessButton)
      .menuIndicator(.hidden)
      .frame(width: 22, height: primaryActionHeight)
      .csFocusRing(cornerRadius: 8)
      .help("More actions")
      .accessibilityLabel("More actions")
      .accessibilityIdentifier("overlay-primary-action-menu")
    }
    .foregroundStyle(CSColor.textBody)
    .background(CSColor.surfaceRaised(0.06))
    .overlay(shape.strokeBorder(CSColor.hairline(0.14), lineWidth: 1))
    .clipShape(shape)
    .fixedSize()
  }

  @ViewBuilder
  func secondaryActionButtons(for kind: OverlayPrimaryActionKind) -> some View {
    switch kind {
    case .finish:
      if state.canCopy {
        Button("Copy") { state.copyToPasteboard() }
      }
    case .insert:
      if state.canCopy {
        Button("Copy") { state.copyToPasteboard() }
      }
      Button(OverlayActionPresentation.sendTitle) { state.sendToAgent() }
    }
  }
}
