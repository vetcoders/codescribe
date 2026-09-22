//! Recovery of a custom lexicon from one of its own rotation backups.
//!
//! WHY THIS EXISTS. `--replay-corrections --apply` copies the live lexicon to
//! `.lexicon.custom.jsonl.bak-replay-<unix>` before every write, and those
//! rotations are the only record of months of curation: on 2026-09-18 the live
//! file held one row while the 2026-08-16 backup held 781 rows and 2191 variant
//! spellings, grown from 678 rows on 2026-07-18.
//!
//! **A shrunken lexicon is not evidence of a bug.** The 2026-09-18 clearing was
//! the operator's own decision — "leksykon wywaliłem ja - był poisoned i psuł
//! najprostsze transkrypcje" — so this module restores only when asked, never
//! on its own initiative, and `lexicon show` reports a recoverable backup as
//! information rather than as a fault.
//!
//! The two provenances in a backup are not equally trustworthy:
//!
//! - rows with `source: "correction"` were extracted automatically and are
//!   always re-adjudicated through [`crate::quality::lexicon_gate`]; the
//!   2026-08-16 backup shows why (`to <- ten`, `kiedy <- jak`, `Zerknij <- tak`).
//! - rows without a `source` are hand-written. They come back verbatim by
//!   default — but "hand-written" does not mean "correct". The same backup
//!   carries the curated row `Monika <- Monia, Moniki, Monikę, Monisia`, whose
//!   middle two entries are inflections of the term itself, so speaking
//!   "Moniki" gets rewritten to "Monika" and the grammar of the sentence is
//!   destroyed. That class is what the operator was clearing. `gate_curated`
//!   therefore puts the hand-written half through the same admission gate.
//!
//! The merge is a union keyed on the casefolded term, so restoring never drops
//! a rule the live file gained after the backup was taken.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::quality::lexicon_gate::{ProtectedTerms, adjudicate_lexicon_candidates};

/// One lexicon row as it lives on disk, in the shape the loader already reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LexiconRow {
    term: String,
    #[serde(default)]
    mispronunciations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
}

/// What a restore did, in the numbers an operator needs to trust it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexiconRestoreReport {
    /// Rows read from the backup file.
    pub backup_rows: usize,
    /// Hand-curated backup rows taken verbatim.
    pub curated_rows_restored: usize,
    /// `source: correction` backup rows re-adjudicated by the gate.
    pub auto_rows_examined: usize,
    /// Variant→canonical pairs from those rows the gate accepted.
    pub auto_pairs_accepted: usize,
    /// Rows already present in the live lexicon before the restore.
    pub live_rows_before: usize,
    /// Rows in the live lexicon after the merge.
    pub live_rows_after: usize,
    /// Distinct variant spellings after the merge.
    pub variants_after: usize,
}

/// Parse a lexicon JSONL file, skipping blank and malformed lines.
///
/// A malformed line is warned about rather than fatal: a half-written row must
/// not make a whole recovery impossible.
fn read_rows(path: &Path) -> Result<Vec<LexiconRow>> {
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- backup path is operator-supplied on the command line for offline recovery; matches the justified suppression on the replay reader in overlay_quality.rs.
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("read lexicon {}", path.display()));
        }
    };
    let mut rows = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<LexiconRow>(line) {
            Ok(row) if !row.term.trim().is_empty() => rows.push(row),
            Ok(_) => tracing::warn!("restore: skip row without a term at line {}", index + 1),
            Err(error) => {
                tracing::warn!("restore: skip malformed line {}: {}", index + 1, error)
            }
        }
    }
    Ok(rows)
}

/// Fold rows into a term-keyed map, unioning variant lists.
///
/// Later rows win on casing of the term itself but never drop an earlier
/// variant: a restore must be additive or it is not a restore.
fn merge_into(
    target: &mut BTreeMap<String, LexiconRow>,
    rows: impl IntoIterator<Item = LexiconRow>,
) {
    for row in rows {
        let key: String = row
            .term
            .trim()
            .chars()
            .flat_map(char::to_lowercase)
            .collect();
        let entry = target.entry(key).or_insert_with(|| LexiconRow {
            term: row.term.trim().to_string(),
            mispronunciations: Vec::new(),
            source: row.source.clone(),
        });
        // Curation outranks extraction: once a human has owned a term, a later
        // auto row must not relabel it as machine-derived.
        if row.source.is_none() {
            entry.source = None;
        }
        for variant in row.mispronunciations {
            let variant = variant.trim().to_string();
            if variant.is_empty() {
                continue;
            }
            let already = entry
                .mispronunciations
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&variant));
            if !already {
                entry.mispronunciations.push(variant);
            }
        }
    }
}

