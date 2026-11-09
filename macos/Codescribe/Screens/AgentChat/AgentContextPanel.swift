import AppKit
import SwiftUI

/// User-facing copy for the conversation header and the context card.
enum AgentContextPanelCopy {
  static var contextButton: String {
    String(localized: "Context", comment: "Agent header button that opens the context panel")
  }

  static var openHelp: String {
    String(
      localized: "Show this conversation's context and tool settings",
      comment: "Tooltip on the Context button while the context panel is closed")
  }

  static var closeHelp: String {
    String(
      localized: "Close the context panel",
      comment: "Tooltip and accessibility label of the context panel's close button")
  }

  static var conversationSection: String {
    String(
      localized: "This conversation",
      comment: "Context panel section about the selected conversation")
  }

  static var toolsSection: String {
    String(localized: "Tools", comment: "Context panel section that links to tool settings")
  }

  static var toolSettings: String {
    String(
      localized: "Tool settings",
      comment: "Context panel row that opens Settings › Agent › Tools")
  }

  static var toolSettingsHelp: String {
    String(
      localized: "Open Settings › Agent › Tools",
      comment: "Tooltip on the Tool settings row of the context panel")
  }

  static var queuedFact: String {
    String(localized: "Queued", comment: "Context panel fact: accepted turns waiting to run")
  }

  static var messagesFact: String {
    String(
      localized: "Messages",
      comment: "Context panel fact: how many turns the conversation holds")
  }

  static var updatedFact: String {
    String(
      localized: "Last updated",
      comment: "Context panel fact: when the conversation last changed")
  }
}

// MARK: - Conversation header

/// The conversation's identity, in the window bar beside the wordmark: the
/// thread title with the live status, and "Agent · model" under it. It lived
/// in a row of its own under the bar for one build; the founders asked for one
/// bar of the same height as the Settings window (2026-10-21), so the row
/// folded into the bar and its controls (Voice Lab mark, width, Context) sit
/// in the bar's trailing cluster. Nothing else repeats it.
struct AgentConversationHeader: View {
  @ObservedObject var store: AgentChatStore

  /// Long titles truncate here instead of pushing the trailing cluster.
  static let maxWidth: CGFloat = 360

  var body: some View {
    VStack(alignment: .leading, spacing: 2) {
      HStack(spacing: 8) {
        Text(verbatim: title)
          .font(CSFont.ui(14, .semibold))
          .foregroundStyle(Color.primary)
          .lineLimit(1)
          .truncationMode(.tail)
          .layoutPriority(1)
        AgentLiveStatusPill(activity: store.selectedHeaderActivity)
          .layoutPriority(2)
      }
      Text(verbatim: Self.identityLine(model: store.selectedModelIdentity))
        .font(CSFont.mono(10, .medium))
        .foregroundStyle(CSColor.secondaryInk)
        .lineLimit(1)
        .truncationMode(.tail)
    }
    .frame(maxWidth: Self.maxWidth, alignment: .leading)
    .accessibilityElement(children: .combine)
  }

  private var title: String {
    store.currentThread.map { ThreadRowTitle.labelTitle(for: $0) } ?? "—"
  }

  /// "Agent · grok-4.5" when the thread carries a model, "Agent" otherwise.
  /// The store supplies the active turn's accepted model, then its historical
  /// value after that execution ends; this line owns no model selection.
  static func identityLine(model: String?) -> String {
    if let model = model?.trimmingCharacters(in: .whitespacesAndNewlines), !model.isEmpty {
      return String(
        localized: "Agent · \(model)",
        comment: "Conversation header subtitle; the placeholder is the conversation's model")
    }
    return String(localized: "Agent", comment: "Conversation header subtitle without a model")
  }
}

/// Header phase comes from the store's lifecycle projection. Message
/// `isStreaming` stays sticky for the caret and must not choose this label.
struct AgentLiveStatusPill: View {
  let activity: AgentHeaderActivity

