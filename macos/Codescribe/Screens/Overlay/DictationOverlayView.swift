import SwiftUI

// The floating dictation overlay content — pixel-faithful to
// "codescribe App - Dictation Overlay.dc.html".
//
// Layout (top → bottom):
//   header      brand wordmark · status pill · mic/settings/more glyphs
//   mode + meta tag chip (DICTATION/FINAL) · meta line
//   body        listening = waveform + word-reveal transcript w/ caret
//               formatted = editable finalized transcript
//   action row  recording: Finish; finalized: Copy · Format · Send to Agent · Close
//   footer      ● local whisper (olive) · meta on the right
//
// A transient toast (no-speech / error) floats over the bottom edge.
struct DictationOverlayView: View {
    @ObservedObject var state: OverlayState

    // Mock-derived geometry constants (not design tokens — local to this surface).
    // The window is user-resizable; content flows to fill whatever frame it gets,
    // never narrower than `windowMinWidth`. `windowMinWidth` MUST stay ≥ the action
    // row's intrinsic width and `DictationOverlayWindow.minSize.height` MUST stay ≥
    // the chrome + `bodyMinHeight` sum — otherwise the content column overflows the
    // window frame and GlassPanel paints its rounded background past the window rect,
    // squaring the visible corners (see DictationOverlayWindow's corner note).
    private let windowMinWidth: CGFloat = 390
    private let bodyMinHeight: CGFloat = 48
    private let buttonRadius: CGFloat = 10

    /// Scroll bookkeeping for the live transcript (follow-tail with pause-on-scroll).
    @State private var followTail = true
    private let transcriptScrollSpace = "overlayTranscriptScroll"
    private let transcriptBottomAnchor = "overlayTranscriptBottom"

    var body: some View {
        GlassPanel(cornerRadius: CSRadius.window) {
            VStack(alignment: .leading, spacing: 0) {
                header
                hairline(0.06)
                modeMetaRow
                bodySection
                hairline(0.06)
                actionRow
                hairline(0.05)
                footer
            }
        }
        .frame(minWidth: windowMinWidth, maxWidth: .infinity, maxHeight: .infinity)
        .overlay(alignment: .bottom) {
            if let toast = state.toast {
                ToastPill(text: toast)
                    .padding(.bottom, 14)
                    .transition(.opacity.combined(with: .offset(y: 8)))
            }
        }
        .animation(CSMotion.floatIn, value: state.toast)
        .onAppear {
            FontLoader.register()
        }
    }

