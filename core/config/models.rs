//! Runtime fallback model management for Whisper models.
//!
//! This module owns the runtime Whisper fallback truth for the `develop`
//! branch. If embedded Whisper is unavailable, every caller should resolve a
//! model from here instead of re-implementing its own precedence rules.

use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::{Path, PathBuf};

use crate::hf_cache;

/// Default Whisper model name used for runtime fallback lookup.
pub const DEFAULT_MODEL: &str = "whisper-large-v3-turbo";
/// Hugging Face repo backing [`DEFAULT_MODEL`], used for cache lookup and for
/// the Settings → Dictation download. fp16 weights: no q8→F32 dequantization
/// on load, at the cost of a larger download than the q8 repo.
pub const DEFAULT_WHISPER_REPO: &str = "mlx-community/whisper-large-v3-turbo";
/// Previous default (q8). Kept as a resolution fallback so installs that
/// already carry it keep transcribing without a re-download.
pub const LEGACY_MODEL: &str = "whisper-large-v3-turbo-mlx-q8";
/// Repo behind [`LEGACY_MODEL`]. Also the companion source for
/// [`COMPANION_MODEL_FILES`], which [`DEFAULT_WHISPER_REPO`] does not ship.
pub const LEGACY_WHISPER_REPO: &str = "LibraxisAI/whisper-large-v3-turbo-mlx-q8";
/// Files the mlx-community repo does not ship. Both are quantization-independent
/// (same tokenizer and mel filterbank across q8/fp16), so composing them from
/// the legacy repo yields a correct model directory.
const COMPANION_MODEL_FILES: [&str; 2] = ["tokenizer.json", "mel_filters.npz"];

/// Files that must all be present for a directory to count as a usable model.
const REQUIRED_MODEL_FILES: [&str; 3] = ["config.json", "tokenizer.json", "mel_filters.npz"];
/// Weight file names, of which **any one** satisfies the completeness check —
/// upstream repos ship either `model.safetensors` or `weights.safetensors`.
const REQUIRED_MODEL_WEIGHTS: [&str; 2] = ["weights.safetensors", "model.safetensors"];

/// Canonicalize a path, falling back to the original on failure.
///
/// Resolution must not fail just because a path cannot be canonicalized (a
/// symlink into an unmounted volume, a permission gap): the caller gets a usable
/// path and a warning rather than an error.
fn canonicalize_or_self(path: PathBuf) -> PathBuf {
    match path.canonicalize() {
        Ok(canonical) => canonical,
        Err(err) => {
            tracing::warn!(
                "canonicalize failed for {} ({err}); using non-canonical path",
                path.display()
            );
            path
        }
    }
}

/// Whether `path` holds a fully usable Whisper model.
///
/// Requires every [`REQUIRED_MODEL_FILES`] entry plus at least one of
/// [`REQUIRED_MODEL_WEIGHTS`]. This is the gate that keeps half-downloaded
/// directories from being advertised or resolved as loadable models.
fn is_complete_whisper_model_dir(path: &Path) -> bool {
    REQUIRED_MODEL_FILES
        .iter()
        .all(|name| path.join(name).exists())
        && REQUIRED_MODEL_WEIGHTS
            .iter()
            .any(|name| path.join(name).exists())
}

/// Whether a candidate models root owns at least one complete Whisper model.
///
/// A bundled `Resources/models` directory may contain only another model
/// family (for example the semantic embedder). Treating mere directory
/// existence as Whisper ownership shadows the user-installed fp16 model and
/// incorrectly drops runtime resolution to the legacy q8 cache.
fn models_root_contains_complete_whisper_model(path: &Path) -> bool {
    fs::read_dir(path).is_ok_and(|entries| {
        entries
            .filter_map(std::result::Result::ok)
            .any(|entry| is_complete_whisper_model_dir(&entry.path()))
    })
}

