import SwiftUI

enum CSIconWeight {
  case thin, light, regular, medium, semibold, bold, fill

  var font: Font.Weight {
    switch self {
    case .thin: return .thin
    case .light: return .light
    case .regular: return .regular
    case .medium: return .medium
    case .semibold: return .semibold
    case .bold: return .bold
    case .fill: return .semibold
    }
  }
}

enum CSIcon: CaseIterable {
  // Chrome / navigation
  case settings
  case setupWizard
  case help
  case info
  case power

  // Agent / capture
  case agent
  case mic
  case record
  case stop

  // Content actions
  case copy
  case edit
  case notes
  case notesMode
  case history
  case search
  case send
  case refresh
  case attach
  case shortcuts
  case photo
  case delete
  case remove

  // Window modes
  case dock
  case overlay

  // Status / diagnostics
  case success
  case failure
  case warning
  case error
  case tip
  case caution
  case diagnostics
  case accountVerified

  // Affordances / selection
  case chevronRight
  case chevronDown
  case chevronUpDown
  case close
  case check
  case more
  case star
  case starFill
  case checkboxOn
  case checkboxOff
  case checkCircleFill
  case circleEmpty

  var systemName: String {
    switch self {
    case .settings: "gearshape"
    case .setupWizard: "sparkles"
    case .help: "questionmark.circle"
    case .info: "info.circle"
    case .power: "power"
    case .agent: "bubble.left"
    case .mic: "mic"
    case .record: "record.circle"
    case .stop: "stop"
    case .copy: "doc.on.doc"
    case .edit: "pencil"
    case .notes: "square.and.pencil"
    case .notesMode: "note.text"
    case .history: "clock.arrow.circlepath"
    case .search: "magnifyingglass"
    case .send: "arrow.up.circle.fill"
    case .refresh: "arrow.clockwise"
    case .attach: "paperclip"
    case .shortcuts: "keyboard"
    case .photo: "photo"
    case .delete: "trash"
    case .remove: "minus.circle"
    case .dock: "macwindow"
    case .overlay: "pip"
    case .success: "checkmark"
    case .failure: "xmark"
    case .warning: "exclamationmark.triangle"
    case .error: "exclamationmark.circle"
    case .tip: "lightbulb"
    case .caution: "exclamationmark.octagon"
    case .diagnostics: "stethoscope"
    case .accountVerified: "person.crop.circle.badge.checkmark"
    case .chevronRight: "chevron.right"
    case .chevronDown: "chevron.down"
    case .chevronUpDown: "chevron.up.chevron.down"
    case .close: "xmark"
    case .check: "checkmark"
    case .more: "ellipsis"
    case .star: "star"
    case .starFill: "star.fill"
    case .checkboxOn: "checkmark.square.fill"
    case .checkboxOff: "square"
    case .checkCircleFill: "checkmark.circle.fill"
    case .circleEmpty: "circle"
    }
  }
}

/// Semantic system glyph, shared by the application surfaces.
struct CSIconView: View {
  let icon: CSIcon
  var size: CGFloat = 13
  var weight: CSIconWeight = .regular
  var color: Color? = nil

  var body: some View {
    let glyph = Image(systemName: icon.systemName)
      .symbolVariant(weight == .fill ? .fill : .none)
      .font(.system(size: size, weight: weight.font))
    if let color {
      glyph.foregroundStyle(color)
    } else {
      glyph
    }
  }
}
