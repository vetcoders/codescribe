import Foundation

/// One contract per overlay layout. Klasyczny, Belka and Fiszki share their
/// views (`DictationOverlayView`, `DictationOverlayWindow`, `OverlayState`),
/// so a shared-surface decision must be owned by exactly one layout's
/// contract, named and tested there — never enforced as a global invariant
/// that another layout has to fight (Maciej, 2026-11-06: every view gets
/// what it fights for; the tug-of-war ends at the contract line).
///
/// A contract value is a decision record: its doc comment names who decided
/// and when. Changing one layout's value cannot change another layout's —
/// the per-layout test table in `OverlayLayoutContractTests` is the fence.
protocol OverlayLayoutContract: Sendable {
  var mode: OverlayIntercomPresentationMode { get }

  /// The expanded bar's agent map rides in its header.
  var mapInHeader: Bool { get }

  /// The map setting shapes mini: with the map, mini drops the brand word
  /// (Monika, 2026-11-04, bus brief §3).
  var mapInMini: Bool { get }

  /// The stacked counter beside the brand, the way to the Intercom group
  /// (Maciej, 2026-10-23). Currently off in every layout — the frontier
  /// ships the vertical counter through the Agents list instead; if the
  /// stacked face returns, it returns here, for one layout.
  var counterStacked: Bool { get }

  /// A take alone never resizes this layout's surface. Fiszki will opt out
  /// explicitly: recording start expands the overlay to waveform + clock
  /// (+ transcript view while marked active) — decyzja Macieja, 2026-11-06.
  /// Declared and pinned ahead of that cut; nothing reads it yet.
  var takeResizesSurface: Bool { get }

  /// The bar's waveform and clock keep their room at rest, so starting a
  /// take moves nothing. Maciej (2026-11-06) wants a micro resting overlay
  /// in classic and fiszki; flipping this value is that follow-up cut's
  /// entry point, not a side effect of other work. Nothing reads it yet.
  var meterVisibleAtRest: Bool { get }

  /// A still click on the collapsed overlay's free background opens the
  /// window on its last full view (agents or transcription), not the bar
  /// (Maciej, 2026-11-06, for Klasyczny and Fiszki). Belka keeps Monika's
  /// cycle (2026-11-04, bus brief §12.1): mini opens the bar, the bar folds
  /// back to mini, and the window opens only from the chevron, the Agents
  /// button and the menus.
  var collapsedClickOpensWindow: Bool { get }

  /// A click on the window's header returns to the compact form the window
  /// replaced, mini or the bar (Maciej, 2026-11-06, for Klasyczny and
  /// Fiszki). Belka keeps the bar as the header's one destination.
  var windowHeaderClickRestoresCompactForm: Bool { get }
}

/// Klasyczny (default): the proven strip. No agent map anywhere; the bar is
/// the 410 × 46 pt classic geometry and mini carries the brand word.
struct ClassicLayoutContract: OverlayLayoutContract {
  let mode = OverlayIntercomPresentationMode.classic
  let mapInHeader = false
  let mapInMini = false
  let counterStacked = false
  let takeResizesSurface = false
  let meterVisibleAtRest = true
  let collapsedClickOpensWindow = true
  let windowHeaderClickRestoresCompactForm = true
}

/// Belka (beta): the horizontal bar with the agent map in its header; mini
/// trades the brand word for the map (Monika, 2026-11-04, bus brief §3).
struct BarLayoutContract: OverlayLayoutContract {
  let mode = OverlayIntercomPresentationMode.horizontal
  let mapInHeader = true
  let mapInMini = true
  let counterStacked = false
  let takeResizesSurface = false
  let meterVisibleAtRest = true
  let collapsedClickOpensWindow = false
  let windowHeaderClickRestoresCompactForm = false
}

/// Fiszki (beta): the vertical Intercom of floating cards. The map lives in
/// the Intercom group, not in the bar or mini.
struct FiszkiLayoutContract: OverlayLayoutContract {
  let mode = OverlayIntercomPresentationMode.vertical
  let mapInHeader = false
  let mapInMini = false
  let counterStacked = false
  let takeResizesSurface = false
  let meterVisibleAtRest = true
  let collapsedClickOpensWindow = true
  let windowHeaderClickRestoresCompactForm = true
}

extension OverlayIntercomPresentationMode {
  /// The one place a mode becomes behavior.
  var contract: any OverlayLayoutContract {
    switch self {
    case .classic: ClassicLayoutContract()
    case .horizontal: BarLayoutContract()
    case .vertical: FiszkiLayoutContract()
    }
  }
}