/// Restore the live custom lexicon from `backup_path`, merging into whatever is
/// there now.
///
/// `source: correction` rows are always re-gated. Curated rows return verbatim
/// unless `gate_curated`, which puts them through the same admission gate — use
/// it when the backup is known to carry hand-written poison such as inflections
/// listed as mispronunciations. The live file is replaced atomically, and the
/// caller is responsible for having made its own backup first: this function
/// does not rotate, because the backup it is reading from may be the only copy
/// left.
pub fn restore_custom_lexicon_from_backup(
    backup_path: &Path,
    gate_curated: bool,
) -> Result<LexiconRestoreReport> {
    let backup_rows = read_rows(backup_path)?;
    anyhow::ensure!(
        !backup_rows.is_empty(),
        "backup {} holds no usable lexicon rows",
        backup_path.display()
    );

    let config_dir = Config::config_dir();
    let live_path = config_dir.join("lexicon.custom.jsonl");
    let live_rows = read_rows(&live_path)?;

    let (auto_rows, curated_rows): (Vec<LexiconRow>, Vec<LexiconRow>) = backup_rows
        .iter()
        .cloned()
        .partition(|row| row.source.as_deref() == Some("correction"));

    // Re-adjudicate the auto-extracted half, pair by pair, under the same gate
    // that guards new extraction.
    let protected = ProtectedTerms::load_from(&ProtectedTerms::default_path(&config_dir));
    let pairs: Vec<(String, String)> = auto_rows
        .iter()
        .flat_map(|row| {
            row.mispronunciations
                .iter()
                .map(move |variant| (variant.clone(), row.term.clone()))
        })
        .collect();
    let verdicts = adjudicate_lexicon_candidates(&pairs, &protected);
    let mut gated_auto: BTreeMap<String, LexiconRow> = BTreeMap::new();
    let mut auto_pairs_accepted = 0usize;
    for ((variant, canonical), verdict) in pairs.iter().zip(&verdicts) {
        if !verdict.is_accepted() {
            continue;
        }
        auto_pairs_accepted += 1;
        merge_into(
            &mut gated_auto,
            [LexiconRow {
                term: canonical.clone(),
                mispronunciations: vec![variant.clone()],
                source: Some("correction".to_string()),
            }],
        );
    }

    // Optionally hold the hand-written half to the same bar as the extracted
    // half. Off by default: a human who wrote a rule usually meant it.
    let curated_for_merge: Vec<LexiconRow> = if gate_curated {
        let curated_pairs: Vec<(String, String)> = curated_rows
            .iter()
            .flat_map(|row| {
                row.mispronunciations
                    .iter()
                    .map(move |variant| (variant.clone(), row.term.clone()))
            })
            .collect();
        let curated_verdicts = adjudicate_lexicon_candidates(&curated_pairs, &protected);
        let mut kept: BTreeMap<String, LexiconRow> = BTreeMap::new();
        for ((variant, canonical), verdict) in curated_pairs.iter().zip(&curated_verdicts) {
            if !verdict.is_accepted() {
                continue;
            }
            merge_into(
                &mut kept,
                [LexiconRow {
                    term: canonical.clone(),
                    mispronunciations: vec![variant.clone()],
                    source: None,
                }],
            );
        }
        kept.into_values().collect()
    } else {
        curated_rows.clone()
    };

    // Live first, so a rule learned after the backup survives; then curated
    // history; then the surviving auto rows.
    let mut merged: BTreeMap<String, LexiconRow> = BTreeMap::new();
    merge_into(&mut merged, live_rows.iter().cloned());
    merge_into(&mut merged, curated_for_merge.iter().cloned());
    merge_into(&mut merged, gated_auto.into_values());

    let mut out = String::new();
    for row in merged.values() {
        out.push_str(&serde_json::to_string(row)?);
        out.push('\n');
    }
    let tmp = live_path.with_file_name(format!(
        ".lexicon.custom.jsonl.tmp.restore.{}",
        std::process::id()
    ));
    std::fs::write(&tmp, out.as_bytes())
        .with_context(|| format!("write staged lexicon {}", tmp.display()))?;
    std::fs::rename(&tmp, &live_path)
        .with_context(|| format!("replace lexicon {}", live_path.display()))?;

    Ok(LexiconRestoreReport {
        backup_rows: backup_rows.len(),
        curated_rows_restored: curated_rows.len(),
        auto_rows_examined: auto_rows.len(),
        auto_pairs_accepted,
        live_rows_before: live_rows.len(),
        live_rows_after: merged.len(),
        variants_after: merged.values().map(|row| row.mispronunciations.len()).sum(),
    })
}

