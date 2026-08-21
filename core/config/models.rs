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
/// Former quantized model alias retained only for source compatibility.
#[deprecated(note = "quantized Whisper is unsupported; no runtime fallback uses this alias")]
pub const LEGACY_MODEL: &str = "whisper-large-v3-turbo-mlx-q8";
/// Former quantized model repository retained only for source compatibility.
#[deprecated(note = "quantized Whisper is unsupported; no runtime fallback uses this repository")]
pub const LEGACY_WHISPER_REPO: &str = "LibraxisAI/whisper-large-v3-turbo-mlx-q8";
/// Official Transformers tokenizer paired with Whisper large-v3-turbo.
pub(crate) const TOKENIZER_WHISPER_REPO: &str = "openai/whisper-large-v3-turbo";
/// Pinned OpenAI Whisper asset. The checksum is asserted by the installer.
pub(crate) const MEL_FILTERS_URL: &str = "https://raw.githubusercontent.com/openai/whisper/5f86d1d86363843179951550570367b37c5d6f78/whisper/assets/mel_filters.npz";
/// Files that must all be present for a directory to count as a usable model.
const REQUIRED_MODEL_FILES: [&str; 3] = ["config.json", "tokenizer.json", "mel_filters.npz"];
/// Weight file names, of which **any one** satisfies the completeness check —
/// upstream repos ship either `model.safetensors` or `weights.safetensors`.
const REQUIRED_MODEL_WEIGHTS: [&str; 2] = crate::whisper_weights::SUPPORTED_NAMES;

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

/// Expand a leading `~/` in operator-provided model paths.
fn expand_home_path(value: &str) -> PathBuf {
    if let Some(relative) = value.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(relative);
    }
    PathBuf::from(value)
}

/// Whether `path` holds a fully usable, structurally valid Whisper model.
fn is_complete_whisper_model_dir(path: &Path) -> bool {
    validate_whisper_model_bundle(path).is_ok()
}

/// Validate every artifact required by the runtime loader.
///
/// Safetensors validation is structural rather than cryptographic: the format
/// has no payload checksum. The validator checks the complete tensor table,
/// dtype allowlist, byte sizes, contiguous offsets, and final file length.
pub fn validate_whisper_model_bundle(path: &Path) -> Result<()> {
    crate::whisper_weights::validate_whisper_model_bundle(path)
}

/// Reject unsupported or malformed weights before the expensive engine load.
/// This narrower payload gate is also used by `LocalWhisperEngine::new`, where
/// tokenizer and mel errors retain their own loader diagnostics.
pub(crate) fn is_unquantized_whisper_model_dir(path: &Path) -> bool {
    crate::whisper_weights::validate_whisper_model_pair(path).is_ok()
}

#[cfg(test)]
use crate::whisper_weights::resolve_valid_whisper_weights_path;
/// Resolve the first structurally valid supported weight file.
///
/// Upstream snapshots may contain either filename, and stale composition can
/// leave both behind. Preserve the documented filename priority, but never let
/// an invalid primary shadow a valid alternative that the runtime can load.
pub(crate) use crate::whisper_weights::{
    resolve_compatible_whisper_weights_path, validate_safetensors_file,
};

/// Whether a candidate models root owns at least one complete Whisper model.
///
/// A bundled `Resources/models` directory may contain only another model
/// family (for example the semantic embedder). Treating mere directory
/// existence as Whisper ownership shadows the user-installed fp16 model and
/// could incorrectly select a quantized cache instead.
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

    let repo = if trimmed.contains('/') {
        trimmed
    } else if trimmed == DEFAULT_MODEL {
        DEFAULT_WHISPER_REPO
    } else {
        return None;
    };
    hf_cache::find_snapshot_with_any_matching(
        repo,
        &REQUIRED_MODEL_FILES,
        &REQUIRED_MODEL_WEIGHTS,
        is_complete_whisper_model_dir,
    )
}

/// Find a warm default-repo snapshot containing one valid config/weights pair.
fn find_cached_default_model_pair() -> Option<PathBuf> {
    hf_cache::find_snapshot_with_any_matching(
        DEFAULT_WHISPER_REPO,
        &["config.json"],
        &REQUIRED_MODEL_WEIGHTS,
        |snapshot| crate::whisper_weights::validate_whisper_model_pair(snapshot).is_ok(),
    )
}

