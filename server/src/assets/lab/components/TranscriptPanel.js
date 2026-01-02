import { html } from "../lib/react.js";

export function TranscriptPanel({ controller, state }) {
  return html`
    <section className="vista-panel">
      <div className="flex-between">
        <h2>Live Transcript</h2>
        <button type="button" className="secondary" onClick=${() => controller.copyTranscript()}>
          Copy transcript
        </button>
      </div>
      <div className="transcript-box">${state.transcript || "(no transcript yet)"}</div>
      <div className="transcript-history">
        ${state.transcriptHistory.map(
          (entry) => html`<div className="history-chip">[${entry.ts}] ${entry.text}</div>`,
        )}
      </div>
    </section>
  `;
}
