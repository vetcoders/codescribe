//! Shared Whisper safetensors validation for runtime and fat-build selection.

use anyhow::{Context, Result, anyhow};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Supported weight filenames in deterministic preference order.
pub const SUPPORTED_NAMES: [&str; 2] = ["weights.safetensors", "model.safetensors"];
/// SHA-256 of the pinned official OpenAI mel filterbank.
pub const MEL_FILTERS_SHA256: &str =
    "7450ae70723a5ef9d341e3cee628c7cb0177f36ce42c44b7ed2bf3325f0f6d4c";

/// Validate every artifact required by runtime and embedded Whisper loaders.
pub fn validate_whisper_model_bundle(path: &Path) -> Result<()> {
    validate_whisper_config(&path.join("config.json"))?;
    let tokenizer_path = path.join("tokenizer.json");
    tokenizers::Tokenizer::from_file(&tokenizer_path).map_err(|err| {
        anyhow!(
            "invalid Whisper tokenizer {}: {err}",
            tokenizer_path.display()
        )
    })?;
    verify_mel_filters(&path.join("mel_filters.npz"))?;
    resolve_valid_whisper_weights_path(path).map(|_| ())
}

/// Validate the config schema and reject every declared quantization mode.
pub(crate) fn validate_whisper_config(path: &Path) -> Result<()> {
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Read-only model inspection. `path` is an operator-selected local model config or an internally resolved bundle/cache child; no network/request path component reaches it.
    let raw = fs::read_to_string(path)
        .with_context(|| format!("read Whisper config {}", path.display()))?;
    let config: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("parse Whisper config {}", path.display()))?;
    if !config.is_object() {
        return Err(anyhow!(
            "Whisper config must be a JSON object: {}",
            path.display()
        ));
    }
    if config
        .get("quantization")
        .is_some_and(|value| !value.is_null())
        || config
            .get("quantization_config")
            .is_some_and(|value| !value.is_null())
    {
        return Err(anyhow!("quantized Whisper config is unsupported"));
    }
    Ok(())
}

/// Verify the pinned mel filterbank used by Whisper.
pub(crate) fn verify_mel_filters(path: &Path) -> Result<()> {
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Read-only checksum of an operator-selected local model artifact or internally resolved download destination.
    let bytes = fs::read(path).with_context(|| format!("read {} for checksum", path.display()))?;
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != MEL_FILTERS_SHA256 {
        return Err(anyhow!(
            "SHA-256 mismatch for {}: expected {}, got {}",
            path.display(),
            MEL_FILTERS_SHA256,
            actual
        ));
    }
    Ok(())
}

/// Resolve the first structurally valid supported weight file.
pub fn resolve_valid_whisper_weights_path(path: &Path) -> Result<PathBuf> {
    let mut failures = Vec::new();
    for name in SUPPORTED_NAMES {
        let candidate = path.join(name);
        if !candidate.is_file() {
            continue;
        }
        match validate_safetensors_file(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) => failures.push(format!("{name}: {err:#}")),
        }
    }

    if failures.is_empty() {
        Err(anyhow!(
            "Whisper weights are missing from {}",
            path.display()
        ))
    } else {
        Err(anyhow!(
            "no valid Whisper weights in {} ({})",
            path.display(),
            failures.join("; ")
        ))
    }
}