  var body: some View {
    switch activity {
    case .idle:
      EmptyView()
    case .thinking:
      StatusPill(text: activity.statusLabel ?? "", color: CSColor.amberLight, rippling: true)
    case .answering:
      StatusPill(text: activity.statusLabel ?? "", color: CSColor.secondaryInk, rippling: true)
    case .usingTools:
      StatusPill(text: activity.statusLabel ?? "", color: CSColor.secondaryInk, rippling: true)
    case .awaitingApproval:
      StaticStatusPill(text: activity.statusLabel ?? "", color: CSColor.amberLight)
    case .stopping:
      StaticStatusPill(text: activity.statusLabel ?? "", color: CSColor.textTertiary)
    case .failed:
      StaticStatusPill(text: activity.statusLabel ?? "", color: CSColor.dangerLight)
    }
  }
}

/// One-click conversation width, beside Context: the founders use the width
/// often enough that the submenu under "More actions" was too far away
/// (2026-10-21). Cycles Standard → Wide → Full width over the same persisted
/// preference the menu writes; the menu stays as the place that names them.
struct AgentWidthModeButton: View {
  @AppStorage(ChatLayoutPolicy.defaultsKey) private var widthModeRaw = ChatLayoutPolicy.defaultMode
    .rawValue

  private var mode: ChatWidthMode { ChatWidthMode.resolve(widthModeRaw) }

  var body: some View {
    Button {
      widthModeRaw = mode.next.rawValue
    } label: {
      CSIconView(icon: .conversationWidth, size: 12, weight: .medium)
        .foregroundStyle(CSColor.secondaryInk)
        .frame(width: 26, height: 22)
        .overlay(
          Capsule(style: .continuous)
            .strokeBorder(Color.primary.opacity(CSAlpha.line), lineWidth: 1)
        )
        .contentShape(Rectangle())
    }
    .buttonStyle(.borderless)
    .csFocusRing(cornerRadius: CSRadius.window)
    .fixedSize()
    .help(mode.label)
    .accessibilityLabel(
      String(localized: "Conversation width", comment: "Submenu of the Agent header menu")
    )
    .accessibilityValue(mode.label)
    .accessibilityIdentifier("agent-width-mode")
  }
}

/// "Context": opens and closes the context card, in the pin toggle's style
/// (accent fill and stroke while on). A click on it while the card is open
/// closes the card; the outside-click rule leaves this button to its action.
struct AgentContextToggle: View {
  @ObservedObject var presenter: AgentContextCardPresenter
  @FocusState private var isFocused: Bool
  @AccessibilityFocusState private var isAccessibilityFocused: Bool

  var body: some View {
    let isOpen = presenter.isOpen
    Button {
      presenter.toggle()
    } label: {
      HStack(spacing: 5) {
        CSIconView(icon: .info, size: 12, weight: .medium)
        Text(AgentContextPanelCopy.contextButton)
          .font(CSFont.ui(12, .medium))
          .lineLimit(1)
      }
      .foregroundStyle(isOpen ? CSColor.chromeAccent : CSColor.secondaryInk)
      .padding(.horizontal, 8)
      .frame(height: 22)
      .background(
        Capsule(style: .continuous)
          .fill(isOpen ? CSColor.chromeAccent.opacity(0.16) : Color.clear)
      )
      .overlay(
        Capsule(style: .continuous)
          .strokeBorder(
            isOpen ? CSColor.chromeAccent.opacity(0.32) : Color.primary.opacity(CSAlpha.line),
            lineWidth: 1)
      )
      .contentShape(Rectangle())
      .background(AgentContextHitAnchor { presenter.registerToggle($0) })
    }
    // The focus-ring style is the button's only style: a `.borderless` set
    // closer to the button would win over it, and with the native effect
    // disabled keyboard focus on Context would be invisible.
    .csFocusRing(cornerRadius: CSRadius.window)
    .focused($isFocused)
    .accessibilityFocused($isAccessibilityFocused)
    // A keyboard or VoiceOver close removed the focused control with the
    // card; the button that opened it takes focus back.
    .onChange(of: presenter.keyboardFocusReturn) { _, _ in isFocused = true }
    .onChange(of: presenter.accessibilityFocusReturn) { _, _ in isAccessibilityFocused = true }
    .fixedSize()
    .help(isOpen ? AgentContextPanelCopy.closeHelp : AgentContextPanelCopy.openHelp)
    .accessibilityLabel(AgentContextPanelCopy.contextButton)
    .accessibilityValue(
      isOpen
        ? String(localized: "On", comment: "Toggle state")
        : String(localized: "Off", comment: "Toggle state")
    )
    .accessibilityAddTraits(.isToggle)
    .accessibilityAddTraits(isOpen ? .isSelected : [])
    .accessibilityIdentifier("agent-context-toggle")
  }
}

