import SwiftUI

/// Bottom composer: inert 📎 attach, the message field, the ripple mic
/// (shares the dictation core later), and the terracotta send ↑ button.
/// Below: the affordance row mirroring the mock's capability hints.
struct Composer: View {
    @ObservedObject var store: AgentChatStore
    @FocusState private var fieldFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack(spacing: 10) {
                // Attach (inert this pass — no attachment surface in core)
                Text("📎")
                    .font(.system(size: 15))
                    .foregroundStyle(CSColor.textFaint)

                TextField("", text: $store.draft, prompt:
                    Text("Type a message, or hold Fn to speak…")
                        .font(CSFont.ui(13.5))
                        .foregroundColor(CSColor.textFaint)
                )
                .textFieldStyle(.plain)
                .font(CSFont.ui(13.5))
                .foregroundStyle(CSColor.textBody)
                .focused($fieldFocused)
                .onSubmit { store.send() }

                RippleMic()

                Button(action: { store.send() }) {
                    Text("↑")
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundStyle(ChatPalette.sendGlyph)
                        .frame(width: 32, height: 32)
                        .background(CSColor.terracotta)
                        .clipShape(RoundedRectangle(cornerRadius: CSRadius.input, style: .continuous))
                }
                .buttonStyle(.plain)
                .disabled(store.draft.trimmingCharacters(in: .whitespaces).isEmpty)
            }
            .padding(.leading, 13)
            .padding(.trailing, 11)
            .padding(.vertical, 9)
            .background(CSColor.surfaceRaised(0.04))
            .overlay(
                RoundedRectangle(cornerRadius: 13, style: .continuous)
                    .strokeBorder(CSColor.hairline(0.09), lineWidth: 1)
            )
            .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))

            // Affordance row
            HStack(spacing: 16) {
                ForEach(affordances, id: \.self) { item in
                    Text(item)
                        .font(CSFont.mono(10, .medium))
                        .foregroundStyle(CSColor.textFaintAlt)
                }
            }
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 14)
        .overlay(alignment: .top) {
            Rectangle().fill(CSColor.hairline(0.06)).frame(height: 1)
        }
    }

    private let affordances = [
        "· streaming",
        "· thread memory",
        "· attach file / image",
        "· context: selection · clipboard · frontmost app",
    ]
}
