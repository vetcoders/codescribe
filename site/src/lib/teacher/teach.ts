/** Same contract as `codescribe_core::quality::teacher::teach`. Pure. No mic. */

export type AttentionKind =
  | "live_only"
  | "whisper_excess"
  | "disagreement"
  | "whisper_error_at_live_weakness"
  | "live_miss_whisper_ok";

export type AlignOp =
  | { kind: "equal"; a: number; b: number }
  | { kind: "delete_a"; a: number }
  | { kind: "insert_b"; b: number }
  | { kind: "substitute"; a: number; b: number };

export type Token = { surface: string; norm: string };

export type TeacherInput = {
  live: string;
  whisper: string;
  human?: string | null;
  label?: string | null;
};

export type AttentionSpan = {
  kind: AttentionKind;
  live_tokens: string[];
  whisper_tokens: string[];
  human_tokens: string[];
  note: string;
};

export type LexiconHint = {
  from_whisper: string;
  to_human: string;
  reason: string;
};

export type TeacherReport = {
  label: string | null;
  live_token_count: number;
  whisper_token_count: number;
  human_token_count: number | null;
  equal_ops: number;
  attention: AttentionSpan[];
  lexicon_hints: LexiconHint[];
  gap_hallucination_hit_rate: number | null;
  whisper_errors_vs_human: number;
  whisper_errors_at_live_weak: number;
  thesis_summary: string;
};

const EDGE_PUNCT = new Set([
  ",",
  ".",
  ";",
  ":",
  "!",
  "?",
  '"',
  "'",
  "(",
  ")",
  "[",
  "]",
  "…",
  "—",
  "-",
]);

export function normalizeToken(token: string): string {
  let start = 0;
  let end = token.length;
  while (start < end && EDGE_PUNCT.has(token[start] ?? "")) start += 1;
  while (end > start && EDGE_PUNCT.has(token[end - 1] ?? "")) end -= 1;
  return token.slice(start, end).toLowerCase();
}

export function tokenize(text: string): Token[] {
  return text
    .split(/\s+/)
    .filter(Boolean)
    .map((surface) => ({ surface, norm: normalizeToken(surface) }));
}

export function alignWords(a: Token[], b: Token[]): AlignOp[] {
  const n = a.length;
  const m = b.length;
  const dp: number[][] = Array.from({ length: n + 1 }, () =>
    Array(m + 1).fill(0)
  );
  for (let i = n - 1; i >= 0; i -= 1) {
    for (let j = m - 1; j >= 0; j -= 1) {
      dp[i]![j] =
        a[i]!.norm === b[j]!.norm
          ? (dp[i + 1]![j + 1] ?? 0) + 1
          : Math.max(dp[i + 1]![j] ?? 0, dp[i]![j + 1] ?? 0);
    }
  }

  const ops: AlignOp[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i]!.norm === b[j]!.norm) {
      ops.push({ kind: "equal", a: i, b: j });
      i += 1;
      j += 1;
    } else if ((dp[i + 1]![j] ?? 0) >= (dp[i]![j + 1] ?? 0)) {
      ops.push({ kind: "delete_a", a: i });
      i += 1;
    } else {
      ops.push({ kind: "insert_b", b: j });
      j += 1;
    }
  }
  while (i < n) {
    ops.push({ kind: "delete_a", a: i });
    i += 1;
  }
  while (j < m) {
    ops.push({ kind: "insert_b", b: j });
    j += 1;
  }
  return coalesceSubstitutes(ops);
}

function coalesceSubstitutes(ops: AlignOp[]): AlignOp[] {
  const out: AlignOp[] = [];
  let idx = 0;
  while (idx < ops.length) {
    const cur = ops[idx];
    const next = ops[idx + 1];
    if (cur?.kind === "delete_a" && next?.kind === "insert_b") {
      out.push({ kind: "substitute", a: cur.a, b: next.b });
      idx += 2;
    } else if (cur?.kind === "insert_b" && next?.kind === "delete_a") {
      out.push({ kind: "substitute", a: next.a, b: cur.b });
      idx += 2;
    } else if (cur) {
      out.push(cur);
      idx += 1;
    } else {
      break;
    }
  }
  return out;
}