// MARK: - Context card

/// Open/closed state of the Agent context card and the rules that close it.
///
/// View state only: no thread, draft, scroll position, model, recording or
/// agent call reads or writes it, and it is never persisted. It starts closed
/// in every new window and closes when the window is closed or ordered out,
/// so showing the Agent window again shows it without the card, while
/// re-activating a window that stayed on screen leaves the card as it was.
///
/// While open, plain Escape in this window closes the card first (after any
/// text a field is still composing), and a click anywhere outside the visible
/// card closes it and still reaches its target. A keyboard or VoiceOver close
/// that removes the focused control hands focus back to the Context button in
/// the same window; a click or a hide never moves focus.
@MainActor
final class AgentContextCardPresenter: ObservableObject {
  @Published private(set) var isOpen = false
  /// Bumped when a keyboard close should return focus to the Context button.
  @Published private(set) var keyboardFocusReturn = 0
  /// Bumped when a VoiceOver close should return its cursor to the button.
  @Published private(set) var accessibilityFocusReturn = 0
  /// Observation of the card's own controls, read when it closes; the
  /// controls stay the owners of focus.
  var cardHoldsKeyboardFocus = false

  /// How a close came about, which decides whether focus goes back.
  enum CloseCause {
    /// The button, a click outside, or the window leaving the screen.
    case pointerOrHide
    /// Escape while the card's control had keyboard focus.
    case keyboard
    /// VoiceOver's escape gesture on the card.
    case accessibility
  }

  private weak var window: NSWindow?
  private weak var toggleAnchor: NSView?
  private weak var cardAnchor: NSView?
  // Removed in deinit, which is nonisolated; touched only on the main actor.
  nonisolated(unsafe) private var eventMonitor: Any?
  nonisolated(unsafe) private var windowObservers: [NSObjectProtocol] = []

  deinit {
    if let eventMonitor { NSEvent.removeMonitor(eventMonitor) }
    for observer in windowObservers {
      NotificationCenter.default.removeObserver(observer)
    }
  }

  func toggle() {
    isOpen ? close() : open()
  }

  func open() {
    guard !isOpen else { return }
    cardHoldsKeyboardFocus = false
    isOpen = true
    installEventMonitor()
  }

  func close(_ cause: CloseCause = .pointerOrHide) {
    guard isOpen else { return }
    let returnKeyboard = cause == .keyboard && cardHoldsKeyboardFocus
    cardHoldsKeyboardFocus = false
    isOpen = false
    removeEventMonitor()
    // Focus goes back only inside the key window that owns both the card and
    // the button; in full screen the button lives in another window, and
    // focus is never moved into a window the person is not using.
    guard canReturnFocus else { return }
    if returnKeyboard { keyboardFocusReturn &+= 1 }
    if cause == .accessibility { accessibilityFocusReturn &+= 1 }
  }

  private var canReturnFocus: Bool {
    guard let window, window.isKeyWindow else { return false }
    return toggleAnchor?.window === window
  }

  /// The Context button's frame, for the outside-click rule. The button lives
  /// in the titlebar accessory, which AppKit can host in a window of its own
  /// (the full-screen toolbar window), so it never decides which window the
  /// card belongs to; a click there is another window's event and reaches
  /// the button, whose own action closes the card.
  func registerToggle(_ view: NSView) {
    toggleAnchor = view
  }

  /// The card's frame, for the outside-click rule. The card's window is the
  /// one whose Escape, clicks, close and order-out apply to it.
  func registerCard(_ view: NSView) {
    cardAnchor = view
    attach(to: view.window)
  }

  /// Applies one window event while the card is open. Returns `false` when
  /// the event is consumed (Escape); clicks are never consumed.
  func handle(_ event: AgentContextCardEvent) -> Bool {
    guard isOpen, let window, event.windowNumber == window.windowNumber else { return true }
    switch event.kind {
    case .escape:
      // Escape first ends text being composed (input methods, press-and-hold
      // accents) in the focused field; only a later Escape is the card's.
      if (window.firstResponder as? NSTextInputClient)?.hasMarkedText() == true { return true }
      close(.keyboard)
      return false
    case .click(let point):
      if contains(cardAnchor, point) || contains(toggleAnchor, point) { return true }
      close()
      return true
    case .other:
      return true
    }
  }