/// Newest `.lexicon.custom.jsonl.bak-replay-*` in `dir` that holds more rows
/// than the live file, or `None` when no backup improves on the current state.
///
/// "Newest" is the rotation suffix, not mtime: a file copy preserves neither.
pub fn newest_recoverable_backup(dir: &Path) -> Option<std::path::PathBuf> {
    let live_terms = curated_terms(&read_rows(&dir.join("lexicon.custom.jsonl")).ok()?);
    let mut best: Option<(u64, usize, std::path::PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(stamp) = name.strip_prefix(".lexicon.custom.jsonl.bak-replay-") else {
            continue;
        };
        let Ok(stamp) = stamp.parse::<u64>() else {
            continue;
        };
        let path = entry.path();
        let Ok(rows) = read_rows(&path) else {
            continue;
        };
        // Only curated terms count. Auto rows the gate refuses stay missing
        // from the live file by design, so counting them would report every
        // completed restore as still outstanding.
        let missing = curated_terms(&rows).difference(&live_terms).count();
        if missing == 0 {
            continue;
        }
        let better = match &best {
            None => true,
            Some((best_stamp, best_missing, _)) => {
                missing > *best_missing || (missing == *best_missing && stamp > *best_stamp)
            }
        };
        if better {
            best = Some((stamp, missing, path));
        }
    }
    best.map(|(_, _, path)| path)
}

/// What a removal took out of the live lexicon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexiconRemovalReport {
    /// Rows deleted outright.
    pub rows_removed: usize,
    /// Variant spellings deleted from rows that survived.
    pub variants_removed: usize,
    /// Rows in the lexicon afterwards.
    pub rows_after: usize,
    /// Backup taken before the write, when one was due.
    pub backup: Option<std::path::PathBuf>,
}