export function teach(input: TeacherInput): TeacherReport {
  const liveToks = tokenize(input.live);
  const whisperToks = tokenize(input.whisper);
  const humanToks = input.human ? tokenize(input.human) : null;

  const ops = alignWords(liveToks, whisperToks);
  const equalOps = ops.filter((op) => op.kind === "equal").length;

  const attention: AttentionSpan[] = [];
  const lexiconHints: LexiconHint[] = [];

  for (const op of ops) {
    if (op.kind === "equal") continue;
    if (op.kind === "delete_a") {
      const surface = liveToks[op.a]!.surface;
      attention.push({
        kind: "live_only",
        live_tokens: [surface],
        whisper_tokens: [],
        human_tokens: [],
        note: `Live kept «${surface}» — Whisper has no counterpart (possible Apple-only residue or Whisper drop)`,
      });
    } else if (op.kind === "insert_b") {
      const surface = whisperToks[op.b]!.surface;
      attention.push({
        kind: "whisper_excess",
        live_tokens: [],
        whisper_tokens: [surface],
        human_tokens: [],
        note: `Whisper excess «${surface}» — absent in live (gap-fill / hallucination candidate)`,
      });
    } else {
      const live = liveToks[op.a]!.surface;
      const whisper = whisperToks[op.b]!.surface;
      attention.push({
        kind: "disagreement",
        live_tokens: [live],
        whisper_tokens: [whisper],
        human_tokens: [],
        note: `Disagreement live«${live}» vs whisper«${whisper}»`,
      });
    }
  }

  let whisperErrors = 0;
  let whisperErrorsAtLiveWeak = 0;

  if (humanToks) {
    const liveSet = new Set(liveToks.map((token) => token.norm));
    const opsWh = alignWords(whisperToks, humanToks);
    for (const op of opsWh) {
      if (op.kind === "delete_a") {
        whisperErrors += 1;
        const w = whisperToks[op.a]!;
        if (!liveSet.has(w.norm)) {
          whisperErrorsAtLiveWeak += 1;
          attention.push({
            kind: "whisper_error_at_live_weakness",
            live_tokens: [],
            whisper_tokens: [w.surface],
            human_tokens: [],
            note: `Whisper«${w.surface}» not in human and not in live — excess into live gap`,
          });
        }
      } else if (op.kind === "substitute") {
        whisperErrors += 1;
        const w = whisperToks[op.a]!;
        const h = humanToks[op.b]!;
        const liveWeak = !liveSet.has(w.norm) && !liveSet.has(h.norm);
        const liveHasHuman = liveSet.has(h.norm);
        const atWeak = liveWeak || !liveHasHuman;
        if (atWeak) {
          whisperErrorsAtLiveWeak += 1;
          attention.push({
            kind: "whisper_error_at_live_weakness",
            live_tokens: [],
            whisper_tokens: [w.surface],
            human_tokens: [h.surface],
            note: `Whisper«${w.surface}» → human«${h.surface}»; live did not carry human form (weak locus)`,
          });
        }
        lexiconHints.push({
          from_whisper: w.surface,
          to_human: h.surface,
          reason: "whisper_vs_human_substitute",
        });
      }
    }

    const whisperSet = new Set(whisperToks.map((token) => token.norm));
    const opsLh = alignWords(liveToks, humanToks);
    for (const op of opsLh) {
      if (op.kind === "substitute") {
        const h = humanToks[op.b]!;
        if (whisperSet.has(h.norm)) {
          attention.push({
            kind: "live_miss_whisper_ok",
            live_tokens: [liveToks[op.a]!.surface],
            whisper_tokens: [],
            human_tokens: [h.surface],
            note: `Live«${liveToks[op.a]!.surface}» missed human«${
              h.surface
            }» which Whisper carried — Apple/live gap`,
          });
        }
      } else if (op.kind === "insert_b") {
        const h = humanToks[op.b]!;
        if (whisperSet.has(h.norm)) {
          attention.push({
            kind: "live_miss_whisper_ok",
            live_tokens: [],
            whisper_tokens: [],
            human_tokens: [h.surface],
            note: `Live omitted human«${h.surface}» present in Whisper — classic under-gen gap`,
          });
        }
      }
    }
  }

  const hitRate =
    whisperErrors > 0 ? whisperErrorsAtLiveWeak / whisperErrors : null;
  let thesisSummary: string;
  if (hitRate != null && hitRate >= 0.5) {
    thesisSummary = `LIVE-WEAK × WHISPER-ERR co-locates ${whisperErrorsAtLiveWeak}/${whisperErrors} (${Math.round(
      hitRate * 100
    )}%). Valid Apple-gap≡Whisper-halu bet ONLY if live leg is Apple; candle-live is same-family baseline / lexicon harvest.`;
  } else if (hitRate != null) {
    thesisSummary = `LIVE-WEAK × WHISPER-ERR co-locates only ${whisperErrorsAtLiveWeak}/${whisperErrors} (${Math.round(
      hitRate * 100
    )}%). Need more sessions, time-tagged gaps, or true Apple live leg (candle-live alone cannot falsify/prove the Apple bet).`;
  } else if (!input.human) {
    thesisSummary = `No human ref — emitted ${attention.length} live×whisper attention spans for Needs attention UI. Add --human to score co-location; Apple bet still needs Apple live.`;
  } else {
    thesisSummary = "No Whisper errors vs human — nothing to score.";
  }

  lexiconHints.sort((left, right) => {
    const a = `${left.from_whisper}\0${left.to_human}`;
    const b = `${right.from_whisper}\0${right.to_human}`;
    return a < b ? -1 : a > b ? 1 : 0;
  });
  const deduped: LexiconHint[] = [];
  for (const hint of lexiconHints) {
    const last = deduped[deduped.length - 1];
    if (
      last &&
      last.from_whisper === hint.from_whisper &&
      last.to_human === hint.to_human
    )
      continue;
    deduped.push(hint);
  }

  return {
    label: input.label ?? null,
    live_token_count: liveToks.length,
    whisper_token_count: whisperToks.length,
    human_token_count: humanToks ? humanToks.length : null,
    equal_ops: equalOps,
    attention,
    lexicon_hints: deduped,
    gap_hallucination_hit_rate: hitRate,
    whisper_errors_vs_human: whisperErrors,
    whisper_errors_at_live_weak: whisperErrorsAtLiveWeak,
    thesis_summary: thesisSummary,
  };
}

