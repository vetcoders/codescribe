//! Seal Atlas HTML — the quality-report surface `codescribe-corpus` writes.
//!
//! Gold visual is take 01 (`docs/quality-reports/seal-atlas.take01.html`).
//! This renderer emits a handshake-valid atlas for a corpus profile when a
//! live dump is not attached. Waveform SVG is omitted until
//! `CODESCRIBE_SEAL_ATLAS_DUMP` is present; the page is still a Seal Atlas,
//! not a Qube WER table.

use super::engine_contract::{
    ENGINE_CONTRACT_ID, QUALITY_REPORT_SURFACE, engine_contract_css, render_engine_contract_html,
};

/// Numbers Voice Lab lifts out of `.stat` cards.
#[derive(Debug, Clone)]
pub struct SealAtlasStats {
    pub word_grain: String,
    pub sealed_spans: String,
    pub per_word_spans: String,
    pub clock_lies: String,
    pub silero_threshold: String,
}

impl Default for SealAtlasStats {
    fn default() -> Self {
        Self {
            word_grain: "n/a".into(),
            sealed_spans: "n/a".into(),
            per_word_spans: "n/a".into(),
            clock_lies: "n/a".into(),
            silero_threshold: "0.5".into(),
        }
    }
}

/// One Seal Atlas HTML document.
#[derive(Debug, Clone)]
pub struct SealAtlasPage {
    pub title: String,
    pub lede: String,
    pub stats: SealAtlasStats,
    pub findings: Vec<String>,
    pub dump_present: bool,
}

impl Default for SealAtlasPage {
    fn default() -> Self {
        Self {
            title: "Seal Atlas".into(),
            lede: "One take, one PCM clock. Words from SealedSpan.words — not from the final string.".into(),
            stats: SealAtlasStats::default(),
            findings: Vec::new(),
            dump_present: false,
        }
    }
}

/// Handshake-valid Seal Atlas HTML. Title contains `Seal Atlas`.
pub fn render_seal_atlas_html(page: &SealAtlasPage) -> String {
    let title = if page.title.to_ascii_lowercase().contains("seal atlas") {
        page.title.clone()
    } else {
        format!("Seal Atlas — {}", page.title)
    };
    let stats = [
        (&page.stats.word_grain, "word-grain ≥75% speech"),
        (&page.stats.sealed_spans, "sealed spans"),
        (&page.stats.per_word_spans, "spans with per-word pins"),
        (&page.stats.clock_lies, "clock-lie"),
        (&page.stats.silero_threshold, "Silero threshold"),
    ]
    .into_iter()
    .map(|(value, label)| {
        format!(
            "<div class=\"stat\"><b>{}</b><span>{}</span></div>",
            html_escape(value),
            label
        )
    })
    .collect::<String>();

    let findings = if page.findings.is_empty() {
        "<li>No dump attached. This page is the contract surface; waveform waits on <code>CODESCRIBE_SEAL_ATLAS_DUMP</code>.</li>".into()
    } else {
        page.findings
            .iter()
            .map(|line| format!("<li>{}</li>", html_escape(line)))
            .collect::<String>()
    };

    let dump_note = if page.dump_present {
        "<p>Waveform from the live dump. Production Silero via <code>vad_atlas_probe</code>. Letter ticks = równomierna interpolacja, not measurement.</p>"
    } else {
        "<p>Waveform omitted — no <code>CODESCRIBE_SEAL_ATLAS_DUMP</code>. whisper_words still map backward onto pcm_time when a dump arrives. Per-word pins are real where they exist and not guaranteed. Utterance-grain vs word-grain stay labeled. Clock-lie is a finding.</p>"
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<meta name="engine-contract" content="{contract}"/>
<meta name="quality-report-surface" content="{surface}"/>
<title>{title}</title>
<style>
:root {{ --bg:#0e1319; --panel:#151c25; --ink:#dae4ee; --mut:#8598ab; --line:#2a3644; --ok:#3fd6a0; --warn:#e0a33c; --lie:#e5584d; }}
body {{ margin:0; background:var(--bg); color:var(--ink); font:15px/1.55 "Avenir Next", system-ui, sans-serif; }}
main {{ max-width:980px; margin:0 auto; padding:28px 20px 60px; display:flex; flex-direction:column; gap:18px; }}
h1 {{ margin:0 0 6px; font-size:26px; }}
p {{ margin:0; color:var(--mut); max-width:78ch; }}
.stats {{ display:flex; flex-wrap:wrap; gap:10px; margin-top:14px; }}
.stat {{ background:var(--panel); border:1px solid var(--line); border-radius:6px; padding:8px 14px; }}
.stat b {{ display:block; font-size:20px; font-variant-numeric:tabular-nums; }}
.stat span {{ font-size:12px; color:var(--mut); text-transform:uppercase; letter-spacing:.06em; }}
section {{ background:var(--panel); border:1px solid var(--line); border-radius:8px; padding:16px 18px; }}
.legend {{ display:flex; flex-wrap:wrap; gap:14px; font-size:12.5px; color:var(--mut); }}
code {{ color:#fecaca; }}
{css}
</style>
</head>
<body>
<main>
<header>
<h1>{title}</h1>
<p>{lede}</p>
<div class="stats">{stats}</div>
</header>
{plate}
<section>
<h2>Lanes</h2>
<p class="legend">Silero p(mowa) · word-grain · utterance-grain · clock-lie · whisper_words</p>
{dump_note}
<p>Words from <code>SealedSpan.words</code> / the live dump — never rebuilt from the final string. HQ / Cloud stay proposals.</p>
</section>
<section>
<h2>Findings</h2>
<ul>{findings}</ul>
</section>
</main>
</body>
</html>
"#,
        contract = ENGINE_CONTRACT_ID,
        surface = QUALITY_REPORT_SURFACE,
        title = html_escape(&title),
        lede = html_escape(&page.lede),
        stats = stats,
        plate = render_engine_contract_html(),
        dump_note = dump_note,
        findings = findings,
        css = engine_contract_css(),
    )
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&")
        .replace('<', "<")
        .replace('>', ">")
        .replace('"', """)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::engine_contract::validate_quality_html;

    #[test]
    fn renderer_passes_the_html_handshake() {
        let html = render_seal_atlas_html(&SealAtlasPage {
            title: "profile apple-layer0".into(),
            ..SealAtlasPage::default()
        });
        let failures = validate_quality_html(&html);
        assert!(failures.is_empty(), "{failures:?}");
        assert!(html.contains("Seal Atlas — profile apple-layer0"));
        assert!(!html.contains("Avg WER"));
        assert!(!html.contains("Codescribe Quality Report"));
    }
}