/// Delete a term, or one variant of it, from the live lexicon.
///
/// WHY THIS EXISTS. On 2026-08-22 the operator had to fight a rule out of the
/// dictionary by hand, because nothing in the product could remove one. A
/// lexicon that can only grow is a lexicon whose mistakes are permanent — and a
/// rule keyed on a name ("Klaudiusz") is precisely the kind that must be
/// removable in one command rather than by editing JSONL.
///
/// With `variant`, only that spelling goes; a row left with no variants is
/// dropped, because a rule with nothing to match is a husk. Matching is
/// casefolded on both sides. A backup is rotated first.
pub fn remove_from_custom_lexicon(
    term: &str,
    variant: Option<&str>,
) -> Result<LexiconRemovalReport> {
    let config_dir = Config::config_dir();
    let path = config_dir.join("lexicon.custom.jsonl");
    let rows = read_rows(&path)?;
    anyhow::ensure!(
        !rows.is_empty(),
        "custom lexicon {} is empty or missing",
        path.display()
    );

    let wanted: String = term.trim().chars().flat_map(char::to_lowercase).collect();
    let wanted_variant = variant.map(|v| {
        v.trim()
            .chars()
            .flat_map(char::to_lowercase)
            .collect::<String>()
    });

    let mut kept: Vec<LexiconRow> = Vec::new();
    let mut rows_removed = 0usize;
    let mut variants_removed = 0usize;
    let mut matched = false;

    for mut row in rows {
        let key: String = row
            .term
            .trim()
            .chars()
            .flat_map(char::to_lowercase)
            .collect();
        if key != wanted {
            kept.push(row);
            continue;
        }
        matched = true;
        match &wanted_variant {
            None => rows_removed += 1,
            Some(target) => {
                let before = row.mispronunciations.len();
                row.mispronunciations.retain(|m| {
                    let folded: String = m.trim().chars().flat_map(char::to_lowercase).collect();
                    folded != *target
                });
                variants_removed += before - row.mispronunciations.len();
                if row.mispronunciations.is_empty() {
                    rows_removed += 1;
                } else {
                    kept.push(row);
                }
            }
        }
    }

    anyhow::ensure!(matched, "no lexicon row for term {term:?}");
    anyhow::ensure!(
        rows_removed + variants_removed > 0,
        "term {term:?} carries no variant {:?}",
        variant.unwrap_or_default()
    );

    let backup = rotate_lexicon_backup_if_stale(&config_dir);
    let mut out = String::new();
    for row in &kept {
        out.push_str(&serde_json::to_string(row)?);
        out.push('\n');
    }
    let tmp = path.with_file_name(format!(
        ".lexicon.custom.jsonl.tmp.remove.{}",
        std::process::id()
    ));
    std::fs::write(&tmp, out.as_bytes())
        .with_context(|| format!("write staged lexicon {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("replace lexicon {}", path.display()))?;

    Ok(LexiconRemovalReport {
        rows_removed,
        variants_removed,
        rows_after: kept.len(),
        backup,
    })
}

/// A rotation backup that holds hand-curated rows, if any exists in `dir`.
///
/// Used by the live upsert path as a *tripwire*, not for recovery: when the
/// lexicon file cannot be read but a curated backup is sitting right next to
/// it, the correct response is to stop, not to create a fresh one-row lexicon
/// on top of the loss. See `overlay_quality::upsert_corrections_unlocked`.
pub fn any_backup_with_curated_rows(dir: &Path) -> Option<std::path::PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(".lexicon.custom.jsonl.bak-replay-") {
            continue;
        }
        let path = entry.path();
        let Ok(rows) = read_rows(&path) else {
            continue;
        };
        if !curated_terms(&rows).is_empty() {
            return Some(path);
        }
    }
    None
}

/// Copy the current lexicon aside as a rotation backup, at most once an hour.
///
/// The `--apply` replay has always rotated; the live learning path never did,
/// which is why the 2026-09-18 truncation left no copy of the moment it
/// happened. Rate-limiting keeps a per-pair learner from filling the directory
/// while still guaranteeing that any given hour of curation is recoverable.
pub fn rotate_lexicon_backup_if_stale(dir: &Path) -> Option<std::path::PathBuf> {
    const MIN_BACKUP_INTERVAL_SECS: u64 = 3600;
    let live = dir.join("lexicon.custom.jsonl");
    if read_rows(&live).map(|rows| rows.is_empty()).unwrap_or(true) {
        return None;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let newest = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .strip_prefix(".lexicon.custom.jsonl.bak-replay-")
                .and_then(|stamp| stamp.parse::<u64>().ok())
        })
        .max()
        .unwrap_or(0);
    if now.saturating_sub(newest) < MIN_BACKUP_INTERVAL_SECS {
        return None;
    }
    let backup = dir.join(format!(".lexicon.custom.jsonl.bak-replay-{now}"));
    std::fs::copy(&live, &backup).ok()?;
    Some(backup)
}

