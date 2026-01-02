const MAX_PROGRESS = 10 * 1024 * 1024;
const SYSTEM_PROMPT = "You are CodeScribe's AI assistant. Answer clearly and concisely.";

function computeDefaultWsUrl() {
  const protocol = window.location.protocol === "https:" ? "wss" : "ws";
  return `${protocol}://${window.location.host}/ws/transcribe`;
}

export class VoiceChatLabController extends EventTarget {
  constructor() {
    super();
    this.state = {
      endpoints: {
        transcribeUrl: "/transcribe",
        sttAndFormatUrl: "/stt_and_format",
        ndjsonUrl: "/stream/transcribe",
        wsUrl: computeDefaultWsUrl(),
        responsesUrl: "/demo/chat",
        apiKey: "",
        model: "gpt-4o-mini",
      },
      responsesStream: true,
      chatThreadId: null,
      provider: "local",
      status: { text: "idle", tone: "#92b4ff" },
      isStreaming: false,
      bufferBytes: 0,
      streamResponse: "",
      streamSummary: { acks: 0, finals: [] },
      logEntries: [],
      rawStream: "",
      transcript: "",
      transcriptHistory: [],
      endpointOutput: "Select or record audio to run endpoint checks.",
      chatHistory: [{ role: "system", content: SYSTEM_PROMPT }],
      chatStatus: "",
      chatBusy: false,
      activeSection: "lab",
      logView: "log",
      selectedFileName: "",
    };
    this.selectedFile = null;
    this.canvas = null;
    this.ctx = null;
    this.drawHandle = null;
    this.mediaStream = null;
    this.audioCtx = null;
    this.analyser = null;
    this.processor = null;
    this.source = null;
    this.flushTimer = null;
    this.ws = null;
    this.streamEvents = [];
    this.fullTranscript = "";
  }

  getSnapshot() {
    return { ...this.state, endpoints: { ...this.state.endpoints } };
  }

  onChange(handler) {
    const wrapped = (evt) => handler(evt.detail);
    this.addEventListener("state", wrapped);
    return () => this.removeEventListener("state", wrapped);
  }

  _emit() {
    this.dispatchEvent(new CustomEvent("state", { detail: this.getSnapshot() }));
  }

  _setState(mutator) {
    const next = typeof mutator === "function" ? mutator(this.state) : { ...this.state, ...mutator };
    this.state = next;
    this._emit();
  }

  setActiveSection(section) {
    this._setState({ activeSection: section });
  }

  setLogView(view) {
    this._setState({ logView: view });
  }

  setEndpointField(key, value) {
    this._setState((prev) => {
      const nextValue = value ?? prev.endpoints[key];
      return {
        ...prev,
        endpoints: { ...prev.endpoints, [key]: nextValue },
      };
    });
  }

  setFile(file) {
    this.selectedFile = file || null;
    this._setState({ selectedFileName: file ? file.name : "" });
  }

  setStatus(text, tone = "#92b4ff") {
    this._setState({ status: { text, tone } });
  }

  appendLog(payload) {
    const entry = {
      ...payload,
      at: new Date().toLocaleTimeString(),
    };
    this._setState((prev) => ({
      ...prev,
      logEntries: [entry, ...prev.logEntries].slice(0, 80),
    }));
  }

  appendStreamResponse(line) {
    this._setState((prev) => {
      const existing = prev.rawStream ? `${prev.rawStream}\n${line}` : line;
      const trimmed = existing.split("\n").slice(-400).join("\n");
      return { ...prev, rawStream: trimmed };
    });
  }

  appendStreamParsed(obj) {
    try {
      const parsed = typeof obj === "string" ? JSON.parse(obj) : obj;
      this.streamEvents.push(parsed);
      this._setState((prev) => {
        const nextSummary = {
          acks: parsed.type === "ack" ? prev.streamSummary.acks + 1 : prev.streamSummary.acks,
          finals:
            parsed.type === "transcript.final" && parsed.text
              ? [...prev.streamSummary.finals, parsed.text]
              : prev.streamSummary.finals,
        };
        const parts = [`acks: ${nextSummary.acks}`];
        if (nextSummary.finals.length) {
          parts.push(`finals (${nextSummary.finals.length}):`);
          nextSummary.finals.forEach((txt, idx) => {
            parts.push(`  ${idx + 1}. ${txt}`);
          });
        }
        const linesOut = this.streamEvents
          .map((evt) => {
            try {
              return JSON.stringify(evt);
            } catch {
              return String(evt);
            }
          })
          .slice(-200)
          .join("\n");
        const summaryText = parts.join("\n");
        return {
          ...prev,
          streamSummary: nextSummary,
          streamResponse: linesOut ? `${summaryText}

${linesOut}` : summaryText,
        };
      });
    } catch (err) {
      console.warn("Failed to parse stream event:", err);
    }
  }