/// Find a warm OpenAI snapshot containing a parseable Whisper tokenizer.
fn find_cached_whisper_tokenizer() -> Option<PathBuf> {
    hf_cache::find_snapshot_with_any_matching(
        TOKENIZER_WHISPER_REPO,
        &["tokenizer.json"],
        &[],
        |snapshot| validate_model_file("tokenizer.json", &snapshot.join("tokenizer.json")).is_ok(),
    )
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
            let p = expand_home_path(path.trim());
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

    Err(anyhow!(
        "Unquantized Whisper runtime model not available.\n\
         Public builds do not embed Whisper; install it from Settings → Dictation,\n\
         set CODESCRIBE_MODEL_PATH, configure LOCAL_MODEL, or warm the Hugging Face cache.\n\n\
         Quantized q8 models are intentionally refused.\n\n\
         Download with: make download-model\n\
         Or: Settings → Dictation → Download Whisper",
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

    // Compose from warm official sources first, so Settings "Download" is a
    // no-op when the pieces are already on disk. Config and weights come from
    // mlx-community's fp16 conversion; tokenizer comes from OpenAI's matching
    // Transformers repository. The pinned mel filterbank is fetched below.
    let mut paired_default_model = false;
    if let Some(snapshot) = find_cached_default_model_pair()
        && snapshot != dest
    {
        paired_default_model = copy_default_model_pair(&snapshot, &dest)?;
    }
    if let Some(snapshot) = find_cached_whisper_tokenizer() {
        copy_model_files(&snapshot, &dest, &["tokenizer.json"], true)?;
    }
    if paired_default_model && is_complete_whisper_model_dir(&dest) {
        return Ok(canonicalize_or_self(dest));
    }

    fs::create_dir_all(&dest).with_context(|| format!("create {}", dest.display()))?;

    let client = reqwest::blocking::Client::builder()
        .user_agent(format!(
            "codescribe-whisper-download/{}",
            env!("CARGO_PKG_VERSION")
        ))
        .timeout(std::time::Duration::from_secs(60 * 30))
        .build()
        .context("build HTTP client for Whisper download")?;

    // Small files first so a failed auth fails fast before multi-GB weights.
    download_hf_file(
        &client,
        DEFAULT_WHISPER_REPO,
        "config.json",
        &dest.join("config.json"),
        &mut on_progress,
        true,
    )?;
    download_hf_file(
        &client,
        TOKENIZER_WHISPER_REPO,
        "tokenizer.json",
        &dest.join("tokenizer.json"),
        &mut on_progress,
        true,
    )?;
    download_url_file(
        &client,
        MEL_FILTERS_URL,
        "mel_filters.npz",
        &dest.join("mel_filters.npz"),
        &mut on_progress,
    )?;
    let weights_dest = dest.join("weights.safetensors");
    let weights_alt = dest.join("model.safetensors");
    // An incomplete default bundle is repaired as one model generation. Even
    // structurally valid installed weights may belong to another architecture,
    // so pair them with the freshly selected default config instead of reusing
    // them independently.
    match download_hf_file(
        &client,
        DEFAULT_WHISPER_REPO,
        "weights.safetensors",
        &weights_dest,
        &mut on_progress,
        true,
    ) {
        Ok(()) => remove_other_weight_file(&dest, "weights.safetensors")?,
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
                true,
            )?;
            remove_other_weight_file(&dest, "model.safetensors")?;
        }
    }

    validate_whisper_model_bundle(&dest).with_context(|| {
        format!(
            "Whisper download finished but bundle validation failed: {}",
            dest.display()
        )
    })?;

    Ok(canonicalize_or_self(dest))
}

