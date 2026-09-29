import SwiftUI

/// One permission in the Dictation matrix. A missing permission is a button:
/// in-app request while undetermined, System Settings deep link afterwards.
struct PermissionMatrixCell: View {
  let kind: PermissionKind
  let state: PermissionState
  /// Re-probe hook fired after an in-app permission request resolves.
  var onStateChanged: (@MainActor () -> Void)? = nil

  private var granted: Bool { state.isGranted }
  private var accent: Color { granted ? CSColor.olive : CSColor.terracotta }
  private var accentLight: Color { granted ? CSColor.oliveLight : CSColor.terracottaLight }

  var body: some View {
    Button(action: grant) {
      HStack(spacing: 10) {
        CSIconView(
          icon: granted ? .success : .warning, size: 11, weight: .semibold, color: accentLight)
        Text(kind.rawValue)
          .font(CSFont.ui(12.5, .medium))
          .foregroundStyle(CSColor.textBodyAlt)
          .frame(maxWidth: .infinity, alignment: .leading)
        Text(granted ? "granted" : state.label)
          .font(CSFont.mono(10, .semibold))
          .foregroundStyle(accentLight)
      }
      .padding(.horizontal, 14)
      .padding(.vertical, 11)
      .background {
        RoundedRectangle(cornerRadius: 10)
          .fill(accent.opacity(0.08))
          .strokeBorder(accent.opacity(0.2), lineWidth: 1)
      }
      .contentShape(.rect)
    }
    .buttonStyle(.plain)
    .csFocusRing()
    .accessibilityHint(granted ? "Already granted." : "Grants \(kind.rawValue) access.")
  }

  private func grant() {
    guard !granted else { return }
    // Same grant path as onboarding: Speech Recognition can be requested
    // straight from the app while undetermined — the system dialog grants the
    // app's own TCC identity, which the bridge child inherits. Once determined,
    // macOS never re-prompts, so fall through to the System Settings deep link.
    if state == .notDetermined, kind.supportsInAppPermissionRequest {
      Task { @MainActor in
        _ = await kind.requestInApp()
        onStateChanged?()
      }
    } else {
      kind.openSystemSettings()
    }
  }
}