  pushTranscript(text) {
    if (!text) return;
    this.fullTranscript = `${this.fullTranscript}${this.fullTranscript ? "\n" : ""}${text}`.trim();
    this._setState((prev) => {
      const history = [{ text, ts: new Date().toLocaleTimeString() }, ...prev.transcriptHistory];
      return {
        ...prev,
        transcript: this.fullTranscript || "(no transcript yet)",
        transcriptHistory: history.slice(0, 6),
      };
    });
  }

  setEndpointOutput(text) {
    this._setState({ endpointOutput: text });
  }

  async init() {
    await Promise.allSettled([this.fetchLabConfig(), this.fetchHealth()]);
  }

  async fetchLabConfig() {
    try {
      const resp = await fetch("/lab/config");
      const cfg = await resp.json();
      this._setState((prev) => ({
        ...prev,
        endpoints: {
          ...prev.endpoints,
          transcribeUrl: cfg.stt_upload_url || prev.endpoints.transcribeUrl,
          sttAndFormatUrl: cfg.stt_and_format_url || prev.endpoints.sttAndFormatUrl,
          ndjsonUrl: cfg.stt_ndjson_url || prev.endpoints.ndjsonUrl,
          wsUrl: cfg.stt_ws_url || prev.endpoints.wsUrl,
          responsesUrl: cfg.responses_url || prev.endpoints.responsesUrl,
          model: cfg.harmony_model || prev.endpoints.model,
        },
      }));
      if (cfg.ai_provider) {
        this._setState({ provider: cfg.ai_provider });
      }
    } catch (error) {
      console.warn("lab-config", error);
    }
  }

  async fetchHealth() {
    try {
      const resp = await fetch("/healthz");
      const health = await resp.json();
      const provider = health?.ai?.provider || "local";
      this._setState({ provider });
    } catch {
      this._setState({ provider: "unknown" });
    }
  }

  attachCanvas(canvas) {
    this.canvas = canvas;
    if (canvas) {
      this.ctx = canvas.getContext("2d");
      canvas.width = canvas.clientWidth;
      canvas.height = canvas.clientHeight;
      this._drawSpectrogram();
    }
    return () => {
      if (this.drawHandle) cancelAnimationFrame(this.drawHandle);
      if (this.canvas === canvas) {
        this.canvas = null;
        this.ctx = null;
      }
    };
  }

  _drawSpectrogram() {
    if (!this.ctx || !this.canvas) {
      this.drawHandle = requestAnimationFrame(() => this._drawSpectrogram());
      return;
    }
    const canvas = this.canvas;
    const ctx = this.ctx;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    if (this.analyser) {
      const freqData = new Uint8Array(this.analyser.frequencyBinCount);
      this.analyser.getByteFrequencyData(freqData);
      ctx.strokeStyle = "rgba(255,255,255,0.08)";
      ctx.lineWidth = 1;
      const gridLines = 4;
      for (let i = 0; i <= gridLines; i += 1) {
        const y = (i / gridLines) * canvas.height;
        ctx.beginPath();
        ctx.moveTo(0, y);
        ctx.lineTo(canvas.width, y);
        ctx.stroke();
      }
      const bars = 96;
      const step = Math.max(1, Math.floor(freqData.length / bars));
      const barWidth = canvas.width / bars;
      for (let i = 0; i < bars; i += 1) {
        const idx = Math.min(freqData.length - 1, i * step);
        const value = freqData[idx] / 255;
        const height = Math.max(2, value * canvas.height);
        const x = i * barWidth;
        const gradient = ctx.createLinearGradient(x, canvas.height - height, x, canvas.height);
        gradient.addColorStop(0, `rgba(102,224,208,${0.9 * value + 0.1})`);
        gradient.addColorStop(1, "rgba(20,32,51,0.4)");
        ctx.fillStyle = gradient;
        ctx.fillRect(x + 1, canvas.height - height, barWidth - 2, height);
      }
    }
    this.drawHandle = requestAnimationFrame(() => this._drawSpectrogram());
  }

