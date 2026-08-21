//! Shared Whisper safetensors validation for runtime and fat-build selection.

use anyhow::{Context, Result, anyhow};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Supported weight filenames in deterministic preference order.
pub const SUPPORTED_NAMES: [&str; 2] = ["weights.safetensors", "model.safetensors"];
/// SHA-256 of the pinned official OpenAI mel filterbank.
pub const MEL_FILTERS_SHA256: &str =
    "7450ae70723a5ef9d341e3cee628c7cb0177f36ce42c44b7ed2bf3325f0f6d4c";
const REQUIRED_TOKENIZER_TOKENS: [&str; 2] = ["<|startoftranscript|>", "<|endoftext|>"];
/// MLX Whisper architecture shared by validation, disk loading, and embedding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WhisperArchitecture {
    pub n_mels: usize,
    pub n_audio_ctx: usize,
    pub n_audio_state: usize,
    pub n_audio_head: usize,
    pub n_audio_layer: usize,
    pub n_vocab: usize,
    pub n_text_ctx: usize,
    pub n_text_state: usize,
    pub n_text_head: usize,
    pub n_text_layer: usize,
}

/// Validate every artifact required by runtime and embedded Whisper loaders.
pub fn validate_whisper_model_bundle(path: &Path) -> Result<()> {
    let architecture = load_whisper_architecture(&path.join("config.json"))?;
    validate_whisper_tokenizer_for_architecture(&path.join("tokenizer.json"), architecture)?;
    verify_mel_filters(&path.join("mel_filters.npz"))?;
    resolve_compatible_whisper_weights_path(path, architecture).map(|_| ())
}