/// Find a complete Hugging Face cache snapshot for a model reference.
///
/// A reference containing `/` is treated as a repo id and looked up directly.
/// The bare [`DEFAULT_MODEL`] alias maps to [`DEFAULT_WHISPER_REPO`]. Any other
/// bare name is a models-dir alias, not a repo, so it yields `None` here.
fn hf_snapshot_for_model(model_ref: &str) -> Option<PathBuf> {
    let trimmed = model_ref.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.contains('/') {
        return hf_cache::find_snapshot_with_any(
            trimmed,
            &REQUIRED_MODEL_FILES,
            &REQUIRED_MODEL_WEIGHTS,
        );
    }

    if trimmed == DEFAULT_MODEL {
        return hf_cache::find_snapshot_with_any(
            DEFAULT_WHISPER_REPO,
            &REQUIRED_MODEL_FILES,
            &REQUIRED_MODEL_WEIGHTS,
        );
    }

    if trimmed == LEGACY_MODEL {
        return hf_cache::find_snapshot_with_any(
            LEGACY_WHISPER_REPO,
            &REQUIRED_MODEL_FILES,
            &REQUIRED_MODEL_WEIGHTS,
        );
    }

    None
}

/// Owner of the resolved runtime models directory.
///
/// Scope is deliberately narrow: it locates and inspects model directories on
/// disk. It performs no loading and no downloading, and it is only consulted
/// when embedded Whisper is unavailable.
pub struct ModelManager {
    models_dir: PathBuf,
}

impl ModelManager {
    /// Create a new ModelManager for runtime fallback lookup.
    ///
    /// Resolves the runtime models directory:
    /// 1. Bundled .app: Contents/Resources/models/
    /// 2. Development: ./models/ relative to executable
    /// 3. Fallback: ~/.codescribe/models/
    pub fn new() -> Result<Self> {
        let models_dir = Self::resolve_models_dir()?;
        Ok(Self { models_dir })
    }

    /// Locate the models directory, creating the user-level fallback if needed.
    ///
    /// Order: `CODESCRIBE_MODELS_DIR` override, bundled `Contents/Resources/models`,
    /// the development tree two levels above the executable, a repo-root-relative
    /// `../../models`, and finally `~/.codescribe/models` (created on demand, so
    /// this tier always succeeds).
    fn resolve_models_dir() -> Result<PathBuf> {
        // Environment override
        if let Ok(path) = std::env::var("CODESCRIBE_MODELS_DIR") {
            let p = PathBuf::from(&path);
            if p.exists() {
                return Ok(p);
            }
        }

        let exe = std::env::current_exe().context("Failed to get executable path")?;
        let exe_dir = exe.parent().context("Failed to get executable directory")?;

        // 1. Bundled .app: Contents/MacOS/binary -> Contents/Resources/models/
        let bundled_path = exe_dir.join("../Resources/models");
        if models_root_contains_complete_whisper_model(&bundled_path) {
            return bundled_path
                .canonicalize()
                .context("Failed to canonicalize bundled models path");
        }

        // 2. Development: exe in target/debug/ -> ../../models/
        // NOTE: deliberately NOT matched from target/debug/deps (test
        // binaries): the repo models/ dir holds only Silero, and models_dir
        // means "directory with ALL models" — hijacking it from tests sends
        // runtime Whisper resolution to the wrong place.
        let dev_path = exe_dir.join("../../models");
        if models_root_contains_complete_whisper_model(&dev_path) {
            return dev_path
                .canonicalize()
                .context("Failed to canonicalize dev models path");
        }

        // 3. Direct ./models/ (running from repo root)
        let local_path = PathBuf::from("../../models");
        if models_root_contains_complete_whisper_model(&local_path) {
            return local_path
                .canonicalize()
                .context("Failed to canonicalize local models path");
        }

        // 4. Fallback: ~/.codescribe/models/ (lowercase!)
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let user_models = PathBuf::from(&home).join(".codescribe/models");
        fs::create_dir_all(&user_models).context("Failed to create user models directory")?;
        Ok(user_models)
    }