  /// The card anchor spans the visible card — or, on a short window, the
  /// scroll viewport — so a click on the conversation beside a scrolled card
  /// is outside it.
  private func contains(_ anchor: NSView?, _ pointInWindow: NSPoint) -> Bool {
    guard let anchor, anchor.window === window else { return false }
    return anchor.bounds.contains(anchor.convert(pointInWindow, from: nil))
  }

  private func attach(to newWindow: NSWindow?) {
    guard let newWindow, newWindow !== window else { return }
    for observer in windowObservers {
      NotificationCenter.default.removeObserver(observer)
    }
    window = newWindow
    let center = NotificationCenter.default
    let closeOnHide: @Sendable (Notification) -> Void = { [weak self] _ in
      MainActor.assumeIsolated { self?.close() }
    }
    // Close and order-out are the two ways the Agent window leaves the
    // screen; activation and occlusion by other windows are not hides.
    windowObservers = [
      center.addObserver(
        forName: NSWindow.willCloseNotification, object: newWindow, queue: .main,
        using: closeOnHide),
      center.addObserver(
        forName: HidingWindow.didOrderOutNotification, object: newWindow, queue: .main,
        using: closeOnHide),
    ]
  }

  private func installEventMonitor() {
    guard eventMonitor == nil else { return }
    eventMonitor = NSEvent.addLocalMonitorForEvents(
      matching: [.leftMouseDown, .rightMouseDown, .otherMouseDown, .keyDown]
    ) { [weak self] event in
      // Local monitors deliver on the main thread; reduce the non-Sendable
      // NSEvent to value fields before assuming the main actor.
      let reduced = AgentContextCardEvent(event)
      let passes = MainActor.assumeIsolated { self?.handle(reduced) ?? true }
      return passes ? event : nil
    }
  }

  private func removeEventMonitor() {
    if let eventMonitor { NSEvent.removeMonitor(eventMonitor) }
    eventMonitor = nil
  }
}

/// The parts of a window event the card's close rules read.
struct AgentContextCardEvent: Sendable {
  enum Kind: Sendable, Equatable {
    case escape
    case click(NSPoint)
    case other
  }

  let windowNumber: Int
  let kind: Kind

  init(windowNumber: Int, kind: Kind) {
    self.windowNumber = windowNumber
    self.kind = kind
  }

  init(_ event: NSEvent) {
    windowNumber = event.windowNumber
    switch event.type {
    case .keyDown where event.keyCode == 53:
      // Only plain Escape (Shift aside) dismisses; a chorded Escape is not ours.
      let chord = event.modifierFlags.intersection([.command, .option, .control])
      kind = chord.isEmpty ? .escape : .other
    case .leftMouseDown, .rightMouseDown, .otherMouseDown:
      kind = .click(event.locationInWindow)
    default:
      kind = .other
    }
  }
}

/// An inert AppKit view that reports where a SwiftUI control sits in its
/// window. It never takes hits, focus or accessibility.
struct AgentContextHitAnchor: NSViewRepresentable {
  let register: (NSView) -> Void

  func makeNSView(context: Context) -> AnchorView {
    let view = AnchorView()
    view.onWindow = register
    return view
  }

  func updateNSView(_ view: AnchorView, context: Context) {
    view.onWindow = register
  }

  final class AnchorView: NSView {
    var onWindow: ((NSView) -> Void)?

    override func viewDidMoveToWindow() {
      super.viewDidMoveToWindow()
      if window != nil { onWindow?(self) }
    }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func isAccessibilityElement() -> Bool { false }
  }
}

/// "Context": a compact card under the window header, at the trailing edge of
/// the conversation, opened on demand from the header's Context button. It
/// floats over the conversation without taking width from it, and shows only
/// facts the header does not: messages, last update, queued turns when there
/// are any, plus the way to tool settings. The conversation's title, model and
/// live activity stay in the header; the card controls no running work.
struct AgentContextCard: View {
  @ObservedObject var store: AgentChatStore
  @ObservedObject var presenter: AgentContextCardPresenter
  let openToolSettings: () -> Void
  @Environment(\.colorScheme) private var colorScheme
  @FocusedValue(\.agentContextCardFocus) private var focusedPresenter

