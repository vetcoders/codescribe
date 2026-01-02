import { React, html } from "../lib/react.js";

const { useState } = React;

export function ChatPanel({ controller, state }) {
  const [draft, setDraft] = useState("");

  const handleSend = () => {
    const text = draft.trim();
    if (!text || state.chatBusy) return;
    controller.sendChatMessage(text);
    setDraft("");
  };

  const handleKeyDown = (event) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      handleSend();
    }
  };

  return html`
    <section className="vista-panel chat-layout">
      <div>
        <h2>Assistant conversation</h2>
        <div className="chat-messages">
          ${state.chatHistory
            .filter((msg) => msg.role !== "system")
            .map(
              (msg, idx) => html`<div className=${`chat-bubble ${msg.role}`} key=${idx}>${msg.content}</div>`,
            )}
        </div>
        <div className="chat-composer">
          <textarea
            value=${draft}
            placeholder="Ask something… (Shift+Enter for newline)"
            onInput=${(event) => setDraft(event.target.value)}
            onKeyDown=${handleKeyDown}
          ></textarea>
          <div className="chat-actions">
            <div style=${{ color: "var(--vista-text-muted)" }}>
              ${state.chatStatus || (state.chatThreadId ? `Thread: ${state.chatThreadId}` : "")}
            </div>
            <div style=${{ display: "flex", gap: "10px" }}>
              <button type="button" className="secondary" onClick=${() => controller.resetChat()}>
                Reset chat
              </button>
              <button type="button" disabled=${state.chatBusy} onClick=${handleSend}>Send</button>
            </div>
          </div>
        </div>
      </div>
      <div className="chat-side-card" style=${{ border: "1px solid var(--vista-border-default)", borderRadius: "var(--vista-radius-lg)", padding: "16px", background: "rgba(255,255,255,0.02)" }}>
        <strong>How it works</strong>
        <p>
          Messages are sent through the configured Harmony / Libraxis endpoint. The backend proxies the
          request so your keys stay local. Responses live in this session only – reset chat to start fresh.
        </p>
        <p className="label-muted">
          Tip: run the streaming tab while chatting to verify transcription + formatter parity.
        </p>
      </div>
    </section>
  `;
}
