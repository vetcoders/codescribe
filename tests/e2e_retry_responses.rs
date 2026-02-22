use std::collections::HashSet;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::json;
use serial_test::serial;

const DEFAULT_PORTS: [u16; 5] = [8089, 8100, 8101, 10240, 11434];
const RESPONSES_PATH: &str = "/v1/responses";
const MODELS_PATH: &str = "/v1/models";

#[derive(Debug, Clone)]
struct LiveTarget {
    port: u16,
    endpoint: String,
    model: String,
    api_key: String,
}

#[derive(Debug, Deserialize)]
struct ModelList {
    #[serde(default)]
    data: Vec<ModelItem>,
    #[serde(default)]
    models: Vec<ModelItem>,
}

#[derive(Debug, Deserialize)]
struct ModelItem {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    model: String,
}

impl ModelItem {
    fn resolved_id(&self) -> Option<&str> {
        if !self.id.trim().is_empty() {
            Some(self.id.trim())
        } else if !self.name.trim().is_empty() {
            Some(self.name.trim())
        } else if !self.model.trim().is_empty() {
            Some(self.model.trim())
        } else {
            None
        }
    }
}

#[derive(Debug, Deserialize)]
struct StreamChunk {
    #[serde(rename = "type")]
    chunk_type: String,
    #[serde(default)]
    delta: Option<String>,
    #[serde(default)]
    sequence_number: Option<u64>,
    #[serde(default)]
    response: Option<StreamChunkResponse>,
}

#[derive(Debug, Deserialize)]
struct StreamChunkResponse {
    #[serde(default)]
    id: String,
}

#[derive(Debug, Deserialize)]
struct NonStreamResponse {
    #[serde(default)]
    id: String,
    #[serde(default)]
    output: Vec<OutputItem>,
}

#[derive(Debug, Deserialize)]
struct OutputItem {
    #[serde(rename = "type")]
    item_type: String,
    #[serde(default)]
    content: Option<Vec<OutputPart>>,
}

#[derive(Debug, Deserialize)]
struct OutputPart {
    #[serde(rename = "type")]
    part_type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    summary: Option<String>,
}