    /// Where a model of this name would live: the path itself if `model_name` is
    /// an existing absolute path, otherwise the name joined onto the models dir.
    ///
    /// Purely positional — the returned path is not checked for completeness and
    /// need not exist.
    pub fn get_model_path(&self, model_name: &str) -> PathBuf {
        // Check if it's an absolute path that exists
        let candidate = PathBuf::from(model_name);
        if candidate.is_absolute() && candidate.exists() {
            return candidate;
        }

        self.models_dir.join(model_name)
    }

    /// Resolve a reference that may be a path or a models-dir alias.
    ///
    /// Differs from [`Self::get_model_path`] by accepting *relative* paths that
    /// exist (canonicalizing them) before falling back to alias semantics.
    pub fn resolve_model_reference(&self, model_ref: &str) -> PathBuf {
        let candidate = PathBuf::from(model_ref);
        if candidate.exists() {
            return canonicalize_or_self(candidate);
        }

        self.models_dir.join(model_ref)
    }

    /// Whether the reference resolves to a *complete* model directory.
    ///
    /// A directory that exists but is missing weights or metadata reports
    /// `false` — existence alone is not the contract.
    pub fn check_model_exists(&self, model_name: &str) -> bool {
        let path = self.resolve_model_reference(model_name);
        is_complete_whisper_model_dir(&path)
    }

    /// Sorted names of every complete model in the models directory.
    ///
    /// Half-downloaded directories are filtered out rather than listed, so the
    /// result is safe to surface as user-selectable options. A missing models
    /// directory yields an empty list, not an error.
    pub fn list_models(&self) -> Result<Vec<String>> {
        if !self.models_dir.exists() {
            return Ok(Vec::new());
        }

        let mut out = Vec::new();
        let entries = fs::read_dir(&self.models_dir).context("Failed to read models directory")?;
        for entry in entries {
            let entry = entry.context("Failed to read models directory entry")?;
            let path = entry.path();
            if path.is_dir() {
                // Only advertise fully usable Whisper models, not half-downloaded shells.
                if is_complete_whisper_model_dir(&path)
                    && let Some(name) = path.file_name().and_then(|s| s.to_str())
                {
                    out.push(name.to_string());
                }
            }
        }
        out.sort();
        Ok(out)
    }

    /// The resolved models directory this manager is anchored to.
    pub fn models_dir(&self) -> &PathBuf {
        &self.models_dir
    }
}

/// Resolve the authoritative runtime Whisper fallback model path.
///
/// Precedence:
/// 1. Explicit `CODESCRIBE_MODEL_PATH`
/// 2. Configured local model path / models-dir alias
/// 3. Configured Hugging Face repo snapshot
/// 4. Default models-dir alias (`whisper-large-v3-turbo`)
/// 5. Default Hugging Face snapshot (`mlx-community/whisper-large-v3-turbo`)
/// 6. Legacy models-dir alias (`whisper-large-v3-turbo-mlx-q8`)
/// 7. Legacy Hugging Face snapshot (`LibraxisAI/whisper-large-v3-turbo-mlx-q8`)
pub fn resolve_runtime_whisper_model_path(configured_model: Option<&str>) -> Result<PathBuf> {
    if let Ok(path) = std::env::var("CODESCRIBE_MODEL_PATH") {
        let candidate = PathBuf::from(path.trim());
        if is_complete_whisper_model_dir(&candidate) {
            return Ok(canonicalize_or_self(candidate));
        }
    }

    let manager = ModelManager::new()?;
    let configured_model = configured_model
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let Some(model_ref) = configured_model {
        let local_candidate = manager.resolve_model_reference(model_ref);
        if is_complete_whisper_model_dir(&local_candidate) {
            return Ok(canonicalize_or_self(local_candidate));
        }

        if let Some(snapshot) = hf_snapshot_for_model(model_ref) {
            return Ok(snapshot);
        }
    }

    let default_local = manager.get_model_path(DEFAULT_MODEL);
    if is_complete_whisper_model_dir(&default_local) {
        return Ok(canonicalize_or_self(default_local));
    }

    if let Some(snapshot) = hf_snapshot_for_model(DEFAULT_MODEL) {
        return Ok(snapshot);
    }

    let legacy_local = manager.get_model_path(LEGACY_MODEL);
    if is_complete_whisper_model_dir(&legacy_local) {
        return Ok(canonicalize_or_self(legacy_local));
    }

    if let Some(snapshot) = hf_snapshot_for_model(LEGACY_MODEL) {
        return Ok(snapshot);
    }

    Err(anyhow!(
        "Whisper runtime fallback model not available.\n\
         Public builds do not embed Whisper; install it from Settings → Dictation,\n\
         set CODESCRIBE_MODEL_PATH, configure LOCAL_MODEL, or warm the Hugging Face cache.\n\n\
         Download with: hf download {}\n\
         Or: Settings → Dictation → Download Whisper",
        DEFAULT_WHISPER_REPO
    ))
}

