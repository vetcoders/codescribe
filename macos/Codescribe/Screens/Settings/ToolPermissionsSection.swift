import SwiftUI

// B2 — tool permissions panel: global defaults + per-capability tri-state.
// Backed by the same registry the agent dispatcher uses (listToolCapabilities)
// and durable settings.json agent.permissions via the MCP admin bridge.

struct ToolPermissionsSection: View {
    @ObservedObject var model: SettingsViewModel
    @State private var toolsExpanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SettingsSectionLabel("Tool permissions")

            Text("Allow · Ask · Deny. Defaults: read-only allow, side-effectful ask. "
                + "\"Always allow\" from the approval card writes the same identity key.")
                .font(CSFont.mono(11, .medium))
                .foregroundStyle(CSColor.textFaint)
                .padding(.top, 4)

            defaultsCard
                .padding(.top, 11)

            if model.toolCapabilities.isEmpty {
                emptyCapabilities
                    .padding(.top, 12)
            } else {
                DisclosureGroup(
                    "Tool overrides · \(model.toolCapabilities.count)",
                    isExpanded: $toolsExpanded
                ) {
                    ScrollView {
                        LazyVStack(spacing: 6) {
                            ForEach(model.toolCapabilities, id: \.identity) { cap in
                                ToolCapabilityRow(
                                    capability: cap,
                                    onLevel: { model.setToolPermission(identity: cap.identity, level: $0) }
                                )
                            }
                        }
                    }
                    .frame(maxHeight: 360)
                    .padding(.top, 8)
                }
                .padding(.top, 12)
                .font(CSFont.ui(12.5, .semibold))
                .foregroundStyle(CSColor.textBody)
            }
        }
        .onAppear { model.reloadToolPermissions() }
    }

    private var defaultsCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Defaults")
                .font(CSFont.ui(12.5, .semibold))
                .foregroundStyle(CSColor.textBody)

            HStack(spacing: 12) {
                defaultPicker(
                    title: "Read-only",
                    selection: Binding(
                        get: { model.permissionPolicy.readOnlyDefault },
                        set: { model.setPermissionDefault(kind: .readOnly, level: $0) }
                    )
                )
                defaultPicker(
                    title: "Side effects",
                    selection: Binding(
                        get: { model.permissionPolicy.sideEffectDefault },
                        set: { model.setPermissionDefault(kind: .sideEffect, level: $0) }
                    )
                )
                defaultPicker(
                    title: "Global / unknown",
                    selection: Binding(
                        get: { model.permissionPolicy.defaultLevel },
                        set: { model.setPermissionDefault(kind: .global, level: $0) }
                    )
                )
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .fill(CSColor.surfaceRaised(0.02))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .strokeBorder(CSColor.hairline(0.07), lineWidth: 1)
        )
    }

    private func defaultPicker(title: String, selection: Binding<String>) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(CSFont.mono(10, .medium))
                .foregroundStyle(CSColor.textFaint)
            Picker(title, selection: selection) {
                Text("Allow").tag("allow")
                Text("Ask").tag("ask")
                Text("Deny").tag("deny")
            }
            .labelsHidden()
            .pickerStyle(.segmented)
            .frame(maxWidth: 180)
        }
    }

    private var emptyCapabilities: some View {
        Text("No tools registered yet — open the agent once or add an MCP server.")
            .font(CSFont.mono(11, .medium))
            .foregroundStyle(CSColor.textFaint)
            .padding(.vertical, 10)
    }
}

// MARK: - One capability row

private struct ToolCapabilityRow: View {
    let capability: CsToolCapability
    let onLevel: (String) -> Void

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(capability.name)
                    .font(CSFont.ui(12.5, .semibold))
                    .foregroundStyle(CSColor.textBody)
                    .lineLimit(1)
                Text(capability.identity)
                    .font(CSFont.mono(10, .medium))
                    .foregroundStyle(CSColor.textFaint)
                    .lineLimit(1)
                Text("\(capability.origin) · \(capability.risk)")
                    .font(CSFont.mono(10, .medium))
                    .foregroundStyle(CSColor.textFaint)
            }
            Spacer(minLength: 8)
            Picker("Level", selection: Binding(
                get: { capability.effective },
                set: { onLevel($0) }
            )) {
                Text("Allow").tag("allow")
                Text("Ask").tag("ask")
                Text("Deny").tag("deny")
            }
            .labelsHidden()
            .pickerStyle(.segmented)
            .frame(width: 180)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(CSColor.surfaceRaised(0.02))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(CSColor.hairline(0.06), lineWidth: 1)
        )
    }
}
