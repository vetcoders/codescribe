//! Teacher report: Needs attention spans + lexicon hints + hypothesis score.

use super::align::{AlignOp, align_words};
use super::tokenize::tokenize;
use serde::Serialize;

/// Inputs to one Teacher lesson (standalone proof or production session).
#[derive(Debug, Clone)]
pub struct TeacherInput {
    /// Live / Apple assembly (under-generate bias).
    pub live: String,
    /// Whisper / file final (over-generate bias).
    pub whisper: String,
    /// Optional human ground truth.
    pub human: Option<String>,
    /// Optional session label for reports.
    pub label: Option<String>,
}

/// Why a span needs attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    /// Live has a token Whisper does not (Apple kept, Whisper dropped/skipped region).
    LiveOnly,
    /// Whisper invented/excess token not in live (classic over-fill into a gap).
    WhisperExcess,
    /// Both present but disagree (substitution).
    Disagreement,
    /// Whisper disagrees with human at a locus where live also failed or was thin.
    WhisperErrorAtLiveWeakness,
    /// Live disagrees with human while Whisper matched human (live under-gen miss).
    LiveMissWhisperOk,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttentionSpan {
    pub kind: AttentionKind,
    pub live_tokens: Vec<String>,
    pub whisper_tokens: Vec<String>,
    pub human_tokens: Vec<String>,
    /// Operator-facing one-liner.
    pub note: String,
}

