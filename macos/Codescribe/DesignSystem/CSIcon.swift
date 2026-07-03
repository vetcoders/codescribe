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
    /// invisible to call sites. `sf` is retained so a glyph can fall back to an
    /// SF Symbol if Phosphor ever lacks one; today the whole set is Phosphor so
    /// the UI reads as a single icon family (operator decision, 02-07-2026).
    enum Backend {
        case sf(String)
        case phosphor(Ph)
        case phosphorFill(Ph) // forces the fill weight — for genuinely filled-state glyphs
    }

    var backend: Backend {
        switch self {
        // Chrome / navigation
        case .settings: return .phosphor(.gear)
        case .setupWizard: return .phosphor(.sparkle)
        case .help: return .phosphor(.question)
        case .info: return .phosphor(.info)
        case .power: return .phosphor(.power)

        // Agent / capture
        case .agent: return .phosphor(.chatCircle) // opens the agent conversation — a chat bubble
        case .mic: return .phosphor(.microphone)
        case .record: return .phosphor(.record)
        case .stop: return .phosphor(.stop)

        // Content actions
        case .copy: return .phosphor(.copy)
        case .edit: return .phosphor(.pencilSimple)
        case .notes: return .phosphor(.notePencil)
        case .notesMode: return .phosphor(.note)
        case .history: return .phosphor(.clockCounterClockwise)
        case .search: return .phosphor(.magnifyingGlass)
        case .send: return .phosphor(.arrowUp)
        case .refresh: return .phosphor(.arrowClockwise)
        case .attach: return .phosphor(.paperclip)
        case .shortcuts: return .phosphor(.keyboard)
        case .photo: return .phosphor(.image)
        case .delete: return .phosphor(.trash)
        case .remove: return .phosphor(.minusCircle)

        // Window modes
        case .dock: return .phosphor(.appWindow)
        case .overlay: return .phosphor(.pictureInPicture)

        // Status / diagnostics
        case .success: return .phosphor(.check)
        case .failure: return .phosphor(.x)
        case .warning: return .phosphor(.warning)
        case .error: return .phosphor(.warningCircle)
        case .tip: return .phosphor(.lightbulb)
        case .caution: return .phosphor(.warningOctagon)
        case .diagnostics: return .phosphor(.stethoscope)
        case .accountVerified: return .phosphor(.userCircleCheck)

        // Affordances / selection
        case .chevronRight: return .phosphor(.caretRight)
        case .chevronDown: return .phosphor(.caretDown)
        case .chevronUpDown: return .phosphor(.caretUpDown)
        case .close: return .phosphor(.x)
        case .check: return .phosphor(.check)
        case .more: return .phosphor(.dotsThree)
        case .star: return .phosphor(.star)
        case .starFill: return .phosphorFill(.star)
        case .checkboxOn: return .phosphorFill(.checkSquare)
        case .checkboxOff: return .phosphor(.square)
        case .checkCircleFill: return .phosphorFill(.checkCircle)
        case .circleEmpty: return .phosphor(.circle)
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
    /// `nil` lets the glyph inherit the ambient `foregroundStyle` (matches call
    /// sites that tint a whole row). Phosphor assets ship with a template
    /// rendering intent, so — exactly like SF Symbols — they either inherit the
    /// ambient tint or accept an explicit one through `foregroundStyle`.
    var color: Color? = nil

    var body: some View {
        switch icon.backend {
        case .sf(let name):
            tinted(Image(systemName: name).font(.system(size: size, weight: weight.font)))
        case .phosphor(let ph):
            sized(ph.weight(weight.phosphor))
        case .phosphorFill(let ph):
            sized(ph.fill)
        }
    }

    /// A Phosphor glyph is sized to a square box (an SF Symbol gets its size
    /// from the font instead).
    @ViewBuilder private func sized(_ image: Image) -> some View {
        tinted(image.resizable().scaledToFit().frame(width: size, height: size))
    }

    /// Apply an explicit tint, or leave the view to inherit the ambient one.
    @ViewBuilder private func tinted(_ view: some View) -> some View {
        if let color {
            view.foregroundStyle(color)
        } else {
            view
        }
    }
}
