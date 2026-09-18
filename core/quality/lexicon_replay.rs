//! Operator-facing replay job: adjudicated corrections in, three tiers out.
//!
//! The policy lives in [`crate::quality::lexicon_gate`] and the extraction in
//! [`crate::quality::overlay_quality`]; this module owns only the artifacts an
//! operator reads afterwards. It exists as a library function rather than
//! inline in a binary because two entry points call it — `codescribe lexicon
//! replay` and the legacy `qube-report --replay-corrections` — and a rejected
//! pair must land in the same file with the same reason whichever one ran.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::quality::overlay_quality::{ReplayCandidate, replay_corrections_through_extractor};

/// Where a replay put its three tiers and its report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayOutcome {
    /// Every adjudicated pair, in source order.
    pub table: Vec<ReplayCandidate>,
    /// Tier label → count, richest tier first.
    pub tier_counts: Vec<(String, usize)>,
    /// Accepted tier; the file `teach_dictionary_from_store` promotes.
    pub accepted_path: PathBuf,
    /// Quarantined tier, for manual promotion.
    pub review_path: PathBuf,
    /// Refused tier, kept so a decision stays inspectable.
    pub rejected_path: PathBuf,
    /// Human-readable summary with the full pair table.
    pub report_path: PathBuf,
}

impl ReplayOutcome {
    /// How many pairs the gate accepted.
    pub fn accepted(&self) -> usize {
        self.table.iter().filter(|row| row.accepted).count()
    }
}

/// Replay `corrections_path` through the extractor and the gate, writing the
/// three tier files and the markdown report under `out_dir`.
///
/// With `apply`, the accepted tier is additionally upserted into the live
/// lexicon (the extractor rotates a backup first). The other tiers are never
/// written to the lexicon under any flag.
pub fn run_lexicon_replay(
    corrections_path: &Path,
    out_dir: &Path,
    apply: bool,
) -> Result<ReplayOutcome> {
    let table = replay_corrections_through_extractor(corrections_path, apply)?;
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("create replay output dir {}", out_dir.display()))?;

    let accepted_path = out_dir.join("lexicon.custom.proposed.jsonl");
    let review_path = out_dir.join("lexicon.custom.review.jsonl");
    let rejected_path = out_dir.join("lexicon.custom.rejected.jsonl");
    let report_path = out_dir.join("lexicon_replay_report.md");

    let mut tier_counts: Vec<(String, usize)> = Vec::new();
    for row in &table {
        match tier_counts
            .iter_mut()
            .find(|(tier, _)| *tier == row.verdict)
        {
            Some((_, count)) => *count += 1,
            None => tier_counts.push((row.verdict.clone(), 1)),
        }
    }
    tier_counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    {
        let mut accepted = std::fs::File::create(&accepted_path)?;
        let mut review = std::fs::File::create(&review_path)?;
        let mut rejected = std::fs::File::create(&rejected_path)?;
        for row in &table {
            let line = serde_json::json!({
                "term": row.canonical,
                "mispronunciations": [row.variant],
                "source": "correction",
                "correction_id": row.correction_id,
                "source_line": row.line,
                "verdict": row.verdict,
            });
            let sink: &mut std::fs::File = if row.accepted {
                &mut accepted
            } else if row.verdict.starts_with("review:") {
                &mut review
            } else {
                &mut rejected
            };
            writeln!(sink, "{line}")?;
        }
    }

    {
        let mut report = std::fs::File::create(&report_path)?;
        writeln!(report, "# Lexicon corrections replay")?;
        writeln!(report)?;
        writeln!(report, "- source: `{}`", corrections_path.display())?;
        writeln!(report, "- candidate pairs: {}", table.len())?;
        writeln!(
            report,
            "- mode: {}",
            if apply {
                "apply (accepted tier written to the live lexicon after backup)"
            } else {
                "dry-run (live lexicon untouched)"
            }
        )?;
        writeln!(report, "- accepted file: `{}`", accepted_path.display())?;
        writeln!(report, "- review file: `{}`", review_path.display())?;
        writeln!(report, "- rejected file: `{}`", rejected_path.display())?;
        writeln!(report)?;
        writeln!(report, "## Gate tiers")?;
        writeln!(report)?;
        writeln!(report, "| tier | pairs |")?;
        writeln!(report, "| --- | --- |")?;
        for (tier, count) in &tier_counts {
            writeln!(report, "| `{tier}` | {count} |")?;
        }
        writeln!(report)?;
        writeln!(report, "## Pairs")?;
        writeln!(report)?;
        writeln!(
            report,
            "| line | correction_id | variant | canonical | verdict |"
        )?;
        writeln!(report, "| --- | --- | --- | --- | --- |")?;
        for row in &table {
            writeln!(
                report,
                "| {} | `{}` | {} | {} | `{}` |",
                row.line, row.correction_id, row.variant, row.canonical, row.verdict
            )?;
        }
    }

    Ok(ReplayOutcome {
        table,
        tier_counts,
        accepted_path,
        review_path,
        rejected_path,
        report_path,
    })
}
