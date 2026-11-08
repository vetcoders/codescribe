//! Native Gemini / Cloud Code Assist streaming Agent provider.
//!
//! The session owns history, tool execution and approvals. This provider parses
//! in-flight Google wire parts and hands one clean response to canonical Message
//! metadata. Signatures are never reconstructed from text or moved to a part.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use codescribe_core::agent::{
    AgentAssetStore, AgentEvent, AgentProvider, AgentProviderHost, AgentUiEvent, ContentBlock,
    ImageAsset, MCP_SEARCH_TOOL_NAME, Message, ProviderResponseMetadata, Role, StreamOptions,
    ToolDefinition,
};
use codescribe_core::config::{RuntimeAiRequestTiming, RuntimeLlmLane};
use codescribe_core::llm::account_auth::{
    GoogleCredential, redact_google_secrets, resolve_google_account_credential,
    resolve_google_credential,
};
use codescribe_core::llm::responses_streaming_manager::StreamWatchdog;
use codescribe_core::llm::vendors::google;

const STREAM_DEADLINE: Duration = Duration::from_secs(600);
const MAX_SSE_EVENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_SSE_DATA_LINES: usize = 8192;
const MAX_TURN_PART_BYTES: usize = 8 * 1024 * 1024;
const MAX_TURN_PARTS: usize = 8192;
const MAX_RETAINED_HISTORY_BYTES: usize = 16 * 1024 * 1024;
const MAX_RETAINED_TURNS: usize = 128;

/// Google protocol client. A provider instance belongs to one AgentSession.
#[derive(Clone)]
pub struct GoogleProvider {
    client: reqwest::Client,
    endpoint: String,
    default_model: String,
    credential_account: Option<String>,
    fixture_key: Option<String>,
    initial_timeout: Duration,
    chunk_timeout: Duration,
    first_event_timeout: Duration,
    completed_response: Arc<Mutex<Option<ProviderResponseMetadata>>>,
    turn_lock: Arc<tokio::sync::Mutex<()>>,
    host: Arc<Mutex<Option<AgentProviderHost>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SkippedGoogleTool {
    registered_name: String,
    server: String,
    tool: String,
    reason: String,
}

impl SkippedGoogleTool {
    fn warning_message(&self) -> String {
        format!(
            "Skipped MCP tool '{}' (server: '{}', tool: '{}') on Google Gemini: {}",
            self.registered_name, self.server, self.tool, self.reason
        )
    }
}

#[derive(Clone)]
struct Call {
    id: String,
    name: String,
    arguments: Value,
    wire_id: Option<String>,
}

/// A bounded projection of canonical message metadata for this one request.
/// It is rebuilt from Message/ThreadMessage; the provider retains no history.
struct RetainedTurn {
    prefix_len: usize,
    message_count: usize,
    parts: Vec<Value>,
    calls: Vec<Call>,
    metadata_bytes: usize,
}

struct PendingTurn {
    context_digest: String,
    provider_id: String,
    model: String,
    history_bytes: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GoogleTurnPayload {
    parts: Vec<Value>,
    /// In original functionCall part order; these are canonical native call IDs.
    call_ids: Vec<String>,
    /// Absent on responses whose context used the original Message serialization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context_digest_version: Option<u8>,
}

impl GoogleProvider {
    /// Resolve credentials at each send, so rotation and ADC refresh are live.
    pub fn from_lane(lane: &RuntimeLlmLane, timing: &RuntimeAiRequestTiming) -> Result<Self> {
        let mut provider = Self::from_configuration(
            lane.endpoint().to_string(),
            lane.model().to_string(),
            String::new(),
            timing,
        )?;
        provider.credential_account = Some(lane.credential().key_account().to_string());
        Ok(provider)
    }

