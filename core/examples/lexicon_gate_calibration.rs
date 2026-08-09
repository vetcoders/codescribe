//! Calibrate the semantic gate for lexicon candidates against real data.
//!
//! The lexical gate (`is_sensible_lexicon_candidate`) admits any pair within 20
//! edit operations, which is how "Monika → weszły" and "myjnię poza → maina
//! tylko wylądowały" became rules on the operator's machine: unrelated words
//! that happen to sit close enough in edit distance. The fix is the reason
//! MiniLM was embedded in the first place — a MEANING check the string
//! heuristics cannot express.
//!
//! A threshold guessed from intuition is worthless, so this binary measures the
//! separation on two real populations before anything is wired:
//!
//!   GOOD  — accepted rules from the operator's live `lexicon.custom.jsonl`
//!           (mispronunciation → canonical term), i.e. pairs the product got right
//!   BAD   — the poisoned pairs observed in the wild, passed on the command line
//!
//! Usage:
//!   cargo run -p codescribe-core --example lexicon_gate_calibration -- \
//!     "Monika=weszły" "myjnię poza=maina tylko wylądowały" "wylądował=wylądowało"

use std::collections::BTreeMap;

use codescribe_core::embedder;

/// Nearest-rank percentile of an already-sorted slice; `NaN` when empty.
fn percentile(sorted: &[f32], p: f64) -> f32 {
    if sorted.is_empty() {
        return f32::NAN;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

/// Score both populations and print the threshold sweep.
///
/// The output is deliberately a cost/benefit table rather than a single
/// recommended number: every threshold that catches more poisoned pairs also
/// loses accepted rules, and only the operator can price that trade. Pairs
/// whose embedding came back degenerate are dropped and counted, so a
/// misbehaving embedder shows up as a number instead of silently skewing the
/// percentiles.
fn main() -> anyhow::Result<()> {
    embedder::init()?;

    let lexicon_path = std::path::PathBuf::from(std::env::var("HOME").expect("HOME"))
        .join(".codescribe/lexicon.custom.jsonl");
    let raw = std::fs::read_to_string(&lexicon_path)?;

    // GOOD population: every (mispronunciation, term) pair the product accepted.
    let mut good: Vec<(String, String, f32)> = Vec::new();
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let Some(term) = value.get("term").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(variants) = value.get("mispronunciations").and_then(|v| v.as_array()) else {
            continue;
        };
        for variant in variants.iter().filter_map(|v| v.as_str()) {
            let score = embedder::similarity(&embedder::embed(variant)?, &embedder::embed(term)?);
            good.push((variant.to_string(), term.to_string(), score));
        }
    }

    // BAD population: pairs named on the command line as `variant=canonical`.
    let mut bad: Vec<(String, String, f32)> = Vec::new();
    for arg in std::env::args().skip(1) {
        let Some((variant, canonical)) = arg.split_once('=') else {
            continue;
        };
        let score = embedder::similarity(&embedder::embed(variant)?, &embedder::embed(canonical)?);
        bad.push((variant.to_string(), canonical.to_string(), score));
    }

    let mut good_scores: Vec<f32> = good.iter().map(|(_, _, s)| *s).collect();
    let nan_count = good_scores.iter().filter(|s| s.is_nan()).count();
    good_scores.retain(|s| !s.is_nan());
    good_scores.sort_by(f32::total_cmp);
    if nan_count > 0 {
        println!("(dropped {nan_count} pairs whose embedding was degenerate)");
    }

    println!("GOOD pairs (live lexicon): n={}", good_scores.len());
    if !good_scores.is_empty() {
        println!(
            "  min={:.3} p01={:.3} p05={:.3} p10={:.3} median={:.3} max={:.3}",
            good_scores[0],
            percentile(&good_scores, 0.01),
            percentile(&good_scores, 0.05),
            percentile(&good_scores, 0.10),
            percentile(&good_scores, 0.50),
            good_scores[good_scores.len() - 1]
        );
        println!("  lowest-scoring accepted rules (these constrain the floor):");
        for (variant, term, score) in good.iter().take(0).chain({
            let mut sorted: Vec<_> = good.iter().collect();
            sorted.sort_by(|a, b| a.2.total_cmp(&b.2));
            sorted.into_iter().take(8)
        }) {
            println!("    {score:.3}  {variant} -> {term}");
        }
    }

    println!("\nBAD pairs (observed poisoning): n={}", bad.len());
    for (variant, canonical, score) in &bad {
        println!("  {score:.3}  {variant} -> {canonical}");
    }

    // What each candidate threshold would cost and buy.
    if !good_scores.is_empty() && !bad.is_empty() {
        println!("\nthreshold sweep (reject when similarity < T):");
        let mut table: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for step in 0..=12 {
            let t = 0.20 + 0.05 * step as f32;
            let good_lost = good_scores.iter().filter(|s| **s < t).count();
            let bad_caught = bad.iter().filter(|(_, _, s)| *s < t).count();
            table.insert(format!("{t:.2}"), (good_lost, bad_caught));
        }
        println!("     T     good_rules_lost   bad_pairs_caught");
        for (t, (lost, caught)) in table {
            println!(
                "  {t}   {lost:>4} / {:<5}      {caught} / {}",
                good_scores.len(),
                bad.len()
            );
        }
    }

    Ok(())
}