/// Copy selected model files from a local source into the user models directory.
///
/// Lets Settings → Download complete without network traffic when the pieces are
/// already on disk (warm official caches). Every copied file is validated in a
/// sibling `.partial` path before atomic promotion. Callers may preserve valid
/// destinations or deliberately replace a config/weights generation as a pair.
fn copy_model_files(src: &Path, dest: &Path, names: &[&str], replace_valid: bool) -> Result<()> {
    if !src.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(dest).with_context(|| format!("create {}", dest.display()))?;
    for name in names {
        let from = src.join(name);
        let to = dest.join(name);
        if !from.is_file() || (!replace_valid && validate_model_file(name, &to).is_ok()) {
            continue;
        }
        if let Err(err) = validate_model_file(name, &from) {
            tracing::warn!(source = %from.display(), error = %err, "ignoring invalid cached model artifact");
            continue;
        }
        let partial = partial_path(&to);
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Both ends are internal: `name` comes from compile-time model file constants, and callers pass HF cache snapshot dirs or ModelManager::get_model_path outputs. No caller-supplied path component reaches here.
        fs::copy(&from, &partial)
            .with_context(|| format!("copy {} → {}", from.display(), partial.display()))?;
        if let Err(err) = validate_model_file(name, &partial) {
            let _ = fs::remove_file(&partial);
            return Err(err).with_context(|| format!("validate copied {}", name));
        }
        replace_file(&partial, &to)?;
    }
    Ok(())
}

/// Replace config and weights only when both come from one valid default snapshot.
fn copy_default_model_pair(src: &Path, dest: &Path) -> Result<bool> {
    if crate::whisper_weights::validate_whisper_model_pair(src).is_err() {
        return Ok(false);
    }
    let architecture = crate::whisper_weights::parse_whisper_config(
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- `src` is a resolved child of an internal HF cache root selected by repository id; no request path component reaches this repair path.
        &fs::read_to_string(src.join("config.json"))?,
        &src.join("config.json").display().to_string(),
    )?;
    let weights = resolve_compatible_whisper_weights_path(src, architecture)?;
    let Some(weight_name) = weights.file_name().and_then(|name| name.to_str()) else {
        return Ok(false);
    };

    copy_model_files(src, dest, &["config.json"], true)?;
    copy_model_files(src, dest, &[weight_name], true)?;
    remove_other_weight_file(dest, weight_name)?;
    Ok(true)
}

fn remove_other_weight_file(dest: &Path, selected: &str) -> Result<()> {
    for name in REQUIRED_MODEL_WEIGHTS {
        if name == selected {
            continue;
        }
        let stale = dest.join(name);
        if stale.exists() {
            fs::remove_file(&stale)
                .with_context(|| format!("remove stale Whisper weights {}", stale.display()))?;
        }
    }
    Ok(())
}

/// Fetch one file from the Hugging Face resolve endpoint into `dest`.
///
/// Downloads to a sibling `.partial` file and renames on success, so an aborted
/// transfer can never leave a truncated file that passes bundle validation.
/// A valid `dest` is preserved unless the caller is repairing a paired default
/// model generation. `HF_TOKEN` is sent as bearer auth when set, for gated repos.
fn download_hf_file<F>(
    client: &reqwest::blocking::Client,
    repo: &str,
    filename: &str,
    dest: &Path,
    on_progress: &mut F,
    replace_valid: bool,
) -> Result<()>
where
    F: FnMut(&str, u64, Option<u64>),
{
    let url = format!("https://huggingface.co/{repo}/resolve/main/{filename}");
    download_url_file_authenticated(
        client,
        &url,
        filename,
        dest,
        on_progress,
        true,
        replace_valid,
    )
}

fn download_url_file<F>(
    client: &reqwest::blocking::Client,
    url: &str,
    filename: &str,
    dest: &Path,
    on_progress: &mut F,
) -> Result<()>
where
    F: FnMut(&str, u64, Option<u64>),
{
    download_url_file_authenticated(client, url, filename, dest, on_progress, false, false)
}