    /// Explicit embedding configuration, also used by isolated mock fixtures.
    pub fn from_configuration(
        endpoint: String,
        default_model: String,
        api_key: String,
        timing: &RuntimeAiRequestTiming,
    ) -> Result<Self> {
        google::prepare_request(&endpoint, &default_model, json!({}), true)
            .map_err(|error| anyhow!(error))?;
        ensure!(
            !google::is_cloud_code_assist(&endpoint)
                || api_key.is_empty()
                || google::is_loopback_endpoint(&endpoint),
            "Cloud Code Assist requires account credentials; an API key is permitted only in a loopback fixture"
        );
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(STREAM_DEADLINE)
            .build()
            .context("Failed to create Google HTTP client")?;
        Ok(Self {
            client,
            endpoint,
            default_model,
            credential_account: None,
            fixture_key: (!api_key.is_empty()).then_some(api_key),
            initial_timeout: timing.attempt_timeout(),
            chunk_timeout: timing.inter_chunk_timeout(),
            first_event_timeout: timing.first_event_timeout(),
            completed_response: Arc::new(Mutex::new(None)),
            turn_lock: Arc::new(tokio::sync::Mutex::new(())),
            host: Arc::new(Mutex::new(None)),
        })
    }
}

#[async_trait]
impl AgentProvider for GoogleProvider {
    fn bind_host(&self, host: AgentProviderHost) {
        *self
            .host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(host);
    }

    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        options: &StreamOptions,
    ) -> Result<mpsc::Receiver<AgentEvent>> {
        let turn_guard = self.turn_lock.clone().lock_owned().await;
        ensure!(
            self.completed_response
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .is_none(),
            "The canonical session has not consumed the previous Google response metadata"
        );
        let model = if options.model.trim().is_empty() {
            &self.default_model
        } else {
            &options.model
        };
        ensure!(
            !model.is_empty() && model.len() <= 1024,
            "Invalid Google model identity"
        );
        // Endpoint/account identity is private routing provenance, not a key.
        let provider_id = hex::encode(Sha256::digest(
            format!(
                "{}|{}",
                self.endpoint,
                self.credential_account
                    .as_deref()
                    .unwrap_or(google::API_KEY_ACCOUNT)
            )
            .as_bytes(),
        ));
        let mut images = GoogleImageSnapshot::default();
        let (retained, context_digest) = restore_turns(messages, model, &provider_id, &mut images)?;
        ensure!(
            retained.len() < MAX_RETAINED_TURNS,
            "Google signed history reached its retention limit; start a new conversation"
        );
        let host = self
            .host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let (body, skipped_tools) = build_request_with_host(
            messages,
            tools,
            options,
            &retained,
            host.as_ref(),
            &mut images,
        )?;
        let credential = if let Some(key) = &self.fixture_key {
            GoogleCredential::ApiKey(key.clone())
        } else if google::is_cloud_code_assist(&self.endpoint) {
            resolve_google_account_credential().await?
        } else {
            resolve_google_credential(
                self.credential_account
                    .as_deref()
                    .unwrap_or(google::API_KEY_ACCOUNT),
            )
            .await?
        };
        let (endpoint, body) = google::prepare_request_with_credential(
            &self.endpoint,
            model,
            body,
            true,
            Some(&credential),
        )
        .map_err(|error| anyhow!(error))?;
        let (tx, rx) = mpsc::channel(256);
        let transport = self.clone();
        let pending = PendingTurn {
            context_digest,
            provider_id,
            model: model.clone(),
            history_bytes: retained.iter().map(|turn| turn.metadata_bytes).sum(),
        };
        tokio::spawn(async move {
            let _turn_guard = turn_guard;
            for skipped in &skipped_tools {
                let id = format!("google_schema_skip_{}", skipped.registered_name);
                let summary = skipped.warning_message();
                if let Some(host) = &host {
                    tokio::select! {
                        biased;
                        _ = tx.closed() => return,
                        _ = host.ui_tx.send(AgentUiEvent::ToolResult {
                            name: skipped.registered_name.clone(),
                            id,
                            summary,
                            is_error: true,
                        }) => {}
                    }
                } else {
                    if tx
                        .send(AgentEvent::ServerToolProgress {
                            id,
                            name: skipped.registered_name.clone(),
                            done: true,
                            query: Some(summary),
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
            let result = transport
                .run_stream(endpoint, body, credential, pending, &tx)
                .await;
            if let Err(error) = result {
                drop(
                    transport
                        .completed_response
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .take(),
                );
                let _ = tx
                    .send(AgentEvent::ResponseDone {
                        response_id: None,
                        clean: false,
                    })
                    .await;
                let _ = tx
                    .send(AgentEvent::Error(redact_google_secrets(&error.to_string())))
                    .await;
            }
        });
        Ok(rx)
    }

    fn take_completed_response_metadata(&self) -> Option<ProviderResponseMetadata> {
        self.completed_response
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take()
    }

    async fn restore_response_chain(&self, _id: Option<String>) {
        // The receiver's drop cancels the transport. Wait for its handoff to
        // settle, then discard only ephemeral state; canonical history rolls
        // back with the owning AgentSession's message vector.
        let _turn = self.turn_lock.lock().await;
        drop(
            self.completed_response
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .take(),
        );
    }

    fn build_tool_result(
        &self,
        call_id: &str,
        content: Vec<ContentBlock>,
        is_error: bool,
    ) -> Message {
        super::user_tool_result(call_id, content, is_error)
    }
    fn build_image_block(&self, data: &[u8], media_type: &str) -> ContentBlock {
        super::image_block(data, media_type)
    }
    fn stream_timeouts(&self) -> Option<(Duration, Duration)> {
        Some((self.initial_timeout, self.chunk_timeout))
    }
    /// The window of the model this request would go to: `options.model`, or
    /// this provider's default when the session leaves it empty.
    fn context_window(&self, options: &StreamOptions) -> Option<u64> {
        let model = if options.model.trim().is_empty() {
            &self.default_model
        } else {
            &options.model
        };
        Some(codescribe_core::agent::compaction::default_context_window(
            model,
        ))
    }

    fn name(&self) -> &str {
        "google-generate-content"
    }
}

fn hash_message(digest: &mut Sha256, message: &Message) -> Result<()> {
    let bytes = serde_json::to_vec(message)?;
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
    Ok(())
}

/// One request's immutable image bytes, shared by history validation and wire
/// encoding. Resolving through the asset owner also keeps stored paths from
/// becoming arbitrary filesystem reads.
#[derive(Default)]
struct GoogleImageSnapshot {
    assets: HashMap<PathBuf, Arc<[u8]>>,
}

impl GoogleImageSnapshot {
    fn read_asset(&mut self, asset: &ImageAsset) -> Result<Arc<[u8]>> {
        ensure!(
            asset.path.file_stem().and_then(|stem| stem.to_str()) == Some(asset.asset_id.as_str()),
            "Google image asset identity does not match its file"
        );
        let file_name = asset
            .path
            .file_name()
            .context("Google image asset has no file name")?;
        let data = match self.assets.entry(PathBuf::from(file_name)) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.get().clone(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let data: Arc<[u8]> = AgentAssetStore::read_image(&asset.path)?.into();
                entry.insert(data.clone());
                data
            }
        };
        ensure!(
            asset.size_bytes == data.len() as u64,
            "Google image asset size does not match its bytes"
        );
        if let Some(expected) = asset.asset_id.strip_prefix("inline_") {
            ensure!(
                expected == hex::encode(Sha256::digest(&data)),
                "Google inline image asset does not match its content identity"
            );
        }
        Ok(data)
    }

    fn canonical_block(&mut self, block: &ContentBlock, depth: usize) -> Result<Value> {
        ensure!(depth < 32, "Google image history nesting exceeds its limit");
        let image_identity = |data: &[u8], media_type: &str| {
            json!({
                "type": "image",
                "payload": {
                    "media_type": google_image_media_type(media_type),
                    "size_bytes": data.len(),
                    "sha256": hex::encode(Sha256::digest(data)),
                },
            })
        };
        match block {
            ContentBlock::Image { data, media_type } => Ok(image_identity(data, media_type)),
            ContentBlock::ImageAsset(asset) => {
                let data = self.read_asset(asset)?;
                Ok(image_identity(&data, &asset.media_type))
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Ok(json!({
                "type": "tool_result",
                "payload": {
                    "tool_use_id": tool_use_id,
                    "content": content.iter()
                        .map(|child| self.canonical_block(child, depth + 1))
                        .collect::<Result<Vec<_>>>()?,
                    "is_error": is_error,
                },
            })),
            _ => Ok(serde_json::to_value(block)?),
        }
    }

    /// Undo only the known inline-image spill performed by ThreadStore. A
    /// captured screenshot asset was already an asset when its response was
    /// signed and must retain that original representation for v1 validation.
    fn restore_inline_blocks(&mut self, blocks: &mut [ContentBlock], depth: usize) -> Result<()> {
        ensure!(depth < 32, "Google image history nesting exceeds its limit");
        for block in blocks {
            match block {
                ContentBlock::ImageAsset(asset) if asset.asset_id.starts_with("inline_") => {
                    let data = self.read_asset(asset)?;
                    *block = ContentBlock::Image {
                        data: data.to_vec(),
                        media_type: asset.media_type.clone(),
                    };
                }
                ContentBlock::ToolResult { content, .. } => {
                    self.restore_inline_blocks(content, depth + 1)?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn google_image_media_type(media_type: &str) -> &str {
    match media_type.trim() {
        "" => "image/png",
        media_type => media_type,
    }
}

/// v1 readers reconstruct only three known runtime/storage representations.
/// Each must reproduce the saved digest exactly; no mismatch drops signatures
/// or silently replays a modified history. v2 is independent of image storage
/// and of display timestamps, which ThreadStore may populate on restoration.
struct GoogleHistoryDigests {
    canonical: Sha256,
    serialized: Sha256,
    inline: Sha256,
    current_user_turn: Sha256,
    reconstruct_v1: bool,
}

impl GoogleHistoryDigests {
    fn new(reconstruct_v1: bool) -> Self {
        let mut canonical = Sha256::new();
        canonical.update(b"codescribe.google.history\0v2\0");
        Self {
            canonical,
            serialized: Sha256::new(),
            inline: Sha256::new(),
            current_user_turn: Sha256::new(),
            reconstruct_v1,
        }
    }

    fn push(&mut self, message: &Message, images: &mut GoogleImageSnapshot) -> Result<()> {
        if self.reconstruct_v1 {
            if message.role == Role::User
                && message
                    .content
                    .iter()
                    .any(|block| !matches!(block, ContentBlock::ToolResult { .. }))
            {
                // The bridge restores older turns from disk before accepting
                // each new user turn. Only this turn's composer images were
                // still inline during its provider/tool rounds.
                self.current_user_turn = self.serialized.clone();
            }
            hash_message(&mut self.serialized, message)?;
            let mut inline = message.clone();
            images.restore_inline_blocks(&mut inline.content, 0)?;
            hash_message(&mut self.inline, &inline)?;
            hash_message(&mut self.current_user_turn, &inline)?;
        }

        let content = message
            .content
            .iter()
            .map(|block| images.canonical_block(block, 0))
            .collect::<Result<Vec<_>>>()?;
        let bytes = serde_json::to_vec(&json!({
            "role": message.role,
            "content": content,
            "metadata": message.metadata.as_ref(),
        }))?;
        self.canonical.update((bytes.len() as u64).to_be_bytes());
        self.canonical.update(bytes);
        Ok(())
    }

    fn matches(&self, expected: &str, version: Option<u8>) -> Result<bool> {
        match version {
            Some(2) => Ok(expected == self.canonical_digest()),
            None => Ok([&self.serialized, &self.inline, &self.current_user_turn]
                .into_iter()
                .any(|digest| expected == hex::encode((*digest).clone().finalize()))),
            Some(_) => bail!("Unsupported Google context digest version"),
        }
    }

    fn canonical_digest(&self) -> String {
        hex::encode(self.canonical.clone().finalize())
    }
}

fn assistant_segments(text: &str, calls: &[Call]) -> Vec<Vec<ContentBlock>> {
    let mut segments = Vec::new();
    if !text.trim().is_empty() {
        segments.push(vec![ContentBlock::Text(text.trim().to_string())]);
    }
    if !calls.is_empty() {
        segments.push(
            calls
                .iter()
                .map(|call| ContentBlock::ToolUse {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    input: call.arguments.clone(),
                })
                .collect(),
        );
    }
    segments
}

/// Validate a payload before either committing it or replaying it. Occurrence
/// UUIDs establish identity; content comparisons below only check integrity.
fn decode_payload(payload: &GoogleTurnPayload) -> Result<(String, Vec<Call>)> {
    ensure!(
        payload.parts.len() <= MAX_TURN_PARTS
            && serde_json::to_vec(&payload.parts)?.len() <= MAX_TURN_PART_BYTES,
        "Google signed parts exceed their bounds"
    );
    let mut filter = PlanningFilter::default();
    let mut visible = String::new();
    let mut calls = Vec::new();
    let mut ids = HashSet::new();
    let mut wire_ids = HashSet::new();
    for part in &payload.parts {
        let object = part
            .as_object()
            .ok_or_else(|| anyhow!("Malformed Google stored part"))?;
        ensure!(
            object.keys().all(|key| matches!(
                key.as_str(),
                "text" | "thought" | "thoughtSignature" | "functionCall"
            )) && part.get("text").is_none_or(Value::is_string)
                && part.get("thought").is_none_or(Value::is_boolean)
                && part.get("thoughtSignature").is_none_or(Value::is_string)
                && !(part.get("text").is_some() && part.get("functionCall").is_some()),
            "Unsupported Google stored part schema"
        );
        if let Some(text) = part.get("text").and_then(Value::as_str)
            && part.get("thought").and_then(Value::as_bool) != Some(true)
            && let Some(text) = filter.push(text)
        {
            visible.push_str(&text);
        }
        if let Some(raw) = part.get("functionCall") {
            let call = raw
                .as_object()
                .ok_or_else(|| anyhow!("Malformed Google stored call"))?;
            ensure!(
                call.keys()
                    .all(|key| matches!(key.as_str(), "name" | "args" | "id")),
                "Unsupported Google stored function call schema"
            );
            let name = raw
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty() && name.len() <= 1024)
                .ok_or_else(|| anyhow!("Malformed Google stored tool name"))?;
            let arguments = raw.get("args").cloned().unwrap_or_else(|| json!({}));
            ensure!(
                arguments.is_object(),
                "Malformed Google stored tool arguments"
            );
            let id = payload
                .call_ids
                .get(calls.len())
                .ok_or_else(|| anyhow!("Missing Google native call identity"))?;
            ensure!(
                id.strip_prefix("google_")
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok_and(|id| !id.is_nil()))
                    && ids.insert(id.clone()),
                "Invalid or duplicate Google native call identity"
            );
            let wire_id = match raw.get("id") {
                Some(Value::String(id))
                    if !id.is_empty() && id.len() <= 1024 && wire_ids.insert(id.clone()) =>
                {
                    Some(id.clone())
                }
                None => None,
                _ => bail!("Invalid or duplicate Google wire call identity"),
            };
            calls.push(Call {
                id: id.clone(),
                name: name.to_string(),
                arguments,
                wire_id,
            });
        }
    }
    ensure!(
        calls.len() == payload.call_ids.len(),
        "Orphan Google native call identity"
    );
    if let Some(text) = filter.finish() {
        visible.push_str(&text);
    }
    Ok((visible, calls))
}

fn restore_turns(
    messages: &[Message],
    model: &str,
    provider_id: &str,
    images: &mut GoogleImageSnapshot,
) -> Result<(Vec<RetainedTurn>, String)> {
    let mut turns = Vec::new();
    let mut groups = HashSet::new();
    let mut native_calls = HashSet::new();
    // This only skips byte-heavy v1 projections on entirely v2 histories;
    // the ordinary metadata parsing and validation below remain authoritative.
    let reconstruct_v1 = messages.iter().any(|message| {
        let Some(response) = message
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.as_stored().get("provider_wire"))
            .and_then(|wire| wire.get("response"))
        else {
            return false;
        };
        response.get("wire").and_then(Value::as_str) == Some("google-generate-content")
            && response.get("model").and_then(Value::as_str) == Some(model)
            && response.get("provider_id").and_then(Value::as_str) == Some(provider_id)
            && response
                .get("payload")
                .and_then(|payload| payload.get("context_digest_version"))
                .is_none_or(Value::is_null)
    });
    let mut digests = GoogleHistoryDigests::new(reconstruct_v1);
    let mut total_bytes = 0usize;
    let mut index = 0usize;
    while index < messages.len() {
        let metadata = messages[index]
            .metadata
            .as_ref()
            .map(|metadata| metadata.provider_wire())
            .transpose()?
            .flatten();
        let Some(first) = metadata else {
            // Unsigned history, such as the turns of a thread begun on another
            // provider, is replayed as ordinary content. Gemini validates
            // signatures only on the current turn's calls, and this history
            // always precedes the user message that opens the current turn.
            digests.push(&messages[index], images)?;
            index += 1;
            continue;
        };
        ensure!(
            messages[index].role == Role::Assistant
                && first.segment_index == 0
                && groups.insert(first.occurrence_id.clone()),
            "Orphan or duplicate Google response occurrence"
        );
        let count = usize::from(first.segment_count);
        ensure!(
            count > 0 && index + count <= messages.len(),
            "Invalid Google response segment count"
        );
        let response = first
            .response
            .as_ref()
            .ok_or_else(|| anyhow!("Missing Google wire response"))?;
        if response.wire != "google-generate-content"
            || response.model != model
            || response.provider_id != provider_id
        {
            // Signed for another model or account: those signatures do not
            // belong to this request, so its visible content replays unsigned.
            for (offset, message) in messages[index..index + count].iter().enumerate() {
                let segment = message
                    .metadata
                    .as_ref()
                    .map(|metadata| metadata.provider_wire())
                    .transpose()?
                    .flatten()
                    .ok_or_else(|| anyhow!("Missing Google response segment"))?;
                ensure!(
                    message.role == Role::Assistant
                        && segment.occurrence_id == first.occurrence_id
                        && segment.segment_count == first.segment_count
                        && usize::from(segment.segment_index) == offset,
                    "Google response occurrence does not match canonical content"
                );
                digests.push(message, images)?;
            }
            index += count;
            continue;
        }
        let payload: GoogleTurnPayload = serde_json::from_value(response.payload.clone())
            .map_err(|_| anyhow!("Malformed Google signed payload"))?;
        ensure!(
            digests.matches(&response.context_digest, payload.context_digest_version)?,
            "Google history context does not match its signed response"
        );
        let metadata_bytes = serde_json::to_vec(response)?.len();
        total_bytes = total_bytes.saturating_add(metadata_bytes);
        ensure!(
            turns.len() < MAX_RETAINED_TURNS && total_bytes <= MAX_RETAINED_HISTORY_BYTES,
            "Google signed history exceeds its bounds"
        );
        let (visible, calls) = decode_payload(&payload)?;
        let expected = assistant_segments(&visible, &calls);
        ensure!(
            expected.len() == count && index + count <= messages.len(),
            "Invalid Google response segment count"
        );
        for offset in 0..count {
            let message = &messages[index + offset];
            let segment = message
                .metadata
                .as_ref()
                .map(|metadata| metadata.provider_wire())
                .transpose()?
                .flatten()
                .ok_or_else(|| anyhow!("Missing Google response segment"))?;
            ensure!(
                message.role == Role::Assistant
                    && segment.occurrence_id == first.occurrence_id
                    && segment.segment_count == first.segment_count
                    && usize::from(segment.segment_index) == offset
                    && message.content == expected[offset],
                "Google response occurrence does not match canonical content"
            );
            digests.push(message, images)?;
        }
        for call in &calls {
            ensure!(
                native_calls.insert(call.id.clone()),
                "Google call identity belongs to more than one occurrence"
            );
        }
        turns.push(RetainedTurn {
            prefix_len: index,
            message_count: count,
            parts: payload.parts,
            calls,
            metadata_bytes,
        });
        index += count;
    }
    Ok((turns, digests.canonical_digest()))
}

fn parse_tool_origin(name: &str, host: Option<&AgentProviderHost>) -> (String, String) {
    if let Some(host) = host
        && let Some(entry) = host
            .tools
            .mcp_entries()
            .into_iter()
            .find(|entry| entry.registered_name == name || entry.upstream_tool == name)
    {
        return (entry.server.to_string(), entry.upstream_tool.to_string());
    }
    if let Some(rest) = name.strip_prefix("mcp__")
        && let Some((server, tool)) = rest.split_once("__")
        && !server.is_empty()
        && !tool.is_empty()
    {
        return (server.to_string(), tool.to_string());
    }
    for sep in ['/', ':'] {
        if let Some((server, tool)) = name.split_once(sep)
            && !server.is_empty()
            && !tool.is_empty()
        {
            return (server.to_string(), tool.to_string());
        }
    }
    ("mcp".to_string(), name.to_string())
}

#[cfg(test)]
fn build_request(
    messages: &[Message],
    tools: &[ToolDefinition],
    options: &StreamOptions,
    retained: &[RetainedTurn],
) -> Result<Value> {
    build_request_with_host(
        messages,
        tools,
        options,
        retained,
        None,
        &mut GoogleImageSnapshot::default(),
    )
    .map(|(body, _)| body)
}

fn build_request_with_host(
    messages: &[Message],
    tools: &[ToolDefinition],
    options: &StreamOptions,
    retained: &[RetainedTurn],
    host: Option<&AgentProviderHost>,
    images: &mut GoogleImageSnapshot,
) -> Result<(Value, Vec<SkippedGoogleTool>)> {
    let mut contents: Vec<Value> = Vec::new();
    let mut system = Vec::new();
    if let Some(prompt) = options
        .system_prompt
        .as_deref()
        .filter(|text| !text.is_empty())
    {
        system.push(json!({"text": prompt}));
    }
    // Outstanding calls by id: the function name and Gemini's own wire id.
    let mut pending_calls: HashMap<&str, (&str, Option<&str>)> = HashMap::new();
    let mut last_retained = false;
    let mut answered = HashSet::new();
    let mut index = 0;
    while index < messages.len() {
        if let Some(turn) = retained.iter().find(|turn| turn.prefix_len == index) {
            ensure!(
                pending_calls.is_empty(),
                "Google model turn precedes outstanding tool results"
            );
            contents.push(json!({"role":"model","parts":turn.parts}));
            for call in &turn.calls {
                pending_calls.insert(
                    call.id.as_str(),
                    (call.name.as_str(), call.wire_id.as_deref()),
                );
            }
            last_retained = true;
            index += turn.message_count;
            continue;
        }
        // An Assistant message here has no signed parts for this model:
        // another provider wrote it, or another Gemini model or account. It
        // replays unsigned, which Gemini accepts outside the current turn.
        let message = &messages[index];
        let mut parts = Vec::new();
        for block in &message.content {
            match block {
                ContentBlock::Text(text) => {
                    ensure!(
                        pending_calls.is_empty(),
                        "Google conversation text precedes outstanding tool results"
                    );
                    parts.push(json!({"text":text}));
                }
                ContentBlock::Image { data, media_type } => {
                    if !data.is_empty() {
                        parts.push(json!({"inlineData":{"mimeType":google_image_media_type(media_type),"data":BASE64.encode(data)}}));
                    }
                }
                ContentBlock::ImageAsset(asset) => {
                    let data = images.read_asset(asset)?;
                    if !data.is_empty() {
                        parts.push(json!({"inlineData":{"mimeType":google_image_media_type(&asset.media_type),"data":BASE64.encode(&data)}}));
                    }
                }
                ContentBlock::ToolUse { id, name, input } => {
                    ensure!(
                        message.role == Role::Assistant,
                        "Google tool call outside an assistant message"
                    );
                    ensure!(
                        pending_calls
                            .insert(id.as_str(), (name.as_str(), None))
                            .is_none(),
                        "Duplicate Google tool call identity"
                    );
                    let args = if input.is_object() {
                        input.clone()
                    } else {
                        json!({ "arguments": input })
                    };
                    parts.push(json!({"functionCall":{"name":name,"args":args}}));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    ensure!(
                        message.role == Role::User && answered.insert(tool_use_id.as_str()),
                        "Duplicate or non-user Google tool result"
                    );
                    let (name, wire_id) =
                        pending_calls.remove(tool_use_id.as_str()).ok_or_else(|| {
                            anyhow!("Google tool result has no preceding outstanding model call")
                        })?;
                    let mut response = json!({"name":name,"response":{"result":tool_result_text(content),"is_error":is_error}});
                    if let Some(id) = wire_id {
                        response["id"] = json!(id);
                    }
                    parts.push(json!({"functionResponse":response}));
                }
            }
        }
        if message.role == Role::System {
            system.extend(parts);
        } else if !parts.is_empty() {
            let role = if message.role == Role::Assistant {
                "model"
            } else {
                "user"
            };
            let results_only = parts
                .iter()
                .all(|part| part.get("functionResponse").is_some());
            if results_only
                && contents.last().is_some_and(|last| {
                    last["role"] == "user"
                        && last["parts"].as_array().is_some_and(|parts| {
                            parts
                                .iter()
                                .all(|part| part.get("functionResponse").is_some())
                        })
                })
            {
                contents.last_mut().unwrap()["parts"]
                    .as_array_mut()
                    .unwrap()
                    .extend(parts);
            } else if role == "model"
                && !last_retained
                && contents.last().is_some_and(|last| last["role"] == "model")
            {
                // One unsigned model turn, never appended to a signed one.
                contents.last_mut().unwrap()["parts"]
                    .as_array_mut()
                    .unwrap()
                    .extend(parts);
            } else {
                contents.push(json!({"role":role,"parts":parts}));
            }
            last_retained = false;
        }
        index += 1;
    }
    ensure!(
        pending_calls.is_empty(),
        "Google history has incomplete tool results"
    );
    let mut body = json!({"contents":contents});
    let mut generation = json!({});
    if let Some(tokens) = options.max_tokens.filter(|tokens| *tokens > 0) {
        generation["maxOutputTokens"] = json!(tokens);
    }
    if let Some(temperature) = options.temperature {
        generation["temperature"] = json!(temperature);
    }
    if generation
        .as_object()
        .is_some_and(|object| !object.is_empty())
    {
        body["generationConfig"] = generation;
    }
    let mut skipped: Vec<SkippedGoogleTool> = Vec::new();
    if !tools.is_empty() {
        let mut declarations = Vec::new();
        for tool in tools {
            match google::normalize_schema_for_gemini(&tool.input_schema) {
                Ok(schema) => {
                    declarations.push(json!({
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": schema,
                    }));
                }
                Err(reason) => {
                    let (server, upstream_tool) = parse_tool_origin(&tool.name, host);
                    if !skipped.iter().any(|item| item.registered_name == tool.name) {
                        skipped.push(SkippedGoogleTool {
                            registered_name: tool.name.clone(),
                            server,
                            tool: upstream_tool,
                            reason,
                        });
                    }
                }
            }
        }
        for item in &skipped {
            tracing::warn!(
                tool = %item.registered_name,
                upstream_tool = %item.tool,
                server = %item.server,
                reason = %item.reason,
                "{}",
                item.warning_message()
            );
        }
        if declarations.is_empty() {
            let details = skipped
                .iter()
                .map(SkippedGoogleTool::warning_message)
                .collect::<Vec<_>>()
                .join("; ");
            bail!(
                "Google tool schema normalization rejected all {} requested tool(s): {}",
                tools.len(),
                details
            );
        }
        if !skipped.is_empty() {
            let warning_block = skipped
                .iter()
                .map(SkippedGoogleTool::warning_message)
                .collect::<Vec<_>>()
                .join("\n");
            system.push(json!({"text": warning_block}));
            for decl in &mut declarations {
                if decl.get("name").and_then(Value::as_str) == Some(MCP_SEARCH_TOOL_NAME)
                    && let Some(desc) = decl
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                {
                    decl["description"] = Value::String(format!(
                        "{desc}\n\nUnavailable on Google Gemini:\n{warning_block}"
                    ));
                }
            }
        }
        body["tools"] = json!([{"functionDeclarations":declarations}]);
    }
    if !system.is_empty() {
        body["systemInstruction"] = json!({"parts":system});
    }
    Ok((body, skipped))
}

fn tool_result_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.clone()),
            ContentBlock::Image { .. } | ContentBlock::ImageAsset(_) => {
                Some("[image in tool result]".into())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl GoogleProvider {
    async fn run_stream(
        &self,
        endpoint: String,
        body: Value,
        credential: GoogleCredential,
        turn: PendingTurn,
        tx: &mpsc::Sender<AgentEvent>,
    ) -> Result<()> {
        let mut watchdog = StreamWatchdog::start(self.first_event_timeout, self.chunk_timeout);
        let is_cca = google::is_cloud_code_assist(&endpoint);
        let request = google::apply_credential_headers(
            self.client
                .post(endpoint)
                .header("accept", "text/event-stream")
                .json(&body),
            &credential,
            is_cca,
        );
        let response = tokio::select! {
            biased;
            _ = tx.closed() => return Ok(()),
            result = tokio::time::timeout(watchdog.next_wait(), request.send()) => {
                result.map_err(|_| anyhow!("Google response headers timed out"))?
                    .map_err(|error| anyhow!("Google transport failed: {}", redact_google_secrets(&error.without_url().to_string())))?
            }
        };
        if !response.status().is_success() {
            // Never surface provider error bodies: they can echo arbitrary
            // credential material (not just recognized AIza/ya29 prefixes).
            bail!(
                "Google request rejected (HTTP {})",
                response.status().as_u16()
            );
        }
        ensure!(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("text/event-stream")),
            "Google response was not an SSE stream"
        );
        let mut stream = response.bytes_stream();
        let deadline = tokio::time::Instant::now() + STREAM_DEADLINE;
        let mut parser = GoogleSse::default();
        let mut state = ResponseState::default();
        loop {
            let chunk = tokio::select! {
                biased;
                _ = tx.closed() => return Ok(()),
                _ = tokio::time::sleep_until(deadline) => bail!("Google stream exceeded its deadline"),
                chunk = tokio::time::timeout(
                    watchdog.next_wait(),
                    stream.next(),
                ) => chunk.map_err(|_| anyhow!(if state.content_started {
                    "Google stream stalled"
                } else {
                    "Google stream produced no content within the initial request budget"
                }))?,
            };
            match chunk {
                Some(Ok(bytes)) => {
                    for event in parser.push(&bytes)? {
                        state.consume(event, tx).await?;
                        if state.content_started {
                            watchdog.generation_started();
                        }
                    }
                }
                Some(Err(_)) => bail!("Google stream transport failed"),
                None => {
                    for event in parser.finish()? {
                        state.consume(event, tx).await?;
                    }
                    break;
                }
            }
        }
        ensure!(
            state.clean_terminal,
            "Google stream ended without a clean STOP terminal"
        );
        if let Some(text) = state.text.finish() {
            state.emit_text(text, tx).await?;
        }
        ensure!(
            !state.visible.trim().is_empty() || !state.calls.is_empty(),
            "Google returned no answer or tool calls"
        );
        let payload = GoogleTurnPayload {
            parts: state.parts,
            call_ids: state.calls.iter().map(|call| call.id.clone()).collect(),
            context_digest_version: Some(2),
        };
        let (visible, calls) = decode_payload(&payload)?;
        ensure!(
            assistant_segments(&visible, &calls)
                == assistant_segments(&state.visible, &state.calls),
            "Google completed wire state does not match its canonical response"
        );
        let metadata = ProviderResponseMetadata {
            wire: "google-generate-content".into(),
            provider_id: turn.provider_id,
            model: turn.model,
            context_digest: turn.context_digest,
            payload: serde_json::to_value(payload)?,
        };
        ensure!(
            turn.history_bytes
                .saturating_add(serde_json::to_vec(&metadata)?.len())
                <= MAX_RETAINED_HISTORY_BYTES,
            "Google signed history reached its retention limit; start a new conversation"
        );
        // One clean response handoff, not provider-owned conversation history.
        // AgentSession takes this only after accepting the terminal and EOF.
        *self
            .completed_response
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(metadata);
        tx.send(AgentEvent::TextDone(state.visible))
            .await
            .map_err(|_| anyhow!("Google receiver closed"))?;
        tx.send(AgentEvent::ResponseDone {
            response_id: None,
            clean: true,
        })
        .await
        .map_err(|_| anyhow!("Google receiver closed"))?;
        Ok(())
    }
}

/// Byte framing defers UTF-8 decoding until a complete line exists. HTTP may
/// split at any byte, including inside a Polish character or an emoji.
#[derive(Default)]
struct GoogleSse {
    bytes: Vec<u8>,
    data: Vec<String>,
    event_bytes: usize,
}
impl GoogleSse {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<Value>> {
        self.bytes.extend_from_slice(bytes);
        ensure!(
            self.bytes.len() <= MAX_SSE_EVENT_BYTES,
            "Google SSE line exceeds limit"
        );
        let mut events = Vec::new();
        while let Some(end) = self.bytes.iter().position(|byte| *byte == b'\n') {
            let line = self.bytes.drain(..=end).collect::<Vec<_>>();
            let line = std::str::from_utf8(&line[..line.len() - 1])
                .context("Google SSE contains invalid UTF-8")?
                .trim_end_matches('\r');
            self.line(line, &mut events)?;
        }
        Ok(events)
    }
    fn line(&mut self, line: &str, events: &mut Vec<Value>) -> Result<()> {
        if line.is_empty() {
            if !self.data.is_empty() {
                let data = self.data.join("\n");
                self.data.clear();
                self.event_bytes = 0;
                if data != "[DONE]" {
                    events.push(serde_json::from_str(&data).context("Invalid Google SSE JSON")?);
                }
            }
        } else if let Some(data) = line.strip_prefix("data:") {
            let data = data.strip_prefix(' ').unwrap_or(data);
            self.event_bytes = self
                .event_bytes
                .saturating_add(data.len())
                .saturating_add(1);
            ensure!(
                self.event_bytes <= MAX_SSE_EVENT_BYTES && self.data.len() < MAX_SSE_DATA_LINES,
                "Google SSE event exceeds limit"
            );
            self.data.push(data.to_string());
        }
        Ok(())
    }
    fn finish(&mut self) -> Result<Vec<Value>> {
        std::str::from_utf8(&self.bytes).context("Truncated Google SSE UTF-8")?;
        ensure!(
            self.bytes.is_empty() && self.data.is_empty(),
            "Google SSE ended inside an unfinished frame"
        );
        Ok(Vec::new())
    }
}

#[derive(Default)]
struct ResponseState {
    parts: Vec<Value>,
    part_bytes: usize,
    calls: Vec<Call>,
    visible: String,
    text: PlanningFilter,
    clean_terminal: bool,
    content_started: bool,
}
impl ResponseState {
    async fn emit_text(&mut self, text: String, tx: &mpsc::Sender<AgentEvent>) -> Result<()> {
        if !text.is_empty() {
            self.visible.push_str(&text);
            tx.send(AgentEvent::TextDelta(text))
                .await
                .map_err(|_| anyhow!("Google receiver closed"))?;
        }
        Ok(())
    }
    async fn consume(&mut self, envelope: Value, tx: &mpsc::Sender<AgentEvent>) -> Result<()> {
        ensure!(envelope.is_object(), "Google SSE event must be an object");
        ensure!(
            envelope.get("error").is_none(),
            "Google reported an SSE error"
        );
        let value = envelope.get("response").unwrap_or(&envelope);
        ensure!(value.is_object(), "Google response must be an object");
        ensure!(value.get("error").is_none(), "Google reported an SSE error");
        ensure!(
            value.pointer("/promptFeedback/blockReason").is_none(),
            "Google blocked the prompt"
        );
        let Some(raw_candidates) = value.get("candidates") else {
            ensure!(
                value.get("usageMetadata").is_some(),
                "Google SSE event has no candidate or usage metadata"
            );
            return Ok(());
        };
        let candidates = raw_candidates
            .as_array()
            .ok_or_else(|| anyhow!("Google candidates must be an array"))?;
        ensure!(
            candidates.iter().all(|candidate| candidate.is_object()
                && candidate
                    .get("index")
                    .is_none_or(|index| index.as_u64().is_some())),
            "Google candidate or index is malformed"
        );
        ensure!(
            candidates
                .iter()
                .filter(
                    |candidate| candidate.get("index").and_then(Value::as_u64).unwrap_or(0) == 0
                )
                .count()
                <= 1,
            "Google returned more than one primary candidate"
        );
        // The Agent asks for one candidate. Never mix alternative answers/tools.
        let Some(candidate) = candidates
            .iter()
            .find(|candidate| candidate.get("index").and_then(Value::as_u64).unwrap_or(0) == 0)
        else {
            return Ok(());
        };
        if let Some(raw_parts) = candidate.pointer("/content/parts") {
            let parts = raw_parts
                .as_array()
                .ok_or_else(|| anyhow!("Google parts must be an array"))?;
            ensure!(
                !self.clean_terminal,
                "Google emitted content after its terminal"
            );
            for part in parts {
                ensure!(part.is_object(), "Google content part must be an object");
                ensure!(
                    part.get("text").is_none_or(Value::is_string)
                        && part.get("thought").is_none_or(Value::is_boolean)
                        && part.get("thoughtSignature").is_none_or(Value::is_string),
                    "Google content part has malformed text or signature metadata"
                );
                let part_bytes = serde_json::to_vec(part)?.len();
                self.part_bytes = self.part_bytes.saturating_add(part_bytes);
                ensure!(
                    self.parts.len() < MAX_TURN_PARTS && self.part_bytes <= MAX_TURN_PART_BYTES,
                    "Google response exceeds the signed-part retention limit"
                );
                retain_thought_signature(&mut self.parts, part);
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    self.content_started |= !text.is_empty();
                    if part
                        .get("thought")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        tx.send(AgentEvent::ReasoningDelta(text.to_string()))
                            .await
                            .map_err(|_| anyhow!("Google receiver closed"))?;
                    } else if let Some(text) = self.text.push(text) {
                        self.emit_text(text, tx).await?;
                    }
                }
                if let Some(call) = part.get("functionCall") {
                    self.content_started = true;
                    ensure!(
                        call.is_object() && call.get("id").is_none_or(Value::is_string),
                        "Google function call is malformed"
                    );
                    ensure!(
                        call.get("partialArgs").is_none(),
                        "Google streamed partial function arguments without a complete call"
                    );
                    let name = call
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|name| !name.is_empty())
                        .ok_or_else(|| anyhow!("Google function call has no name"))?;
                    let arguments = call.get("args").cloned().unwrap_or_else(|| json!({}));
                    ensure!(
                        arguments.is_object(),
                        "Google function arguments must be an object"
                    );
                    let id = format!("google_{}", uuid::Uuid::new_v4());
                    self.calls.push(Call {
                        id: id.clone(),
                        name: name.to_string(),
                        arguments: arguments.clone(),
                        wire_id: call.get("id").and_then(Value::as_str).map(str::to_string),
                    });
                    tx.send(AgentEvent::ToolCallReady {
                        id,
                        name: name.to_string(),
                        arguments,
                    })
                    .await
                    .map_err(|_| anyhow!("Google receiver closed"))?;
                }
            }
        }
        if let Some(reason) = candidate.get("finishReason") {
            let reason = reason
                .as_str()
                .ok_or_else(|| anyhow!("Google finish reason must be a string"))?;
            ensure!(
                !self.clean_terminal,
                "Google emitted more than one terminal"
            );
            ensure!(
                reason == "STOP",
                "Google did not complete the response (non-STOP finish reason)"
            );
            self.clean_terminal = true;
        }
        Ok(())
    }
}

/// Retain each part verbatim, including empty signature-only text parts. Never
/// merge parts: a signature's position is part of the provider's contract.
fn retain_thought_signature(parts: &mut Vec<Value>, part: &Value) {
    parts.push(part.clone());
}

/// Recognize a leaked planning envelope before any of its bytes reach the UI.
fn is_planning_leak_prefix(text: &str) -> bool {
    let text = text.trim_start();
    let text = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .unwrap_or(text)
        .trim_start();
    let Some(text) = text.strip_prefix('{') else {
        return false;
    };
    let text = text.trim_start();
    ["\"thought\"", "\"thinking\"", "\"plan\""]
        .iter()
        .any(|key| {
            text.strip_prefix(key)
                .is_some_and(|rest| rest.trim_start().starts_with(':'))
        })
}

#[derive(Default)]
struct PlanningFilter {
    pending: String,
    decided: bool,
    suppress_until_object_end: bool,
    suppress_permanently: bool,
}
impl PlanningFilter {
    fn push(&mut self, text: &str) -> Option<String> {
        if self.suppress_permanently {
            return None;
        }
        if self.decided && !self.suppress_until_object_end {
            return Some(text.to_string());
        }
        self.pending.push_str(text);
        let trimmed = self.pending.trim_start();
        if self.suppress_until_object_end || is_planning_leak_prefix(trimmed) {
            self.decided = true;
            self.suppress_until_object_end = true;
            if let Some(remainder) = strip_leading_planning_leak(trimmed) {
                self.suppress_until_object_end = false;
                let out = remainder.to_string();
                self.pending.clear();
                return (!out.is_empty()).then_some(out);
            }
            if self.pending.len() > 16_384 {
                self.suppress_until_object_end = false;
                self.suppress_permanently = true;
                self.pending.clear();
            }
            return None;
        }
        if trimmed.is_empty() {
            if self.pending.len() > 512 {
                self.decided = true;
                self.suppress_permanently = true;
                self.pending.clear();
            }
            return None;
        }
        // Hold a possible object/fence prefix across arbitrary SSE parts. Once
        // the first key differs it is ordinary answer text and can flow.
        let possible = ["{", "```"]
            .iter()
            .any(|prefix| prefix.starts_with(trimmed) || trimmed.starts_with(prefix));
        if possible && !trimmed.contains(':') {
            if self.pending.len() > 512 {
                self.decided = true;
                self.suppress_permanently = true;
                self.pending.clear();
            }
            return None;
        }
        self.decided = true;
        Some(std::mem::take(&mut self.pending))
    }
    fn finish(&mut self) -> Option<String> {
        // An unfinished possible planning JSON must never leak on EOF/STOP.
        if self.suppress_permanently
            || self.suppress_until_object_end
            || (!self.decided && self.pending.trim_start().starts_with(['{', '`']))
        {
            self.pending.clear();
            return None;
        }
        let out = std::mem::take(&mut self.pending);
        (!out.is_empty()).then_some(out)
    }
}

fn strip_leading_planning_leak(trimmed: &str) -> Option<&str> {
    let fenced = trimmed.starts_with("```");
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(str::trim_start)
        .unwrap_or(trimmed);
    let (_, after_obj) = split_leading_json_object(body)?;
    let rest = after_obj.trim_start();
    let rest = if fenced {
        rest.strip_prefix("```")?.trim_start()
    } else {
        rest
    };
    Some(rest)
}

fn split_leading_json_object(text: &str) -> Option<(&str, &str)> {
    let bytes = text.as_bytes();
    if bytes.first().copied() != Some(b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (idx, &byte) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = idx + 1;
                    return Some((&text[..end], &text[end..]));
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use codescribe_core::agent::{ToolExecutionPolicy, ToolOrigin, ToolRegistry, ToolRisk};

    fn timing() -> RuntimeAiRequestTiming {
        codescribe_core::config::Config::runtime_snapshot_from_captured(
            codescribe_core::config::CapturedRuntimeInputs::defaults_at(
                "/fixture/google".into(),
                0,
            ),
        )
        .ai_execution()
        .request_timing()
        .clone()
    }
    fn user(text: &str) -> Message {
        Message::new(Role::User, vec![ContentBlock::Text(text.into())])
    }

    #[test]
    fn google_wire_filter_preserves_lazy_mcp_registry_schemas() {
        let mut registry = ToolRegistry::new();
        registry.register_with_policy(
            ToolDefinition {
                name: "mcp__local__choose".into(), description: "choose a number".into(),
                input_schema: json!({"type":"object","properties":{"n":{"type":"integer","enum":[1,2]}}}),
            },
            Box::new(|_| Box::pin(async { Vec::new() })),
            ToolExecutionPolicy {
                origin: ToolOrigin::Mcp { server: "local".into(), upstream_tool: "choose".into() },
                risk: ToolRisk::ReadOnly, requires_approval: false,
            }, None,
        ).unwrap();
        let (ui_tx, _ui_rx) = mpsc::channel(1);
        let host = AgentProviderHost {
            tools: Arc::new(registry),
            ui_tx,
            approval_handler: None,
            approval_timeout: Duration::from_secs(1),
            session_id: "wire-fixture".into(),
            thread_id: "wire-fixture".into(),
        };
        let tools = host.tools.wire_definitions();
        let (body, skipped) = build_request_with_host(
            &[user("choose")],
            &tools,
            &StreamOptions::default(),
            &[],
            Some(&host),
            &mut GoogleImageSnapshot::default(),
        )
        .expect("valid gateways do not submit the upstream numeric enum to Gemini");
        assert!(skipped.is_empty());
        let names = body["tools"][0]["functionDeclarations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names, ["mcp_search_tools", "call_mcp_tool"]);
        assert_eq!(host.tools.mcp_entries().len(), 1);
        let results = host.tools.search_mcp_tools(&json!({"query":"choose"}));
        assert!(format!("{results:?}").contains("choose"));
    }

    #[test]
    fn google_wire_filter_warns_for_skipped_declarations_and_refuses_an_empty_set() {
        let valid = ToolDefinition {
            name: "valid".into(),
            description: "valid".into(),
            input_schema: json!({"type":"object"}),
        };
        let invalid = ToolDefinition {
            name: "mcp__local__bad".into(),
            description: "invalid".into(),
            input_schema: json!({"type":"object","properties":{"n":{"enum":[1,2]}}}),
        };
        let (body, skipped) = build_request_with_host(
            &[user("hello")],
            &[valid, invalid.clone()],
            &StreamOptions::default(),
            &[],
            None,
            &mut GoogleImageSnapshot::default(),
        )
        .unwrap();
        assert_eq!(
            body["tools"][0]["functionDeclarations"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(body["tools"][0]["functionDeclarations"][0]["name"], "valid");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].registered_name, invalid.name);
        assert!(
            body["systemInstruction"]
                .to_string()
                .contains(&invalid.name)
        );
        assert!(
            build_request_with_host(
                &[user("hello")],
                &[invalid],
                &StreamOptions::default(),
                &[],
                None,
                &mut GoogleImageSnapshot::default(),
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn google_warning_cancellation_releases_turn_with_a_full_ui_queue() {
        let server = mockito::Server::new_async().await;
        let provider = GoogleProvider::from_configuration(
            format!("{}/v1beta", server.url()),
            "gemini-test".into(),
            "fixture-key".into(),
            &timing(),
        )
        .unwrap();
        let (ui_tx, ui_rx) = mpsc::channel(1);
        ui_tx
            .try_send(AgentUiEvent::ToolResult {
                name: "occupied".into(),
                id: "occupied".into(),
                summary: "occupied".into(),
                is_error: false,
            })
            .unwrap();
        provider.bind_host(AgentProviderHost {
            tools: Arc::new(ToolRegistry::new()),
            ui_tx,
            approval_handler: None,
            approval_timeout: Duration::from_secs(5),
            session_id: "cancel-fixture".into(),
            thread_id: "cancel-fixture".into(),
        });
        let tools = [
            ToolDefinition {
                name: "valid".into(),
                description: "valid".into(),
                input_schema: json!({"type":"object"}),
            },
            ToolDefinition {
                name: "bad".into(),
                description: "bad".into(),
                input_schema: json!({"enum":[1,2]}),
            },
        ];
        let rx = provider
            .stream(&[user("hello")], &tools, &StreamOptions::default())
            .await
            .unwrap();
        assert!(provider.turn_lock.try_lock().is_err());
        drop(rx);
        let released = tokio::time::timeout(
            Duration::from_secs(1),
            provider.restore_response_chain(None),
        )
        .await;
        drop(ui_rx);
        if released.is_err() {
            let _ = tokio::time::timeout(
                Duration::from_secs(1),
                provider.restore_response_chain(None),
            )
            .await;
        }
        assert!(
            released.is_ok(),
            "a blocked warning must not retain a cancelled turn"
        );
    }

    #[tokio::test]
    async fn real_session_lazy_mcp_restores_thread_and_rejects_tampering() {
        use codescribe_core::agent::{AgentSession, ToolResultContent};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut server = mockito::Server::new_async().await;
        let path = "/v1beta/models/gemini-test:streamGenerateContent?alt=sse";
        let parts = json!([
            {"thought":true,"text":"planning","thoughtSignature":"thought-distinct"},
            {"text":"","thoughtSignature":"empty-distinct"},
            {"functionCall":{"name":"mcp_search_tools","args":{"query":"read"}},"thoughtSignature":"search-distinct"}
        ]);
        let first = server
            .mock("POST", path)
            .match_request(|request| {
                serde_json::from_slice::<Value>(request.body().unwrap()).unwrap()["contents"]
                    .as_array()
                    .unwrap()
                    .len()
                    == 1
            })
            .with_header("content-type", "text/event-stream")
            .with_body(sse(
                json!({"candidates":[{"content":{"parts":parts},"finishReason":"STOP"}]}),
            ))
            .create_async()
            .await;
        let expected = parts.clone();
        let call_parts = json!([{ "functionCall": {"name":"call_mcp_tool", "id":"wire-read", "args":{"server":"local","tool":"read","arguments":{"path":"fixture.txt"}}}, "thoughtSignature":"read-distinct" }]);
        let second = server
            .mock("POST", path)
            .match_request(move |request| {
                let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                body["contents"].as_array().unwrap().len() == 3
                    && body["contents"][1]["parts"] == expected
                    && body["contents"][2]["parts"][0]["functionResponse"]["name"]
                        == "mcp_search_tools"
            })
            .with_header("content-type", "text/event-stream")
            .with_body(sse(
                json!({"candidates":[{"content":{"parts":call_parts},"finishReason":"STOP"}]}),
            ))
            .create_async()
            .await;
        let expected = parts.clone();
        let expected_call = call_parts.clone();
        let third = server.mock("POST", path)
            .match_request(move |request| {
                let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                body["contents"].as_array().unwrap().len() == 5 && body["contents"][1]["parts"] == expected
                    && body["contents"][3]["parts"] == expected_call
                    && body["contents"][4]["parts"][0]["functionResponse"]["id"] == "wire-read"
            })
            .with_header("content-type", "text/event-stream")
            .with_body(sse(json!({"candidates":[{"content":{"parts":[{"text":"done"}]},"finishReason":"STOP"}]})))
            .create_async().await;
        let count = Arc::new(AtomicUsize::new(0));
        let called = count.clone();
        let mut tools = ToolRegistry::new();
        tools.register_with_policy(ToolDefinition {name:"mcp__local__read".into(),description:"read fixture".into(),input_schema:json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]})}, Box::new(move |_| {
            let called = called.clone();
            Box::pin(async move {called.fetch_add(1, Ordering::SeqCst); vec![ToolResultContent::Text("fixture result".into())]})
        }), ToolExecutionPolicy {origin:ToolOrigin::Mcp {server:"local".into(),upstream_tool:"read".into()},risk:ToolRisk::ReadOnly,requires_approval:false},None).unwrap();
        let endpoint = format!("{}/v1beta", server.url());
        let provider = GoogleProvider::from_configuration(
            endpoint.clone(),
            "gemini-test".into(),
            "fixture-key".into(),
            &timing(),
        )
        .unwrap();
        let (tx, _rx) = mpsc::channel(128);
        let mut session = AgentSession::new(Box::new(provider), Arc::new(tools), tx);
        session
            .send("read".into(), Vec::new(), &StreamOptions::default())
            .await
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        for mock in [first, second, third] {
            mock.assert_async().await;
        }
        use codescribe_core::agent::thread_store::{Thread, ThreadMessage, ThreadStore};
        let root = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(root.path().join("threads")).unwrap();
        let now = chrono::Utc::now();
        let thread = Thread {
            id: ThreadStore::generate_id(),
            created_at: now,
            updated_at: now,
            title: "fixture".into(),
            title_is_custom: false,
            title_is_generated: false,
            mode: "assistive".into(),
            tags: Vec::new(),
            notes: Vec::new(),
            messages: session.messages().iter().map(ThreadMessage::from).collect(),
            summary: None,
            total_tokens: None,
            provider: "google-gemini".into(),
            model: "gemini-test".into(),
        };
        store.save_thread(&thread).unwrap();
        assert!(
            std::fs::read_to_string(store.thread_file_path(&thread.id).unwrap())
                .unwrap()
                .contains("empty-distinct")
        );
        for include_details in [false, true] {
            assert!(
                !codescribe_core::agent::thread_export::thread_to_markdown(
                    &thread,
                    include_details
                )
                .contains("empty-distinct")
            );
        }
        assert!(!format!("{thread:?}").contains("empty-distinct"));
        let restored_messages = store
            .load_thread(&thread.id)
            .unwrap()
            .messages
            .iter()
            .map(ThreadMessage::try_to_message)
            .collect::<Result<Vec<_>>>()
            .unwrap();
        let fourth = server.mock("POST", path)
            .match_request(|request| {
                let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                body["contents"].as_array().unwrap().len() == 7
                    && body["contents"][1]["parts"][1]["thoughtSignature"] == "empty-distinct"
                    && body["contents"][6]["parts"][0]["text"] == "again"
            })
            .with_header("content-type", "text/event-stream")
            .with_body(sse(json!({"candidates":[{"content":{"parts":[{"text":"second answer"}]},"finishReason":"STOP"}]})))
            .create_async().await;
        let restored = GoogleProvider::from_configuration(
            endpoint.clone(),
            "gemini-test".into(),
            "fixture-key".into(),
            &timing(),
        )
        .unwrap();
        let (tx, _rx) = mpsc::channel(128);
        let mut resumed = AgentSession::new(Box::new(restored), Arc::new(ToolRegistry::new()), tx);
        resumed.restore_messages(restored_messages);
        resumed
            .send("again".into(), Vec::new(), &StreamOptions::default())
            .await
            .unwrap();
        fourth.assert_async().await;
        assert!(!format!("{:?}", resumed.messages()).contains("empty-distinct"));
        for mutation in ["version", "context", "call", "missing"] {
            let mut messages = resumed.messages().to_vec();
            let message = messages
                .iter_mut()
                .find(|message| message.role == Role::Assistant)
                .unwrap();
            let mut raw = message.metadata.as_ref().unwrap().as_stored().clone();
            match mutation {
                "version" => raw["provider_wire"]["version"] = json!(2),
                "context" => {
                    raw["provider_wire"]["response"]["context_digest"] = json!("0".repeat(64))
                }
                "call" => {
                    raw["provider_wire"]["response"]["payload"]["parts"][2]["functionCall"]["args"]
                        ["query"] = json!("tampered")
                }
                _ => {}
            }
            message.metadata = (mutation != "missing")
                .then(|| codescribe_core::agent::MessageMetadata::from_stored(raw));
            let provider = GoogleProvider::from_configuration(
                endpoint.clone(),
                "gemini-test".into(),
                "fixture-key".into(),
                &timing(),
            )
            .unwrap();
            assert!(
                provider
                    .stream(&messages, &[], &StreamOptions::default())
                    .await
                    .is_err(),
                "mutation={mutation}"
            );
        }
    }

    /// 2026-11-08 08:16Z: a thread that had run eight turns on OpenAI was
    /// switched to Gemini, and every send was refused before any request
    /// left. Those turns carry no Google wire state. They replay unsigned,
    /// and the first Gemini answer then signs its own occurrence over that
    /// history, so the turn after it still restores.
    #[tokio::test]
    async fn a_thread_begun_on_another_provider_continues_on_gemini() {
        use codescribe_core::agent::AgentSession;
        let mut server = mockito::Server::new_async().await;
        let path = "/v1beta/models/gemini-test:streamGenerateContent?alt=sse";
        let foreign = vec![
            Message::new(Role::User, vec![ContentBlock::Text("read the plan".into())]),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text("Reading it.".into())],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "call_openai_1".into(),
                    name: "read_file".into(),
                    input: json!({"path":"TASK.md"}),
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "call_openai_1".into(),
                    content: vec![ContentBlock::Text("# Plan".into())],
                    is_error: false,
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text("The plan has one heading.".into())],
            ),
        ];
        let history = json!([
            {"role":"user","parts":[{"text":"read the plan"}]},
            {"role":"model","parts":[
                {"text":"Reading it."},
                {"functionCall":{"name":"read_file","args":{"path":"TASK.md"}}}
            ]},
            {"role":"user","parts":[
                {"functionResponse":{"name":"read_file","response":{"result":"# Plan","is_error":false}}}
            ]},
            {"role":"model","parts":[{"text":"The plan has one heading."}]}
        ]);
        let signed = json!([{"text":"Continuing on Gemini.","thoughtSignature":"gemini-first"}]);
        let mut first_contents = history.as_array().unwrap().clone();
        first_contents.push(json!({"role":"user","parts":[{"text":"continue on gemini"}]}));
        let first_contents = Value::Array(first_contents);
        let first = server
            .mock("POST", path)
            .match_request(move |request| {
                serde_json::from_slice::<Value>(request.body().unwrap()).unwrap()["contents"]
                    == first_contents
            })
            .with_header("content-type", "text/event-stream")
            .with_body(sse(
                json!({"candidates":[{"content":{"parts":signed},"finishReason":"STOP"}]}),
            ))
            .create_async()
            .await;
        let mut second_contents = history.as_array().unwrap().clone();
        second_contents.push(json!({"role":"user","parts":[{"text":"continue on gemini"}]}));
        second_contents.push(json!({"role":"model","parts":signed}));
        second_contents.push(json!({"role":"user","parts":[{"text":"again"}]}));
        let second_contents = Value::Array(second_contents);
        let second = server
            .mock("POST", path)
            .match_request(move |request| {
                serde_json::from_slice::<Value>(request.body().unwrap()).unwrap()["contents"]
                    == second_contents
            })
            .with_header("content-type", "text/event-stream")
            .with_body(sse(
                json!({"candidates":[{"content":{"parts":[{"text":"done"}]},"finishReason":"STOP"}]}),
            ))
            .create_async()
            .await;
        let provider = GoogleProvider::from_configuration(
            format!("{}/v1beta", server.url()),
            "gemini-test".into(),
            "fixture-key".into(),
            &timing(),
        )
        .unwrap();
        let (tx, _rx) = mpsc::channel(128);
        let mut session = AgentSession::new(Box::new(provider), Arc::new(ToolRegistry::new()), tx);
        session.restore_messages(foreign);
        session
            .send(
                "continue on gemini".into(),
                Vec::new(),
                &StreamOptions::default(),
            )
            .await
            .unwrap();
        session
            .send("again".into(), Vec::new(), &StreamOptions::default())
            .await
            .unwrap();
        first.assert_async().await;
        second.assert_async().await;
    }

    /// A Gemini occurrence signed for another model replays as its visible
    /// content, without the other model's signatures.
    #[tokio::test]
    async fn an_occurrence_signed_for_another_gemini_model_replays_unsigned() {
        use codescribe_core::agent::AgentSession;
        let mut server = mockito::Server::new_async().await;
        let old = server
            .mock(
                "POST",
                "/v1beta/models/gemini-old:streamGenerateContent?alt=sse",
            )
            .with_header("content-type", "text/event-stream")
            .with_body(sse(json!({"candidates":[{"content":{"parts":[
                {"thought":true,"text":"planning","thoughtSignature":"old-thought"},
                {"text":"Old answer.","thoughtSignature":"old-text"}
            ]},"finishReason":"STOP"}]})))
            .create_async()
            .await;
        let expected = json!([
            {"role":"user","parts":[{"text":"hello"}]},
            {"role":"model","parts":[{"text":"Old answer."}]},
            {"role":"user","parts":[{"text":"next"}]}
        ]);
        let new = server
            .mock("POST", "/v1beta/models/gemini-test:streamGenerateContent?alt=sse")
            .match_request(move |request| {
                serde_json::from_slice::<Value>(request.body().unwrap()).unwrap()["contents"]
                    == expected
            })
            .with_header("content-type", "text/event-stream")
            .with_body(sse(
                json!({"candidates":[{"content":{"parts":[{"text":"New answer."}]},"finishReason":"STOP"}]}),
            ))
            .create_async()
            .await;
        let endpoint = format!("{}/v1beta", server.url());
        let provider = |model: &str| {
            GoogleProvider::from_configuration(
                endpoint.clone(),
                model.into(),
                "fixture-key".into(),
                &timing(),
            )
            .unwrap()
        };
        let (tx, _rx) = mpsc::channel(128);
        let mut earlier = AgentSession::new(
            Box::new(provider("gemini-old")),
            Arc::new(ToolRegistry::new()),
            tx,
        );
        earlier
            .send("hello".into(), Vec::new(), &StreamOptions::default())
            .await
            .unwrap();
        let (tx, _rx) = mpsc::channel(128);
        let mut later = AgentSession::new(
            Box::new(provider("gemini-test")),
            Arc::new(ToolRegistry::new()),
            tx,
        );
        later.restore_messages(earlier.messages().to_vec());
        later
            .send("next".into(), Vec::new(), &StreamOptions::default())
            .await
            .unwrap();
        old.assert_async().await;
        new.assert_async().await;
    }

    #[tokio::test]
    async fn real_session_never_executes_google_tool_from_dirty_terminal() {
        use codescribe_core::agent::{AgentSession, ToolResultContent};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut server = mockito::Server::new_async().await;
        let mock = server.mock("POST", "/v1beta/models/gemini-test:streamGenerateContent?alt=sse")
            .with_header("content-type", "text/event-stream")
            .with_body(sse(json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":"fixture_mutation","args":{}}}]},"finishReason":"MAX_TOKENS"}]})))
            .create_async().await;
        let count = Arc::new(AtomicUsize::new(0));
        let called = count.clone();
        let mut tools = ToolRegistry::new();
        tools
            .register_with_policy(
                ToolDefinition {
                    name: "fixture_mutation".into(),
                    description: "fixture".into(),
                    input_schema: json!({"type":"object"}),
                },
                Box::new(move |_| {
                    let called = called.clone();
                    Box::pin(async move {
                        called.fetch_add(1, Ordering::SeqCst);
                        Vec::<ToolResultContent>::new()
                    })
                }),
                ToolExecutionPolicy::native(ToolRisk::ReadOnly),
                None,
            )
            .unwrap();
        let provider = GoogleProvider::from_configuration(
            format!("{}/v1beta", server.url()),
            "gemini-test".into(),
            "fixture-key".into(),
            &timing(),
        )
        .unwrap();
        let (tx, _rx) = mpsc::channel(32);
        let mut session = AgentSession::new(Box::new(provider), Arc::new(tools), tx);
        assert!(
            session
                .send("run".into(), Vec::new(), &StreamOptions::default())
                .await
                .is_err()
        );
        assert_eq!(count.load(Ordering::SeqCst), 0);
        mock.assert_async().await;
    }
    fn sse(body: Value) -> String {
        format!("data: {body}\n\n")
    }
    async fn events(
        provider: &GoogleProvider,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Vec<AgentEvent> {
        let mut rx = provider
            .stream(messages, tools, &StreamOptions::default())
            .await
            .unwrap();
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    /// F1 uses the actual HTTP/SSE provider seam and a session-shaped second
    /// step, so signatures cannot pass by testing only a serialization helper.
    #[tokio::test]
    async fn google_provider_signed_tool_roundtrip_cca_and_parallel_responses() {
        use codescribe_core::agent::{AgentSession, AgentUiEvent, ToolResultContent};
        let mut server = mockito::Server::new_async().await;
        let path = "/v1internal:streamGenerateContent?alt=sse";
        let parts = json!([
            {"thought":true,"text":"Sprawdzę","thoughtSignature":"sig_thought"},
            {"text":"Zobaczę."},
            {"functionCall":{"name":"mcp_search_tools","args":{"query":"repo"}},"thoughtSignature":"sig_abc123"},
            {"functionCall":{"name":"call_mcp_tool","id":"wire-call","args":{"server":"local","tool":"read","arguments":{"path":"fixture.txt"}}},"thoughtSignature":"sig_parallel"}
        ]);
        let first = server.mock("POST",path).match_header("x-goog-api-key","fixture-key")
            .match_request(|request| {
                let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                body["request"]["contents"].as_array().unwrap().len() == 1 && body["project"] == "p"
            })
            .with_header("content-type","text/event-stream")
            .with_body(sse(json!({"response":{"candidates":[{"content":{"parts":parts},"finishReason":"STOP"}]}})))
            .create_async().await;
        let expected = parts.clone();
        let second = server.mock("POST",path).match_request(move |request| {
            let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
            let contents = &body["request"]["contents"];
            contents.as_array().unwrap().len() == 3 && contents[1]["parts"] == expected
                && contents[2]["parts"].as_array().unwrap().len() == 2
                && contents[2]["parts"][0]["functionResponse"]["name"] == "mcp_search_tools"
                && contents[2]["parts"][1]["functionResponse"]["id"] == "wire-call"
        }).with_header("content-type","text/event-stream")
            .with_body(sse(json!({"response":{"candidates":[{"content":{"parts":[{"text":"Gotowe 🐾"}]},"finishReason":"STOP"}]}})))
            .create_async().await;
        let provider = GoogleProvider::from_configuration(
            format!("{}/v1internal?project=p", server.url()),
            "gemini-test".into(),
            "fixture-key".into(),
            &timing(),
        )
        .unwrap();
        let retained_handle = provider.clone();
        let mut tools = ToolRegistry::new();
        tools.register_with_policy(ToolDefinition {name:"mcp__local__read".into(),description:"read fixture repository".into(),input_schema:json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]})}, Box::new(|_| Box::pin(async {vec![ToolResultContent::Text("found".into())]})), ToolExecutionPolicy {origin:ToolOrigin::Mcp {server:"local".into(),upstream_tool:"read".into()},risk:ToolRisk::ReadOnly,requires_approval:false},None).unwrap();
        let (tx, mut rx) = mpsc::channel(128);
        let mut session = AgentSession::new(Box::new(provider), Arc::new(tools), tx);
        session
            .send("Szukaj".into(), Vec::new(), &StreamOptions::default())
            .await
            .unwrap();
        let mut reasoning = false;
        let mut done = false;
        while let Ok(event) = rx.try_recv() {
            reasoning |= matches!(&event,AgentUiEvent::ReasoningDelta(text) if text=="Sprawdzę");
            done |= matches!(&event,AgentUiEvent::TextDone(text) if text=="Gotowe 🐾");
            assert!(!format!("{event:?}").contains("sig_parallel"));
        }
        assert!(reasoning && done);
        first.assert_async().await;
        second.assert_async().await;
        let mut rolled_back = session.messages().to_vec();
        rolled_back[0] = user("Different thread");
        assert!(
            retained_handle
                .stream(&rolled_back, &[], &StreamOptions::default())
                .await
                .is_err(),
            "No cross-thread signature reuse"
        );
    }

    /// F2 exercises the real MCP projection, rather than hand-copying the two
    /// gateway schemas and accidentally missing changes to wire_definitions.
    #[test]
    fn google_provider_normalizes_real_mcp_gateway_schemas() {
        let mut registry = ToolRegistry::new();
        registry.register_with_policy(ToolDefinition{name:"mcp__local__read".into(),description:"read".into(),input_schema:json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]})},Box::new(|_| Box::pin(async {Vec::new()})),ToolExecutionPolicy{origin:ToolOrigin::Mcp{server:"local".into(),upstream_tool:"read".into()},risk:ToolRisk::ReadOnly,requires_approval:false},None).unwrap();
        let tools = registry.wire_definitions();
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["mcp_search_tools", "call_mcp_tool"]
        );
        let body = build_request(&[user("hello")], &tools, &StreamOptions::default(), &[]).unwrap();
        for (tool, decl) in tools
            .iter()
            .zip(body["tools"][0]["functionDeclarations"].as_array().unwrap())
        {
            assert_eq!(decl["name"], tool.name);
            assert_eq!(
                decl["parameters"].get("required"),
                tool.input_schema.get("required")
            );
            validate_schema(&decl["parameters"]);
        }
    }
    fn validate_schema(schema: &Value) {
        assert!(schema.is_object());
        assert!(schema.get("$schema").is_none());
        assert!(schema.get("additionalProperties").is_none());
        assert!(!schema.get("type").is_some_and(Value::is_array));
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for value in properties.values() {
                validate_schema(value);
            }
        }
        if let Some(items) = schema.get("items") {
            validate_schema(items);
        }
    }

    #[test]
    fn google_provider_sse_split_utf8_multiline_and_truncated_stream() {
        let wire = ": ping\r\ndata: {\"candidates\":\r\ndata: [{\"content\":{\"parts\":[{\"text\":\"Zażółć 🐾\"}]},\"finishReason\":\"STOP\"}]}\r\n\r\n";
        for split in 0..wire.len() {
            let mut parser = GoogleSse::default();
            let mut result = parser.push(&wire.as_bytes()[..split]).unwrap();
            result.extend(parser.push(&wire.as_bytes()[split..]).unwrap());
            result.extend(parser.finish().unwrap());
            assert_eq!(result.len(), 1);
            assert_eq!(
                result[0]["candidates"][0]["content"]["parts"][0]["text"],
                "Zażółć 🐾"
            );
        }
        let mut parser = GoogleSse::default();
        parser.push(b"data: {\"x\":\"").unwrap();
        assert!(parser.finish().is_err());
    }

    /// F3 checks both rejected status codes at the real transport boundary.
    #[tokio::test]
    async fn google_provider_rejects_dirty_terminals_and_redacts_http_errors() {
        for status in [401, 403] {
            let mut server = mockito::Server::new_async().await;
            let mock = server
                .mock(
                    "POST",
                    "/v1beta/models/gemini-test:streamGenerateContent?alt=sse",
                )
                .with_status(status)
                .with_body(concat!(
                    "AI",
                    "zaDoNotLeak ya",
                    "29.NeverReveal arbitrary-fixture-secret"
                ))
                .create_async()
                .await;
            let provider = GoogleProvider::from_configuration(
                format!("{}/v1beta", server.url()),
                "gemini-test".into(),
                "arbitrary-fixture-secret".into(),
                &timing(),
            )
            .unwrap();
            let result = events(&provider, &[user("test")], &[]).await;
            let rendered = format!("{result:?}");
            assert!(!rendered.contains("AIza"));
            assert!(!rendered.contains("ya29."));
            assert!(!rendered.contains("arbitrary-fixture-secret"));
            assert!(
                result
                    .iter()
                    .any(|event| matches!(event, AgentEvent::ResponseDone { clean: false, .. }))
            );
            mock.assert_async().await;
        }
        let (tx, mut rx) = mpsc::channel(20);
        let mut state = ResponseState::default();
        state
            .consume(
                json!({"candidates":[{"content":{"parts":[{"text":"{\"th"}]}}]}),
                &tx,
            )
            .await
            .unwrap();
        state.consume(json!({"candidates":[{"content":{"parts":[{"text":"ought\": \"planuję...\"}"}]},"finishReason":"STOP"}]}),&tx).await.unwrap();
        drop(tx);
        while let Some(event) = rx.recv().await {
            assert!(!matches!(event, AgentEvent::TextDelta(_)));
        }
        assert!(
            state
                .consume(
                    json!({"candidates":[{"finishReason":"SAFETY"}]}),
                    &mpsc::channel(1).0
                )
                .await
                .is_err()
        );

        let mut filter = PlanningFilter::default();
        assert_eq!(filter.push("{\"thought\": \"plan"), None);
        assert_eq!(
            filter.push("ning step\"} Real visible answer"),
            Some("Real visible answer".into())
        );
        assert_eq!(filter.push(" continues."), Some(" continues.".into()));
        assert_eq!(filter.finish(), None);
    }

    async fn m3a_ticks<F: std::future::Future>(future: F) -> Option<F::Output> {
        use std::task::Poll;
        tokio::pin!(future);
        for _ in 0..10_000 {
            let polled = std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await;
            if let Poll::Ready(value) = polled {
                return Some(value);
            }
            tokio::task::yield_now().await;
        }
        None
    }

    fn m3a_schema_tools() -> [ToolDefinition; 2] {
        [
            ToolDefinition {
                name: "choose_colour".into(),
                description: "a formal string choice".into(),
                input_schema: json!({"type":"object","properties":{"colour":{"type":"string","enum":["red","blue"]}},"required":["colour"]}),
            },
            ToolDefinition {
                name: "mcp__wire__numeric".into(),
                description: "cannot be a Gemini declaration".into(),
                input_schema: json!({"type":"object","properties":{"n":{"type":"integer","enum":[7,9]}}}),
            },
        ]
    }

    /// Fix 71ad0e25a / e3acfb6de; kills M3a-15, M3a-17, M3a-18, M3a-19, M3a-20, M3a-23, M3a-24.
    /// Inspect the actual submitted JSON, real lazy registry and UI warning.
    #[tokio::test(start_paused = true)]
    async fn m3a_google_wire_keeps_valid_enum_and_gateway_but_skips_bad_declaration() {
        let mut server = mockito::Server::new_async().await;
        let submitted = Arc::new(Mutex::new(None));
        let captured = Arc::clone(&submitted);
        let endpoint = server.mock("POST", "/v1beta/models/gemini-test:streamGenerateContent?alt=sse")
            .match_request(move |request| {
                *captured.lock().unwrap() = Some(serde_json::from_slice::<Value>(request.body().unwrap()).unwrap());
                true
            })
            .with_header("content-type", "text/event-stream")
            .with_body(sse(json!({"candidates":[{"content":{"parts":[{"text":"wire-exam"}]},"finishReason":"STOP"}]})))
            .expect(1).create_async().await;
        let mut registry = ToolRegistry::new();
        registry.register_with_policy(
            ToolDefinition {
                name: "mcp__vet__numeric".into(), description: "upstream numeric choice".into(),
                input_schema: json!({"type":"object","properties":{"n":{"type":"integer","enum":[7,9]}}}),
            },
            Box::new(|_| Box::pin(async { Vec::new() })),
            ToolExecutionPolicy {
                origin: ToolOrigin::Mcp { server: "vet".into(), upstream_tool: "numeric".into() },
                risk: ToolRisk::ReadOnly, requires_approval: false,
            }, None,
        ).unwrap();
        let (ui_tx, mut ui_rx) = mpsc::channel(8);
        let host = AgentProviderHost {
            tools: Arc::new(registry),
            ui_tx,
            approval_handler: None,
            approval_timeout: Duration::from_secs(1),
            session_id: "m3a-wire".into(),
            thread_id: "m3a-wire".into(),
        };
        let mut definitions = host.tools.wire_definitions();
        definitions.extend(m3a_schema_tools());
        let mut provider = GoogleProvider::from_configuration(
            format!("{}/v1beta", server.url()),
            "gemini-test".into(),
            "exam-key".into(),
            &timing(),
        )
        .unwrap();
        provider.client = reqwest::Client::builder().no_proxy().build().unwrap();
        provider.bind_host(host.clone());
        let received = m3a_ticks(events(&provider, &[user("wire exam")], &definitions))
            .await
            .expect("loopback settled without time");
        assert!(
            received
                .iter()
                .any(|e| matches!(e, AgentEvent::ResponseDone { clean: true, .. })),
            "{received:?}"
        );
        assert!(
            !received.iter().any(|e| matches!(e, AgentEvent::Error(_))),
            "{received:?}"
        );
        endpoint.assert_async().await;
        let body = submitted
            .lock()
            .unwrap()
            .take()
            .expect("actual HTTP payload captured");
        let declarations = body["tools"][0]["functionDeclarations"].as_array().unwrap();
        let names = declarations
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            ["mcp_search_tools", "call_mcp_tool", "choose_colour"]
        );
        assert_eq!(
            declarations[2]["parameters"]["properties"]["colour"]["enum"],
            json!(["red", "blue"])
        );
        assert_eq!(declarations[2]["parameters"]["required"], json!(["colour"]));
        let catalogue = declarations[0]["description"].as_str().unwrap();
        assert!(catalogue.contains("vet"), "{catalogue}");
        assert!(
            catalogue.contains("mcp__wire__numeric"),
            "unavailable wire declaration must be named: {catalogue}"
        );
        let warning = body["systemInstruction"].to_string();
        for literal in ["mcp__wire__numeric", "wire", "numeric", "enum"] {
            assert!(warning.contains(literal), "{warning}");
        }
        // Upstream schemas are gateway data, not Gemini declarations. They
        // must remain searchable under the canonical registry (e3acfb6de).
        let entries = host.tools.mcp_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].registered_name, "mcp__vet__numeric");
        assert_eq!(
            entries[0].input_schema["properties"]["n"]["enum"],
            json!([7, 9])
        );
        assert!(
            format!(
                "{:?}",
                host.tools.search_mcp_tools(&json!({"query":"numeric"}))
            )
            .contains("mcp__vet__numeric")
        );
        match ui_rx.try_recv().expect("one visible skip warning") {
            AgentUiEvent::ToolResult {
                name,
                id,
                summary,
                is_error,
            } => {
                assert_eq!(name, "mcp__wire__numeric");
                assert_eq!(id, "google_schema_skip_mcp__wire__numeric");
                assert!(is_error);
                assert!(summary.contains("mcp__wire__numeric") && summary.contains("enum"));
            }
            event => panic!("skip warning used a different UI event: {event:?}"),
        }
        assert!(matches!(
            ui_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    /// Fix f9ab3524e; kills M3a-16, including the ghost-warning race after cancel.
    #[tokio::test(start_paused = true)]
    async fn m3a_google_cancel_emits_no_warning_when_ui_capacity_returns() {
        let provider = GoogleProvider::from_configuration(
            "http://127.0.0.1:9/v1beta".into(),
            "gemini-test".into(),
            "exam-key".into(),
            &timing(),
        )
        .unwrap();
        let (ui_tx, mut ui_rx) = mpsc::channel(1);
        ui_tx
            .try_send(AgentUiEvent::ToolResult {
                name: "occupied".into(),
                id: "occupied".into(),
                summary: "occupied".into(),
                is_error: false,
            })
            .unwrap();
        provider.bind_host(AgentProviderHost {
            tools: Arc::new(ToolRegistry::new()),
            ui_tx,
            approval_handler: None,
            approval_timeout: Duration::from_secs(1),
            session_id: "m3a-cancel".into(),
            thread_id: "m3a-cancel".into(),
        });
        let rx = provider
            .stream(
                &[user("cancel exam")],
                &m3a_schema_tools(),
                &StreamOptions::default(),
            )
            .await
            .unwrap();
        tokio::task::yield_now().await;
        assert!(provider.turn_lock.try_lock().is_err());
        assert_eq!(ui_rx.len(), 1);
        let before = tokio::time::Instant::now();
        drop(rx);
        // Cancellation and send capacity are now ready together. The biased
        // cancellation branch must win, not enqueue a result for a dead turn.
        assert!(
            matches!(ui_rx.try_recv().unwrap(), AgentUiEvent::ToolResult { name, .. } if name == "occupied")
        );
        let guard = m3a_ticks(provider.turn_lock.lock())
            .await
            .expect("cancelled warning retained turn_guard");
        assert!(
            matches!(ui_rx.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
            "cancelled turn emitted a skip warning"
        );
        assert_eq!(tokio::time::Instant::now(), before);
        drop(guard);
    }

    /// Fix 71ad0e25a; kills M3a-21 and M3a-22 with literal empty/all-invalid lists.
    #[test]
    fn m3a_google_empty_input_is_legal_but_nonempty_all_invalid_refuses() {
        let (body, skipped) = build_request_with_host(
            &[user("no tools")],
            &[],
            &StreamOptions::default(),
            &[],
            None,
            &mut GoogleImageSnapshot::default(),
        )
        .unwrap();
        assert!(skipped.is_empty());
        assert!(body.get("tools").is_none());
        let invalid = m3a_schema_tools().into_iter().nth(1).unwrap();
        let error = build_request_with_host(
            &[user("bad tools")],
            &[invalid],
            &StreamOptions::default(),
            &[],
            None,
            &mut GoogleImageSnapshot::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("rejected all 1 requested tool(s)"),
            "{error}"
        );
        assert!(error.contains("mcp__wire__numeric"), "{error}");
    }
    fn screenshot_history_roundtrip(messages: &[Message]) -> Vec<Message> {
        use codescribe_core::agent::thread_store::{Thread, ThreadMessage, ThreadStore};
        let root = tempfile::tempdir().unwrap();
        let store = ThreadStore::new_in(root.path().join("threads")).unwrap();
        let now = chrono::Utc::now();
        let thread = Thread {
            id: ThreadStore::generate_id(),
            created_at: now,
            updated_at: now,
            title: "screenshot continuation".into(),
            title_is_custom: false,
            title_is_generated: false,
            mode: "assistive".into(),
            tags: vec![],
            notes: vec![],
            messages: messages.iter().map(ThreadMessage::from).collect(),
            summary: None,
            total_tokens: None,
            provider: "google-gemini".into(),
            model: "gemini-test".into(),
        };
        store.save_thread(&thread).unwrap();
        store
            .load_thread(&thread.id)
            .unwrap()
            .messages
            .iter()
            .map(ThreadMessage::try_to_message)
            .collect::<Result<Vec<_>>>()
            .unwrap()
    }

    /// Reproduce exactly the historical serialization at the moment the last
    /// response was created, before ThreadStore spills this turn's screenshot.
    fn mark_last_response_as_v1(messages: &mut [Message]) {
        let last = messages
            .iter()
            .rposition(|m| m.role == Role::Assistant)
            .unwrap();
        let mut digest = Sha256::new();
        for message in &messages[..last] {
            hash_message(&mut digest, message).unwrap();
        }
        let message = &mut messages[last];
        let mut metadata = message.metadata.as_ref().unwrap().as_stored().clone();
        metadata["provider_wire"]["response"]["context_digest"] =
            json!(hex::encode(digest.finalize()));
        metadata["provider_wire"]["response"]["payload"]
            .as_object_mut()
            .unwrap()
            .remove("context_digest_version");
        message.metadata = Some(codescribe_core::agent::MessageMetadata::from_stored(
            metadata,
        ));
    }

    #[tokio::test]
    async fn google_screenshot_continuation_survives_real_storage_and_fresh_providers_v2_and_v1() {
        use codescribe_core::agent::{AgentSession, ImageAttachment};
        for legacy in [false, true] {
            let mut server = mockito::Server::new_async().await;
            let path = "/v1beta/models/gemini-test:streamGenerateContent?alt=sse";
            let endpoint = format!("{}/v1beta", server.url());
            let provider = || {
                GoogleProvider::from_configuration(
                    endpoint.clone(),
                    "gemini-test".into(),
                    "fixture-key".into(),
                    &timing(),
                )
                .unwrap()
            };
            let first = server.mock("POST", path)
                .match_request(|request| {
                    let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                    body["contents"][0]["parts"].as_array().unwrap().iter()
                        .any(|p| p.get("inlineData").is_some())
                })
                .with_header("content-type", "text/event-stream")
                .with_body(sse(json!({"candidates":[{"content":{"parts":[{"text":"seen one","thoughtSignature":"signature-one"}]},"finishReason":"STOP"}]})))
                .create_async().await;
            let image = format!(
                "screenshot-{}-{legacy}",
                chrono::Utc::now().timestamp_nanos_opt().unwrap()
            )
            .into_bytes();
            let (tx, _rx) = mpsc::channel(128);
            let mut session =
                AgentSession::new(Box::new(provider()), Arc::new(ToolRegistry::new()), tx);
            session
                .send(
                    "inspect screenshot".into(),
                    vec![ImageAttachment {
                        data: image,
                        media_type: " image/png ".into(),
                    }],
                    &StreamOptions::default(),
                )
                .await
                .unwrap();
            first.assert_async().await;
            let mut messages = session.messages().to_vec();
            // Historical composer image loading used canonical MIME types.
            if legacy {
                if let ContentBlock::Image { media_type, .. } = &mut messages[0].content[1] {
                    *media_type = "image/png".into();
                }
                mark_last_response_as_v1(&mut messages);
            }
            let restored = screenshot_history_roundtrip(&messages);
            assert!(
                restored[0]
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ImageAsset(_)))
            );
            let second = server.mock("POST", path)
                .match_request(|request| {
                    let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                    body["contents"][1]["parts"][0]["thoughtSignature"] == "signature-one"
                        && body["contents"].as_array().unwrap().len() == 3
                })
                .with_header("content-type", "text/event-stream")
                .with_body(sse(json!({"candidates":[{"content":{"parts":[{"text":"seen two","thoughtSignature":"signature-two"}]},"finishReason":"STOP"}]})))
                .create_async().await;
            let (tx, _rx) = mpsc::channel(128);
            let mut resumed =
                AgentSession::new(Box::new(provider()), Arc::new(ToolRegistry::new()), tx);
            resumed.restore_messages(restored);
            resumed
                .send(
                    "another screenshot".into(),
                    vec![ImageAttachment {
                        data: format!("another-screenshot-{legacy}").into_bytes(),
                        media_type: "image/png".into(),
                    }],
                    &StreamOptions::default(),
                )
                .await
                .unwrap();
            second.assert_async().await;
            let mut messages = resumed.messages().to_vec();
            if legacy {
                mark_last_response_as_v1(&mut messages);
            }
            let restored = screenshot_history_roundtrip(&messages);
            let third = server.mock("POST", path)
                .match_request(|request| {
                    let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                    body["contents"][1]["parts"][0]["thoughtSignature"] == "signature-one"
                        && body["contents"][3]["parts"][0]["thoughtSignature"] == "signature-two"
                        && body["contents"].as_array().unwrap().len() == 5
                })
                .with_header("content-type", "text/event-stream")
                .with_body(sse(json!({"candidates":[{"content":{"parts":[{"text":"continued"}]},"finishReason":"STOP"}]})))
                .create_async().await;
            let (tx, _rx) = mpsc::channel(128);
            let mut again =
                AgentSession::new(Box::new(provider()), Arc::new(ToolRegistry::new()), tx);
            again.restore_messages(restored.clone());
            again
                .send("continue".into(), vec![], &StreamOptions::default())
                .await
                .unwrap();
            assert!(
                again
                    .messages()
                    .last()
                    .unwrap()
                    .content
                    .contains(&ContentBlock::Text("continued".into()))
            );
            third.assert_async().await;
            // Mutation remains refused before any further network request.
            let mut changed = restored.clone();
            changed[0].content[0] = ContentBlock::Text("different instruction".into());
            assert!(
                provider()
                    .stream(&changed, &[], &StreamOptions::default())
                    .await
                    .is_err()
            );
            let mut changed = restored.clone();
            let asset = changed[0]
                .content
                .iter_mut()
                .find_map(|b| match b {
                    ContentBlock::ImageAsset(asset) => Some(asset),
                    _ => None,
                })
                .unwrap();
            asset.size_bytes += 1;
            assert!(
                provider()
                    .stream(&changed, &[], &StreamOptions::default())
                    .await
                    .is_err()
            );
            let asset = restored[0]
                .content
                .iter()
                .find_map(|b| match b {
                    ContentBlock::ImageAsset(asset) => Some(asset),
                    _ => None,
                })
                .unwrap();
            let mut changed_bytes = std::fs::read(&asset.path).unwrap();
            changed_bytes[0] ^= 1;
            std::fs::write(&asset.path, changed_bytes).unwrap();
            assert!(
                provider()
                    .stream(&restored, &[], &StreamOptions::default())
                    .await
                    .is_err()
            );
            std::fs::remove_file(&asset.path).unwrap();
            assert!(
                provider()
                    .stream(&restored, &[], &StreamOptions::default())
                    .await
                    .is_err()
            );
        }
    }
}