/// Casefolded terms of the hand-curated rows — the work a restore must recover.
fn curated_terms(rows: &[LexiconRow]) -> std::collections::BTreeSet<String> {
    rows.iter()
        .filter(|row| row.source.as_deref() != Some("correction"))
        .map(|row| {
            row.term
                .trim()
                .chars()
                .flat_map(char::to_lowercase)
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_curated_row_survives_the_gate_untouched() {
        // `100k <- sto tysięcy` is a phrase rewrite by token count and would be
        // rejected as an extraction candidate. As curation it is authoritative.
        let row = LexiconRow {
            term: "100k".to_string(),
            mispronunciations: vec!["sto tysięcy".to_string(), "sto ka".to_string()],
            source: None,
        };
        let mut merged = BTreeMap::new();
        merge_into(&mut merged, [row]);
        assert_eq!(merged["100k"].mispronunciations.len(), 2);
        assert!(merged["100k"].source.is_none(), "curation keeps no source");
    }

    #[test]
    fn merging_is_additive_and_case_insensitive_on_the_term() {
        let mut merged = BTreeMap::new();
        merge_into(
            &mut merged,
            [LexiconRow {
                term: "Codescribe".to_string(),
                mispronunciations: vec!["Cowscribe".to_string()],
                source: None,
            }],
        );
        merge_into(
            &mut merged,
            [LexiconRow {
                term: "codescribe".to_string(),
                mispronunciations: vec!["kodskrajb".to_string(), "COWSCRIBE".to_string()],
                source: Some("correction".to_string()),
            }],
        );
        assert_eq!(merged.len(), 1, "one term, not two casings");
        let row = &merged["codescribe"];
        assert_eq!(row.term, "Codescribe", "first spelling of the term wins");
        assert_eq!(
            row.mispronunciations,
            vec!["Cowscribe".to_string(), "kodskrajb".to_string()],
            "new variant added, duplicate casing not repeated"
        );
    }

    #[test]
    fn a_blank_variant_never_enters_the_merge() {
        let mut merged = BTreeMap::new();
        merge_into(
            &mut merged,
            [LexiconRow {
                term: "grepa".to_string(),
                mispronunciations: vec!["  ".to_string(), "grypa".to_string()],
                source: None,
            }],
        );
        assert_eq!(merged["grepa"].mispronunciations, vec!["grypa".to_string()]);
    }

    #[test]
    fn a_malformed_line_does_not_abort_the_read() {
        let dir = std::env::temp_dir().join(format!("codescribe-restore-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("lex.jsonl");
        std::fs::write(
            &path,
            "{\"term\":\"grepa\",\"mispronunciations\":[\"grypa\"]}\nnot json\n\n{\"term\":\"Bashu\"}\n",
        )
        .expect("write");
        let rows = read_rows(&path).expect("read");
        assert_eq!(rows.len(), 2, "the good rows both survive");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn only_a_backup_holding_unrecovered_curation_is_offered() {
        let dir = std::env::temp_dir().join(format!("codescribe-pick-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(
            dir.join("lexicon.custom.jsonl"),
            "{\"term\":\"a\"}\n{\"term\":\"b\"}\n",
        )
        .expect("write live");
        std::fs::write(
            dir.join(".lexicon.custom.jsonl.bak-replay-1000"),
            "{\"term\":\"a\"}\n",
        )
        .expect("write thin backup");
        assert!(newest_recoverable_backup(&dir).is_none());

        std::fs::write(
            dir.join(".lexicon.custom.jsonl.bak-replay-2000"),
            "{\"term\":\"a\"}\n{\"term\":\"b\"}\n{\"term\":\"c\"}\n",
        )
        .expect("write rich backup");
        let picked = newest_recoverable_backup(&dir).expect("the richer backup is offered");
        assert!(picked.to_string_lossy().ends_with("bak-replay-2000"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_completed_restore_stops_advertising_itself() {
        // The backup keeps MORE rows than the live file on purpose: its
        // `source: correction` rows were refused by the gate. Row counts would
        // call this recovery outstanding forever; curated coverage does not.
        let dir = std::env::temp_dir().join(format!("codescribe-done-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(
            dir.join(".lexicon.custom.jsonl.bak-replay-3000"),
            "{\"term\":\"grepa\"}\n             {\"term\":\"to\",\"mispronunciations\":[\"ten\"],\"source\":\"correction\"}\n             {\"term\":\"kiedy\",\"mispronunciations\":[\"jak\"],\"source\":\"correction\"}\n",
        )
        .expect("write backup");
        std::fs::write(dir.join("lexicon.custom.jsonl"), "{\"term\":\"grepa\"}\n")
            .expect("write live");

        assert!(
            newest_recoverable_backup(&dir).is_none(),
            "all curated work is already live; only refused auto rows differ"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
