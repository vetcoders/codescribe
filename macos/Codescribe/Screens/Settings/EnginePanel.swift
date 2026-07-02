import SwiftUI

// Engine panel: runtime truth + engine controls. The key/value runtime rows are
// READ-ONLY (sourced from the live CsSettings snapshot, not hardcoded) and the
// permission matrix reflects live status. The "Engine controls" section below the
// runtime rows is editable (F1 layered transcription): STT engine selector +
// layered-transcription toggle, persisted through the promoted-key config router.

struct EnginePanel: View {
    @ObservedObject var model: SettingsViewModel

    private let matrixOrder: [PermissionKind] = [
        .microphone, .accessibility, .inputMonitoring, .screenRecording
    ]
    private let columns = [
        GridItem(.flexible(), spacing: 8),
        GridItem(.flexible(), spacing: 8)
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                EyebrowLabel(text: "Settings · Engine")
                Text("RUNTIME TRUTH · READ-ONLY ROWS")
                    .font(CSFont.mono(9, .medium))
                    .foregroundStyle(CSColor.textMutedAlt)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 2)
                    .background(
                        RoundedRectangle(cornerRadius: 6, style: .continuous)
                            .fill(CSColor.surfaceRaised(0.04))
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: 6, style: .continuous)
                            .strokeBorder(CSColor.hairline(0.08), lineWidth: 1)
                    )
            }

            Text("What's actually running.")
                .font(CSFont.ui(26, .bold))
                .tracking(-0.5)
                .foregroundStyle(CSColor.textHigh)
                .padding(.top, 6)

            runtimeRows
                .padding(.top, 20)

            SettingsSectionLabel("Engine controls")
                .padding(.top, 22)
            engineControls
                .padding(.top, 11)

            SettingsSectionLabel("Permission matrix")
                .padding(.top, 22)
            LazyVGrid(columns: columns, spacing: 8) {
                ForEach(matrixOrder) { kind in
                    PermissionMatrixCell(kind: kind, state: model.permissions.state(kind))
                }
            }
            .padding(.top, 11)

            HStack(spacing: 8) {
                Text("●").font(CSFont.mono(11, .medium)).foregroundStyle(CSColor.olive)
                Text("runtime rows reflect the live engine — engine controls apply from the next recording session")
                    .font(CSFont.mono(11, .medium))
                    .foregroundStyle(CSColor.textFaint)
            }
            .padding(.top, 16)

            AgentStatusSection(model: model)
                .padding(.top, 30)

            MCPServersSection(model: model)
                .padding(.top, 26)
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 24)
    }

    // MARK: Runtime key/value rows

    private var runtimeRows: some View {
        VStack(spacing: 0) {
            RuntimeRow(key: "Active STT", value: model.activeSTT,
                       tint: true, trailing: .dot(model.sttHealthy ? CSColor.oliveLight : CSColor.amber))
            divider
            RuntimeRow(key: "STT model", value: model.sttModelDescription,
                       tint: false, mono: true, trailing: .none)
            divider
            RuntimeRow(key: "Whisper language", value: model.whisperLanguageCode,
                       tint: true, mono: true, trailing: .none)
            divider
            RuntimeRow(key: "AI formatting", value: model.formattingDescription,
                       tint: false, trailing: .none)
            divider
            RuntimeRow(key: "LLM model", value: model.llmModelDescription,
                       tint: true, mono: true, trailing: .none)
            divider
            RuntimeRow(key: "LLM endpoint", value: model.llmEndpointDescription,
                       tint: false, mono: true, trailing: .none)
            divider
            RuntimeRow(key: "API keys", value: model.apiKeysDescription,
                       tint: true,
                       trailing: model.apiKeysStored ? .text("secure", CSColor.oliveLight) : .text("missing", CSColor.amber))
        }
        .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 13, style: .continuous)
                .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
        )
    }

    private var divider: some View {
        Rectangle().fill(CSColor.hairline(0.05)).frame(height: 1)
    }

    // MARK: Engine controls (editable — F1 layered transcription)

    /// Selectable engines. "onnx" is deliberately NOT exposed (experimental,
    /// frozen); "auto" defers to the core policy (Apple live when available).
    private static let sttEngineOptions: [(id: String, label: String)] = [
        ("auto", "Auto"),
        ("apple", "Apple (live)"),
        ("whisper", "Whisper (Candle)"),
    ]

    private var layeredBinding: Binding<Bool> {
        Binding(get: { model.layeredTranscriptionEnabled },
                set: { model.setLayeredTranscription($0) })
    }

    private var engineControls: some View {
        VStack(spacing: 8) {
            SettingsControlRow(title: "STT engine",
                               subtitle: "Auto prefers Apple live speech, else Whisper") {
                Menu {
                    ForEach(Self.sttEngineOptions, id: \.id) { option in
                        Button {
                            model.setSttEngine(option.id)
                        } label: {
                            if option.id == model.sttEngineId {
                                Label(option.label, systemImage: "checkmark")
                            } else {
                                Text(option.label)
                            }
                        }
                    }
                } label: {
                    EngineMenuLabel(text: model.sttEngineLabel)
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
            }
            SettingsControlRow(title: "Layered transcription",
                               subtitle: "Experimental: Apple live layer + Whisper tail patches") {
                Toggle("", isOn: layeredBinding)
                    .toggleStyle(.switch)
                    .labelsHidden()
                    .tint(CSColor.terracotta)
            }
        }
    }
}