  /// Preferred width; the card narrows to the space the conversation has.
  static let preferredWidth: CGFloat = 280
  /// Gap to the header above and to the conversation's trailing edge.
  static let inset: CGFloat = CSSpace.md

  var body: some View {
    VStack(alignment: .leading, spacing: CSSpace.md) {
      Text(AgentContextPanelCopy.contextButton)
        .font(CSFont.ui(13, .semibold))
        .foregroundStyle(Color.primary)
        .lineLimit(1)
        .accessibilityAddTraits(.isHeader)
      if let thread = store.currentThread, hasFacts(thread) {
        section(AgentContextPanelCopy.conversationSection) {
          facts(thread)
        }
      }
      section(AgentContextPanelCopy.toolsSection) {
        toolSettingsRow
      }
    }
    .padding(CSSpace.card)
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(CSColor.sheet)
    .clipShape(RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous))
    .overlay(
      RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
        .strokeBorder(Color.primary.opacity(CSAlpha.stroke), lineWidth: 1)
        .allowsHitTesting(false)
    )
    // The overlay canvas's floating-surface shadow, so both floating sheets
    // sit at the same height above their content.
    .shadow(
      color: .black.opacity(colorScheme == .dark ? 0.20 : 0.16), radius: 20, x: 0, y: 9
    )
    .onChange(of: focusedPresenter === presenter, initial: true) { _, focused in
      presenter.cardHoldsKeyboardFocus = focused
    }
    .accessibilityElement(children: .contain)
    .accessibilityLabel(AgentContextPanelCopy.contextButton)
    // VoiceOver's escape gesture closes the card like the Escape key does.
    .accessibilityAction(.escape) { presenter.close(.accessibility) }
    .accessibilityIdentifier("agent-context-card")
  }

  private func hasFacts(_ thread: ChatThread) -> Bool {
    Self.knownMessageCount(of: thread) != nil || thread.updatedAt != nil
      || !store.queuedTurns(in: thread.id).isEmpty
  }

  /// The thread's message count when it is known, zero included. A persisted
  /// thread whose history has not been read yet (the store loads it lazily)
  /// has no known count, so the card shows none rather than a false zero.
  static func knownMessageCount(of thread: ChatThread) -> Int? {
    if !thread.messages.isEmpty || thread.messagesLoaded || thread.backendId == nil {
      return thread.messages.count
    }
    return nil
  }

  private func section<Content: View>(
    _ title: String, @ViewBuilder content: () -> Content
  ) -> some View {
    VStack(alignment: .leading, spacing: CSSpace.xs) {
      Text(title)
        .font(CSFont.ui(11, .semibold))
        .foregroundStyle(CSColor.secondaryInk)
        .accessibilityAddTraits(.isHeader)
      content()
    }
  }

  @ViewBuilder
  private func facts(_ thread: ChatThread) -> some View {
    VStack(alignment: .leading, spacing: CSSpace.xs) {
      if let count = Self.knownMessageCount(of: thread) {
        fact(AgentContextPanelCopy.messagesFact) {
          Text(count, format: .number)
        }
        .accessibilityIdentifier("agent-context-messages")
      }
      if let updatedAt = thread.updatedAt {
        fact(AgentContextPanelCopy.updatedFact) {
          MessageTimestampLabel(date: updatedAt)
        }
        .accessibilityIdentifier("agent-context-updated")
      }
      let queued = store.queuedTurns(in: thread.id).count
      if queued > 0 {
        fact(AgentContextPanelCopy.queuedFact) {
          Text(queued, format: .number)
        }
        .accessibilityIdentifier("agent-context-queued-count")
      }
    }
  }

  /// Label and value on one line while both fit; on a narrow card or with a
  /// long localized label the value moves under its label instead of being
  /// truncated or pushed past the edge.
  private func fact<Value: View>(_ label: String, @ViewBuilder value: () -> Value) -> some View {
    let value = value()
    return ViewThatFits(in: .horizontal) {
      HStack(alignment: .firstTextBaseline, spacing: CSSpace.sm) {
        factLabel(label).fixedSize()
        Spacer(minLength: CSSpace.sm)
        factValue(value).fixedSize()
      }
      VStack(alignment: .leading, spacing: 2) {
        factLabel(label).fixedSize(horizontal: false, vertical: true)
        factValue(value).fixedSize(horizontal: false, vertical: true)
      }
    }
    .accessibilityElement(children: .combine)
  }

  private func factLabel(_ label: String) -> some View {
    Text(label)
      .font(CSFont.ui(12, .regular))
      .foregroundStyle(CSColor.secondaryInk)
  }

  private func factValue<Value: View>(_ value: Value) -> some View {
    value
      .font(CSFont.metaMono)
      .foregroundStyle(Color.primary)
  }

  private var toolSettingsRow: some View {
    Button(action: openToolSettings) {
      HStack(spacing: CSSpace.sm) {
        CSIconView(icon: .settings, size: 12, color: CSColor.secondaryInk)
        Text(AgentContextPanelCopy.toolSettings)
          .font(CSFont.ui(13, .medium))
          .fixedSize(horizontal: false, vertical: true)
        Spacer(minLength: CSSpace.sm)
        CSIconView(
          icon: .chevronRight, size: 9, weight: .semibold, color: CSColor.textTertiary)
      }
      .padding(.horizontal, CSSpace.sm)
      .padding(.vertical, CSSpace.xs)
      .frame(maxWidth: .infinity, alignment: .leading)
      .contentShape(Rectangle())
    }
    .buttonStyle(.borderless)
    .conversationActionHover(
      restingColor: Color.primary, hoverColor: Color.primary,
      hoverBackground: Color.primary.opacity(CSAlpha.fill)
    )
    // Publishes "focus is in this card" without a second focus binding beside
    // the hover modifier's own focus outline.
    .focusedValue(\.agentContextCardFocus, presenter)
    .padding(.horizontal, -CSSpace.sm)
    .help(AgentContextPanelCopy.toolSettingsHelp)
    .accessibilityLabel(AgentContextPanelCopy.toolSettings)
    .accessibilityIdentifier("agent-context-tool-settings")
  }
}

