import SwiftUI

/// Persisted open/closed state of the Agent context panel. Per app, closed by
/// default (Founder decision, 2026-10-20). It is view state only: no thread,
/// draft, model, recording or agent call reads or writes it.
enum AgentContextPanelPreference {
  static let defaultsKey = "AgentChat.contextPanelOpen.v1"
  static let defaultIsOpen = false
}

/// User-facing copy for the conversation header and the context panel.
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

  static var panelTitle: String {
    String(localized: "Agent context", comment: "Title of the Agent window's context panel")
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

  static var modelFact: String {
    String(localized: "Model", comment: "Context panel fact: the conversation's model")
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

/// The conversation's one identity row, at the top of the conversation column
/// under the toolbar: the thread title, "Agent · model", the live status, the
/// Voice Lab mark when it applies, and the Context entry. The toolbar does not
/// repeat any of it.
struct AgentConversationHeader: View {
  @ObservedObject var store: AgentChatStore
  @Binding var isContextOpen: Bool

  var body: some View {
    HStack(alignment: .center, spacing: 10) {
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
        Text(verbatim: Self.identityLine(model: store.currentThread?.model))
          .font(CSFont.mono(10, .medium))
          .foregroundStyle(Color.secondary)
          .lineLimit(1)
          .truncationMode(.tail)
      }
      .layoutPriority(1)

      Spacer(minLength: 8)

      DeveloperPowerMark()
        .fixedSize()
      AgentContextToggle(isOpen: $isContextOpen)
    }
    .padding(.horizontal, ChatLayoutPolicy.listPadding)
    .frame(maxWidth: .infinity)
    .frame(height: AgentWindowMetrics.conversationHeaderHeight)
  }

  private var title: String {
    store.currentThread.map { ThreadRowTitle.labelTitle(for: $0) } ?? "—"
  }

  /// "Agent · grok-4.5" when the thread carries a model, "Agent" otherwise.
  /// Shows only what the thread already stores; there is no model switcher.
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
      StatusPill(text: activity.statusLabel ?? "", color: CSColor.amber, rippling: true)
    case .answering:
      StatusPill(text: activity.statusLabel ?? "", color: CSColor.terracotta, rippling: true)
    case .usingTools:
      StatusPill(text: activity.statusLabel ?? "", color: CSColor.oliveLight, rippling: true)
    case .awaitingApproval:
      StaticStatusPill(text: activity.statusLabel ?? "", color: CSColor.amber)
    case .stopping:
      StaticStatusPill(text: activity.statusLabel ?? "", color: CSColor.textTertiary)
    case .failed:
      StaticStatusPill(text: activity.statusLabel ?? "", color: CSColor.terracotta)
    }
  }
}

/// "Context": opens and closes the panel. It writes only the persisted flag,
/// in the pin toggle's style (accent fill and stroke while on).
struct AgentContextToggle: View {
  @Binding var isOpen: Bool

  var body: some View {
    Button {
      isOpen.toggle()
    } label: {
      HStack(spacing: 5) {
        CSIconView(icon: .info, size: 12, weight: .medium)
        Text(AgentContextPanelCopy.contextButton)
          .font(CSFont.ui(12, .medium))
          .lineLimit(1)
      }
      .foregroundStyle(isOpen ? CSColor.chromeAccent : Color.secondary)
      .padding(.horizontal, 8)
      .frame(height: 22)
      .background(
        RoundedRectangle(cornerRadius: CSRadius.pill, style: .continuous)
          .fill(isOpen ? CSColor.chromeAccent.opacity(0.16) : Color.clear)
      )
      .overlay(
        RoundedRectangle(cornerRadius: CSRadius.pill, style: .continuous)
          .strokeBorder(
            isOpen ? CSColor.chromeAccent.opacity(0.32) : Color.primary.opacity(0.09),
            lineWidth: 1)
      )
      .contentShape(Rectangle())
    }
    .buttonStyle(.borderless)
    .csFocusRing(cornerRadius: CSRadius.pill)
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

// MARK: - Context panel

/// "Agent context": a fixed-width trailing column with what the selected
/// conversation already stores and one way into the tool settings. It owns no
/// state and starts nothing: every fact comes from `ChatThread`, and the tools
/// row only opens Settings › Agent › Tools.
struct AgentContextPanel: View {
  let thread: ChatThread?
  let close: () -> Void
  let openToolSettings: () -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      header
      Divider()
      VStack(alignment: .leading, spacing: 18) {
        if let thread {
          section(AgentContextPanelCopy.conversationSection) {
            conversationCard(thread)
          }
        }
        section(AgentContextPanelCopy.toolsSection) {
          toolSettingsRow
        }
      }
      .padding(.horizontal, 14)
      .padding(.top, 14)
      Spacer(minLength: 0)
    }
    // A fixed width, not a minimum: a long title wraps inside the column and
    // never widens it (Founder, 2026-10-20).
    .frame(width: AgentWindowMetrics.contextPanelWidth)
    .frame(maxHeight: .infinity, alignment: .top)
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("agent-context-panel")
  }