    /// 1px separator matching the mock's hairline borders.
    private func hairline(_ alpha: Double) -> some View {
        CSColor.hairline(alpha).frame(height: 1)
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: 12) {
            Wordmark(size: 15)
            // Swap the whole VIEW TYPE on live vs idle, not just a flag: the
            // animated pill (with @State + repeatForever) exists ONLY while live,
            // and is replaced by a static pill of different identity in idle/final,
            // so SwiftUI tears down its animation instead of leaving it ticking.
            if state.statusRippling {
                StatusPill(
                    text: state.statusText,
                    color: state.statusColor,
                    rippling: true
                )
                .padding(.leading, 6)
            } else {
                StaticStatusPill(text: state.statusText, color: state.statusColor)
                    .padding(.leading, 6)
            }
            Spacer(minLength: 0)
            HStack(spacing: 14) {
                Image(systemName: "mic")
                Image(systemName: "gearshape")
                Image(systemName: "ellipsis")
            }
            .font(CSFont.ui(15, .medium))
            .foregroundStyle(CSColor.textFaint)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    // MARK: Mode + meta row

    private var modeMetaRow: some View {
        HStack(spacing: 10) {
            Text(state.tagText)
                .font(CSFont.tagMono)
                .tracking(0.8)
                .foregroundStyle(state.tagColor)
                .padding(.horizontal, 9)
                .padding(.vertical, 3)
                .background(state.tagColor.opacity(0.1))
                .overlay(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .strokeBorder(state.tagColor.opacity(0.28), lineWidth: 1)
                )
                .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
            Text(state.metaText)
                .font(CSFont.metaMono)
                .foregroundStyle(CSColor.textFaint)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 20)
        .padding(.top, 10)
        .padding(.bottom, 6)
    }

    // MARK: Body

    private var bodySection: some View {
        Group {
            if state.mode == .listening {
                listeningBody
                    .transition(.opacity.combined(with: .offset(y: 8)))
            } else {
                formattedBody
                    .transition(.opacity.combined(with: .offset(y: 8)))
            }
        }
        .frame(maxWidth: .infinity, minHeight: bodyMinHeight, maxHeight: .infinity, alignment: .topLeading)
        .padding(.horizontal, 20)
        .padding(.top, 6)
        .padding(.bottom, 14)
        .animation(CSMotion.floatIn, value: state.mode)
    }

    private var listeningBody: some View {
        VStack(alignment: .leading, spacing: 0) {
            WaveformView(active: state.audioReady || state.vadActive)
                .padding(.top, 6)
                .padding(.bottom, 14)
            transcriptScroll
        }
    }

    /// Scrollable live transcript. Follows the tail (auto-scrolls to the newest
    /// text) by default; scrolling up manually pauses the follow, and scrolling
    /// back to the bottom resumes it — the standard "follow tail, pause on scroll"
    /// pattern. Pre-macOS-15 detection: measure the content's bottom edge in the
    /// scroll's coordinate space and compare against the viewport height.
    private var transcriptScroll: some View {
        GeometryReader { viewport in
            ScrollViewReader { proxy in
                ScrollView(.vertical, showsIndicators: true) {
                    VStack(alignment: .leading, spacing: 0) {
                        HStack(alignment: .bottom, spacing: 2) {
                            Text(state.listeningDisplay)
                                .font(CSFont.ui(15, .medium))
                                .lineSpacing(5)
                                .foregroundStyle(CSColor.textBody)
                                .fixedSize(horizontal: false, vertical: true)
                            BlinkingCaret()
                        }
                        Color.clear
                            .frame(height: 1)
                            .id(transcriptBottomAnchor)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(
                        GeometryReader { content in
                            Color.clear.preference(
                                key: TranscriptBottomKey.self,
                                value: content.frame(in: .named(transcriptScrollSpace)).maxY
                            )
                        }
                    )
                }
                .coordinateSpace(name: transcriptScrollSpace)
                .onPreferenceChange(TranscriptBottomKey.self) { contentBottom in
                    // At bottom when the content's bottom edge is within a small
                    // slack of the viewport's bottom; drives follow on/off.
                    followTail = contentBottom <= viewport.size.height + 28
                }
                .onChange(of: state.listeningDisplay) { _, _ in
                    guard followTail else { return }
                    withAnimation(.easeOut(duration: 0.18)) {
                        proxy.scrollTo(transcriptBottomAnchor, anchor: .bottom)
                    }
                }
            }
        }
    }

    private var formattedBody: some View {
        TextEditor(text: $state.formattedText)
            .font(CSFont.ui(15, .regular))
            .foregroundStyle(CSColor.textHigh)
            .lineSpacing(5)
            .scrollContentBackground(.hidden)
            .background(Color.clear)
            .frame(minHeight: bodyMinHeight)
    }

    // MARK: Action row

    private var actionRow: some View {
        HStack(spacing: 8) {
            if state.mode == .listening {
                Button(action: { state.stop() }) {
                    Text("Finish")
                        .font(CSFont.bodyStrong)
                        .foregroundStyle(CSColor.ink)
                        .padding(.horizontal, 18)
                        .padding(.vertical, 10)
                        .background(CSColor.terracotta)
                        .clipShape(RoundedRectangle(cornerRadius: buttonRadius, style: .continuous))
                }
                .buttonStyle(.plain)
            } else {
                Button(action: { state.copyToPasteboard() }) {
                    Text("Copy")
                        .font(CSFont.bodyStrong)
                        .foregroundStyle(CSColor.ink)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 10)
                        .background(CSColor.terracotta)
                        .clipShape(RoundedRectangle(cornerRadius: buttonRadius, style: .continuous))
                }
                .buttonStyle(.plain)

                Button(action: { state.formatTranscript() }) {
                    Text(state.isFormatting ? "Formatting..." : "Format")
                        .font(CSFont.bodyStrong)
                        .foregroundStyle(CSColor.textBody)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 10)
                        .background(CSColor.surfaceRaised(0.04))
                        .overlay(
                            RoundedRectangle(cornerRadius: buttonRadius, style: .continuous)
                                .strokeBorder(CSColor.hairline(0.12), lineWidth: 1)
                        )
                        .clipShape(RoundedRectangle(cornerRadius: buttonRadius, style: .continuous))
                }
                .buttonStyle(.plain)
                .disabled(!state.canFormat)
                .opacity(state.canFormat ? 1 : 0.45)

                Button(action: { state.sendToAgent() }) {
                    Text("Send")
                        .font(CSFont.bodyStrong)
                        .foregroundStyle(CSColor.textBody)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 10)
                        .background(CSColor.surfaceRaised(0.04))
                        .overlay(
                            RoundedRectangle(cornerRadius: buttonRadius, style: .continuous)
                                .strokeBorder(CSColor.hairline(0.12), lineWidth: 1)
                        )
                        .clipShape(RoundedRectangle(cornerRadius: buttonRadius, style: .continuous))
                }
                .buttonStyle(.plain)
                .help("Send transcript to the agent")
            }

            Spacer(minLength: 0)

            Button(action: { state.close() }) {
                Text("Close")
                    .font(CSFont.bodyStrong)
                    .foregroundStyle(CSColor.textMuted)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
                    .background(Color.clear)
                    .overlay(
                        RoundedRectangle(cornerRadius: buttonRadius, style: .continuous)
                            .strokeBorder(CSColor.hairline(0.12), lineWidth: 1)
                    )
                    .clipShape(RoundedRectangle(cornerRadius: buttonRadius, style: .continuous))
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 10)
    }

