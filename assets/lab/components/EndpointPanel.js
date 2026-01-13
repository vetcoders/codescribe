import { html } from "../lib/react.js";

const FIELD_DEFS = [
  { key: "transcribeUrl", label: "Upload (STT) endpoint", placeholder: "/transcribe" },
  { key: "sttAndFormatUrl", label: "STT + Format endpoint", placeholder: "/stt_and_format" },
  { key: "ndjsonUrl", label: "NDJSON stream endpoint", placeholder: "/stream/transcribe" },
  { key: "wsUrl", label: "WebSocket stream", placeholder: "wss://…/ws/transcribe" },
  { key: "responsesUrl", label: "Harmony /v1/responses endpoint", placeholder: "/demo/chat" },
  { key: "apiKey", label: "Bearer token", placeholder: "sk-…" },
  { key: "model", label: "Model", placeholder: "gpt-4o-mini" },
];

export function EndpointPanel({ controller, state }) {
  return html`
    <section className="vista-panel">
      <h2>Endpoint & Capture Controls</h2>
      <div className="input-stack">
        <label for="lab-file-input">Audio file</label>
        <input
          id="lab-file-input"
          type="file"
          accept="audio/*"
          onChange=${(event) => {
            const file = event.target.files?.[0] || null;
            controller.setFile(file);
          }}
        />
        <small className="label-muted">
          ${state.selectedFileName ? `Selected: ${state.selectedFileName}` : "Select a clip to upload"}
        </small>
      </div>
      <div style=${{ marginTop: "14px", display: "flex", gap: "12px", flexWrap: "wrap" }}>
        <button type="button" className="secondary" onClick=${() => controller.uploadSingleFile("transcribe")}>
          Upload → STT
        </button>
        <button type="button" className="secondary" onClick=${() => controller.uploadSingleFile("format")}>
          Upload → STT+Format
        </button>
        <button type="button" onClick=${() => controller.testNdjsonStream()}>Test NDJSON stream</button>
        <button type="button" onClick=${() => controller.recordPipelineUpload()}>Record & Upload Sample</button>
      </div>
      <div className="endpoint-grid" style=${{ marginTop: "16px" }}>
        ${FIELD_DEFS.map(
          ({ key, label, placeholder }) => html`
            <div className="input-stack" key=${key}>
              <label>${label}</label>
              <input
                value=${state.endpoints[key] || ""}
                placeholder=${placeholder}
                onChange=${(event) => controller.setEndpointField(key, event.target.value)}
              />
            </div>
          `,
        )}
      </div>
      <pre className="endpoint-output">${state.endpointOutput}</pre>
    </section>
  `;
}