/// Deterministic lexicon candidate from a human-confirmed (or auto-hinted) pair.
#[derive(Debug, Clone, Serialize)]
pub struct LexiconHint {
    pub from_whisper: String,
    pub to_human: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TeacherReport {
    pub label: Option<String>,
    pub live_token_count: usize,
    pub whisper_token_count: usize,
    pub human_token_count: Option<usize>,
    pub equal_ops: usize,
    pub attention: Vec<AttentionSpan>,
    pub lexicon_hints: Vec<LexiconHint>,
    /// Among Whisper errors vs human, fraction that co-locate with live weakness.
    pub gap_hallucination_hit_rate: Option<f64>,
    pub whisper_errors_vs_human: usize,
    pub whisper_errors_at_live_weak: usize,
    pub thesis_summary: String,
}

/// Run the Teacher triangle. Pure function — CLI / HTML / tests all call this.
pub fn teach(input: TeacherInput) -> TeacherReport {
    let live_toks = tokenize(&input.live);
    let whisper_toks = tokenize(&input.whisper);
    let human_toks = input.human.as_deref().map(tokenize);

    let ops = align_words(&live_toks, &whisper_toks);
    let equal_ops = ops
        .iter()
        .filter(|o| matches!(o, AlignOp::Equal { .. }))
        .count();

    let mut attention = Vec::new();
    let mut lexicon_hints = Vec::new();

    // Live × Whisper attention (no human required).
    for op in &ops {
        match *op {
            AlignOp::Equal { .. } => {}
            AlignOp::DeleteA { a } => {
                attention.push(AttentionSpan {
                    kind: AttentionKind::LiveOnly,
                    live_tokens: vec![live_toks[a].surface.clone()],
                    whisper_tokens: vec![],
                    human_tokens: vec![],
                    note: format!(
                        "Live kept «{}» — Whisper has no counterpart (possible Apple-only residue or Whisper drop)",
                        live_toks[a].surface
                    ),
                });
            }
            AlignOp::InsertB { b } => {
                attention.push(AttentionSpan {
                    kind: AttentionKind::WhisperExcess,
                    live_tokens: vec![],
                    whisper_tokens: vec![whisper_toks[b].surface.clone()],
                    human_tokens: vec![],
                    note: format!(
                        "Whisper excess «{}» — absent in live (gap-fill / hallucination candidate)",
                        whisper_toks[b].surface
                    ),
                });
            }
            AlignOp::Substitute { a, b } => {
                attention.push(AttentionSpan {
                    kind: AttentionKind::Disagreement,
                    live_tokens: vec![live_toks[a].surface.clone()],
                    whisper_tokens: vec![whisper_toks[b].surface.clone()],
                    human_tokens: vec![],
                    note: format!(
                        "Disagreement live«{}» vs whisper«{}»",
                        live_toks[a].surface, whisper_toks[b].surface
                    ),
                });
            }
        }
    }

    // With human: score the thesis + lexicon hints.
    let mut whisper_errors = 0usize;
    let mut whisper_errors_at_live_weak = 0usize;

    if let Some(ref human) = human_toks {
        let live_set: std::collections::HashSet<_> =
            live_toks.iter().map(|t| t.norm.as_str()).collect();
        let ops_wh = align_words(&whisper_toks, human);
        for op in &ops_wh {
            match *op {
                AlignOp::InsertB { b } => {
                    // Human has token whisper missed — less relevant for "excess" thesis.
                    let _ = b;
                }
                AlignOp::DeleteA { a } => {
                    // Whisper-only vs human = insertion/hallucination.
                    whisper_errors += 1;
                    let w = &whisper_toks[a];
                    let live_weak = !live_set.contains(w.norm.as_str());
                    if live_weak {
                        whisper_errors_at_live_weak += 1;
                        attention.push(AttentionSpan {
                            kind: AttentionKind::WhisperErrorAtLiveWeakness,
                            live_tokens: vec![],
                            whisper_tokens: vec![w.surface.clone()],
                            human_tokens: vec![],
                            note: format!(
                                "Whisper«{}» not in human and not in live — excess into live gap",
                                w.surface
                            ),
                        });
                    }
                }
                AlignOp::Substitute { a, b } => {
                    whisper_errors += 1;
                    let w = &whisper_toks[a];
                    let h = &human[b];
                    let live_weak =
                        !live_set.contains(w.norm.as_str()) && !live_set.contains(h.norm.as_str());
                    // Also weak if live has neither correct human form.
                    let live_has_human = live_set.contains(h.norm.as_str());
                    let at_weak = live_weak || !live_has_human;
                    if at_weak {
                        whisper_errors_at_live_weak += 1;
                        attention.push(AttentionSpan {
                            kind: AttentionKind::WhisperErrorAtLiveWeakness,
                            live_tokens: vec![],
                            whisper_tokens: vec![w.surface.clone()],
                            human_tokens: vec![h.surface.clone()],
                            note: format!(
                                "Whisper«{}» → human«{}»; live did not carry human form (weak locus)",
                                w.surface, h.surface
                            ),
                        });
                    }
                    lexicon_hints.push(LexiconHint {
                        from_whisper: w.surface.clone(),
                        to_human: h.surface.clone(),
                        reason: "whisper_vs_human_substitute".into(),
                    });
                }
                AlignOp::Equal { .. } => {}
            }
        }

        // Live misses where Whisper got human right.
        let ops_lh = align_words(&live_toks, human);
        let whisper_set: std::collections::HashSet<_> =
            whisper_toks.iter().map(|t| t.norm.as_str()).collect();
        for op in &ops_lh {
            if let AlignOp::Substitute { a, b } = *op {
                let h = &human[b];
                if whisper_set.contains(h.norm.as_str()) {
                    attention.push(AttentionSpan {
                        kind: AttentionKind::LiveMissWhisperOk,
                        live_tokens: vec![live_toks[a].surface.clone()],
                        whisper_tokens: vec![],
                        human_tokens: vec![h.surface.clone()],
                        note: format!(
                            "Live«{}» missed human«{}» which Whisper carried — Apple/live gap",
                            live_toks[a].surface, h.surface
                        ),
                    });
                }
            }
            if let AlignOp::InsertB { b } = *op {
                let h = &human[b];
                if whisper_set.contains(h.norm.as_str()) {
                    attention.push(AttentionSpan {
                        kind: AttentionKind::LiveMissWhisperOk,
                        live_tokens: vec![],
                        whisper_tokens: vec![],
                        human_tokens: vec![h.surface.clone()],
                        note: format!(
                            "Live omitted human«{}» present in Whisper — classic under-gen gap",
                            h.surface
                        ),
                    });
                }
            }
        }
    }

    let hit_rate = if whisper_errors > 0 {
        Some(whisper_errors_at_live_weak as f64 / whisper_errors as f64)
    } else {
        None
    };

    // Hit-rate is engine-agnostic: "live weakness × whisper error co-location".
    // It only measures the *Apple* bet when live is Apple (see label / caller).
    // Candle-live × whisper-file is a baseline / lexicon harvest, not that bet.
    let thesis_summary = match hit_rate {
        Some(r) if r >= 0.5 => format!(
            "LIVE-WEAK × WHISPER-ERR co-locates {whisper_errors_at_live_weak}/{whisper_errors} \
             ({:.0}%). Valid Apple-gap≡Whisper-halu bet ONLY if live leg is Apple; \
             candle-live is same-family baseline / lexicon harvest.",
            r * 100.0
        ),
        Some(r) => format!(
            "LIVE-WEAK × WHISPER-ERR co-locates only {whisper_errors_at_live_weak}/{whisper_errors} \
             ({:.0}%). Need more sessions, time-tagged gaps, or true Apple live leg \
             (candle-live alone cannot falsify/prove the Apple bet).",
            r * 100.0
        ),
        None if input.human.is_none() => format!(
            "No human ref — emitted {} live×whisper attention spans for Needs attention UI. \
             Add --human to score co-location; Apple bet still needs Apple live.",
            attention.len()
        ),
        None => "No Whisper errors vs human — nothing to score.".into(),
    };

    // Dedup lexicon hints by pair.
    lexicon_hints
        .sort_by(|a, b| (&a.from_whisper, &a.to_human).cmp(&(&b.from_whisper, &b.to_human)));
    lexicon_hints.dedup_by(|a, b| a.from_whisper == b.from_whisper && a.to_human == b.to_human);

    TeacherReport {
        label: input.label,
        live_token_count: live_toks.len(),
        whisper_token_count: whisper_toks.len(),
        human_token_count: human_toks.as_ref().map(|h| h.len()),
        equal_ops,
        attention,
        lexicon_hints,
        gap_hallucination_hit_rate: hit_rate,
        whisper_errors_vs_human: whisper_errors,
        whisper_errors_at_live_weak,
        thesis_summary,
    }
}

/// Render a self-contained HTML report (browser-openable proof).
pub fn report_to_html(report: &TeacherReport) -> String {
    let mut rows = String::new();
    for (i, span) in report.attention.iter().enumerate() {
        let kind = format!("{:?}", span.kind);
        rows.push_str(&format!(
            "<tr><td>{}</td><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>\n",
            i + 1,
            html_escape(&kind),
            html_escape(&span.live_tokens.join(" ")),
            html_escape(&span.whisper_tokens.join(" ")),
            html_escape(&span.human_tokens.join(" ")),
            html_escape(&span.note),
        ));
    }
    let mut lex = String::new();
    for h in &report.lexicon_hints {
        lex.push_str(&format!(
            "<li><code>{}</code> → <code>{}</code> <em>({})</em></li>\n",
            html_escape(&h.from_whisper),
            html_escape(&h.to_human),
            html_escape(&h.reason),
        ));
    }
    let hit = report
        .gap_hallucination_hit_rate
        .map(|r| format!("{:.1}%", r * 100.0))
        .unwrap_or_else(|| "n/a".into());
    format!(
        r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8"/>
<title>Codescribe Teacher — proof</title>
<style>
  body {{ font-family: ui-sans-serif, system-ui, sans-serif; margin: 2rem; background: #0f1115; color: #e8eaed; }}
  h1 {{ font-size: 1.4rem; }}
  .thesis {{ background: #1a2332; border-left: 4px solid #6ee7b7; padding: 1rem 1.2rem; margin: 1rem 0; }}
  table {{ border-collapse: collapse; width: 100%; font-size: 0.9rem; }}
  th, td {{ border: 1px solid #333; padding: 0.4rem 0.6rem; vertical-align: top; }}
  th {{ background: #1c1f26; text-align: left; }}
  code {{ color: #fbbf24; }}
  .meta {{ color: #9aa0a6; font-size: 0.85rem; }}
</style></head><body>
<h1>Codescribe Teacher</h1>
<p class="meta">label: {label} · live tokens: {live_n} · whisper tokens: {wh_n} ·
equal ops: {eq} · whisper errors: {we} · at live-weak: {wl} · hit-rate: {hit}</p>
<div class="thesis"><strong>Thesis:</strong> {thesis}</div>
<h2>Needs attention</h2>
<table>
<tr><th>#</th><th>kind</th><th>live</th><th>whisper</th><th>human</th><th>note</th></tr>
{rows}
</table>
<h2>Lexicon hints</h2>
<ul>{lex}</ul>
<p class="meta">Generated by codescribe-teacher · stream floor + Whisper excess → human → lexicon</p>
</body></html>
"#,
        label = html_escape(report.label.as_deref().unwrap_or("(untitled)")),
        live_n = report.live_token_count,
        wh_n = report.whisper_token_count,
        eq = report.equal_ops,
        we = report.whisper_errors_vs_human,
        wl = report.whisper_errors_at_live_weak,
        hit = hit,
        thesis = html_escape(&report.thesis_summary),
        rows = rows,
        lex = if lex.is_empty() {
            "<li><em>none</em></li>".into()
        } else {
            lex
        },
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
