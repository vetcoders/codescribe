import { React, html } from "../lib/react.js";

const { useEffect, useRef } = React;

export function SpectrogramPanel({ controller, state, bufferPct }) {
  const canvasRef = useRef(null);

  useEffect(() => {
    if (!canvasRef.current) return undefined;
    const detach = controller.attachCanvas(canvasRef.current);
    return () => {
      if (detach) detach();
    };
  }, [controller]);

  return html`
    <section className="vista-panel">
      <div className="flex-between">
        <h2>Streaming Spectrogram</h2>
        <span className="status-pill" style=${{ color: state.status.tone }}>
          ${state.status.text}
        </span>
      </div>
      <canvas ref=${canvasRef} className="spectrogram-canvas"></canvas>
      <div className="controls" style=${{ display: "flex", gap: "12px", marginTop: "14px" }}>
        <button type="button" disabled=${state.isStreaming} onClick=${() => controller.startStreaming()}>
          Start streaming
        </button>
        <button type="button" className="secondary" disabled=${!state.isStreaming} onClick=${() => controller.stopStreaming()}>
          Stop
        </button>
      </div>
      <div className="progress-wrap">
        <progress max="1" value=${bufferPct}></progress>
        <span>${(state.bufferBytes / 1024).toFixed(1)} KB buffered</span>
      </div>
    </section>
  `;
}
