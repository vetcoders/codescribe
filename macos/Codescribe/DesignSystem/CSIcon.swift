import SwiftUI
import PhosphorSwift

// CSIcon — the single icon language for codescribe's UI.
//
// One semantic vocabulary for the whole app: call sites name *what an icon
// means* (`.settings`, `.record`, `.diagnostics`), never *which glyph or which
// backend* draws it. The backend split (Apple SF Symbols vs Phosphor) lives
// here and here only, so a glyph can be re-pointed per-icon later without
// touching a single screen.
//
// Backend policy (per SUBAGENT_07 research, operator-GREEN):
//   • SF Symbols — native, zero-bundle, baseline-aligned. Used for the utility
//     glyphs and for symbols the app already speaks (e.g. `gearshape` in the
//     overlay), so the language stays consistent across screens.
//   • Phosphor (MIT, first-party Swift pkg) — the brand stroke layer. Geometric,
//     mono-forward, tints cleanly to the terracotta/olive palette. Used where a
//     macOS-system glyph would fight the bespoke, anti-system vibe.
//
// Rendering is template/tintable in both branches: SF via `foregroundStyle`,
// Phosphor via its `.color(_:)` source-atop blend. Size and weight share one
// API surface (`CSIconWeight`) mapped onto each backend.

/// Unified icon weight, mapped onto both SF Symbols (font weight) and Phosphor
/// (`Ph.IconWeight`). Regular is the UI default; fill marks an active/brand
/// state; thin/light read as meta.
enum CSIconWeight {
    case thin, light, regular, medium, semibold, bold, fill

    var phosphor: Ph.IconWeight {
        switch self {
        case .thin: return .thin
        case .light: return .light
        case .regular: return .regular
        case .medium: return .regular // Phosphor has no medium; regular is the nearest step
        case .semibold: return .bold // Phosphor has no semibold; bold is the nearest step
        case .bold: return .bold
        case .fill: return .fill
        }
    }

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

/// Semantic icon set. Cases are derived from the app-wide emoji/dingbat
/// inventory (SUBAGENT_07). M1 wires the Tray; the remaining cases are mapped
/// and ready so later screens (Chat / Settings / Overlay / Onboarding) are a
/// pure wiring job.
enum CSIcon {
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

    /// Which library draws this semantic icon. Owned by the design system,
    /// invisible to call sites.
    enum Backend {
        case sf(String)
        case phosphor(Ph)
    }

    var backend: Backend {
        switch self {
        // Chrome / navigation
        case .settings: return .sf("gearshape") // matches the overlay's existing SF gear
        case .setupWizard: return .phosphor(.sparkle)
        case .help: return .sf("questionmark.circle")
        case .info: return .sf("info.circle")
        case .power: return .phosphor(.power)

        // Agent / capture
        case .agent: return .phosphor(.chatCircle) // opens the agent conversation — a chat bubble, not a grid

        case .mic: return .sf("mic")
        case .record: return .phosphor(.record)
        case .stop: return .phosphor(.stop)

        // Content actions
        case .copy: return .sf("doc.on.doc")
        case .edit: return .phosphor(.pencilSimple)
        case .notes: return .phosphor(.notePencil)
        case .notesMode: return .sf("note.text")
        case .history: return .phosphor(.clockCounterClockwise)
        case .search: return .sf("magnifyingglass")
        case .send: return .sf("arrow.up")
        case .refresh: return .sf("arrow.clockwise")
        case .attach: return .sf("paperclip")
        case .shortcuts: return .sf("keyboard")
        case .photo: return .sf("photo")
        case .delete: return .sf("trash")
        case .remove: return .sf("minus.circle")

        // Window modes
        case .dock: return .sf("macwindow")
        case .overlay: return .sf("rectangle.on.rectangle")

        // Status / diagnostics
        case .success: return .sf("checkmark")
        case .failure: return .sf("xmark")
        case .warning: return .sf("exclamationmark.triangle")
        case .error: return .sf("exclamationmark.circle")
        case .tip: return .sf("lightbulb")
        case .caution: return .sf("exclamationmark.octagon")
        case .diagnostics: return .phosphor(.stethoscope)
        case .accountVerified: return .sf("person.crop.circle.badge.checkmark")

        // Affordances / selection
        case .chevronRight: return .sf("chevron.right")
        case .chevronDown: return .sf("chevron.down")
        case .chevronUpDown: return .sf("chevron.up.chevron.down")
        case .close: return .sf("xmark")
        case .check: return .sf("checkmark")
        case .more: return .sf("ellipsis")
        case .star: return .sf("star")
        case .starFill: return .sf("star.fill")
        case .checkboxOn: return .sf("checkmark.square.fill")
        case .checkboxOff: return .sf("square")
        case .checkCircleFill: return .sf("checkmark.circle.fill")
        case .circleEmpty: return .sf("circle")
        }
    }
}

/// Renders a `CSIcon` at a uniform size/weight/color across both backends.
/// Call sites always go through this — never `Image(systemName:)` or `Ph.*`
/// directly — so the icon language stays single-sourced.
struct CSIconView: View {
    let icon: CSIcon
    var size: CGFloat = 13
    var weight: CSIconWeight = .regular
    /// `nil` lets an SF glyph inherit the ambient `foregroundStyle` (matches
    /// call sites that tint a whole row). Phosphor cannot inherit (it tints via
    /// a source-atop blend), so it falls back to `CSColor.textBody`.
    var color: Color? = nil

    var body: some View {
        switch icon.backend {
        case .sf(let name):
            if let color {
                Image(systemName: name)
                    .font(.system(size: size, weight: weight.font))
                    .foregroundStyle(color)
            } else {
                Image(systemName: name)
                    .font(.system(size: size, weight: weight.font))
            }
        case .phosphor(let ph):
            ph.weight(weight.phosphor)
                .frame(width: size, height: size)
                .color(color ?? CSColor.textBody)
        }
    }
}