export const PROOF_TAKE = {
  label: "01_no-to-dobra e2e candle",
  live: "Teraz po parę słów, korzystając z surowej transkrypcji przez Codescribe mamy już pierwsze słowo do analizy wobec czego chcę, żebyś za chwilę, żebyś wziął ten plik WAV i puść i przychodził go na nasz endpoint. bo muszę mieć pewność czy leksykon działa Więc po prostu po to, aby Duże bazy kodowe przestały być tajemnicą Czarną, dziurą, na agentu veiłej. korzysta z rusta w wersji Toolchain 2024",
  whisper:
    "No to dobra, teraz generalnie powiem parę słów korzystając z surowej transkrypcji przez Codescribe. Mamy już pierwsze słowo do analizy, wobec czego chcę, żebyś wziął ten blik Wave i puścił go na... nasz endpoint, bo muszę mieć pewność czy leksykon działa, więc po prostu na temu biuz dupy. LOCK3 to aplikacja stworzona po to aby duże bazy kodowe przestały być tajemnicą i czarną dziurą dla agentów AI. Korzystam z Rust w wersji Tooltrain 2024. Dziękuję.",
  human:
    "No to dobra. Teraz generalnie powiem parę słów korzystając z surowej transkrypcji przez Codescribe (mamy już pierwsze słowo do analizy), do czego chcę, żebyś, nie wiem, żebyś wziął ten plik WAV i puścił go na nasz endpoint, bo muszę mieć pewność, hmmm, czy leksykon działa więc po [(niewyraźnie)]. Loctree to aplikacja stworzona po to, aby duże bazy kodowe przestały być tajemnicą i czarną dziurą dla agentów AI. Korzysta z Rusta, w wersji Toolchain 2024.",
};