  private var header: some View {
    HStack(spacing: 8) {
      Text(AgentContextPanelCopy.panelTitle)
        .font(CSFont.ui(13, .semibold))
        .foregroundStyle(Color.primary)
        .lineLimit(1)
        .truncationMode(.tail)
        .accessibilityAddTraits(.isHeader)
      Spacer(minLength: 8)
      Button(action: close) {
        CSIconView(icon: .close, size: 11, weight: .semibold, color: CSColor.textTertiary)
          .frame(width: 24, height: 20)
          .contentShape(Rectangle())
      }
      .buttonStyle(.borderless)
      .csFocusRing(cornerRadius: CSRadius.pill)
      .help(AgentContextPanelCopy.closeHelp)
      .accessibilityLabel(AgentContextPanelCopy.closeHelp)
    }
    .padding(.horizontal, 14)
    .frame(height: AgentWindowMetrics.conversationHeaderHeight)
  }

  private func section<Content: View>(
    _ title: String, @ViewBuilder content: () -> Content
  ) -> some View {
    VStack(alignment: .leading, spacing: 7) {
      Text(title)
        .font(CSFont.ui(11, .semibold))
        .foregroundStyle(Color.secondary)
        .accessibilityAddTraits(.isHeader)
      content()
    }
  }

  private func conversationCard(_ thread: ChatThread) -> some View {
    VStack(alignment: .leading, spacing: 8) {
      HStack(alignment: .firstTextBaseline, spacing: 8) {
        CSIconView(icon: .agent, size: 12, color: CSColor.textTertiary)
        Text(verbatim: ThreadRowTitle.labelTitle(for: thread))
          .font(CSFont.ui(12.5, .medium))
          .foregroundStyle(Color.primary)
          .lineLimit(3)
          .truncationMode(.tail)
          .fixedSize(horizontal: false, vertical: true)
          .frame(maxWidth: .infinity, alignment: .leading)
      }
      if let model = Self.model(of: thread) {
        fact(AgentContextPanelCopy.modelFact) {
          Text(verbatim: model)
        }
      }
      if !thread.messages.isEmpty {
        fact(AgentContextPanelCopy.messagesFact) {
          Text(verbatim: "\(thread.messages.count)")
        }
      }
      if let updatedAt = thread.updatedAt {
        fact(AgentContextPanelCopy.updatedFact) {
          MessageTimestampLabel(date: updatedAt)
        }
      }
    }
    .padding(.horizontal, 11)
    .padding(.vertical, 10)
    .frame(maxWidth: .infinity, alignment: .leading)
    .fixedSize(horizontal: false, vertical: true)
    .modifier(AgentContextCardSurface())
  }

  private func fact<Value: View>(_ label: String, @ViewBuilder value: () -> Value) -> some View {
    HStack(alignment: .firstTextBaseline, spacing: 8) {
      Text(label)
        .font(CSFont.ui(11, .regular))
        .foregroundStyle(Color.secondary)
        .lineLimit(1)
      Spacer(minLength: 8)
      value()
        .font(CSFont.mono(10.5, .medium))
        .foregroundStyle(Color.primary)
        .lineLimit(1)
        .truncationMode(.middle)
    }
    .accessibilityElement(children: .combine)
  }

  private var toolSettingsRow: some View {
    Button(action: openToolSettings) {
      HStack(spacing: 8) {
        CSIconView(icon: .settings, size: 12, color: Color.secondary)
        Text(AgentContextPanelCopy.toolSettings)
          .font(CSFont.ui(12.5, .medium))
          .foregroundStyle(Color.primary)
          .lineLimit(2)
          .fixedSize(horizontal: false, vertical: true)
        Spacer(minLength: 8)
        CSIconView(
          icon: .chevronRight, size: 9, weight: .semibold, color: CSColor.textTertiary)
      }
      .padding(.horizontal, 11)
      .padding(.vertical, 10)
      .frame(maxWidth: .infinity, alignment: .leading)
      .modifier(AgentContextCardSurface())
      .contentShape(Rectangle())
    }
    .csFocusRing(cornerRadius: CSRadius.card)
    .help(AgentContextPanelCopy.toolSettingsHelp)
    .accessibilityIdentifier("agent-context-tool-settings")
  }

  static func model(of thread: ChatThread) -> String? {
    guard let model = thread.model?.trimmingCharacters(in: .whitespacesAndNewlines),
      !model.isEmpty
    else { return nil }
    return model
  }
}

/// The card surface the conversation already uses for tool activity and
/// queued turns: a faint fill and a hairline on the card radius.
private struct AgentContextCardSurface: ViewModifier {
  func body(content: Content) -> some View {
    content
      .background(Color.primary.opacity(0.04))
      .overlay(
        RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous)
          .strokeBorder(Color.primary.opacity(0.07), lineWidth: 1)
      )
      .clipShape(RoundedRectangle(cornerRadius: CSRadius.card, style: .continuous))
  }
}