  async uploadSelectedFile(endpoint) {
    if (!this.selectedFile || !endpoint) {
      alert("Select an audio file first.");
      return;
    }
    try {
      this.setEndpointOutput(`Uploading ${this.selectedFile.name} → ${endpoint}…`);
      const form = new FormData();
      form.append("audio", this.selectedFile, this.selectedFile.name || "clip.wav");
      const resp = await fetch(endpoint, { method: "POST", body: form });
      const text = await resp.text();
      let summary = text;
      try {
        summary = JSON.stringify(JSON.parse(text), null, 2);
      } catch {}
      this.setEndpointOutput(`[POST ${endpoint}] HTTP ${resp.status}\n${summary}`);
    } catch (error) {
      this.setEndpointOutput(`Upload failed: ${error?.message || error}`);
    }
  }

  async capturePcmChunks(ms = 3000) {
    const audioContext = new AudioContext();
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    const source = audioContext.createMediaStreamSource(stream);
    const processor = audioContext.createScriptProcessor(4096, 1, 1);
    const chunks = [];
    processor.onaudioprocess = (event) => {
      const channel = event.inputBuffer.getChannelData(0);
      chunks.push(this.floatToBase64(channel));
    };
    source.connect(processor);
    processor.connect(audioContext.destination);
    await new Promise((resolve) => setTimeout(resolve, ms));
    stream.getTracks().forEach((track) => track.stop());
    processor.disconnect();
    source.disconnect();
    await audioContext.close();
    return { chunks, sampleRate: audioContext.sampleRate };
  }

  floatToBase64(chunk) {
    const buffer = new ArrayBuffer(chunk.length * 2);
    const view = new DataView(buffer);
    for (let i = 0; i < chunk.length; i += 1) {
      let sample = Math.max(-1, Math.min(1, chunk[i]));
      view.setInt16(i * 2, sample < 0 ? sample * 0x8000 : sample * 0x7fff, true);
    }
    const bytes = new Uint8Array(buffer);
    let binary = "";
    for (let i = 0; i < bytes.byteLength; i += 1) {
      binary += String.fromCharCode(bytes[i]);
    }
    return btoa(binary);
  }

  async testNdjsonStream() {
    try {
      this.setEndpointOutput("Recording sample for streaming endpoint…");
      const capture = await this.capturePcmChunks(3000);
      const lines = capture.chunks.map((chunk, idx) =>
        JSON.stringify({
          type: "chunk",
          audio_base64: chunk,
          sample_rate: capture.sampleRate,
          encoding: "pcm16",
          last: idx === capture.chunks.length - 1,
        }),
      );
      lines.push(JSON.stringify({ type: "end" }));
      const payload = `${lines.join("\n")}\n`;
      const target = this.state.endpoints.ndjsonUrl || "/stream/transcribe";
      const resp = await fetch(target, {
        method: "POST",
        headers: { "Content-Type": "application/x-ndjson" },
        body: payload,
      });
      const bodyText = await resp.text();
      const transcripts = [];
      bodyText
        .trim()
        .split("\n")
        .forEach((line) => {
          try {
            const evt = JSON.parse(line);
            if (evt.type === "transcript.final") transcripts.push(evt.text || "—");
          } catch {
            /* ignore */
          }
        });
      const summary = transcripts.length
        ? `Transcripts:\n- ${transcripts.join("\n- ")}`
        : "No transcript events.";
      this.setEndpointOutput(`[POST ${target}] HTTP ${resp.status}\n${summary}\n\n${bodyText}`);
    } catch (error) {
      this.setEndpointOutput(`NDJSON test failed: ${error?.message || error}`);
    }
  }

  async captureWebmBlob(ms = 5000) {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    const recorder = new MediaRecorder(stream, { mimeType: "audio/webm" });
    const chunks = [];
    recorder.ondataavailable = (evt) => {
      if (evt.data.size) chunks.push(evt.data);
    };
    recorder.start();
    await new Promise((resolve) => setTimeout(resolve, ms));
    await new Promise((resolve) => {
      recorder.onstop = resolve;
      recorder.stop();
    });
    stream.getTracks().forEach((track) => track.stop());
    return new Blob(chunks, { type: "audio/webm" });
  }

  async recordPipelineUpload() {
    try {
      this.setEndpointOutput("Recording standard pipeline sample…");
      const blob = await this.captureWebmBlob(5000);
      const form = new FormData();
      form.append("audio", blob, "lab.webm");
      const target = this.state.endpoints.sttAndFormatUrl || "/stt_and_format";
      const resp = await fetch(target, { method: "POST", body: form });
      const text = await resp.text();
      let summary = text;
      try {
        summary = JSON.stringify(JSON.parse(text), null, 2);
      } catch {}
      this.setEndpointOutput(`[POST ${target}] HTTP ${resp.status}\n${summary}`);
    } catch (error) {
      this.setEndpointOutput(`Recording failed: ${error?.message || error}`);
    }
  }