fn download_url_file_authenticated<F>(
    client: &reqwest::blocking::Client,
    url: &str,
    filename: &str,
    dest: &Path,
    on_progress: &mut F,
    use_hf_token: bool,
    replace_valid: bool,
) -> Result<()>
where
    F: FnMut(&str, u64, Option<u64>),
{
    if !replace_valid && validate_model_file(filename, dest).is_ok() {
        on_progress(
            filename,
            dest.metadata().map(|metadata| metadata.len()).unwrap_or(0),
            None,
        );
        return Ok(());
    }
    let mut request = client.get(url);
    if use_hf_token && let Ok(token) = std::env::var("HF_TOKEN") {
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
    let partial = partial_path(dest);

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

    if let Err(err) = validate_model_file(filename, &partial) {
        let _ = fs::remove_file(&partial);
        return Err(err).with_context(|| format!("validate downloaded {filename}"));
    }
    replace_file(&partial, dest)?;
    on_progress(filename, done, total.or(Some(done)));
    Ok(())
}

fn partial_path(dest: &Path) -> PathBuf {
    dest.with_file_name(format!(
        "{}.partial",
        dest.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("download")
    ))
}

fn replace_file(partial: &Path, dest: &Path) -> Result<()> {
    if dest.exists() {
        fs::remove_file(dest)
            .with_context(|| format!("remove invalid model artifact {}", dest.display()))?;
    }
    fs::rename(partial, dest)
        .with_context(|| format!("rename {} → {}", partial.display(), dest.display()))
}

fn validate_model_file(filename: &str, path: &Path) -> Result<()> {
    match filename {
        "config.json" => crate::whisper_weights::validate_whisper_config(path),
        "tokenizer.json" => crate::whisper_weights::validate_whisper_tokenizer(path),
        "mel_filters.npz" => crate::whisper_weights::verify_mel_filters(path),
        "weights.safetensors" | "model.safetensors" => validate_safetensors_file(path),
        _ => Err(anyhow!("unsupported Whisper model artifact: {filename}")),
    }
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

        /// Set `key` to a literal string, including shell-like path syntax.
        fn set_str(key: &'static str, value: &str) -> Self {
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
        fs::write(
            path.join("config.json"),
            include_str!("../../tests/fixtures/whisper_test_config.json"),
        )
        .unwrap();
        let mut tokenizer = tokenizers::Tokenizer::new(tokenizers::models::bpe::BPE::default());
        tokenizer.add_special_tokens(&[
            tokenizers::AddedToken::from("<|startoftranscript|>", true),
            tokenizers::AddedToken::from("<|endoftext|>", true),
            tokenizers::AddedToken::from("<|transcribe|>", true),
        ]);
        tokenizer.save(path.join("tokenizer.json"), false).unwrap();
        fs::write(
            path.join("mel_filters.npz"),
            decode_hex(include_str!(
                "../../tests/fixtures/whisper_mel_filters.npz.hex"
            )),
        )
        .unwrap();
        let architecture = crate::whisper_weights::parse_whisper_config(
            include_str!("../../tests/fixtures/whisper_test_config.json"),
            "test fixture",
        )
        .unwrap();
        crate::whisper_weights::write_test_whisper_weights(
            &path.join("model.safetensors"),
            architecture,
        )
        .unwrap();
    }

    fn decode_hex(raw: &str) -> Vec<u8> {
        let digits: String = raw.chars().filter(|ch| !ch.is_whitespace()).collect();
        assert!(digits.len().is_multiple_of(2));
        digits
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    fn create_q8_whisper_model(path: &Path) {
        create_complete_whisper_model(path);
        fs::write(
            path.join("config.json"),
            r#"{"quantization":{"group_size":32,"bits":8}}"#,
        )
        .unwrap();
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
        fs::write(embedder.join("model.safetensors"), "not-a-safetensors-file").unwrap();

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

        let model_names = ["whisper-base-fp16", "whisper-medium-fp16", DEFAULT_MODEL];

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

    /// A leading `~/` in the supported models-root override resolves via HOME.
    #[test]
    #[serial]
    fn model_manager_expands_tilde_models_root() {
        let temp_dir = TempDir::new().unwrap();
        let home = temp_dir.path().join("home");
        let models_dir = home.join("custom-models");
        create_complete_whisper_model(&models_dir.join(DEFAULT_MODEL));

        let _home = EnvGuard::set("HOME", &home);
        let _models_dir = EnvGuard::set_str("CODESCRIBE_MODELS_DIR", "~/custom-models");

        let manager = ModelManager::new().unwrap();
        assert_eq!(manager.models_dir(), models_dir.as_path());
        assert!(manager.check_model_exists(DEFAULT_MODEL));
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

    /// Q8 is refused even when every expected file exists.
    #[test]
    #[serial]
    fn model_manager_rejects_complete_q8_model() {
        let temp_dir = TempDir::new().unwrap();
        let models_dir = temp_dir.path().join("models");
        let q8 = models_dir.join("renamed-as-fp16");
        create_q8_whisper_model(&q8);

        let _models_dir = EnvGuard::set("CODESCRIBE_MODELS_DIR", &models_dir);
        let manager = ModelManager::new().unwrap();
        assert!(!manager.check_model_exists("renamed-as-fp16"));
        assert!(manager.list_models().unwrap().is_empty());
    }

    /// Presence alone is insufficient when the loader architecture is absent.
    #[test]
    fn model_manager_rejects_config_without_required_dimensions() {
        let temp_dir = TempDir::new().unwrap();
        let model = temp_dir.path().join("model");
        create_complete_whisper_model(&model);
        fs::write(model.join("config.json"), "{}").unwrap();

        let err = validate_whisper_model_bundle(&model).unwrap_err();
        assert!(format!("{err:#}").contains("n_mels"));
        assert!(!is_complete_whisper_model_dir(&model));
    }

    /// A syntactically valid tokenizer must cover every configured vocabulary id.
    #[test]
    fn model_manager_rejects_tokenizer_smaller_than_configured_vocabulary() {
        let temp_dir = TempDir::new().unwrap();
        let model = temp_dir.path().join("model");
        create_complete_whisper_model(&model);
        fs::write(
            model.join("config.json"),
            include_str!("../../tests/fixtures/whisper_config.json"),
        )
        .unwrap();

        let err = validate_whisper_model_bundle(&model).unwrap_err();
        assert!(format!("{err:#}").contains("does not cover configured vocabulary"));
        assert!(!is_complete_whisper_model_dir(&model));
    }

    /// A structurally valid safetensors file is not a Whisper model without its required tensors.
    #[test]
    fn model_manager_rejects_weights_missing_required_whisper_tensors() {
        let temp_dir = TempDir::new().unwrap();
        let model = temp_dir.path().join("model");
        create_complete_whisper_model(&model);
        let header = br#"{"model.weight":{"dtype":"F16","shape":[1],"data_offsets":[0,2]}}"#;
        let mut safetensors = (header.len() as u64).to_le_bytes().to_vec();
        safetensors.extend_from_slice(header);
        safetensors.extend_from_slice(&[0, 0]);
        fs::write(model.join("model.safetensors"), safetensors).unwrap();

        let err = validate_whisper_model_bundle(&model).unwrap_err();
        assert!(format!("{err:#}").contains("missing tensor"));
        assert!(!is_complete_whisper_model_dir(&model));
    }

    /// Header-level detection catches packed q8 even if config metadata lies.
    #[test]
    fn model_manager_rejects_q8_tensor_header_without_quantization_config() {
        let temp_dir = TempDir::new().unwrap();
        let model = temp_dir.path().join("model");
        create_complete_whisper_model(&model);
        let header = br#"{"encoder.weight":{"dtype":"U32","shape":[1],"data_offsets":[0,4]},"encoder.scales":{"dtype":"F16","shape":[1],"data_offsets":[4,6]}}"#;
        let mut safetensors = (header.len() as u64).to_le_bytes().to_vec();
        safetensors.extend_from_slice(header);
        safetensors.extend_from_slice(&[0; 6]);
        fs::write(model.join("model.safetensors"), safetensors).unwrap();

        assert!(!is_complete_whisper_model_dir(&model));
    }

    /// A non-Q8 integer tensor is still outside the fp16/fp32 runtime contract.
    #[test]
    fn model_manager_rejects_non_allowlisted_integer_tensor() {
        let temp_dir = TempDir::new().unwrap();
        let model = temp_dir.path().join("model");
        create_complete_whisper_model(&model);
        let header = br#"{"encoder.weight":{"dtype":"I32","shape":[1],"data_offsets":[0,4]}}"#;
        let mut safetensors = (header.len() as u64).to_le_bytes().to_vec();
        safetensors.extend_from_slice(header);
        safetensors.extend_from_slice(&[0; 4]);
        fs::write(model.join("model.safetensors"), safetensors).unwrap();

        assert!(!is_complete_whisper_model_dir(&model));
    }

    /// A stale invalid primary filename must not shadow a valid alternative.
    #[test]
    fn model_manager_uses_valid_alternative_weights() {
        let temp_dir = TempDir::new().unwrap();
        let model = temp_dir.path().join("model");
        create_complete_whisper_model(&model);
        fs::write(model.join("weights.safetensors"), b"stale invalid weights").unwrap();

        let resolved = resolve_valid_whisper_weights_path(&model).unwrap();
        assert_eq!(
            resolved.file_name().and_then(|name| name.to_str()),
            Some("model.safetensors")
        );
        assert!(is_complete_whisper_model_dir(&model));
    }

    /// Filename priority remains deterministic when both alternatives validate.
    #[test]
    fn model_manager_prefers_valid_primary_weights() {
        let temp_dir = TempDir::new().unwrap();
        let model = temp_dir.path().join("model");
        create_complete_whisper_model(&model);
        fs::copy(
            model.join("model.safetensors"),
            model.join("weights.safetensors"),
        )
        .unwrap();

        let resolved = resolve_valid_whisper_weights_path(&model).unwrap();
        assert_eq!(
            resolved.file_name().and_then(|name| name.to_str()),
            Some("weights.safetensors")
        );
    }

    /// Existing alternatives do not count when neither payload is valid.
    #[test]
    fn model_manager_rejects_all_invalid_weight_alternatives() {
        let temp_dir = TempDir::new().unwrap();
        let model = temp_dir.path().join("model");
        create_complete_whisper_model(&model);
        fs::write(model.join("weights.safetensors"), b"bad primary").unwrap();
        fs::write(model.join("model.safetensors"), b"bad alternative").unwrap();

        let err = resolve_valid_whisper_weights_path(&model).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("weights.safetensors"));
        assert!(message.contains("model.safetensors"));
        assert!(!is_complete_whisper_model_dir(&model));
    }

    /// Metadata alone is not a model and must not satisfy discovery.
    #[test]
    fn model_manager_rejects_safetensors_without_tensor_entries() {
        let temp_dir = TempDir::new().unwrap();
        let model = temp_dir.path().join("model");
        create_complete_whisper_model(&model);
        let header = br#"{"__metadata__":{"format":"mlx"}}"#;
        let mut safetensors = (header.len() as u64).to_le_bytes().to_vec();
        safetensors.extend_from_slice(header);
        fs::write(model.join("model.safetensors"), safetensors).unwrap();

        assert!(!is_complete_whisper_model_dir(&model));
    }

    /// Discovery must enforce the metadata schema used by the runtime loader.
    #[test]
    fn model_manager_rejects_malformed_safetensors_metadata() {
        let temp_dir = TempDir::new().unwrap();
        let model = temp_dir.path().join("model");
        create_complete_whisper_model(&model);
        let header = br#"{"__metadata__":{"format":1},"model.weight":{"dtype":"F16","shape":[1],"data_offsets":[0,2]}}"#;
        let mut safetensors = (header.len() as u64).to_le_bytes().to_vec();
        safetensors.extend_from_slice(header);
        safetensors.extend_from_slice(&[0, 0]);
        fs::write(model.join("model.safetensors"), safetensors).unwrap();

        assert!(!is_complete_whisper_model_dir(&model));
    }

    /// Structurally empty tensors cannot represent a loadable Whisper model.
    #[test]
    fn model_manager_rejects_zero_element_tensor() {
        let temp_dir = TempDir::new().unwrap();
        let model = temp_dir.path().join("model");
        create_complete_whisper_model(&model);
        let header = br#"{"model.weight":{"dtype":"F16","shape":[0],"data_offsets":[0,0]}}"#;
        let mut safetensors = (header.len() as u64).to_le_bytes().to_vec();
        safetensors.extend_from_slice(header);
        fs::write(model.join("model.safetensors"), safetensors).unwrap();

        assert!(!is_complete_whisper_model_dir(&model));
    }

    /// String-valued safetensors metadata is compatible with the runtime loader.
    #[test]
    fn model_manager_accepts_string_safetensors_metadata() {
        let temp_dir = TempDir::new().unwrap();
        let model = temp_dir.path().join("model");
        create_complete_whisper_model(&model);
        let header = br#"{"__metadata__":{"format":"mlx"},"model.weight":{"dtype":"F16","shape":[1],"data_offsets":[0,2]}}"#;
        let mut safetensors = (header.len() as u64).to_le_bytes().to_vec();
        safetensors.extend_from_slice(header);
        safetensors.extend_from_slice(&[0, 0]);
        fs::write(model.join("model.safetensors"), safetensors).unwrap();

        assert!(validate_safetensors_file(&model.join("model.safetensors")).is_ok());
    }

    /// Header offsets must describe the actual payload, not a truncated file.
    #[test]
    fn model_manager_rejects_truncated_safetensors_payload() {
        let temp_dir = TempDir::new().unwrap();
        let model = temp_dir.path().join("model");
        create_complete_whisper_model(&model);
        let weights = model.join("model.safetensors");
        let len = fs::metadata(&weights).unwrap().len();
        fs::OpenOptions::new()
            .write(true)
            .open(&weights)
            .unwrap()
            .set_len(len - 1)
            .unwrap();

        assert!(!is_complete_whisper_model_dir(&model));
    }

    /// A checksum mismatch cannot leave a directory advertised as complete.
    #[test]
    fn model_manager_rejects_corrupt_mel_filters() {
        let temp_dir = TempDir::new().unwrap();
        let model = temp_dir.path().join("model");
        create_complete_whisper_model(&model);
        fs::write(model.join("mel_filters.npz"), b"corrupt").unwrap();

        assert!(!is_complete_whisper_model_dir(&model));
    }

    /// Parseable non-Whisper tokenizers must not be advertised as ready.
    #[test]
    fn model_manager_rejects_tokenizer_without_control_tokens() {
        let temp_dir = TempDir::new().unwrap();
        let model = temp_dir.path().join("model");
        create_complete_whisper_model(&model);
        tokenizers::Tokenizer::new(tokenizers::models::bpe::BPE::default())
            .save(model.join("tokenizer.json"), false)
            .unwrap();

        assert!(!is_complete_whisper_model_dir(&model));
        assert!(
            validate_whisper_model_bundle(&model)
                .unwrap_err()
                .to_string()
                .contains("missing required token")
        );
    }

    /// A valid existing mel asset is reused without contacting the network.
    #[test]
    fn valid_existing_mel_skips_download() {
        let temp_dir = TempDir::new().unwrap();
        let mel = temp_dir.path().join("mel_filters.npz");
        fs::write(
            &mel,
            decode_hex(include_str!(
                "../../tests/fixtures/whisper_mel_filters.npz.hex"
            )),
        )
        .unwrap();
        let client = reqwest::blocking::Client::new();
        let mut progress = |_name: &str, _done: u64, _total: Option<u64>| {};

        download_url_file(
            &client,
            "http://127.0.0.1:0/must-not-be-called",
            "mel_filters.npz",
            &mel,
            &mut progress,
        )
        .unwrap();
    }

    /// Invalid warm destinations are replaced from validated cache artifacts.
    #[test]
    fn cached_composition_repairs_invalid_destination_files() {
        let temp_dir = TempDir::new().unwrap();
        let source = temp_dir.path().join("source");
        let destination = temp_dir.path().join("destination");
        create_complete_whisper_model(&source);
        create_complete_whisper_model(&destination);
        fs::write(destination.join("config.json"), b"not json").unwrap();
        fs::write(destination.join("tokenizer.json"), b"not json").unwrap();
        fs::write(destination.join("mel_filters.npz"), b"bad mel").unwrap();
        fs::write(destination.join("model.safetensors"), b"bad weights").unwrap();

        copy_model_files(&source, &destination, &REQUIRED_MODEL_FILES, false).unwrap();
        copy_model_files(&source, &destination, &REQUIRED_MODEL_WEIGHTS, false).unwrap();

        validate_whisper_model_bundle(&destination).unwrap();
        assert!(!destination.join("config.json.partial").exists());
        assert!(!destination.join("model.safetensors.partial").exists());
    }

    /// Default repair replaces individually valid weights from another bundle.
    #[test]
    fn cached_default_pair_replaces_stale_valid_weights() {
        let temp_dir = TempDir::new().unwrap();
        let source = temp_dir.path().join("source");
        let destination = temp_dir.path().join("destination");
        create_complete_whisper_model(&source);
        create_complete_whisper_model(&destination);
        fs::rename(
            destination.join("model.safetensors"),
            destination.join("weights.safetensors"),
        )
        .unwrap();

        assert!(copy_default_model_pair(&source, &destination).unwrap());
        assert_eq!(
            fs::read(destination.join("model.safetensors")).unwrap(),
            fs::read(source.join("model.safetensors")).unwrap()
        );
        assert!(!destination.join("weights.safetensors").exists());
        validate_whisper_model_bundle(&destination).unwrap();
    }

    /// Offline repair skips invalid newest cache entries for both model pieces.
    #[test]
    #[serial]
    fn cached_repair_falls_back_to_older_valid_snapshots() {
        use std::fs::FileTimes;
        use std::time::{Duration, SystemTime};

        let temp_dir = TempDir::new().unwrap();
        let cache = temp_dir.path().join("cache");
        let home = temp_dir.path().join("home");
        fs::create_dir_all(&home).unwrap();

        let _home = EnvGuard::set("HOME", &home);
        let _cache = EnvGuard::set("CODESCRIBE_HF_CACHE", &cache);
        let _hf_home = EnvGuard::unset("HF_HOME");
        let _hf_hub = EnvGuard::unset("HF_HUB_CACHE");
        let _huggingface_hub = EnvGuard::unset("HUGGINGFACE_HUB_CACHE");

        let snapshot = |repo: &str, revision: &str| {
            cache
                .join(format!("models--{}", repo.replace('/', "--")))
                .join("snapshots")
                .join(revision)
        };
        let set_modified = |path: &Path, seconds: u64| {
            fs::File::open(path)
                .unwrap()
                .set_times(
                    FileTimes::new()
                        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
                )
                .unwrap();
        };

        let older_model = snapshot(DEFAULT_WHISPER_REPO, "older");
        let newer_model = snapshot(DEFAULT_WHISPER_REPO, "newer");
        create_complete_whisper_model(&older_model);
        create_complete_whisper_model(&newer_model);
        fs::write(newer_model.join("model.safetensors"), b"corrupt").unwrap();
        set_modified(&older_model, 10);
        set_modified(&newer_model, 20);
        assert_eq!(find_cached_default_model_pair(), Some(older_model));

        let older_tokenizer = snapshot(TOKENIZER_WHISPER_REPO, "older");
        let newer_tokenizer = snapshot(TOKENIZER_WHISPER_REPO, "newer");
        create_complete_whisper_model(&older_tokenizer);
        create_complete_whisper_model(&newer_tokenizer);
        fs::write(newer_tokenizer.join("tokenizer.json"), "{}").unwrap();
        set_modified(&older_tokenizer, 10);
        set_modified(&newer_tokenizer, 20);
        assert_eq!(find_cached_whisper_tokenizer(), Some(older_tokenizer));
    }

    /// A downloaded checksum mismatch is never promoted to the final mel path.
    #[test]
    fn corrupt_download_is_removed_before_promotion() {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\ncorrupt")
                .unwrap();
        });

        let temp_dir = TempDir::new().unwrap();
        let mel = temp_dir.path().join("mel_filters.npz");
        let client = reqwest::blocking::Client::new();
        let mut progress = |_name: &str, _done: u64, _total: Option<u64>| {};
        let result = download_url_file(
            &client,
            &format!("http://{address}/mel_filters.npz"),
            "mel_filters.npz",
            &mel,
            &mut progress,
        );
        server.join().unwrap();

        assert!(result.is_err());
        assert!(!mel.exists(), "corrupt download must not be promoted");
        assert!(
            !partial_path(&mel).exists(),
            "failed partial must be removed"
        );
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

    /// All-empty fallback chain returns guidance mentioning env and the composer.
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
            message.contains("make download-model"),
            "error must point to the complete-model composer, got: {message}"
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
