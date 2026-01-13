import { html } from "../lib/react.js";

export function LogPanel({ controller, state }) {
  const summary = state.streamSummary || { acks: 0, finals: [] };
  return html`
    <section className="vista-panel">
      <div className="flex-between">
        <h2>Stream Inspector</h2>
        <div className="controls" style=${{ display: "flex", gap: "8px" }}>
          <button
            className=${`tab-pill ${state.logView === "log" ? "active" : ""}`}
            onClick=${() => controller.setLogView("log")}
          >
            Stream log
          </button>
          <button
            className=${`tab-pill ${state.logView === "stream" ? "active" : ""}`}
            onClick=${() => controller.setLogView("stream")}
          >
            Raw stream
          </button>
        </div>
      </div>
      <pre className="stream-summary">
acks: ${summary.acks}
finals (${summary.finals.length})
${summary.finals.map((t, idx) => `  ${idx + 1}. ${t}`).join("\n")}

${state.streamResponse || "—"}
      </pre>
      ${state.logView === "log"
        ? html`<div className="vista-log">
            ${state.logEntries.map(
              (entry) => html`<div className=${`log-line ${entry.type === "error" ? "error" : ""}`}>
                <span className="evt-type">${entry.type || "event"}</span>
                <span>${entry.text || entry.message || ""}</span>
                <span style=${{ color: "var(--vista-text-muted)" }}>${entry.at}</span>
              </div>`,
            )}
          </div>`
        : html`<div className="stream-raw"><pre>${state.rawStream || "(no data)"}</pre></div>`}
    </section>
  `;
}