  async uploadSingleFile(kind) {
    if (kind === "transcribe") {
      await this.uploadSelectedFile(this.state.endpoints.transcribeUrl);
    } else if (kind === "format") {
      await this.uploadSelectedFile(this.state.endpoints.sttAndFormatUrl);
    }
  }

  async startStreaming() {
    if (this.state.isStreaming) return;
    try {
      this.mediaStream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch (err) {
      alert("Microphone permission is required.");
      return;
    }

    this.audioCtx = new AudioContext();
    this.analyser = this.audioCtx.createAnalyser();
    this.analyser.fftSize = 2048;
    this.source = this.audioCtx.createMediaStreamSource(this.mediaStream);
    this.processor = this.audioCtx.createScriptProcessor(4096, 1, 1);
    this.processor.onaudioprocess = (event) => {
      if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;
      const channel = event.inputBuffer.getChannelData(0);
      const payload = {
        type: "chunk",
        audio_base64: this.floatToBase64(channel),
        sample_rate: this.audioCtx.sampleRate,
        encoding: "pcm16",
      };
      this.ws.send(JSON.stringify(payload));
    };
    this.source.connect(this.analyser);
    this.source.connect(this.processor);
    this.processor.connect(this.audioCtx.destination);
    this.streamEvents = [];
    this._setState({
      isStreaming: true,
      streamResponse: "",
      rawStream: "",
      streamSummary: { acks: 0, finals: [] },
      bufferBytes: 0,
    });
    this.fullTranscript = "";

    const wsUrl = (this.state.endpoints.wsUrl || computeDefaultWsUrl()).trim() || computeDefaultWsUrl();
    this.ws = new WebSocket(wsUrl);
    this.ws.onopen = () => {
      this.appendLog({ type: "socket", text: `connected: ${wsUrl}` });
      this.setStatus("streaming", "#66e0d0");
      this.flushTimer = setInterval(() => {
        if (this.ws && this.ws.readyState === WebSocket.OPEN) {
          this.ws.send(JSON.stringify({ type: "flush" }));
        }
      }, 2500);
    };
    this.ws.onmessage = (event) => {
      if (event.data) this.appendStreamResponse(event.data);
      try {
        const msg = JSON.parse(event.data);
        if (msg.type === "ack") {
          const bytes = Number(msg.received_bytes || 0);
          this._setState({ bufferBytes: bytes });
          return;
        }
        if (msg.type === "transcript.final") {
          this.pushTranscript(msg.text || "—");
        }
        this.appendStreamParsed(msg);
        this.appendLog(msg);
      } catch {
        this.appendLog({ type: "socket", text: event.data });
      }
    };
    this.ws.onerror = (event) => {
      this.appendLog({ type: "error", text: `socket-error: ${event.message || event}` });
      this.stopStreaming();
    };
    this.ws.onclose = () => {
      this.appendLog({ type: "socket", text: "closed" });
      this.stopStreaming();
    };
  }

  stopStreaming() {
    this.fullTranscript = "";
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify({ type: "flush" }));
      this.ws.send(JSON.stringify({ type: "end" }));
      this.ws.close();
    }
    this.ws = null;
    if (this.flushTimer) clearInterval(this.flushTimer);
    this.flushTimer = null;
    if (this.processor) {
      this.processor.disconnect();
      this.processor.onaudioprocess = null;
    }
    if (this.source) this.source.disconnect();
    if (this.mediaStream) this.mediaStream.getTracks().forEach((track) => track.stop());
    if (this.audioCtx) this.audioCtx.close();
    this.processor = null;
    this.source = null;
    this.mediaStream = null;
    this.audioCtx = null;
    this.analyser = null;
    this._setState({ isStreaming: false, bufferBytes: 0 });
    this.setStatus("idle");
  }

  async sendChatMessage(content) {
    const trimmed = (content || "").trim();
    if (!trimmed) return;
    this._setState((prev) => ({
      ...prev,
      chatHistory: [...prev.chatHistory, { role: "user", content: trimmed }],
      chatBusy: true,
    }));
    this._setState({ chatStatus: "Thinking…" });
    const target = (this.state.endpoints.responsesUrl || "").trim() || "/demo/chat";
    const headers = { "Content-Type": "application/json" };
    if (this.state.endpoints.apiKey) headers.Authorization = `Bearer ${this.state.endpoints.apiKey}`;
    const useResponses = target.includes("/responses");
    try {
      let resp;
      if (useResponses) {
        const payload = {
          model: this.state.endpoints.model || "gpt-4o-mini",
          input: this.state.chatHistory
            .filter((msg) => (msg.content || "").trim())
            .map((msg) => ({
              role: msg.role,
              content: [{ type: "input_text", text: msg.content }],
            })),
          stream: !!this.state.responsesStream,
          previous_response_id: this.state.chatThreadId || undefined,
        };
        if (payload.stream) {
          await this._streamResponses(target, headers, payload);
          return;
        }
        resp = await fetch(target, { method: "POST", headers, body: JSON.stringify(payload) });
      } else {
        resp = await fetch(target, {
          method: "POST",
          headers,
          body: JSON.stringify({ messages: this.state.chatHistory }),
        });
      }
      if (!resp.ok) {
        const text = await resp.text();
        throw new Error(text || `HTTP ${resp.status}`);
      }
      const data = await resp.json();
      const textOut = useResponses ? this.extractResponsesText(data) : data.text;
      const respId = useResponses ? data.id || null : null;
      this._setState((prev) => ({
        ...prev,
        chatHistory: [...prev.chatHistory, { role: "assistant", content: textOut || "—" }],
        chatStatus: "",
        chatBusy: false,
        chatThreadId: respId || prev.chatThreadId,
      }));
    } catch (error) {
      this._setState({ chatStatus: `Error: ${error?.message || error}`, chatBusy: false });
    }
  }

  resetChat() {
    this._setState({
      chatHistory: [{ role: "system", content: SYSTEM_PROMPT }],
      chatStatus: "",
      chatBusy: false,
      chatThreadId: null,
    });
  }

  extractResponsesText(data) {
    const chunks = [];
    const walk = (node) => {
      if (node === null || node === undefined) return;
      if (typeof node === "string" || typeof node === "number") {
        chunks.push(String(node));
        return;
      }
      if (Array.isArray(node)) {
        node.forEach(walk);
        return;
      }
      if (typeof node === "object") {
        if (node.text) chunks.push(node.text);
        if (node.output_text) chunks.push(node.output_text);
        if (node.content) walk(node.content);
        if (node.output) walk(node.output);
        if (node.message) walk(node.message);
        if (node.delta) walk(node.delta);
      }
    };
    walk(data);
    const joined = chunks.join(" ").trim();
    return joined || "—";
  }

  async _streamResponses(target, headers, payload) {
    this._setState({ chatStatus: "Streaming…", chatBusy: true });
    let assistant = "";
    let respId = this.state.chatThreadId;
    try {
      const resp = await fetch(target, {
        method: "POST",
        headers: { ...headers, Accept: "text/event-stream" },
        body: JSON.stringify({ ...payload, stream: true }),
      });
      if (!resp.ok || !resp.body) {
        const text = await resp.text();
        throw new Error(text || `HTTP ${resp.status}`);
      }
      const reader = resp.body.getReader();
      const decoder = new TextDecoder("utf-8");
      let buffer = "";
      let eventName = "";
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let nl;
        while ((nl = buffer.indexOf("\n")) >= 0) {
          const line = buffer.slice(0, nl).trimEnd();
          buffer = buffer.slice(nl + 1);
          if (!line) continue;
          if (line.startsWith("event:")) {
            eventName = line.slice(6).trim();
            continue;
          }
          if (!line.startsWith("data:")) continue;
          const dataStr = line.slice(5).trim();
          if (dataStr === "[DONE]") {
            break;
          }
          let obj;
          try {
            obj = JSON.parse(dataStr);
          } catch {
            continue;
          }
          if (obj && obj.id && !respId) respId = obj.id;
          // Harmony deltas often come as output_text_delta / delta
          const delta =
            obj.output_text_delta ||
            (obj.delta && (obj.delta.output_text_delta || obj.delta.output_text)) ||
            obj.output_text ||
            obj.text;
          if (typeof delta === "string" && delta) {
            assistant += delta;
            this._setState({ chatStatus: `Streaming… (${assistant.length} chars)` });
          }
        }
      }
      const finalText = assistant || "—";
      this._setState((prev) => ({
        ...prev,
        chatHistory: [...prev.chatHistory, { role: "assistant", content: finalText }],
        chatStatus: "",
        chatBusy: false,
        chatThreadId: respId || prev.chatThreadId,
      }));
    } catch (err) {
      this._setState({ chatStatus: `Error: ${err?.message || err}`, chatBusy: false });
    }
  }

  copyTranscript() {
    const text = (this.fullTranscript || "").trim();
    if (!text) {
      alert("Transcript is empty.");
      return;
    }
    navigator.clipboard
      .writeText(text)
      .then(() => {
        this.appendLog({ type: "info", text: "Transcript copied" });
      })
      .catch((err) => {
        alert(`Copy failed: ${err?.message || err}`);
      });
  }
}