/// Live status of the default local Whisper model for Settings / bridge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhisperModelStatus {
    /// Transcription can start: either embedded, or a complete model on disk.
    pub available: bool,
    /// The binary carries an embedded payload, so no download is ever required.
    pub embedded: bool,
    /// Resolved on-disk model directory, when one was found.
    pub path: Option<String>,
    /// Model alias the runtime fallback looks for ([`DEFAULT_MODEL`]).
    pub model_id: String,
    /// Hugging Face repo a download would pull from ([`DEFAULT_WHISPER_REPO`]).
    pub repo: String,
    /// Short human size hint for the UI (not a network probe).
    pub size_hint: String,
}

/// Snapshot whether Whisper is embedded, already on disk, or still needs download.
pub fn whisper_model_status() -> WhisperModelStatus {
    let embedded = crate::stt::whisper::embedded::is_embedded_available();
    let path = resolve_runtime_whisper_model_path(None)
        .ok()
        .map(|p| p.display().to_string());
    WhisperModelStatus {
        available: embedded || path.is_some(),
        embedded,
        path,
        model_id: DEFAULT_MODEL.to_string(),
        repo: DEFAULT_WHISPER_REPO.to_string(),
        size_hint: "~1.6 GB".to_string(),
    }
}

/// Download the default Whisper model into `~/.codescribe/models/<DEFAULT_MODEL>/`.
///
/// Files are fetched from the Hugging Face resolve endpoint (same repo as
/// `make download-model`). Optional `on_progress(file, bytes_done, bytes_total)`
/// is called during each file transfer. Uses `HF_TOKEN` when set.
pub fn download_default_whisper_model<F>(mut on_progress: F) -> Result<PathBuf>
where
    F: FnMut(&str, u64, Option<u64>),
{
    let manager = ModelManager::new()?;
    let dest = manager.get_model_path(DEFAULT_MODEL);
    if is_complete_whisper_model_dir(&dest) {
        return Ok(canonicalize_or_self(dest));
    }

    // Compose from warm local sources first, so Settings "Download" is a no-op
    // when the pieces are already on disk. The primary repo ships only
    // config.json + weights; tokenizer.json and mel_filters.npz come from the
    // legacy q8 sources. config.json and weights must stay paired to the
    // primary repo — legacy weights under the new alias would mislabel q8 as
    // fp16, so legacy sources only ever contribute the companion files.
    if let Some(snapshot) = hf_cache::find_snapshot(DEFAULT_WHISPER_REPO, &["config.json"])
        && snapshot != dest
    {
        copy_model_files(&snapshot, &dest, &["config.json"])?;
        copy_model_files(&snapshot, &dest, &REQUIRED_MODEL_WEIGHTS)?;
    }
    let legacy_local = manager.get_model_path(LEGACY_MODEL);
    copy_model_files(&legacy_local, &dest, &COMPANION_MODEL_FILES)?;
    if let Some(snapshot) = hf_snapshot_for_model(LEGACY_MODEL) {
        copy_model_files(&snapshot, &dest, &COMPANION_MODEL_FILES)?;
    }
    if is_complete_whisper_model_dir(&dest) {
        return Ok(canonicalize_or_self(dest));
    }

    fs::create_dir_all(&dest).with_context(|| format!("create {}", dest.display()))?;

    let client = reqwest::blocking::Client::builder()
        .user_agent("codescribe-whisper-download/0.13")
        .timeout(std::time::Duration::from_secs(60 * 30))
        .build()
        .context("build HTTP client for Whisper download")?;

    // Small files first so a failed auth fails fast before multi-GB weights.
    // config.json describes the primary weights, so it has no fallback source;
    // the companion files fall back to the legacy repo when the primary 404s.
    download_hf_file(
        &client,
        DEFAULT_WHISPER_REPO,
        "config.json",
        &dest.join("config.json"),
        &mut on_progress,
    )?;
    for name in COMPANION_MODEL_FILES {
        let target = dest.join(name);
        if let Err(err) = download_hf_file(
            &client,
            DEFAULT_WHISPER_REPO,
            name,
            &target,
            &mut on_progress,
        ) {
            tracing::warn!(
                error = %err,
                file = name,
                "primary repo does not ship this file; falling back to legacy repo"
            );
            download_hf_file(
                &client,
                LEGACY_WHISPER_REPO,
                name,
                &target,
                &mut on_progress,
            )?;
        }
    }

    let weights_dest = dest.join("weights.safetensors");
    let weights_alt = dest.join("model.safetensors");
    if !weights_dest.exists() && !weights_alt.exists() {
        // mlx-community ships weights.safetensors; fall back to model.safetensors if 404.
        match download_hf_file(
            &client,
            DEFAULT_WHISPER_REPO,
            "weights.safetensors",
            &weights_dest,
            &mut on_progress,
        ) {
            Ok(()) => {}
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "weights.safetensors missing; trying model.safetensors"
                );
                download_hf_file(
                    &client,
                    DEFAULT_WHISPER_REPO,
                    "model.safetensors",
                    &weights_alt,
                    &mut on_progress,
                )?;
            }
        }
    }

    if !is_complete_whisper_model_dir(&dest) {
        return Err(anyhow!(
            "Whisper download finished but model dir is incomplete: {}",
            dest.display()
        ));
    }

    Ok(canonicalize_or_self(dest))
}