/// Parse the tokenizer and require the control tokens used by every decode.
pub(crate) fn validate_whisper_tokenizer(path: &Path) -> Result<()> {
    let tokenizer = tokenizers::Tokenizer::from_file(path)
        .map_err(|err| anyhow!("invalid Whisper tokenizer {}: {err}", path.display()))?;
    for token in REQUIRED_TOKENIZER_TOKENS {
        if tokenizer.token_to_id(token).is_none() {
            return Err(anyhow!(
                "Whisper tokenizer {} is missing required token {token}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn validate_whisper_tokenizer_for_architecture(
    path: &Path,
    architecture: WhisperArchitecture,
) -> Result<()> {
    validate_whisper_tokenizer(path)?;
    let tokenizer = tokenizers::Tokenizer::from_file(path)
        .map_err(|err| anyhow!("invalid Whisper tokenizer {}: {err}", path.display()))?;
    let mut covered = vec![false; architecture.n_vocab];
    for id in tokenizer.get_vocab(true).into_values() {
        if let Some(slot) = covered.get_mut(id as usize) {
            *slot = true;
        }
    }
    if covered.iter().any(|present| !present) {
        return Err(anyhow!(
            "Whisper tokenizer {} does not cover configured vocabulary 0..{}",
            path.display(),
            architecture.n_vocab
        ));
    }
    Ok(())
}

/// Validate the config schema and reject every declared quantization mode.
pub(crate) fn validate_whisper_config(path: &Path) -> Result<()> {
    load_whisper_architecture(path).map(|_| ())
}

fn load_whisper_architecture(path: &Path) -> Result<WhisperArchitecture> {
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Read-only model inspection. `path` is an operator-selected local model config or an internally resolved bundle/cache child; no network/request path component reaches it.
    let raw = fs::read_to_string(path)
        .with_context(|| format!("read Whisper config {}", path.display()))?;
    parse_whisper_config(&raw, &path.display().to_string())
}

/// Parse and validate the MLX architecture consumed by Candle's Whisper loader.
pub(crate) fn parse_whisper_config(raw: &str, source: &str) -> Result<WhisperArchitecture> {
    let config: serde_json::Value =
        serde_json::from_str(raw).with_context(|| format!("parse Whisper config {source}"))?;
    if !config.is_object() {
        return Err(anyhow!("Whisper config must be a JSON object: {source}"));
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
    let dimension = |name: &str| -> Result<usize> {
        let value = config
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| anyhow!("Whisper config {source} requires a positive integer {name}"))?;
        Ok(value)
    };
    let architecture = WhisperArchitecture {
        n_mels: dimension("n_mels")?,
        n_audio_ctx: dimension("n_audio_ctx")?,
        n_audio_state: dimension("n_audio_state")?,
        n_audio_head: dimension("n_audio_head")?,
        n_audio_layer: dimension("n_audio_layer")?,
        n_vocab: dimension("n_vocab")?,
        n_text_ctx: dimension("n_text_ctx")?,
        n_text_state: dimension("n_text_state")?,
        n_text_head: dimension("n_text_head")?,
        n_text_layer: dimension("n_text_layer")?,
    };
    if architecture.n_vocab > u32::MAX as usize {
        return Err(anyhow!(
            "Whisper config {source} requires n_vocab to fit tokenizer u32 IDs"
        ));
    }
    if !matches!(architecture.n_mels, 80 | 128) {
        return Err(anyhow!(
            "Whisper config {source} requires n_mels to be 80 or 128"
        ));
    }
    if architecture.n_audio_ctx > u32::MAX as usize {
        return Err(anyhow!(
            "Whisper config {source} requires n_audio_ctx to fit in u32"
        ));
    }
    if architecture.n_audio_state != architecture.n_text_state {
        return Err(anyhow!(
            "Whisper config {source} requires n_audio_state to equal n_text_state"
        ));
    }
    if architecture.n_audio_state < 4 || !architecture.n_audio_state.is_multiple_of(2) {
        return Err(anyhow!(
            "Whisper config {source} requires an even n_audio_state of at least 4"
        ));
    }
    if !architecture
        .n_audio_state
        .is_multiple_of(architecture.n_audio_head)
    {
        return Err(anyhow!(
            "Whisper config {source} requires n_audio_state divisible by n_audio_head"
        ));
    }
    if !architecture
        .n_text_state
        .is_multiple_of(architecture.n_text_head)
    {
        return Err(anyhow!(
            "Whisper config {source} requires n_text_state divisible by n_text_head"
        ));
    }
    Ok(architecture)
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
#[cfg(test)]
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

/// Resolve the first supported weight file matching the configured architecture.
pub(crate) fn resolve_compatible_whisper_weights_path(
    path: &Path,
    architecture: WhisperArchitecture,
) -> Result<PathBuf> {
    let mut failures = Vec::new();
    for name in SUPPORTED_NAMES {
        let candidate = path.join(name);
        if !candidate.is_file() {
            continue;
        }
        match validate_whisper_weights_for_architecture(&candidate, architecture) {
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
            "no architecture-compatible Whisper weights in {} ({})",
            path.display(),
            failures.join("; ")
        ))
    }
}

/// Validate the config/weights generation used by warm-cache composition.
pub(crate) fn validate_whisper_model_pair(path: &Path) -> Result<()> {
    let architecture = load_whisper_architecture(&path.join("config.json"))?;
    resolve_compatible_whisper_weights_path(path, architecture).map(|_| ())
}

/// Validate the complete safetensors structure without loading tensor data.
pub(crate) fn validate_safetensors_file(path: &Path) -> Result<()> {
    read_validated_tensor_shapes(path).map(|_| ())
}

fn read_validated_tensor_shapes(path: &Path) -> Result<BTreeMap<String, Vec<usize>>> {
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
    let mut tensor_shapes = BTreeMap::new();

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
        let dimensions = shape
            .iter()
            .map(|dim| {
                dim.as_u64()
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or_else(|| anyhow!("tensor {name} has an invalid shape"))
            })
            .collect::<Result<Vec<_>>>()?;
        let element_count = dimensions.iter().try_fold(1_u64, |count, dim| {
            count
                .checked_mul(*dim as u64)
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
        tensor_shapes.insert(name.clone(), dimensions);
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
    Ok(tensor_shapes)
}

fn validate_whisper_weights_for_architecture(
    path: &Path,
    architecture: WhisperArchitecture,
) -> Result<()> {
    let tensors = read_validated_tensor_shapes(path)?;
    for (name, expected) in expected_whisper_tensor_shapes(architecture)? {
        let actual = tensors.get(&name).ok_or_else(|| {
            anyhow!(
                "Whisper weights {} are missing tensor {name}",
                path.display()
            )
        })?;
        if actual != &expected {
            return Err(anyhow!(
                "Whisper tensor {name} in {} has shape {:?}, expected {:?}",
                path.display(),
                actual,
                expected
            ));
        }
    }
    Ok(())
}

fn expected_whisper_tensor_shapes(
    architecture: WhisperArchitecture,
) -> Result<BTreeMap<String, Vec<usize>>> {
    fn add(out: &mut BTreeMap<String, Vec<usize>>, name: String, shape: &[usize]) {
        out.insert(name, shape.to_vec());
    }

    fn add_attention(out: &mut BTreeMap<String, Vec<usize>>, prefix: &str, model_width: usize) {
        add(
            out,
            format!("{prefix}.key.weight"),
            &[model_width, model_width],
        );
        add(
            out,
            format!("{prefix}.query.weight"),
            &[model_width, model_width],
        );
        add(out, format!("{prefix}.query.bias"), &[model_width]);
        add(
            out,
            format!("{prefix}.value.weight"),
            &[model_width, model_width],
        );
        add(out, format!("{prefix}.value.bias"), &[model_width]);
        add(
            out,
            format!("{prefix}.out.weight"),
            &[model_width, model_width],
        );
        add(out, format!("{prefix}.out.bias"), &[model_width]);
    }

    fn add_block_tail(
        out: &mut BTreeMap<String, Vec<usize>>,
        prefix: &str,
        model_width: usize,
        feed_forward_width: usize,
    ) {
        add(out, format!("{prefix}.attn_ln.weight"), &[model_width]);
        add(out, format!("{prefix}.attn_ln.bias"), &[model_width]);
        add(
            out,
            format!("{prefix}.mlp1.weight"),
            &[feed_forward_width, model_width],
        );
        add(out, format!("{prefix}.mlp1.bias"), &[feed_forward_width]);
        add(
            out,
            format!("{prefix}.mlp2.weight"),
            &[model_width, feed_forward_width],
        );
        add(out, format!("{prefix}.mlp2.bias"), &[model_width]);
        add(out, format!("{prefix}.mlp_ln.weight"), &[model_width]);
        add(out, format!("{prefix}.mlp_ln.bias"), &[model_width]);
    }

    let mut out = BTreeMap::new();
    let d = architecture.n_audio_state;
    let ff = d
        .checked_mul(4)
        .ok_or_else(|| anyhow!("Whisper feed-forward width overflows"))?;
    add(
        &mut out,
        "encoder.conv1.weight".into(),
        &[d, 3, architecture.n_mels],
    );
    add(&mut out, "encoder.conv1.bias".into(), &[d]);
    add(&mut out, "encoder.conv2.weight".into(), &[d, 3, d]);
    add(&mut out, "encoder.conv2.bias".into(), &[d]);
    add(&mut out, "encoder.ln_post.weight".into(), &[d]);
    add(&mut out, "encoder.ln_post.bias".into(), &[d]);
    add(
        &mut out,
        "decoder.token_embedding.weight".into(),
        &[architecture.n_vocab, d],
    );
    add(
        &mut out,
        "decoder.positional_embedding".into(),
        &[architecture.n_text_ctx, d],
    );
    add(&mut out, "decoder.ln.weight".into(), &[d]);
    add(&mut out, "decoder.ln.bias".into(), &[d]);

    for layer in 0..architecture.n_audio_layer {
        let prefix = format!("encoder.blocks.{layer}");
        add_attention(&mut out, &format!("{prefix}.attn"), d);
        add_block_tail(&mut out, &prefix, d, ff);
    }
    for layer in 0..architecture.n_text_layer {
        let prefix = format!("decoder.blocks.{layer}");
        add_attention(&mut out, &format!("{prefix}.attn"), d);
        add_attention(&mut out, &format!("{prefix}.cross_attn"), d);
        add(&mut out, format!("{prefix}.cross_attn_ln.weight"), &[d]);
        add(&mut out, format!("{prefix}.cross_attn_ln.bias"), &[d]);
        add_block_tail(&mut out, &prefix, d, ff);
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) fn write_test_whisper_weights(
    path: &Path,
    architecture: WhisperArchitecture,
) -> Result<()> {
    let mut offset = 0_u64;
    let mut header = serde_json::Map::new();
    for (name, shape) in expected_whisper_tensor_shapes(architecture)? {
        let elements = shape.iter().try_fold(1_u64, |count, dim| {
            count
                .checked_mul(*dim as u64)
                .ok_or_else(|| anyhow!("test tensor shape overflows"))
        })?;
        let end = offset
            .checked_add(elements * 2)
            .ok_or_else(|| anyhow!("test tensor payload overflows"))?;
        header.insert(
            name,
            serde_json::json!({
                "dtype": "F16",
                "shape": shape,
                "data_offsets": [offset, end]
            }),
        );
        offset = end;
    }
    let header = serde_json::to_vec(&header)?;
    let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
    bytes.extend_from_slice(&header);
    bytes.resize(bytes.len() + offset as usize, 0);
    fs::write(path, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> serde_json::Value {
        serde_json::from_str(include_str!("../tests/fixtures/whisper_config.json")).unwrap()
    }

    #[test]
    fn official_whisper_architecture_is_accepted() {
        let raw = include_str!("../tests/fixtures/whisper_config.json");
        let architecture = parse_whisper_config(raw, "fixture").unwrap();
        assert_eq!(architecture.n_mels, 128);
        assert_eq!(architecture.n_audio_state, 1280);
        assert_eq!(architecture.n_text_state, 1280);
    }

    #[test]
    fn every_loader_dimension_is_required() {
        let fields = [
            "n_mels",
            "n_audio_ctx",
            "n_audio_state",
            "n_audio_head",
            "n_audio_layer",
            "n_vocab",
            "n_text_ctx",
            "n_text_state",
            "n_text_head",
            "n_text_layer",
        ];
        for field in fields {
            let mut config = valid_config();
            config.as_object_mut().unwrap().remove(field);
            let err = parse_whisper_config(&config.to_string(), "fixture").unwrap_err();
            assert!(err.to_string().contains(field), "{field}: {err:#}");
        }
    }

    #[test]
    fn non_positive_or_non_integer_dimensions_are_rejected() {
        for value in [
            serde_json::Value::Null,
            serde_json::json!("128"),
            serde_json::json!(-1),
            serde_json::json!(0),
            serde_json::json!(80.5),
        ] {
            let mut config = valid_config();
            config["n_mels"] = value;
            assert!(parse_whisper_config(&config.to_string(), "fixture").is_err());
        }
    }

    #[test]
    fn incompatible_architecture_relationships_are_rejected() {
        for (field, value, expected) in [
            ("n_mels", 81, "n_mels"),
            ("n_text_state", 640, "equal"),
            ("n_audio_head", 3, "n_audio_head"),
            ("n_text_head", 3, "n_text_head"),
            ("n_audio_state", 3, "equal"),
        ] {
            let mut config = valid_config();
            config[field] = serde_json::json!(value);
            let err = parse_whisper_config(&config.to_string(), "fixture").unwrap_err();
            assert!(err.to_string().contains(expected), "{field}: {err:#}");
        }

        let mut odd_state = valid_config();
        odd_state["n_audio_state"] = serde_json::json!(3);
        odd_state["n_text_state"] = serde_json::json!(3);
        assert!(
            parse_whisper_config(&odd_state.to_string(), "fixture")
                .unwrap_err()
                .to_string()
                .contains("even")
        );
    }
}