fn env_bool(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| {
            let n = v.trim().to_ascii_lowercase();
            matches!(n.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

fn env_non_empty(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        std::env::var(key).ok().and_then(|v| {
            let trimmed = v.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
    })
}

fn load_codescribe_env() {
    let Ok(home) = std::env::var("HOME") else {
        return;
    };
    let env_path = std::path::PathBuf::from(home).join(".codescribe/.env");
    if !env_path.exists() {
        return;
    }

    if let Ok(content) = std::fs::read_to_string(&env_path) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=')
                && std::env::var(key.trim()).is_err()
            {
                // Test code mutates process-wide env explicitly.
                unsafe { std::env::set_var(key.trim(), value.trim()) };
            }
        }
    }
}

fn configured_ports() -> Vec<u16> {
    if let Some(raw) = env_non_empty(&["CODESCRIBE_E2E_RESPONSES_PORTS"]) {
        let mut ports: Vec<u16> = raw
            .split(',')
            .filter_map(|p| p.trim().parse::<u16>().ok())
            .collect();
        ports.sort_unstable();
        ports.dedup();
        if !ports.is_empty() {
            return ports;
        }
    }
    DEFAULT_PORTS.to_vec()
}

fn configured_chain_ports() -> Vec<u16> {
    if let Some(raw) = env_non_empty(&["CODESCRIBE_E2E_RESPONSES_CHAIN_PORTS"]) {
        let mut ports: Vec<u16> = raw
            .split(',')
            .filter_map(|p| p.trim().parse::<u16>().ok())
            .collect();
        ports.sort_unstable();
        ports.dedup();
        if !ports.is_empty() {
            return ports;
        }
    }
    vec![8089]
}

fn configured_resume_ports() -> Vec<u16> {
    if let Some(raw) = env_non_empty(&["CODESCRIBE_E2E_RESPONSES_RESUME_PORTS"]) {
        let mut ports: Vec<u16> = raw
            .split(',')
            .filter_map(|p| p.trim().parse::<u16>().ok())
            .collect();
        ports.sort_unstable();
        ports.dedup();
        if !ports.is_empty() {
            return ports;
        }
    }
    vec![8089]
}

fn configured_required_ports() -> Vec<u16> {
    if let Some(raw) = env_non_empty(&["CODESCRIBE_E2E_RESPONSES_REQUIRED_PORTS"]) {
        let mut ports: Vec<u16> = raw
            .split(',')
            .filter_map(|p| p.trim().parse::<u16>().ok())
            .collect();
        ports.sort_unstable();
        ports.dedup();
        if !ports.is_empty() {
            return ports;
        }
    }
    vec![8089]
}

fn e2e_responses_enabled() -> bool {
    env_bool("CODESCRIBE_E2E_RESPONSES") || env_bool("CODESCRIBE_E2E_LIVE_RESPONSES")
}

async fn discover_live_targets(client: &Client) -> Vec<LiveTarget> {
    let preferred_model = env_non_empty(&[
        "CODESCRIBE_E2E_RESPONSES_MODEL",
        "LLM_FORMATTING_MODEL",
        "LLM_MODEL",
    ]);
    let api_key = env_non_empty(&[
        "CODESCRIBE_E2E_RESPONSES_API_KEY",
        "LLM_FORMATTING_API_KEY",
        "LLM_ASSISTIVE_API_KEY",
        "LLM_API_KEY",
    ])
    .unwrap_or_else(|| "local-test-key".to_string());

    let mut targets = Vec::new();
    for port in configured_ports() {
        let models_url = format!("http://localhost:{port}{MODELS_PATH}");
        let response = match client
            .get(&models_url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("x-api-key", &api_key)
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(err) => {
                eprintln!("Port {port}: models probe failed ({err})");
                continue;
            }
        };

        if !response.status().is_success() {
            eprintln!(
                "Port {port}: models probe returned HTTP {}",
                response.status()
            );
            continue;
        }

        let payload: ModelList = match response.json().await {
            Ok(models) => models,
            Err(err) => {
                eprintln!("Port {port}: cannot parse /v1/models payload ({err})");
                continue;
            }
        };

        let mut model_ids: Vec<String> = payload
            .data
            .iter()
            .chain(payload.models.iter())
            .filter_map(ModelItem::resolved_id)
            .map(ToString::to_string)
            .collect();
        model_ids.sort();
        model_ids.dedup();

        if model_ids.is_empty() {
            eprintln!("Port {port}: /v1/models returned no models");
            continue;
        }

        let selected_model = select_model(&model_ids, preferred_model.as_deref());

        eprintln!(
            "Port {port}: {} models detected, selected '{}'",
            model_ids.len(),
            selected_model
        );

        targets.push(LiveTarget {
            port,
            endpoint: format!("http://localhost:{port}{RESPONSES_PATH}"),
            model: selected_model,
            api_key: api_key.clone(),
        });
    }

    targets
}

fn select_model(model_ids: &[String], preferred: Option<&str>) -> String {
    if let Some(preferred) = preferred
        && let Some(found) = model_ids.iter().find(|m| m.as_str() == preferred)
    {
        return found.clone();
    }

    // Strong-first model preference for chain/resume stability.
    const HINTS: [&str; 8] = [
        "gpt-oss:120b-cloud",
        "gpt-oss-120b",
        "qwen3-coder:480b-cloud",
        "qwen3-vl:235b-cloud",
        "qwen3-vl-235b",
        "qwen3-4b",
        "qwen2.5-7b",
        "mistral-7b",
    ];

    for hint in HINTS {
        let hint_lower = hint.to_ascii_lowercase();
        if let Some(found) = model_ids
            .iter()
            .find(|m| m.to_ascii_lowercase().contains(&hint_lower))
        {
            return found.clone();
        }
    }

    model_ids[0].clone()
}

fn unique_token(port: u16) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("CHAIN-TOKEN-{port}-{nanos:X}")
}

fn normalize_token_text(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(*c, '-' | '_'))
        .collect::<String>()
        .to_ascii_uppercase()
}

fn extract_output_text(output: &[OutputItem]) -> String {
    let mut parts = Vec::new();
    for item in output.iter().filter(|item| item.item_type == "message") {
        let Some(content) = item.content.as_ref() else {
            continue;
        };
        for part in content {
            if matches!(part.part_type.as_str(), "output_text" | "text")
                && let Some(text) = part
                    .text
                    .as_deref()
                    .or(part.summary.as_deref())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            {
                parts.push(text.to_string());
            }
        }
    }
    parts.join("")
}