/// The presenter whose card holds keyboard focus, published by the card's
/// focusable control and read by the card itself.
private struct AgentContextCardFocusKey: FocusedValueKey {
  typealias Value = AgentContextCardPresenter
}

extension FocusedValues {
  fileprivate var agentContextCardFocus: AgentContextCardPresenter? {
    get { self[AgentContextCardFocusKey.self] }
    set { self[AgentContextCardFocusKey.self] = newValue }
  }
}

/// Places the card under the header at the conversation's trailing edge and
/// fits it to the space there: narrower than its preferred width when the
/// column is narrow, and scrolling inside itself when the window is short.
/// The layer takes no hits outside the card.
struct AgentContextCardLayer: View {
  @ObservedObject var store: AgentChatStore
  @ObservedObject var presenter: AgentContextCardPresenter
  let openToolSettings: () -> Void
  @Environment(\.accessibilityReduceMotion) private var reduceMotion

  var body: some View {
    GeometryReader { geometry in
      let inset = AgentContextCard.inset
      let width = max(0, min(AgentContextCard.preferredWidth, geometry.size.width - 2 * inset))
      let maxHeight = max(0, geometry.size.height - 2 * inset)
      if presenter.isOpen {
        ViewThatFits(in: .vertical) {
          card
          ScrollView { card.padding(.bottom, 1) }
            .frame(maxHeight: maxHeight)
        }
        .frame(width: width)
        // The visible card, or the scroll viewport on a short window — not
        // the scrolled content, whose clipped part lies over the conversation.
        .background(AgentContextHitAnchor { presenter.registerCard($0) })
        .frame(maxHeight: maxHeight, alignment: .top)
        .padding(.top, inset)
        .padding(.trailing, inset)
        .frame(
          width: geometry.size.width, height: geometry.size.height, alignment: .topTrailing
        )
        .transition(
          reduceMotion
            ? .opacity : .opacity.combined(with: .scale(scale: 0.97, anchor: .topTrailing)))
      }
    }
    // A closing card leaves the hit and accessibility trees at once, not when
    // its fade ends.
    .allowsHitTesting(presenter.isOpen)
    .accessibilityHidden(!presenter.isOpen)
    .animation(.easeOut(duration: CSMotion.Tempo.hover), value: presenter.isOpen)
  }

  private var card: some View {
    AgentContextCard(store: store, presenter: presenter, openToolSettings: openToolSettings)
  }
}