/// Validate the complete safetensors structure without loading tensor data.
pub(crate) fn validate_safetensors_file(path: &Path) -> Result<()> {
    const MAX_HEADER_BYTES: u64 = 16 * 1024 * 1024;
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Read-only model inspection. `path` is an operator-selected local model file or an internally resolved bundle/cache child; no network/request path component reaches it.
    let mut file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut len_bytes = [0_u8; 8];
    file.read_exact(&mut len_bytes)
        .with_context(|| format!("read safetensors header length from {}", path.display()))?;
    let header_len = u64::from_le_bytes(len_bytes);
    if header_len == 0 || header_len > MAX_HEADER_BYTES {
        return Err(anyhow!(
            "invalid safetensors header length in {}",
            path.display()
        ));
    }
    let mut header = vec![0_u8; header_len as usize];
    file.seek(SeekFrom::Start(8))?;
    file.read_exact(&mut header)
        .with_context(|| format!("read safetensors header from {}", path.display()))?;
    let metadata: serde_json::Value = serde_json::from_slice(&header)
        .with_context(|| format!("parse safetensors header from {}", path.display()))?;
    let Some(tensors) = metadata.as_object() else {
        return Err(anyhow!(
            "safetensors header is not an object: {}",
            path.display()
        ));
    };

    if let Some(metadata) = tensors.get("__metadata__") {
        let valid = metadata.is_null()
            || metadata
                .as_object()
                .is_some_and(|entries| entries.values().all(serde_json::Value::is_string));
        if !valid {
            return Err(anyhow!(
                "invalid safetensors __metadata__ in {}",
                path.display()
            ));
        }
    }

    let file_len = file.metadata()?.len();
    let data_start = 8_u64
        .checked_add(header_len)
        .ok_or_else(|| anyhow!("safetensors header offset overflow"))?;
    let data_len = file_len
        .checked_sub(data_start)
        .ok_or_else(|| anyhow!("truncated safetensors file: {}", path.display()))?;
    let mut ranges = Vec::new();

    for (name, tensor) in tensors.iter().filter(|(name, _)| *name != "__metadata__") {
        let tensor = tensor
            .as_object()
            .ok_or_else(|| anyhow!("invalid tensor entry {name}"))?;
        let dtype = tensor
            .get("dtype")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("tensor {name} has no dtype"))?;
        let bytes_per_element = match (name.as_str(), dtype) {
            (_, "F16") => 2_u64,
            (_, "F32") => 4_u64,
            ("alignment_heads", "I64") => 8_u64,
            _ => {
                return Err(anyhow!(
                    "unsupported Whisper tensor dtype {dtype} for {name}"
                ));
            }
        };
        if name.ends_with(".scales") || name.ends_with(".biases") {
            return Err(anyhow!(
                "quantized Whisper companion tensor refused: {name}"
            ));
        }

        let shape = tensor
            .get("shape")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| anyhow!("tensor {name} has no shape"))?;
        let element_count = shape.iter().try_fold(1_u64, |count, dim| {
            let dim = dim
                .as_u64()
                .ok_or_else(|| anyhow!("tensor {name} has an invalid shape"))?;
            count
                .checked_mul(dim)
                .ok_or_else(|| anyhow!("tensor {name} shape overflows"))
        })?;
        if element_count == 0 {
            return Err(anyhow!("tensor {name} has an empty shape"));
        }
        let expected_bytes = element_count
            .checked_mul(bytes_per_element)
            .ok_or_else(|| anyhow!("tensor {name} byte size overflows"))?;

        let offsets = tensor
            .get("data_offsets")
            .and_then(serde_json::Value::as_array)
            .filter(|offsets| offsets.len() == 2)
            .ok_or_else(|| anyhow!("tensor {name} has invalid data_offsets"))?;
        let start = offsets[0]
            .as_u64()
            .ok_or_else(|| anyhow!("tensor {name} has invalid start offset"))?;
        let end = offsets[1]
            .as_u64()
            .ok_or_else(|| anyhow!("tensor {name} has invalid end offset"))?;
        if end.checked_sub(start) != Some(expected_bytes) {
            return Err(anyhow!(
                "tensor {name} byte range does not match its shape/dtype"
            ));
        }
        ranges.push((start, end, name));
    }

    if ranges.is_empty() {
        return Err(anyhow!(
            "safetensors file contains no tensors: {}",
            path.display()
        ));
    }
    ranges.sort_by_key(|(start, _, _)| *start);
    let mut cursor = 0_u64;
    for (start, end, name) in ranges {
        if start != cursor {
            return Err(anyhow!("tensor {name} has a non-contiguous data offset"));
        }
        cursor = end;
    }
    if cursor != data_len {
        return Err(anyhow!(
            "safetensors data length mismatch in {}: header covers {cursor}, file has {data_len}",
            path.display()
        ));
    }
    Ok(())
}
