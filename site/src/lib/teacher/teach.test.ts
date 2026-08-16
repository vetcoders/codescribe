import assert from "node:assert/strict";
import test from "node:test";
import { PROOF_TAKE, normalizeToken, teach } from "./teach.ts";

test("normalize strips punct and case", () => {
  assert.equal(normalizeToken("Codescribe."), "codescribe");
  assert.equal(normalizeToken("Loctree,"), "loctree");
});

test("proof take emits needs-attention and jargon lexicon hints", () => {
  const report = teach({
    live: PROOF_TAKE.live,
    whisper: PROOF_TAKE.whisper,
    human: PROOF_TAKE.human,
    label: PROOF_TAKE.label,
  });

  assert.ok(report.attention.length > 0);
  assert.ok(report.whisper_errors_vs_human > 0);

  const joined = report.lexicon_hints
    .map(
      (hint) =>
        `${hint.from_whisper.toLowerCase()}->${hint.to_human.toLowerCase()}`
    )
    .join(" | ");

  const hasJargon = report.lexicon_hints.some((hint) => {
    const from = hint.from_whisper.toLowerCase();
    const to = hint.to_human.toLowerCase();
    return (
      (from.includes("lock") && to.includes("loctree")) ||
      (from.includes("blik") && to.includes("wav")) ||
      (from.includes("wave") && to.includes("wav")) ||
      (from.includes("tooltrain") && to.includes("toolchain")) ||
      to.includes("loctree") ||
      to.includes("toolchain") ||
      to.includes("wav")
    );
  });

  assert.ok(hasJargon, `expected jargon hint, got: ${joined}`);
});

test("teach is idle until called — empty inputs stay empty", () => {
  const report = teach({ live: "", whisper: "" });
  assert.equal(report.attention.length, 0);
  assert.equal(report.lexicon_hints.length, 0);
  assert.match(report.thesis_summary, /No human ref/);
});
