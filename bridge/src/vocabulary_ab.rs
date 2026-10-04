//! Runs the exact T2 runner as an app descendant, inheriting its Speech grant.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use codescribe_core::stt::recognizer_vocabulary::RecognizerVocabulary;

use crate::CsError;

static RUNNING: AtomicBool = AtomicBool::new(false);
struct RunGuard;
impl RunGuard {
    fn acquire() -> Result<Self, CsError> {
        RUNNING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| CsError::Runtime {
                msg: "Vocabulary A/B already running".into(),
            })?;
        Ok(Self)
    }
}
impl Drop for RunGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::Release);
    }
}

/// Swift checks the developer/Lab gate before entering this blocking worker.
/// The executable is resolved beside the host app, never from URL input or PATH.
#[uniffi::export]
pub fn run_vocabulary_ab(sample: u32) -> Result<String, CsError> {
    if !(2..=100).contains(&sample) {
        return Err(CsError::Runtime {
            msg: "Invalid vocabulary sample count".into(),
        });
    }
    let _guard = RunGuard::acquire()?;
    run(sample).map_err(|_| CsError::Runtime {
        msg: "Vocabulary A/B could not complete; inspect the lab metrics if present".into(),
    })
}

fn run(sample: u32) -> anyhow::Result<String> {
    let bridge = std::env::current_exe()?
        .parent()
        .ok_or_else(|| anyhow::anyhow!("No app executable directory"))?
        .join("codescribe-stt-bridge");
    anyhow::ensure!(bridge.is_file(), "Missing bundled Apple bridge");
    let vocabulary = RecognizerVocabulary::load_read_only();
    let result_name = format!("{}.json", chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ"));
    let payload = serde_json::json!({
        "bridge": bridge, "count": sample, "result_name": result_name,
        "vocabulary": {
            "terms": vocabulary.apple_contextual_strings().unwrap_or_default(),
            "sources": {"active_names": vocabulary.sources.active_names,
                "custom_canonicals": vocabulary.sources.custom_canonicals,
                "protected_terms": vocabulary.sources.protected_terms}
        }
    });
    // Embed the shared runner: no extra install resource or runtime script path.
    let script = concat!(
        "__name__ = 'codescribe_app_lab'\n",
        include_str!("../../scripts/stt-vocabulary-ab.py"),
        "\nraise SystemExit(app_main())\n"
    );
    let mut child = Command::new("/usr/bin/python3")
        .args(["-I", "-c", script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let input = serde_json::to_vec(&payload)?;
    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("No runner input"))?
        .write_all(&input);
    if let Err(error) = write_result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }
    let output = child.wait_with_output()?;
    // The runner emits only metric records; log exactly its final numeric summary.
    if let Some(line) = output
        .stdout
        .split(|byte| *byte == b'\n')
        .rfind(|line| !line.is_empty())
    {
        let summary: serde_json::Value = serde_json::from_slice(line)?;
        tracing::info!(
            selected = summary["selected"].as_u64(),
            completed_pairs = summary["completed_pairs"].as_u64(),
            baseline_hits = summary["baseline_hits"].as_u64(),
            vocabulary_hits = summary["vocabulary_hits"].as_u64(),
            omission_red_flags = summary["omission_red_flags"].as_u64(),
            insertion_count = summary["insertion_count"].as_u64(),
            "Vocabulary A/B finished"
        );
    }
    anyhow::ensure!(output.status.success(), "Incomplete vocabulary replay");
    Ok(result_name)
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_one_job_and_release_on_error() {
        assert!(super::run_vocabulary_ab(1).is_err());
        assert!(super::run_vocabulary_ab(101).is_err());
        let first = super::RunGuard::acquire().unwrap();
        assert!(super::RunGuard::acquire().is_err());
        drop(first);
        assert!(super::RunGuard::acquire().is_ok());
    }
}
