import SwiftUI

extension DictationOverlayView {
  /// Split chrome: title runs the primary act; chevron is a separate menu.
  /// One capsule, two hit targets. Kept out of `DictationOverlayView.swift`
  /// so Loctree `body performPrimaryAction` stays a single hop body.
  func splitPrimaryAction(kind: OverlayPrimaryActionKind, compact: Bool = false) -> some View {
    let shape = RoundedRectangle(cornerRadius: buttonRadius, style: .continuous)
    return HStack(spacing: 0) {
      Button {
        performPrimaryAction(kind)
      } label: {
        Group {
          if compact {
            Text(state.primaryActionCompactTitle)
          } else {
            ViewThatFits(in: .horizontal) {
              Text(state.primaryActionTitle).fixedSize(horizontal: true, vertical: false)
              Text(state.primaryActionCompactTitle)
            }
          }
        }
        .font(CSFont.ui(12, .semibold))
        .lineLimit(1)
        .padding(.leading, 10)
        .padding(.trailing, 8)
        .frame(maxWidth: compact ? 72 : 128, minHeight: primaryActionHeight)
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
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
        Button("Close", systemImage: "xmark", role: .destructive) { state.close() }
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
    .foregroundStyle(CSColor.chromeAccent)
    .background(CSColor.chromeAccent.opacity(0.12))
    .overlay(shape.strokeBorder(CSColor.chromeAccent.opacity(0.28), lineWidth: 1))
    .clipShape(shape)
    .fixedSize()
    .accessibilityElement(children: .contain)
    .accessibilityLabel("Dictation actions")
  }

  @ViewBuilder
  func secondaryActionButtons(for kind: OverlayPrimaryActionKind) -> some View {
    switch kind {
    case .finish:
      if state.canCopy {
        Button("Copy", systemImage: "doc.on.doc") { state.copyToPasteboard() }
      }
    case .insert:
      if state.canCopy {
        Button("Copy", systemImage: "doc.on.doc") { state.copyToPasteboard() }
      }
      Button(OverlayActionPresentation.sendTitle, systemImage: "paperplane") {
        state.sendToAgent()
      }
    }
  }
}
