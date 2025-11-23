#!/usr/bin/env node
// Minimal streaming client: mic → ffmpeg → NDJSON → /stream/transcribe
// Requires: ffmpeg installed (brew install ffmpeg). Node >= 18.

import { spawn } from 'node:child_process';
import { Readable, PassThrough } from 'node:stream';
import { argv, exit } from 'node:process';
import { setTimeout as sleep } from 'node:timers/promises';

const opts = parseArgs(argv.slice(2));

const SERVER = opts.server || process.env.VS_SERVER || 'http://127.0.0.1:8237';
const LANG = opts.lang || process.env.WHISPER_LANGUAGE || 'pl';
const SR = Number(opts.sr || process.env.SAMPLE_RATE || 16000);
const ENCODING = 'pcm16';
const CHUNK_MS = Number(opts.chunkMs || 800); // batch ~0.8s of audio per chunk
const PASTE_FINAL = Boolean(Number(opts.pasteFinal || 0));
const PASTE_LIVE = Boolean(Number(opts.pasteLive || 0));

function parseArgs(args) {
  const out = {};
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    if (a.startsWith('--')) {
      const k = a.slice(2);
      const v = (i + 1 < args.length && !args[i + 1].startsWith('--')) ? args[++i] : '1';
      out[k] = v;
    }
  }
  return out;
}

async function pbcopy(text) {
  return new Promise((resolve) => {
    const p = spawn('pbcopy');
    p.stdin.write(text ?? '');
    p.stdin.end();
    p.on('close', () => resolve());
  });
}

async function cmdV() {
  return new Promise((resolve) => {
    const osa = spawn('osascript', ['-e', 'tell application "System Events" to keystroke "v" using {command down}']);
    osa.on('close', () => resolve());
  });
}

function base64(buf) {
  return Buffer.from(buf).toString('base64');
}

async function* ndjsonResponseLines(body) {
  const decoder = new TextDecoder("utf-8");
  let buffer = '';
  for await (const chunk of body) {
    buffer += decoder.decode(chunk, { stream: true });
    let idx;
    while ((idx = buffer.indexOf('\n')) !== -1) {
      const line = buffer.slice(0, idx).trim();
      buffer = buffer.slice(idx + 1);
      if (line) yield line;
    }
  }
  buffer += decoder.decode(); // flush
  const tail = buffer.trim();
  if (tail) yield tail;
}

function startFFmpeg(samplerate) {
  if (process.platform !== "darwin") {
    console.error("[ffmpeg] Error: This client uses 'avfoundation' (macOS only).");
    // We allow it to try anyway if user knows what they're doing, or exit?
    // Exit is safer for a demo script.
    process.exit(1);
  }

  // macOS default input: avfoundation: ":0" (first input device)
  // Output raw PCM s16le mono to stdout
  const args = [
    '-hide_banner', '-loglevel', 'warning', // Show warnings/errors
    '-f', 'avfoundation', '-i', ':0',
    '-ac', '1', '-ar', String(samplerate),
    '-f', 's16le', '-' // raw PCM
  ];
  const ff = spawn('ffmpeg', args, { stdio: ['ignore', 'pipe', 'pipe'] });
  
  // Capture stderr for debugging
  ff.stderr.on('data', (d) => {
    console.error(`[ffmpeg] ${d.toString().trim()}`);
  });
  
  ff.on('exit', (code) => {
    if (code !== 0 && code !== null) {
      console.error(`[ffmpeg] exited with code ${code}. Check microphone permissions.`);
    }
  });
  return ff;
}

async function main() {
  console.error(`[vs-stream] server=${SERVER} sr=${SR} lang=${LANG} chunkMs=${CHUNK_MS}`);
  const reqStream = new PassThrough();
  const res = await fetch(`${SERVER}/stream/transcribe`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-ndjson' },
    body: reqStream,
    duplex: 'half', // Node.js specific
  });

  if (!res.ok) {
    console.error(`[vs-stream] HTTP ${res.status}`);
    exit(2);
  }

  // Send initial settings
  reqStream.write(JSON.stringify({ type: 'set', language: LANG, sample_rate: SR, encoding: ENCODING }) + '\n');

  // Start microphone capture
  const ff = startFFmpeg(SR);

  // Batch raw PCM from ffmpeg stdout and send as base64 NDJSON chunks
  const batchBytes = Math.max(1, Math.floor((SR * 2 /*bytes*/ * CHUNK_MS) / 1000));
  let buf = Buffer.alloc(0);
  let running = true;

  ff.stdout.on('data', (chunk) => {
    buf = Buffer.concat([buf, chunk]);
    while (buf.length >= batchBytes) {
      const piece = buf.subarray(0, batchBytes);
      buf = buf.subarray(batchBytes);
      const line = JSON.stringify({ type: 'chunk', audio_base64: base64(piece), sample_rate: SR, encoding: ENCODING }) + '\n';
      reqStream.write(line);
    }
  });

  // Handle Ctrl+C
  process.on('SIGINT', async () => {
    if (!running) return;
    running = false;
    try { ff.kill('SIGINT'); } catch {}
    if (buf.length) {
      reqStream.write(JSON.stringify({ type: 'chunk', audio_base64: base64(buf), sample_rate: SR, encoding: ENCODING, last: true }) + '\n');
      buf = Buffer.alloc(0);
    }
    reqStream.write(JSON.stringify({ type: 'end' }) + '\n');
    reqStream.end();
  });

  // Read server NDJSON response
  let lastPrinted = '';
  for await (const line of ndjsonResponseLines(res.body)) {
    try {
      const msg = JSON.parse(line);
      if (msg.type === 'transcript.final') {
        const text = (msg.text || '').trim();
        if (!text) continue;
        console.log('\n' + text + '\n');
        if (PASTE_FINAL) {
          await pbcopy(text);
          if (PASTE_LIVE) await cmdV();
        }
        lastPrinted = text;
      }
    } catch (err) {
      console.error('[parse-error]', err?.message || err);
    }
  }
}

main().catch((e) => {
  console.error('[vs-stream] fatal:', e?.stack || e);
  exit(1);
});