async fn send_responses_non_stream(
    client: &Client,
    target: &LiveTarget,
    input_text: &str,
    previous_response_id: Option<&str>,
) -> NonStreamResponse {
    let request = json!({
        "model": target.model,
        "input": [
            {
                "role": "user",
                "content": [
                    {
                        "type": "input_text",
                        "text": input_text
                    }
                ]
            }
        ],
        "previous_response_id": previous_response_id,
        "stream": false
    });

    let response = client
        .post(&target.endpoint)
        .header("Authorization", format!("Bearer {}", target.api_key))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .expect("non-stream request");
    assert!(
        response.status().is_success(),
        "Port {} model {} non-stream request failed: HTTP {}",
        target.port,
        target.model,
        response.status()
    );

    response
        .json::<NonStreamResponse>()
        .await
        .expect("non-stream response JSON")
}

#[tokio::test]
#[serial]
async fn e2e_responses_ports_expose_models_on_all_local_ports() {
    if !e2e_responses_enabled() {
        eprintln!("Skipping live responses matrix test (set CODESCRIBE_E2E_RESPONSES=1)");
        return;
    }

    load_codescribe_env();
    let client = Client::new();
    let expected_ports = configured_ports();
    let required_ports: HashSet<u16> = configured_required_ports().into_iter().collect();
    let targets = discover_live_targets(&client).await;

    let found_ports: HashSet<u16> = targets.iter().map(|t| t.port).collect();
    let mut missing_required_ports: Vec<u16> = required_ports
        .iter()
        .filter(|p| !found_ports.contains(p))
        .copied()
        .collect();
    missing_required_ports.sort_unstable();
    assert!(
        missing_required_ports.is_empty(),
        "Required responses ports missing /v1/models model exposure: {:?}. \
         Hint: if endpoint is auth-protected set CODESCRIBE_E2E_RESPONSES_API_KEY.",
        missing_required_ports
    );

    let mut missing_optional_ports: Vec<u16> = expected_ports
        .into_iter()
        .filter(|p| !required_ports.contains(p) && !found_ports.contains(p))
        .collect();
    missing_optional_ports.sort_unstable();
    if !missing_optional_ports.is_empty() {
        eprintln!(
            "Optional responses ports missing /v1/models model exposure: {:?}",
            missing_optional_ports
        );
    }
}

#[tokio::test]
#[serial]
async fn e2e_responses_continuation_chain_recalls_token_real_endpoint() {
    if !e2e_responses_enabled() {
        eprintln!("Skipping live continuation-chain test (set CODESCRIBE_E2E_RESPONSES=1)");
        return;
    }

    load_codescribe_env();
    let client = Client::new();
    let discovered_targets = discover_live_targets(&client).await;
    assert!(
        !discovered_targets.is_empty(),
        "No live responses targets detected"
    );
    let chain_ports: HashSet<u16> = configured_chain_ports().into_iter().collect();
    let online_ports: HashSet<u16> = discovered_targets.iter().map(|t| t.port).collect();
    let mut missing_chain_ports: Vec<u16> = chain_ports
        .iter()
        .filter(|p| !online_ports.contains(p))
        .copied()
        .collect();
    missing_chain_ports.sort_unstable();
    assert!(
        missing_chain_ports.is_empty(),
        "Missing strict chain ports online: {:?}",
        missing_chain_ports
    );

    let targets: Vec<LiveTarget> = discovered_targets
        .into_iter()
        .filter(|t| chain_ports.contains(&t.port))
        .collect();
    assert!(
        !targets.is_empty(),
        "No live chain targets online (wanted ports: {:?})",
        chain_ports
    );

    let mut supported_chain_ports = Vec::new();
    let mut unsupported_chain_ports = Vec::new();
    let mut hard_failures = Vec::new();

    for target in &targets {
        let token = unique_token(target.port);
        let first_prompt = format!("Remember this token exactly: {token}. Reply with exactly ACK.");
        let first = send_responses_non_stream(&client, target, &first_prompt, None).await;
        assert!(
            !first.id.trim().is_empty(),
            "Port {} model {} did not return response id in first call",
            target.port,
            target.model
        );
        let first_text = extract_output_text(&first.output);
        assert!(
            !first_text.trim().is_empty(),
            "Port {} model {} first call returned empty output text",
            target.port,
            target.model
        );

        let second_prompt = "Return the exact token from the previous request. Output token only.";
        let second =
            send_responses_non_stream(&client, target, second_prompt, Some(&first.id)).await;
        let second_text = extract_output_text(&second.output);
        let normalized_second = normalize_token_text(&second_text);
        let normalized_token = normalize_token_text(&token);
        let normalized_second_prompt = normalize_token_text(second_prompt);

        if normalized_second.contains(&normalized_token) {
            supported_chain_ports.push(target.port);
        } else if normalized_second.contains(&normalized_second_prompt) {
            unsupported_chain_ports.push(target.port);
            eprintln!(
                "Port {} model {} appears to ignore previous_response_id (prompt echoed)",
                target.port, target.model
            );
            continue;
        } else {
            hard_failures.push(format!(
                "port={} model={} token={} response={}",
                target.port, target.model, token, second_text
            ));
            continue;
        }

        assert!(
            !second.id.trim().is_empty(),
            "Port {} model {} did not return response id in second call",
            target.port,
            target.model
        );
        assert_ne!(
            first.id, second.id,
            "Port {} model {} reused the same response id across chain calls",
            target.port, target.model
        );
    }

    assert!(
        hard_failures.is_empty(),
        "Continuation chain failed on supported backend(s): {:?}",
        hard_failures
    );
    assert!(
        unsupported_chain_ports.is_empty(),
        "Strict chain ports did not honor previous_response_id: {:?}",
        unsupported_chain_ports
    );
    assert_eq!(
        supported_chain_ports.len(),
        targets.len(),
        "Not all strict chain ports passed continuation check"
    );
}

