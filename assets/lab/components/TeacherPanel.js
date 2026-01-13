import { React, html } from "../lib/react.js";

const { useState, useEffect } = React;

export function TeacherPanel({ controller }) {
  const [topic, setTopic] = useState("general");
  const [reference, setReference] = useState("");
  const [transcript, setTranscript] = useState("");
  const [isLearning, setIsLearning] = useState(false);
  const [isRecording, setIsRecording] = useState(false);
  const [isGenerating, setIsGenerating] = useState(false);
  const [status, setStatus] = useState("");
  const [sentences, setSentences] = useState([]);
  const [wizardIndex, setWizardIndex] = useState(0);
  const [lexiconPreview, setLexiconPreview] = useState([]);
  const [lexiconCount, setLexiconCount] = useState(0);
  const [isFetchingLexicon, setIsFetchingLexicon] = useState(false);
  const [statusLog, setStatusLog] = useState([]);
  const [calibrationRuns, setCalibrationRuns] = useState([]);

  const pushStatus = (msg, kind = "info") => {
    const line = `[${kind}] ${msg}`;
    setStatus(line);
    setStatusLog((prev) => [line, ...prev].slice(0, 12));
  };

  const refreshLexicon = async () => {
    setIsFetchingLexicon(true);
    try {
      const resp = await fetch(
        `/lab/lexicon?topic=${encodeURIComponent(topic)}&limit=30`
      );
      const data = await resp.json().catch(() => ({}));
      if (!resp.ok) throw new Error(data.error || resp.statusText || "Lexicon fetch failed");
      setLexiconPreview(data.entries || []);
      setLexiconCount(data.count || 0);
      pushStatus(`Lexicon: ${data.count || 0} entries for ${data.topic}`);
    } catch (e) {
      pushStatus(`Lexicon error: ${e.message}`, "err");
    } finally {
      setIsFetchingLexicon(false);
    }
  };

  useEffect(() => {
    refreshLexicon();
  }, [topic]);

  const handleLearn = async () => {
    setIsLearning(true);
    pushStatus("Analyzing errors...");
    let success = false;
    try {
      // Use current transcript from controller if not overridden
      const currentTranscript = transcript || controller.state.transcript;

      const resp = await fetch("/lab/learn", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          topic: topic,
          reference: reference,
          transcript: currentTranscript
        })
      });
      const data = await resp.json().catch(() => ({}));
      if (!resp.ok || !data.ok) {
        throw new Error(data.error || resp.statusText);
      }
      if (data.ai_error) {
        pushStatus(`LLM warning: ${data.ai_error}`, "warn");
      }
      const src = data.source || "diff";
      pushStatus(
        `Learned ${data.learned} new terms (source: ${src}${data.truncated ? ", truncated" : ""}).`
      );
      if (data.metrics) {
        const { wer, distance, ref_tokens, hyp_tokens } = data.metrics;
        const pct = wer != null ? (wer * 100).toFixed(1) : "n/a";
        pushStatus(`WER ${pct}% (dist=${distance}/${ref_tokens}, hyp=${hyp_tokens})`, "info");
        setCalibrationRuns((runs) => [
          {
            sentence: reference,
            transcript: transcript || controller.state.transcript,
            metrics: data.metrics
          },
          ...runs
        ].slice(0, 50));
      }
      success = true;
      refreshLexicon();
    } catch (e) {
      pushStatus(`Error: ${e.message}`, "err");
    } finally {
      setIsLearning(false);
    }
    return success;
  };

  const handleLearnAndNext = async () => {
    const ok = await handleLearn();
    if (ok) {
      nextWizard();
    }
  };

  const exportReport = () => {
    const payload = {
      topic,
      runs: calibrationRuns,
      generated: sentences
    };
    const blob = new Blob([JSON.stringify(payload, null, 2)], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `calibration-${topic}.json`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
    pushStatus("Report exported.");
  };

  const avgWer = () => {
    if (!calibrationRuns.length) return null;
    const sum = calibrationRuns.reduce((acc, r) => acc + (r.metrics?.wer || 0), 0);
    return (sum / calibrationRuns.length) * 100;
  };

  const generateSentences = async () => {
    setIsGenerating(true);
    pushStatus("Generating sentences...");
    try {
      const resp = await fetch(
        `/lab/calibrate/generate?topic=${encodeURIComponent(topic)}`
      );
      const data = await resp.json().catch(() => ({}));
      if (!resp.ok) {
        throw new Error(data.error || resp.statusText);
      }
      if (data.sentences) {
        setSentences(data.sentences);
        pushStatus("Ready to read.");
      } else {
        pushStatus("Failed to generate.");
      }
    } catch (e) {
      pushStatus(`Error: ${e.message}`);
    } finally {
      setIsGenerating(false);
    }
  };

  const copyToRef = (text) => {
      setReference(text);
      // Also clear transcript to prepare for recording
      if (isRecording) {
        controller.stopStreaming();
        setIsRecording(false);
      }
      setTranscript("");
      pushStatus(`Reference set. Click 🎙️ Record and read it aloud.`);
  };

  const runWizard = async () => {
    setIsGenerating(true);
    try {
      const resp = await fetch(
        `/lab/calibrate/wizard?topic=${encodeURIComponent(topic)}`
      );
      const data = await resp.json();
      if (!resp.ok) throw new Error(data.error || resp.statusText);
      setSentences(data.sentences || []);
      setWizardIndex(0);
      pushStatus("Wizard ready. Read each sentence and learn after recording.");
      if (data.sentences?.length) {
        copyToRef(data.sentences[0]);
      }
    } catch (e) {
      pushStatus(`Wizard error: ${e.message}`, "err");
    } finally {
      setIsGenerating(false);
    }
  };

  const nextWizard = () => {
    if (!sentences.length) return;
    const next = Math.min(wizardIndex + 1, sentences.length - 1);
    setWizardIndex(next);
    copyToRef(sentences[next]);
    setTranscript("");
    pushStatus(`Sentence ${next + 1}/${sentences.length}`);
  };

  const clearLexicon = async () => {
    try {
      const resp = await fetch(
        `/lab/lexicon/clear?topic=${encodeURIComponent(topic)}`,
        { method: "POST" }
      );
      const data = await resp.json().catch(() => ({}));
      if (!resp.ok) throw new Error(data.error || resp.statusText);
      setLexiconPreview([]);
      setLexiconCount(0);
      pushStatus("Lexicon cleared.");
    } catch (e) {
      pushStatus(`Clear failed: ${e.message}`, "err");
    }
  };

  const exportLexicon = async () => {
    try {
      const resp = await fetch(
        `/lab/lexicon/export?topic=${encodeURIComponent(topic)}`
      );
      const data = await resp.json().catch(() => ({}));
      if (!resp.ok) throw new Error(data.error || resp.statusText);
      setLexiconPreview(data.entries || []);
      setLexiconCount(data.count || 0);
      pushStatus(`Exported ${data.count || 0} entries for ${data.topic}`);
    } catch (e) {
      pushStatus(`Export failed: ${e.message}`, "err");
    }
  };

  const reloadLexicon = async () => {
    try {
      const resp = await fetch("/lab/lexicon/refresh", { method: "POST" });
      const data = await resp.json().catch(() => ({}));
      if (!resp.ok) throw new Error(data.error || resp.statusText);
      const m = data.metrics || {};
      const last = m.last_reload_ts
        ? new Date(m.last_reload_ts * 1000).toLocaleTimeString()
        : "n/a";
      pushStatus(
        `Reloaded lexicon (${data.count} entries, rules=${m.rules ?? "?"}, reloads=${m.reloads ?? "?"}, last=${last}).`
      );
      refreshLexicon();
    } catch (e) {
      pushStatus(`Reload failed: ${e.message}`, "err");
    }
  };

  // Recording toggle - uses controller's WebSocket streaming
  const toggleRecording = () => {
    if (isRecording) {
      controller.stopStreaming();
      setIsRecording(false);
      pushStatus("Recording stopped. Pulling transcript...");
      // Wait for final transcript
      setTimeout(() => {
        const t = controller.state.transcript;
        if (t) {
          setTranscript(t);
          pushStatus("Transcript ready. Review and click Learn.");
        }
      }, 800);
    } else {
      setTranscript(""); // Clear old transcript
      controller.startStreaming();
      setIsRecording(true);
      pushStatus("Recording... Read the reference text aloud.");
    }
  };

  return html`
    <div className="panel">
      <h3>🎓 The Teacher (Active Learning)</h3>

      <div className="form-row">
        <label>Topic:</label>
        <input
            value=${topic}
            onInput=${(e) => setTopic(e.target.value)}
            placeholder="e.g. liturgia, rust, cooking"
        />
        <button onClick=${generateSentences} disabled=${isLearning || isGenerating}>Generate Set</button>
        <button onClick=${runWizard} disabled=${isLearning || isGenerating}>Wizard (10)</button>
        <button onClick=${refreshLexicon} disabled=${isFetchingLexicon}>Preview</button>
        <button onClick=${reloadLexicon}>Reload</button>
        <button onClick=${clearLexicon}>Clear</button>
        <button onClick=${exportLexicon}>Export</button>
      </div>

      ${sentences.length > 0 && html`
        <div className="calibration-set">
            <h4>Calibration Sentences (${sentences.length})</h4>
            <div className="wizard-meta">
              <span>Step ${wizardIndex + 1} / ${sentences.length}</span>
              <button className="sm-btn" onClick=${nextWizard}>Next ▶</button>
            </div>
            <ul>
                ${sentences.map(s => html`
                    <li>
                        <button className="icon-btn" onClick=${() => copyToRef(s)}>📋</button>
                        ${s}
                    </li>
                `)}
            </ul>
        </div>
      `}

      ${calibrationRuns.length > 0 && html`
        <div className="metrics-card">
          <div className="metrics-header">
            <strong>Calibration metrics</strong>
            <button className="sm-btn" onClick=${exportReport}>Export report</button>
          </div>
          <div className="metric-line">
            Avg WER: ${avgWer()?.toFixed(1) ?? "n/a"}% (runs: ${calibrationRuns.length})
          </div>
        </div>
      `}

      <div className="record-section">
        <button
          className=${`record-btn ${isRecording ? 'recording' : ''}`}
          onClick=${toggleRecording}
          disabled=${isLearning}
        >
          ${isRecording ? '⏹️ Stop Recording' : '🎙️ Record'}
        </button>
        ${isRecording && html`<span className="recording-indicator">● REC</span>`}
      </div>

      <div className="split-view">
        <div className="half">
          <label>Reference Text (What you said):</label>
          <textarea
            value=${reference}
            onInput=${(e) => setReference(e.target.value)}
            rows=5
            placeholder="Paste correct text here..."
          />
        </div>
        <div className="half">
           <label>Transcript (What Whisper heard):</label>
           <textarea
             value=${transcript || controller.state.transcript}
             onInput=${(e) => setTranscript(e.target.value)}
             rows=5
             placeholder="Waiting for transcript..."
           />
        </div>
      </div>

      <div className="actions">
        <button
            className="primary-btn"
            onClick=${handleLearn}
            disabled=${isLearning || !reference}
        >
            ${isLearning ? "Learning..." : "🧠 Fix & Learn"}
        </button>
        ${sentences.length > 0 && html`
          <button
            className="secondary"
            onClick=${handleLearnAndNext}
            disabled=${isLearning || !reference}
          >
            ${isLearning ? "Working..." : "Learn & Next ▶"}
          </button>
        `}
        <span className="status">${status}</span>
      </div>

      <div className="lexicon-preview">
        <div className="lexicon-header">
          <strong>Lexicon preview (${lexiconCount} entries)</strong>
          ${isFetchingLexicon ? html`<span>⏳</span>` : null}
        </div>
        <ul>
          ${lexiconPreview.map((e) => html`
            <li><code>${e.term}</code> ← ${e.mispronunciations.join(", ")}</li>
          `)}
        </ul>
      </div>

      <div className="status-log">
        <strong>Log</strong>
        <ul>
          ${statusLog.map((s, idx) => html`<li key=${idx}>${s}</li>`)}
        </ul>
      </div>

      <style>
        .record-section { display: flex; align-items: center; justify-content: center; gap: 12px; margin: 16px 0; }
        .record-btn {
          padding: 14px 32px;
          font-size: 18px;
          border-radius: 8px;
          border: none;
          background: #2d7a4d;
          color: white;
          cursor: pointer;
          transition: all 0.2s;
        }
        .record-btn:hover { background: #3d9a5d; transform: scale(1.02); }
        .record-btn.recording { background: #c43c3c; animation: pulse 1s infinite; }
        .record-btn.recording:hover { background: #e44c4c; }
        .recording-indicator { color: #ff4444; font-weight: bold; animation: blink 1s infinite; }
        @keyframes pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.8; } }
        @keyframes blink { 0%, 100% { opacity: 1; } 50% { opacity: 0.3; } }
        .split-view { display: flex; gap: 10px; margin: 10px 0; }
        .half { flex: 1; display: flex; flex-direction: column; }
        .half textarea { flex: 1; min-height: 100px; font-family: monospace; }
        .calibration-set { background: #222; padding: 10px; margin: 10px 0; border-radius: 4px; }
        .calibration-set li { margin-bottom: 5px; list-style: none; display: flex; gap: 10px; align-items: center; }
        .icon-btn { background: none; border: none; cursor: pointer; opacity: 0.7; }
        .icon-btn:hover { opacity: 1; }
        .wizard-meta { display: flex; align-items: center; gap: 8px; margin-bottom: 6px; }
        .lexicon-preview { background: #111; padding: 8px; border-radius: 4px; max-height: 200px; overflow: auto; margin-top: 8px; }
        .lexicon-header { display: flex; justify-content: space-between; align-items: center; }
        .lexicon-preview ul, .status-log ul { padding-left: 16px; margin: 4px 0; }
        .status-log { margin-top: 10px; background: #1a1a1a; padding: 8px; border-radius: 4px; }
        .metrics-card { background: #141414; padding: 10px; border-radius: 6px; margin: 8px 0; border: 1px solid #222; }
        .metrics-header { display: flex; justify-content: space-between; align-items: center; gap: 8px; }
        .metric-line { color: #c5c5c5; margin-top: 6px; }
      </style>
    </div>
  `;
}