    // MARK: Footer

    private var footer: some View {
        HStack(spacing: 8) {
            HStack(spacing: 6) {
                Text("●").foregroundStyle(CSColor.olive)
                Text("local whisper").foregroundStyle(CSColor.textFaintAlt)
            }
            Spacer(minLength: 0)
            Text(state.footerRight)
                .foregroundStyle(CSColor.textFaintAlt)
        }
        .font(CSFont.mono(10, .medium))
        .padding(.horizontal, 20)
        .padding(.vertical, 8)
    }
}

/// Carries the live transcript content's bottom-edge Y (in the scroll's coordinate
/// space) up to the follow-tail detector.
private struct TranscriptBottomKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = nextValue()
    }
}

/// Word-reveal caret: 8×18 terracotta block, softpulsing on a 1s cycle (mock).
private struct BlinkingCaret: View {
    @State private var on = false
    var body: some View {
        RoundedRectangle(cornerRadius: 1, style: .continuous)
            .fill(CSColor.terracotta)
            .frame(width: 7, height: 15)
            .padding(.bottom, 3)
            .opacity(on ? 1 : 0.7)
            .onAppear {
                withAnimation(.easeInOut(duration: 1).repeatForever(autoreverses: true)) {
                    on = true
                }
            }
    }
}

/// Transient notice for no-speech / recoverable engine errors.
private struct ToastPill: View {
    let text: String
    var body: some View {
        Text(text)
            .font(CSFont.metaMono)
            .foregroundStyle(CSColor.textBody)
            .padding(.horizontal, 14)
            .padding(.vertical, 8)
            .background(CSColor.surfaceRaised(0.06))
            .overlay(
                Capsule().strokeBorder(CSColor.hairline(0.14), lineWidth: 1)
            )
            .clipShape(Capsule())
    }
}

#if DEBUG
#Preview("Listening") {
    DictationOverlayView(state: .previewListening())
        .padding(44)
        .background(
            LinearGradient(
                colors: [Color(hex: 0x15110E), CSColor.glassUnder],
                startPoint: .topLeading, endPoint: .bottomTrailing
            )
        )
        .preferredColorScheme(.dark)
}

#Preview("Formatted") {
    DictationOverlayView(state: .previewFormatted())
        .padding(44)
        .background(
            LinearGradient(
                colors: [Color(hex: 0x15110E), CSColor.glassUnder],
                startPoint: .topLeading, endPoint: .bottomTrailing
            )
        )
        .preferredColorScheme(.dark)
}
#endif