#[tokio::test]
#[serial]
async fn e2e_responses_resume_stream_after_abort_real_endpoint() {
    if !e2e_responses_enabled() {
        eprintln!("Skipping live resume test (set CODESCRIBE_E2E_RESPONSES=1)");
        return;
    }

    load_codescribe_env();
    let client = Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .expect("reqwest client");
    let discovered_targets = discover_live_targets(&client).await;
    assert!(
        !discovered_targets.is_empty(),
        "No live responses targets detected"
    );
    let resume_ports: HashSet<u16> = configured_resume_ports().into_iter().collect();
    let online_ports: HashSet<u16> = discovered_targets.iter().map(|t| t.port).collect();
    let mut missing_resume_ports: Vec<u16> = resume_ports
        .iter()
        .filter(|p| !online_ports.contains(p))
        .copied()
        .collect();
    missing_resume_ports.sort_unstable();
    assert!(
        missing_resume_ports.is_empty(),
        "Missing strict resume ports online: {:?}",
        missing_resume_ports
    );

    let targets: Vec<LiveTarget> = discovered_targets
        .into_iter()
        .filter(|t| resume_ports.contains(&t.port))
        .collect();
    assert!(
        !targets.is_empty(),
        "No strict resume targets online (wanted ports: {:?})",
        resume_ports
    );

    let mut supported_resume_ports = Vec::new();
    let mut unsupported_resume_ports = Vec::new();
    let mut hard_failures = Vec::new();

    for target in &targets {
        let request = json!({
            "model": target.model,
            "input": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "input_text",
                            "text": "Wypisz liczby od 1 do 220 po spacji. Nie skracaj, nie komentuj."
                        }
                    ]
                }
            ],
            "stream": true
        });

        let response = client
            .post(&target.endpoint)
            .header("Authorization", format!("Bearer {}", target.api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&request)
            .send()
            .await
            .expect("initial stream request");
        assert!(
            response.status().is_success(),
            "Port {} model {} initial stream failed: HTTP {}",
            target.port,
            target.model,
            response.status()
        );

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut partial_text = String::new();
        let mut response_id: Option<String> = None;
        let mut sequence_number: Option<u64> = None;
        let mut saw_done_early = false;
        let capture_deadline = Instant::now() + Duration::from_secs(30);

        while Instant::now() < capture_deadline {
            let next_chunk = match tokio::time::timeout(Duration::from_secs(6), stream.next()).await
            {
                Ok(chunk) => chunk,
                Err(_) => break,
            };
            let Some(chunk_result) = next_chunk else {
                break;
            };
            let chunk = chunk_result.expect("stream chunk read");
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim().to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }
                if let Some(data) = line
                    .strip_prefix("data: ")
                    .or_else(|| line.strip_prefix("data:"))
                {
                    let data = data.trim_start();
                    if data == "[DONE]" {
                        saw_done_early = true;
                        break;
                    }
                    if let Ok(event) = serde_json::from_str::<StreamChunk>(data) {
                        if let Some(seq) = event.sequence_number {
                            sequence_number = Some(seq);
                        }
                        if let Some(resp) = event.response
                            && !resp.id.is_empty()
                        {
                            response_id = Some(resp.id);
                        }
                        if event.chunk_type == "response.output_text.delta"
                            && let Some(delta) = event.delta
                        {
                            partial_text.push_str(&delta);
                        }
                    }
                }
            }

            if saw_done_early
                || (partial_text.len() >= 24 && response_id.is_some() && sequence_number.is_some())
            {
                break;
            }
        }

        drop(stream);
        let response_id_for_log = response_id.clone();
        let sequence_for_log = sequence_number;
        let (response_id, sequence_number) = match (response_id, sequence_number) {
            (Some(response_id), Some(sequence_number)) if !partial_text.is_empty() => {
                (response_id, sequence_number)
            }
            _ => {
                unsupported_resume_ports.push(target.port);
                eprintln!(
                    "Port {} model {} did not provide enough streaming metadata \
                     for resume probe (response_id={:?}, seq={:?}, partial={}B)",
                    target.port,
                    target.model,
                    response_id_for_log,
                    sequence_for_log,
                    partial_text.len()
                );
                continue;
            }
        };

        let resume_url = format!(
            "{}/{response_id}?stream=true&starting_after={sequence_number}",
            target.endpoint.trim_end_matches('/')
        );
        let mut resumed_response = None;
        let mut last_status = None;
        let mut transport_error = None;
        for _ in 0..4 {
            match client
                .get(&resume_url)
                .header("Authorization", format!("Bearer {}", target.api_key))
                .header("Accept", "text/event-stream")
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    resumed_response = Some(resp);
                    break;
                }
                Ok(resp) => {
                    last_status = Some(resp.status());
                    // Some backends expose response ID asynchronously.
                    if resp.status() == StatusCode::NOT_FOUND {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        continue;
                    }
                    break;
                }
                Err(err) => {
                    transport_error = Some(err.to_string());
                    break;
                }
            }
        }

        let Some(resumed_response) = resumed_response else {
            if matches!(
                last_status,
                Some(StatusCode::NOT_FOUND)
                    | Some(StatusCode::BAD_REQUEST)
                    | Some(StatusCode::METHOD_NOT_ALLOWED)
            ) {
                unsupported_resume_ports.push(target.port);
                eprintln!(
                    "Port {} model {} does not expose resume endpoint (status={:?})",
                    target.port, target.model, last_status
                );
                continue;
            }

            hard_failures.push(format!(
                "port={} model={} status={:?} transport_error={:?}",
                target.port, target.model, last_status, transport_error
            ));
            continue;
        };

        let mut resumed_stream = resumed_response.bytes_stream();
        let mut resumed_buffer = String::new();
        let mut resumed_text = String::new();
        let mut resume_done = false;
        let resume_deadline = Instant::now() + Duration::from_secs(30);

        while Instant::now() < resume_deadline {
            let next_chunk =
                match tokio::time::timeout(Duration::from_secs(6), resumed_stream.next()).await {
                    Ok(chunk) => chunk,
                    Err(_) => break,
                };
            let Some(chunk_result) = next_chunk else {
                break;
            };
            let chunk = chunk_result.expect("resume chunk read");
            resumed_buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline_pos) = resumed_buffer.find('\n') {
                let line = resumed_buffer[..newline_pos].trim().to_string();
                resumed_buffer = resumed_buffer[newline_pos + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }
                if let Some(data) = line
                    .strip_prefix("data: ")
                    .or_else(|| line.strip_prefix("data:"))
                {
                    let data = data.trim_start();
                    if data == "[DONE]" {
                        resume_done = true;
                        break;
                    }
                    if let Ok(event) = serde_json::from_str::<StreamChunk>(data)
                        && event.chunk_type == "response.output_text.delta"
                        && let Some(delta) = event.delta
                    {
                        resumed_text.push_str(&delta);
                    }
                }
            }

            if resume_done {
                break;
            }
        }

        assert!(
            resume_done || !resumed_text.is_empty(),
            "Port {} model {} resume returned neither [DONE] nor delta text",
            target.port,
            target.model
        );
        supported_resume_ports.push(target.port);
        eprintln!(
            "Port {} resume ok: partial={}B resumed={}B",
            target.port,
            partial_text.len(),
            resumed_text.len()
        );
    }

    assert!(
        hard_failures.is_empty(),
        "Resume probe failed unexpectedly: {:?}",
        hard_failures
    );
    assert!(
        unsupported_resume_ports.is_empty(),
        "Strict resume ports do not implement resume capability: {:?}",
        unsupported_resume_ports
    );
    assert_eq!(
        supported_resume_ports.len(),
        targets.len(),
        "Not all strict resume ports passed resume probe"
    );
}