/// Copy selected model files from a local source into the user models directory.
///
/// Lets Settings → Download complete without network traffic when the pieces are
/// already on disk (warm HF cache, legacy q8 install). A missing source is a
/// clean no-op and existing destination files are left alone, so an interrupted
/// composition resumes rather than restarting.
fn copy_model_files(src: &Path, dest: &Path, names: &[&str]) -> Result<()> {
    if !src.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(dest).with_context(|| format!("create {}", dest.display()))?;
    for name in names {
        let from = src.join(name);
        let to = dest.join(name);
        if from.exists() && !to.exists() {
            // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Both ends are internal: `name` comes from the REQUIRED_MODEL_* / COMPANION_MODEL_FILES compile-time constants, and callers pass HF cache snapshot dirs or ModelManager::get_model_path outputs. No caller-supplied path component reaches here.
            fs::copy(&from, &to)
                .with_context(|| format!("copy {} → {}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

/// Fetch one file from the Hugging Face resolve endpoint into `dest`.
///
/// Downloads to a sibling `.partial` file and renames on success, so an aborted
/// transfer can never leave a truncated file that passes the completeness check.
/// A non-empty `dest` is treated as already done. `HF_TOKEN` is sent as bearer
/// auth when set, for gated repos.
fn download_hf_file<F>(
    client: &reqwest::blocking::Client,
    repo: &str,
    filename: &str,
    dest: &Path,
    on_progress: &mut F,
) -> Result<()>
where
    F: FnMut(&str, u64, Option<u64>),
{
    if dest.exists() && dest.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        on_progress(
            filename,
            dest.metadata().map(|m| m.len()).unwrap_or(0),
            None,
        );
        return Ok(());
    }

    let url = format!("https://huggingface.co/{repo}/resolve/main/{filename}");
    let mut request = client.get(&url);
    if let Ok(token) = std::env::var("HF_TOKEN") {
        let token = token.trim();
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
    }

    let mut response = request
        .send()
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("HTTP error for {url}"))?;

    let total = response.content_length();
    let partial = dest.with_file_name(format!(
        "{}.partial",
        dest.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("download")
    ));

    if let Some(parent) = partial.parent() {
        fs::create_dir_all(parent)?;
    }
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- `partial` is `dest` with a ".partial" suffix on its own file name; `dest` is built from ModelManager::get_model_path(DEFAULT_MODEL) plus REQUIRED_MODEL_* constants, never from request data. The URL is remote, the path is not.
    let mut file = fs::File::create(&partial)
        .with_context(|| format!("create partial {}", partial.display()))?;

    use std::io::{Read, Write};
    let mut buf = [0u8; 1024 * 256];
    let mut done: u64 = 0;
    loop {
        let n = response
            .read(&mut buf)
            .with_context(|| format!("read body {url}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .with_context(|| format!("write {}", partial.display()))?;
        done += n as u64;
        on_progress(filename, done, total);
    }
    file.flush()?;
    drop(file);

    fs::rename(&partial, dest)
        .with_context(|| format!("rename {} → {}", partial.display(), dest.display()))?;
    on_progress(filename, done, total.or(Some(done)));
    Ok(())
}

/// ModelManager resolution, completeness gates, and env-override isolation tests.
#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::fs;
    use tempfile::TempDir;

    /// Restores a single env var on drop; tests must run under `serial`.
    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        /// Set `key` to `value`, remembering the previous value for `Drop`.
        fn set(key: &'static str, value: &Path) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: these tests run under `serial` and restore the prior env.
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }

        /// Unset `key`, remembering the previous value for `Drop`.
        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: these tests run under `serial` and restore the prior env.
            unsafe { std::env::remove_var(key) };
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        /// Restore the prior env value, or remove the key if it was unset.
        fn drop(&mut self) {
            if let Some(prev) = &self.prev {
                // SAFETY: these tests run under `serial` and restore the prior env.
                unsafe { std::env::set_var(self.key, prev) };
            } else {
                // SAFETY: these tests run under `serial` and restore the prior env.
                unsafe { std::env::remove_var(self.key) };
            }
        }
    }

    /// Create a directory that passes `is_complete_whisper_model_dir`.
    fn create_complete_whisper_model(path: &Path) {
        fs::create_dir_all(path).unwrap();
        fs::write(path.join("config.json"), "{}").unwrap();
        fs::write(path.join("tokenizer.json"), "{}").unwrap();
        fs::write(path.join("mel_filters.npz"), "npz").unwrap();
        fs::write(path.join("model.safetensors"), "weights").unwrap();
    }

    /// A bundle containing only the semantic embedder must not claim ownership
    /// of Whisper resolution and hide the user's complete fp16 install.
    #[test]
    fn embedder_only_models_root_does_not_qualify_as_whisper_root() {
        let temp_dir = TempDir::new().unwrap();
        let models_dir = temp_dir.path().join("models");
        let embedder = models_dir.join("embedder");
        fs::create_dir_all(&embedder).unwrap();
        fs::write(embedder.join("config.json"), "{}").unwrap();
        fs::write(embedder.join("tokenizer.json"), "{}").unwrap();
        fs::write(embedder.join("model.safetensors"), "weights").unwrap();

        assert!(!models_root_contains_complete_whisper_model(&models_dir));

        create_complete_whisper_model(&models_dir.join(DEFAULT_MODEL));
        assert!(models_root_contains_complete_whisper_model(&models_dir));
    }

    /// Smoke: `list_models` succeeds against the live models dir.
    #[test]
    #[serial]
    fn test_model_manager_list_models() {
        let manager = ModelManager::new().unwrap();
        let models = manager.list_models();
        assert!(models.is_ok());
        println!("Models dir: {}", manager.models_dir().display());
        println!("Found models: {:?}", models.unwrap());
    }

    /// Non-existent model ids report missing.
    #[test]
    #[serial]
    fn test_model_manager_check_exists() {
        let manager = ModelManager::new().unwrap();
        // Non-existent model should return false
        assert!(!manager.check_model_exists("nonexistent-model-xyz"));
    }

    /// Complete custom models under `CODESCRIBE_MODELS_DIR` are listed and found.
    #[test]
    #[serial]
    fn test_model_manager_custom_models() {
        let temp_dir = TempDir::new().unwrap();
        let models_dir = temp_dir.path().join("../../models");
        fs::create_dir_all(&models_dir).unwrap();

        let model_names = [
            "whisper-base-mlx-q8",
            "whisper-medium-mlx-q8",
            "whisper-large-v3-turbo-mlx-q8",
        ];

        for name in &model_names {
            let model_path = models_dir.join(name);
            create_complete_whisper_model(&model_path);
        }

        let _models_dir = EnvGuard::set("CODESCRIBE_MODELS_DIR", &models_dir);

        let manager = ModelManager::new().unwrap();
        let models = manager.list_models().unwrap();

        for name in &model_names {
            assert!(models.contains(&name.to_string()));
            assert!(manager.check_model_exists(name));
        }
    }

    /// Incomplete Whisper dirs are neither listed nor treated as existing.
    #[test]
    #[serial]
    fn test_model_manager_rejects_incomplete_whisper_models() {
        let temp_dir = TempDir::new().unwrap();
        let models_dir = temp_dir.path().join("models");
        let complete = models_dir.join("complete-whisper");
        let incomplete = models_dir.join("incomplete-whisper");

        create_complete_whisper_model(&complete);
        fs::create_dir_all(&incomplete).unwrap();
        fs::write(incomplete.join("tokenizer.json"), "{}").unwrap();

        let _models_dir = EnvGuard::set("CODESCRIBE_MODELS_DIR", &models_dir);
        let manager = ModelManager::new().unwrap();

        assert!(manager.check_model_exists("complete-whisper"));
        assert!(!manager.check_model_exists("incomplete-whisper"));
        assert_eq!(manager.list_models().unwrap(), vec!["complete-whisper"]);
    }

    /// Complete `CODESCRIBE_MODEL_PATH` wins over the bundled default tier.
    #[test]
    #[serial]
    fn resolve_runtime_whisper_model_path_prefers_complete_env_override() {
        let temp_dir = TempDir::new().unwrap();
        let env_model = temp_dir.path().join("env-model");
        let models_dir = temp_dir.path().join("models");
        let bundled_default = models_dir.join(DEFAULT_MODEL);

        create_complete_whisper_model(&env_model);
        create_complete_whisper_model(&bundled_default);

        let _env_override = EnvGuard::set("CODESCRIBE_MODEL_PATH", &env_model);
        let _models_dir = EnvGuard::set("CODESCRIBE_MODELS_DIR", &models_dir);
        let _hf_cache = EnvGuard::unset("CODESCRIBE_HF_CACHE");

        let resolved = resolve_runtime_whisper_model_path(Some(DEFAULT_MODEL)).unwrap();
        assert_eq!(resolved, canonicalize_or_self(env_model));
    }

    /// HF-style repo ids resolve to a complete snapshot under the cache root.
    #[test]
    #[serial]
    fn resolve_runtime_whisper_model_path_uses_hf_repo_id_from_cache() {
        let temp_dir = TempDir::new().unwrap();
        let hf_cache = temp_dir.path().join("hf-cache");
        let snapshot = hf_cache
            .join("models--vetcoders--custom-whisper")
            .join("snapshots")
            .join("abc123");

        create_complete_whisper_model(&snapshot);

        let _models_dir = EnvGuard::set(
            "CODESCRIBE_MODELS_DIR",
            temp_dir.path().join("models").as_path(),
        );
        let _env_override = EnvGuard::unset("CODESCRIBE_MODEL_PATH");
        let _hf_cache = EnvGuard::set("CODESCRIBE_HF_CACHE", &hf_cache);

        let resolved =
            resolve_runtime_whisper_model_path(Some("vetcoders/custom-whisper")).unwrap();
        assert_eq!(resolved, snapshot);
    }

    // ── Negative-path coverage (P2-07): each tier of the fallback chain must
    //    either accept a complete model or fall through cleanly. The error path
    //    is user-visible guidance, so we assert against its content.
    //
    //    These tests redirect HOME plus every HF cache env var to temp dirs so
    //    `BaseDirs::new()` does not silently find the developer's real HF cache
    //    and turn the negative path into a false positive.

    /// Point HOME and HF cache env vars at empty temps so real caches cannot leak in.
    fn isolate_from_real_hf_cache(temp_dir: &Path) -> Vec<EnvGuard> {
        let empty_hf_cache = temp_dir.join("empty-hf-cache");
        std::fs::create_dir_all(&empty_hf_cache).unwrap();
        let fake_home = temp_dir.join("fake-home");
        std::fs::create_dir_all(&fake_home).unwrap();

        vec![
            EnvGuard::set("HOME", &fake_home),
            EnvGuard::set("CODESCRIBE_HF_CACHE", &empty_hf_cache),
            EnvGuard::unset("HUGGINGFACE_HUB_CACHE"),
            EnvGuard::unset("HF_HUB_CACHE"),
            EnvGuard::unset("HF_HOME"),
        ]
    }

    /// Incomplete env override does not stick; empty tiers error instead.
    #[test]
    #[serial]
    fn resolve_runtime_whisper_model_path_skips_incomplete_env_override() {
        let temp_dir = TempDir::new().unwrap();
        let incomplete_env = temp_dir.path().join("env-incomplete");
        std::fs::create_dir_all(&incomplete_env).unwrap();
        std::fs::write(incomplete_env.join("tokenizer.json"), "{}").unwrap();

        let empty_models_dir = temp_dir.path().join("empty-models");
        std::fs::create_dir_all(&empty_models_dir).unwrap();

        let _hf_isolation = isolate_from_real_hf_cache(temp_dir.path());
        let _env_override = EnvGuard::set("CODESCRIBE_MODEL_PATH", &incomplete_env);
        let _models_dir = EnvGuard::set("CODESCRIBE_MODELS_DIR", &empty_models_dir);

        let result = resolve_runtime_whisper_model_path(Some(DEFAULT_MODEL));
        assert!(
            result.is_err(),
            "incomplete env override plus empty downstream tiers must fail loudly, got {result:?}"
        );
    }

    /// All-empty fallback chain returns guidance mentioning env and `hf download`.
    #[test]
    #[serial]
    fn resolve_runtime_whisper_model_path_errors_with_guidance_when_all_tiers_empty() {
        let temp_dir = TempDir::new().unwrap();
        let empty_models_dir = temp_dir.path().join("empty-models");
        std::fs::create_dir_all(&empty_models_dir).unwrap();

        let _hf_isolation = isolate_from_real_hf_cache(temp_dir.path());
        let _env_override = EnvGuard::unset("CODESCRIBE_MODEL_PATH");
        let _models_dir = EnvGuard::set("CODESCRIBE_MODELS_DIR", &empty_models_dir);

        let err = resolve_runtime_whisper_model_path(Some(DEFAULT_MODEL))
            .expect_err("all tiers empty must return Err");
        let message = format!("{err:#}");
        assert!(
            message.contains("CODESCRIBE_MODEL_PATH"),
            "error must mention the env override knob, got: {message}"
        );
        assert!(
            message.contains("hf download"),
            "error must hint at HF cache warm-up, got: {message}"
        );
    }

    /// Missing paths return unchanged rather than failing resolution.
    #[test]
    #[serial]
    fn canonicalize_or_self_returns_input_when_path_does_not_exist() {
        let phantom = PathBuf::from("/__codescribe__/nonexistent/phantom/model");
        let returned = canonicalize_or_self(phantom.clone());
        assert_eq!(returned, phantom);
    }

    /// Status surface advertises default model id, repo, and size-hint shape.
    #[test]
    #[serial]
    fn whisper_model_status_reports_default_ids() {
        let status = whisper_model_status();
        assert_eq!(status.model_id, DEFAULT_MODEL);
        assert_eq!(status.repo, DEFAULT_WHISPER_REPO);
        assert!(status.size_hint.contains("GB"));
        // embedded flag must match cfg(embed_model) payload; we only assert type wiring.
        let _ = status.available;
        let _ = status.embedded;
    }
}
