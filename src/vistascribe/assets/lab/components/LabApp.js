import { React, html } from "../lib/react.js";
import { useLabController } from "../hooks/useLabController.js";
import { SpectrogramPanel } from "./SpectrogramPanel.js";
import { TranscriptPanel } from "./TranscriptPanel.js";
import { EndpointPanel } from "./EndpointPanel.js";
import { LogPanel } from "./LogPanel.js";
import { ChatPanel } from "./ChatPanel.js";
import { TeacherPanel } from "./TeacherPanel.js";

const { useMemo } = React;

export function LabApp() {
  const { controller, state } = useLabController();
  const bufferPct = useMemo(() => {
    const denom = 10 * 1024 * 1024;
    return Math.min(1, Math.max(0, state.bufferBytes / denom));
  }, [state.bufferBytes]);

  return html`
    <div className="lab-app">
      <div className="flex-between">
        <h1>VistaScribe Voice & Chat Lab</h1>
        <span className="status-pill">Backend: ${state.provider}</span>
      </div>
      <div className="tab-strip">
        ${["lab", "chat", "teacher"].map(
          (tab) => html`
            <button
              className=${`tab-pill ${state.activeSection === tab ? "active" : ""}`}
              onClick=${() => controller.setActiveSection(tab)}
            >
              ${tab === "lab" ? "Voice Lab" : tab === "chat" ? "Chat" : "Teacher"}
            </button>
          `,
        )}
      </div>
      ${state.activeSection === "lab"
        ? html`<${LabSurface} controller=${controller} state=${state} bufferPct=${bufferPct} />`
        : state.activeSection === "chat"
        ? html`<${ChatPanel} controller=${controller} state=${state} />`
        : html`<${TeacherPanel} controller=${controller} />`}
    </div>
  `;
}

function LabSurface({ controller, state, bufferPct }) {
  return html`
    <div className="lab-layout">
      <div className="vista-grid-top">
        <${SpectrogramPanel}
          controller=${controller}
          state=${state}
          bufferPct=${bufferPct}
        />
        <${TranscriptPanel} controller=${controller} state=${state} />
      </div>
      <div className="vista-grid-bottom">
        <${EndpointPanel} controller=${controller} state=${state} />
        <${LogPanel} controller=${controller} state=${state} />
      </div>
    </div>
  `;
}