// MARK: - Engine dropdown label (mirrors the KeysPanel MenuLabel shape)

private struct EngineMenuLabel: View {
    let text: String

    var body: some View {
        HStack(spacing: 6) {
            Text(text)
                .font(CSFont.ui(12.5, .semibold))
                .foregroundStyle(CSColor.textHigh)
                .lineLimit(1)
            Image(systemName: "chevron.up.chevron.down")
                .font(.system(size: 9, weight: .semibold))
                .foregroundStyle(CSColor.textFaint)
        }
    }
}

// MARK: - Runtime row

private struct RuntimeRow: View {
    enum Trailing {
        case none
        case dot(Color)
        case text(String, Color)
    }

    let key: String
    let value: String
    var tint: Bool = false
    var mono: Bool = false
    var trailing: Trailing = .none

    var body: some View {
        HStack(spacing: 12) {
            Text(key)
                .font(CSFont.mono(12, .medium))
                .foregroundStyle(CSColor.textMutedAlt)
                .frame(width: 160, alignment: .leading)
            Text(value)
                .font(mono ? CSFont.mono(12.5, .semibold) : CSFont.ui(12.5, .semibold))
                .foregroundStyle(mono ? CSColor.textBodyAlt : CSColor.textHigh)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: .infinity, alignment: .leading)
            trailingView
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 13)
        .background(tint ? CSColor.surfaceRaised(0.02) : Color.clear)
    }

    @ViewBuilder
    private var trailingView: some View {
        switch trailing {
        case .none:
            EmptyView()
        case .dot(let color):
            Circle().fill(color).frame(width: 7, height: 7)
        case .text(let label, let color):
            Text(label)
                .font(CSFont.mono(10, .semibold))
                .foregroundStyle(color)
        }
    }
}

// MARK: - Permission matrix cell

private struct PermissionMatrixCell: View {
    let kind: PermissionKind
    let state: PermissionState

    private var granted: Bool { state.isGranted }
    private var accent: Color { granted ? CSColor.olive : CSColor.terracotta }
    private var accentLight: Color { granted ? CSColor.oliveLight : CSColor.terracottaLight }

    var body: some View {
        HStack(spacing: 10) {
            Text(granted ? "✓" : "!")
                .font(CSFont.ui(11, .semibold))
                .foregroundStyle(accentLight)
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
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(accent.opacity(0.08))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(accent.opacity(0.2), lineWidth: 1)
        )
        .contentShape(Rectangle())
        .onTapGesture { if !granted { kind.openSystemSettings() } }
    }
}

// MARK: - Shared section label (mono, muted, wide tracking) — used by all panels.

struct SettingsSectionLabel: View {
    let text: String
    init(_ text: String) { self.text = text }
    var body: some View {
        Text(text.uppercased())
            .font(CSFont.mono(12, .semibold))
            .tracking(0.5)
            .foregroundStyle(CSColor.textMuted)
    }
}

#Preview("Engine panel") {
    ScrollView { EnginePanel(model: .preview(.engine)) }
        .frame(width: 720, height: 620)
        .background(SettingsView.windowGradient)
        .preferredColorScheme(.dark)
}
