//! ACP provider lifecycle. The process owns agent execution; Codescribe owns
//! client permissions, event projection, cancellation and process lifetime.

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use base64::Engine;
use codescribe_core::acp::client_bridge::{AcpClientBridge, AcpClientBridgeConfig};
use codescribe_core::acp::mapper::AcpEventMapper;
use codescribe_core::acp::protocol;
use codescribe_core::acp::transport::AcpTransport;
use codescribe_core::acp::{AcpCommand, AcpError};
use codescribe_core::agent::{
    AgentEvent, AgentProvider, AgentProviderHost, ContentBlock, Message, Role, StreamOptions,
    ToolDefinition,
};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc, watch};
use tokio::task::JoinSet;

/// Host-supplied process and permission context. The host must obtain these
/// from its sealed configuration and bind its approval UI before construction.
#[derive(Clone)]
pub struct AcpProviderConfig {
    pub command: AcpCommand,
    pub bridge: AcpClientBridgeConfig,
    pub acp_session_id: Option<String>,
}

/// A local ACP process provider, with one active turn and a resumable session id.
pub struct AcpProvider {
    command: AcpCommand,
    workspace_roots: Vec<std::path::PathBuf>,
    explicit_bridge: Option<AcpClientBridgeConfig>,
    host: StdMutex<Option<AgentProviderHost>>,
    invalidated_sessions: Arc<StdMutex<std::collections::HashSet<String>>>,
    chain: Arc<Mutex<Option<String>>>,
    turn: Arc<Mutex<()>>,
    cancel: watch::Sender<u64>,
}

impl AcpProvider {
    pub fn new(config: AcpProviderConfig) -> Result<Self> {
        Self::configured(
            config.command,
            config.bridge.workspace_roots.clone(),
            Some(config.bridge),
            config.acp_session_id,
        )
    }

    /// Native selection is already resolved by the immutable settings owner.
    /// Its registry and approvals can only be bound by the owning AgentSession.
    pub fn from_runtime(
        configuration: &codescribe_core::config::RuntimeAcpConfiguration,
    ) -> Result<Self> {
        Self::configured(
            configuration.command.clone(),
            configuration.workspace_roots.clone(),
            None,
            None,
        )
    }

    fn configured(
        mut command: AcpCommand,
        workspace_roots: Vec<std::path::PathBuf>,
        explicit_bridge: Option<AcpClientBridgeConfig>,
        session_id: Option<String>,
    ) -> Result<Self> {
        if command.program.trim().is_empty() {
            anyhow::bail!("ACP executable is empty");
        }
        let cwd = command.cwd.canonicalize()?;
        let roots = workspace_roots
            .iter()
            .map(|root| root.canonicalize())
            .collect::<std::io::Result<Vec<_>>>()?;
        if !cwd.is_dir()
            || !roots
                .iter()
                .any(|root| root.is_dir() && cwd.starts_with(root))
        {
            anyhow::bail!("ACP working directory is outside workspace roots");
        }
        command.cwd = cwd;
        Ok(Self {
            command,
            workspace_roots: roots,
            explicit_bridge,
            host: StdMutex::new(None),
            invalidated_sessions: Arc::default(),
            chain: Arc::new(Mutex::new(session_id)),
            turn: Arc::new(Mutex::new(())),
            cancel: watch::channel(0).0,
        })
    }

    /// Cancel active turns; a later stream subscribes to the new generation.
    pub fn cancel(&self) {
        self.cancel
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }
}

impl Drop for AcpProvider {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[async_trait]
impl AgentProvider for AcpProvider {
    fn bind_host(&self, host: AgentProviderHost) {
        *self
            .host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(host);
    }

    fn supports_context_preparation(&self) -> bool {
        false
    }

    async fn stream(
        &self,
        messages: &[Message],
        _tools: &[ToolDefinition],
        options: &StreamOptions,
    ) -> Result<mpsc::Receiver<AgentEvent>> {
        let host = self
            .host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let bridge_config = if let Some(host) = &host {
            AcpClientBridgeConfig {
                registry: Arc::clone(&host.tools),
                workspace_roots: self.workspace_roots.clone(),
                approval_handler: host.approval_handler.clone(),
                ui_tx: Some(host.ui_tx.clone()),
                approval_timeout: host.approval_timeout,
                thread_id: host.thread_id.clone(),
            }
        } else {
            self.explicit_bridge.clone().ok_or_else(|| {
                anyhow::anyhow!(
                    "ACP host services must be bound by the owning AgentSession before a turn"
                )
            })?
        };
        let mut bridge = AcpClientBridge::new(bridge_config)?;
        if let Some(host) = host {
            bridge.bind_native_session(host.session_id);
        }
        let launch_directory = bridge.open_launch_directory(&self.command.cwd)?;
        let command = self.command.clone();
        let (tx, rx) = mpsc::channel(128);
        let chain = self.chain.clone();
        let invalidated_sessions = self.invalidated_sessions.clone();
        let turn = self.turn.clone();
        let messages = messages.to_vec();
        let options = options.clone();
        let mut cancelled = self.cancel.subscribe();
        tokio::spawn(async move {
            let _turn = tokio::select! {
                guard = turn.lock() => guard,
                _ = tx.closed() => return,
                _ = cancelled.changed() => return,
            };
            if options.reset_chain {
                *chain.lock().await = None;
            }
            let previous = chain.lock().await.clone();
            let mut transport =
                match AcpTransport::spawn_in_directory(&command, launch_directory).await {
                    Ok(transport) => transport,
                    Err(error) => {
                        *chain.lock().await = None;
                        send_failure(&tx, error).await;
                        return;
                    }
                };
            let mut session_id = None;
            let mut active_bridge = None;
            let input = TurnInput {
                messages: &messages,
                options: &options,
                previous: previous.as_deref(),
                command: &command,
            };
            let result = tokio::select! {
                result = run_turn(
                    &mut transport, bridge, &input, &tx, &mut session_id, &mut active_bridge,
                ) => result,
                _ = tx.closed() => Err(AcpError::Cancelled),
                _ = cancelled.changed() => Err(AcpError::Cancelled),
            };
            if let Some(bridge) = active_bridge {
                bridge.shutdown().await;
            }
            transport.shutdown(session_id.as_deref()).await;
            match result {
                Ok(events) => {
                    // Publish a clean terminal only after all resources have
                    // settled and the resumable chain is already authoritative.
                    if cancelled.has_changed().unwrap_or(true) || tx.is_closed() {
                        invalidated_sessions
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .extend(previous.iter().chain(session_id.iter()).cloned());
                        *chain.lock().await = None;
                        send_failure(&tx, AcpError::Cancelled).await;
                        return;
                    }
                    *chain.lock().await = session_id.clone();
                    for event in events {
                        if tx.send(event).await.is_err() {
                            invalidated_sessions
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .extend(previous.iter().chain(session_id.iter()).cloned());
                            *chain.lock().await = None;
                            break;
                        }
                    }
                }
                Err(error) => {
                    // A cancelled/failed turn must never advance the resume id.
                    // ACP session/load is not a snapshot rollback: an aborted
                    // prompt may already have changed the remote conversation.
                    invalidated_sessions
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .extend(previous.iter().chain(session_id.iter()).cloned());
                    *chain.lock().await = None;
                    send_failure(&tx, error).await;
                }
            }
        });
        Ok(rx)
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

    fn name(&self) -> &str {
        "acp"
    }

    async fn response_chain_id(&self) -> Option<String> {
        self.chain.lock().await.clone()
    }

    async fn restore_response_chain(&self, id: Option<String>) {
        self.cancel();
        // Serialize rollback after teardown so a detached cancelled turn
        // cannot overwrite this restore. Tainted ACP sessions cannot rewind.
        let _turn = self.turn.lock().await;
        // The native Stop API passes the pre-turn chain id. An ACP id names
        // a mutable whole conversation, not a point-in-time response. Even if
        // a clean terminal raced Stop, it cannot rewind the remote history.
        let native = self
            .host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some();
        let id = id.filter(|id| {
            !native
                && !self
                    .invalidated_sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contains(id)
        });
        *self.chain.lock().await = id;
    }
}

async fn send_failure(tx: &mpsc::Sender<AgentEvent>, error: AcpError) {
    let detail = codescribe_core::agent::redact_command_line(&error.to_string());
    let _ = tx
        .send(AgentEvent::ResponseDone {
            response_id: None,
            clean: false,
        })
        .await;
    let _ = tx.send(AgentEvent::Error(detail)).await;
}

struct TurnInput<'a> {
    messages: &'a [Message],
    options: &'a StreamOptions,
    previous: Option<&'a str>,
    command: &'a AcpCommand,
}

async fn run_turn(
    transport: &mut AcpTransport,
    mut bridge: AcpClientBridge,
    input: &TurnInput<'_>,
    tx: &mpsc::Sender<AgentEvent>,
    session_id: &mut Option<String>,
    active_bridge: &mut Option<Arc<AcpClientBridge>>,
) -> std::result::Result<Vec<AgentEvent>, AcpError> {
    let TurnInput {
        messages,
        options,
        previous,
        command,
    } = *input;
    let mut mapper = AcpEventMapper::default();
    let initialized = tokio::time::timeout(
        Duration::from_secs(30),
        call(
            transport,
            None,
            &mut mapper,
            tx,
            1,
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": {"readTextFile": true, "writeTextFile": true},
                    "terminal": true
                },
                "clientInfo": {"name": "codescribe", "version": env!("CARGO_PKG_VERSION")}
            }),
        ),
    )
    .await
    .map_err(|_| AcpError::Protocol("initialize timed out".into()))??;
    if initialized["protocolVersion"].as_u64() != Some(1) {
        return Err(AcpError::Protocol(
            "unsupported ACP protocol version".into(),
        ));
    }
    // The launch directory was already pinned through the workspace bridge;
    // do not reinterpret a changed pathname after the child has started.
    let cwd = command.cwd.to_string_lossy().into_owned();
    let (method, params) = if let Some(previous) = previous {
        if initialized["agentCapabilities"]["loadSession"].as_bool() != Some(true) {
            return Err(AcpError::Protocol(
                "agent cannot load the retained ACP session".into(),
            ));
        }
        (
            "session/load",
            json!({"sessionId": previous, "cwd": cwd, "mcpServers": []}),
        )
    } else {
        ("session/new", json!({"cwd": cwd, "mcpServers": []}))
    };
    let session = tokio::time::timeout(
        Duration::from_secs(30),
        call(transport, None, &mut mapper, tx, 2, method, params),
    )
    .await
    .map_err(|_| AcpError::Protocol("session setup timed out".into()))??;
    if let (Some(expected), Some(actual)) = (previous, session["sessionId"].as_str())
        && expected != actual
    {
        return Err(AcpError::Protocol(
            "session/load returned a different identity".into(),
        ));
    }
    let id = previous
        .map(str::to_string)
        .or_else(|| {
            session["sessionId"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(str::to_string)
        })
        .ok_or_else(|| AcpError::Protocol("session/new omitted sessionId".into()))?;
    *session_id = Some(id.clone());
    transport.set_session_id(id.clone());
    bridge.set_session_id(id.clone());
    let bridge = Arc::new(bridge);
    *active_bridge = Some(bridge.clone());
    let prompt = prompt_content(messages, options, previous.is_some())?;
    if prompt
        .iter()
        .any(|part| part["type"].as_str() == Some("image"))
        && initialized["agentCapabilities"]["promptCapabilities"]["image"].as_bool() != Some(true)
    {
        return Err(AcpError::Protocol(
            "selected ACP agent does not support image prompts".into(),
        ));
    }
    let result = call(
        transport,
        Some(bridge.clone()),
        &mut mapper,
        tx,
        3,
        "session/prompt",
        json!({"sessionId": id, "prompt": prompt}),
    )
    .await;
    bridge.shutdown().await;
    let result = result?;
    match result["stopReason"].as_str() {
        Some("cancelled") => return Err(AcpError::Cancelled),
        Some("end_turn") => {}
        Some("max_tokens" | "max_turn_requests" | "refusal") => {
            return Err(AcpError::Protocol(
                "agent ended without completing the requested turn".into(),
            ));
        }
        _ => {
            return Err(AcpError::Protocol(
                "missing or unknown prompt stopReason".into(),
            ));
        }
    }
    Ok(mapper.finish(&id))
}

/// Consume both directions while an agent method is outstanding. Client
/// requests execute independently, so a terminal wait cannot block its kill.
async fn call(
    transport: &mut AcpTransport,
    bridge: Option<Arc<AcpClientBridge>>,
    mapper: &mut AcpEventMapper,
    tx: &mpsc::Sender<AgentEvent>,
    id: u64,
    method: &str,
    params: Value,
) -> std::result::Result<Value, AcpError> {
    transport
        .send(protocol::request(id, method, params))
        .await?;
    let mut incoming: JoinSet<(Value, std::result::Result<Value, AcpError>)> = JoinSet::new();
    loop {
        tokio::select! {
            completed = incoming.join_next(), if !incoming.is_empty() => {
                if let Some(Ok((request_id, result))) = completed {
                    let frame = match result {
                        Ok(result) => protocol::success(request_id, result),
                        Err(error) => protocol::error(request_id, -32000,
                            &codescribe_core::agent::redact_command_line(&error.to_string())),
                    };
                    transport.send(frame).await?;
                }
            }
            frame = transport.recv() => {
                let frame = frame?;
                if let Some(method) = frame["method"].as_str() {
                    let params = frame.get("params").cloned().unwrap_or(Value::Null);
                    if let Some(request_id) = frame.get("id") {
                        let request_id = request_id.clone();
                        if let Some(bridge) = &bridge {
                            if incoming.len() >= 64 {
                                return Err(AcpError::Protocol("too many outstanding client requests".into()));
                            }
                            let bridge = bridge.clone();
                            let method = method.to_string();
                            incoming.spawn(async move {
                                (request_id, bridge.handle(&method, params).await)
                            });
                        } else {
                            transport.send(protocol::error(request_id.clone(), -32000,
                                "client requests require an active session")).await?;
                        }
                    } else if method == "session/update" {
                        // session/load replays history as notifications. The
                        // local AgentSession already owns it: never present it
                        // as this turn's newly generated text or reasoning.
                        if let Some(bridge) = &bridge {
                            if params["sessionId"].as_str() != Some(bridge.session_id()) {
                                return Err(AcpError::Protocol("session update identity mismatch".into()));
                            }
                            if let Some(event) = mapper.update(&params) {
                                tx.send(event).await.map_err(|_| AcpError::Cancelled)?;
                            }
                        }
                    }
                } else if frame["id"].as_u64() == Some(id) {
                    if let Some(error) = frame.get("error") {
                        return Err(AcpError::Protocol(codescribe_core::agent::redact_command_line(
                            error["message"].as_str().unwrap_or("agent request failed"))));
                    }
                    return frame.get("result").cloned()
                        .ok_or_else(|| AcpError::Protocol("response omitted result".into()));
                } else {
                    return Err(AcpError::Protocol("unexpected response id".into()));
                }
            }
        }
    }
}

fn prompt_content(
    messages: &[Message],
    options: &StreamOptions,
    resumed: bool,
) -> std::result::Result<Vec<Value>, AcpError> {
    let mut prompt = Vec::new();
    if !resumed && let Some(system) = &options.system_prompt {
        prompt.push(json!({"type": "text", "text": system}));
    }
    let selected = if resumed {
        let latest = messages
            .iter()
            .rposition(|message| message.role == Role::User)
            .ok_or_else(|| AcpError::Protocol("no user prompt".into()))?;
        &messages[latest..=latest]
    } else {
        messages
    };
    for message in selected {
        if !resumed && selected.len() > 1 {
            prompt.push(json!({"type": "text", "text": format!("{:?}:", message.role)}));
        }
        for block in &message.content {
            match block {
                ContentBlock::Text(text) => prompt.push(json!({"type": "text", "text": text})),
                ContentBlock::Image { data, media_type } => prompt.push(json!({
                    "type": "image", "data": base64::engine::general_purpose::STANDARD.encode(data),
                    "mimeType": media_type
                })),
                ContentBlock::ImageAsset(asset) => {
                    let bytes = codescribe_core::agent::AgentAssetStore::read_image(&asset.path)
                        .map_err(|error| AcpError::Protocol(error.to_string()))?;
                    prompt.push(json!({"type": "image", "data": base64::engine::general_purpose::STANDARD.encode(bytes),
                        "mimeType": asset.media_type}));
                }
                ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. } => {
                    prompt.push(json!({"type": "text", "text": serde_json::to_string(block)
                        .map_err(|error| AcpError::Protocol(error.to_string()))?}));
                }
            }
        }
    }
    if prompt.is_empty() {
        return Err(AcpError::Protocol("empty prompt".into()));
    }
    Ok(prompt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codescribe_core::agent::AgentPermissions;
    use std::collections::BTreeMap;
    use std::path::Path;

    const AGENT: &str = r#"
import json, os, subprocess, sys, time
mode, log = sys.argv[1:]
def record(text):
    with open(log, 'a') as f: f.write(text + '\n')
def send(frame):
    print(json.dumps(dict(jsonrpc='2.0', **frame)), flush=True)
record('pid:' + str(os.getpid()))
if mode == 'malformed':
    print('TOKEN=do-not-leak broken-banner', flush=True)
    time.sleep(60)
for line in sys.stdin:
    frame = json.loads(line)
    method, params = frame.get('method'), frame.get('params', {})
    record(method or 'client-response')
    if method == 'initialize':
        assert params['protocolVersion'] == 1
        assert params['clientCapabilities']['fs']['readTextFile']
        assert params['clientCapabilities']['fs']['writeTextFile']
        assert params['clientCapabilities']['terminal']
        send(dict(id=frame['id'], result=dict(protocolVersion=1, agentCapabilities=dict(loadSession=True, promptCapabilities=dict(image=mode != 'no-images')))))
    elif method in ('session/new', 'session/load'):
        assert os.path.isabs(params['cwd'])
        if method == 'session/load' and mode == 'replay':
            send(dict(method='session/update', params=dict(sessionId='session-fixture', update=dict(sessionUpdate='agent_message_chunk', content=dict(type='text', text='old history')))))
        send(dict(id=frame['id'], result=dict(sessionId='session-fixture')))
    elif method == 'session/prompt':
        assert params['sessionId'] == 'session-fixture'
        assert any(p.get('text') == 'hello' for p in params['prompt'])
        if mode == 'attachments':
            assert any(p.get('type') == 'image' and p['data'] == 'AQID' for p in params['prompt'])
        if mode == 'cancel': continue
        if mode == 'exit-desc':
            child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])
            record('descendant:' + str(child.pid))
            print('TOKEN=do-not-leak parent failed', file=sys.stderr, flush=True)
            os._exit(7)
        if mode == 'denied-read':
            send(dict(id='read-1', method='fs/read_text_file', params=dict(sessionId='session-fixture', path=os.path.join(params.get('cwd', os.getcwd()), '.env'))))
            response=json.loads(sys.stdin.readline())
            assert 'error' in response and 'result' not in response
            record('secret-refused')
        for kind, text in [('agent_thought_chunk', 'thinking'), ('agent_message_chunk', 'answer')]:
            send(dict(method='session/update', params=dict(sessionId='session-fixture', update=dict(sessionUpdate=kind, content=dict(type='text', text=text)))))
        if mode == 'eof': sys.exit(7)
        if mode == 'error': send(dict(id=frame['id'], error=dict(code=-32000,message='TOKEN=do-not-leak failed')))
        else: send(dict(id=frame['id'], result=dict(stopReason=mode if mode in ('cancelled', 'max_tokens', 'max_turn_requests', 'refusal') else 'end_turn')))
        if mode == 'clean-exit': os._exit(0)
    elif method == 'session/cancel':
        record('cancel-seen')
        break
"#;

    fn fixture(root: &Path, mode: &str) -> AcpProviderConfig {
        let script = root.join("agent.py");
        std::fs::write(&script, AGENT).unwrap();
        AcpProviderConfig {
            command: AcpCommand {
                program: "/usr/bin/python3".into(),
                args: vec![
                    "-u".into(),
                    script.display().to_string(),
                    mode.into(),
                    root.join("receipt").display().to_string(),
                ],
                cwd: root.to_path_buf(),
                env: BTreeMap::new(),
            },
            bridge: AcpClientBridgeConfig::for_workspace(
                vec![root.to_path_buf()],
                AgentPermissions::default(),
            )
            .unwrap(),
            acp_session_id: None,
        }
    }

    fn messages() -> Vec<Message> {
        vec![Message::new(
            Role::User,
            vec![ContentBlock::Text("hello".into())],
        )]
    }

    async fn collect(mut rx: mpsc::Receiver<AgentEvent>) -> Vec<AgentEvent> {
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut events = Vec::new();
            while let Some(event) = rx.recv().await {
                events.push(event);
            }
            events
        })
        .await
        .expect("ACP turn must terminate")
    }

    #[tokio::test]
    async fn acp_handshake_stream_thought_separation_and_resume_through_factory() {
        let root = tempfile::tempdir().unwrap();
        let provider = super::super::create_agent_provider(fixture(root.path(), "normal")).unwrap();
        let expected = vec![
            AgentEvent::ReasoningDelta("thinking".into()),
            AgentEvent::TextDelta("answer".into()),
            AgentEvent::TextDone("answer".into()),
            AgentEvent::ResponseDone {
                response_id: Some("session-fixture".into()),
                clean: true,
            },
        ];
        assert_eq!(
            collect(
                provider
                    .stream(&messages(), &[], &StreamOptions::default())
                    .await
                    .unwrap()
            )
            .await,
            expected
        );
        assert_eq!(
            provider.response_chain_id().await.as_deref(),
            Some("session-fixture")
        );
        assert_eq!(
            collect(
                provider
                    .stream(&messages(), &[], &StreamOptions::default())
                    .await
                    .unwrap()
            )
            .await,
            expected
        );
        let log = std::fs::read_to_string(root.path().join("receipt")).unwrap();
        assert!(
            log.contains("initialize\nsession/new\nsession/prompt"),
            "{log}"
        );
        assert!(
            log.contains("initialize\nsession/load\nsession/prompt"),
            "{log}"
        );
    }

    #[tokio::test]
    async fn acp_prompt_carries_attachments_and_denies_unapproved_secret_callback() {
        let root = tempfile::tempdir().unwrap();
        let provider = AcpProvider::new(fixture(root.path(), "attachments")).unwrap();
        let mut messages = messages();
        messages[0].content.push(ContentBlock::Image {
            data: vec![1, 2, 3],
            media_type: "image/png".into(),
        });
        let events = collect(
            provider
                .stream(&messages, &[], &StreamOptions::default())
                .await
                .unwrap(),
        )
        .await;
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::ResponseDone { clean: true, .. }))
        );
        std::fs::write(root.path().join(".env"), "TOP_SECRET=not-for-agent").unwrap();
        let provider = AcpProvider::new(fixture(root.path(), "denied-read")).unwrap();
        let events = collect(
            provider
                .stream(&messages, &[], &StreamOptions::default())
                .await
                .unwrap(),
        )
        .await;
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::ResponseDone { clean: true, .. })),
            "{events:?}"
        );
        assert!(
            std::fs::read_to_string(root.path().join("receipt"))
                .unwrap()
                .contains("secret-refused")
        );
    }

    #[tokio::test]
    async fn acp_malformed_eof_error_and_cancelled_response_are_dirty() {
        for mode in [
            "malformed",
            "eof",
            "error",
            "cancelled",
            "max_tokens",
            "max_turn_requests",
            "refusal",
        ] {
            let root = tempfile::tempdir().unwrap();
            let provider = AcpProvider::new(fixture(root.path(), mode)).unwrap();
            let events = collect(
                provider
                    .stream(&messages(), &[], &StreamOptions::default())
                    .await
                    .unwrap(),
            )
            .await;
            assert!(
                events.iter().any(|event| matches!(
                    event,
                    AgentEvent::ResponseDone {
                        response_id: None,
                        clean: false
                    }
                )),
                "{mode}: {events:?}"
            );
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, AgentEvent::Error(_))),
                "{mode}: {events:?}"
            );
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, AgentEvent::TextDone(_)))
            );
            assert!(!format!("{events:?}").contains("do-not-leak"));
            assert_eq!(provider.response_chain_id().await, None);
        }
    }

    #[tokio::test]
    async fn acp_load_replay_is_not_new_output() {
        let root = tempfile::tempdir().unwrap();
        let provider = AcpProvider::new(fixture(root.path(), "replay")).unwrap();
        for _ in 0..2 {
            let events = collect(
                provider
                    .stream(&messages(), &[], &StreamOptions::default())
                    .await
                    .unwrap(),
            )
            .await;
            assert!(
                events.contains(&AgentEvent::TextDone("answer".into())),
                "{events:?}"
            );
            assert!(!format!("{events:?}").contains("old history"), "{events:?}");
        }
        assert!(
            std::fs::read_to_string(root.path().join("receipt"))
                .unwrap()
                .contains("session/load")
        );
    }

    #[tokio::test]
    async fn acp_parent_exit_is_observed_even_when_descendant_holds_stdout() {
        let root = tempfile::tempdir().unwrap();
        let provider = AcpProvider::new(fixture(root.path(), "exit-desc")).unwrap();
        let events = tokio::time::timeout(
            Duration::from_secs(3),
            collect(
                provider
                    .stream(&messages(), &[], &StreamOptions::default())
                    .await
                    .unwrap(),
            ),
        )
        .await
        .expect("dead parent must not depend on inherited stdout EOF");
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::ResponseDone { clean: false, .. })),
            "{events:?}"
        );
        assert!(!format!("{events:?}").contains("do-not-leak"));
        assert_eq!(provider.response_chain_id().await, None);
        let receipt = std::fs::read_to_string(root.path().join("receipt")).unwrap();
        let descendant: i32 = receipt
            .lines()
            .find_map(|line| line.strip_prefix("descendant:"))
            .unwrap()
            .parse()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                // SAFETY: signal zero probes only this fixture child's liveness.
                if unsafe { libc::kill(descendant, 0) } == -1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("owned descendant must be reaped after group teardown");
    }

    #[tokio::test]
    async fn acp_clean_final_frame_survives_immediate_parent_exit() {
        let root = tempfile::tempdir().unwrap();
        let provider = AcpProvider::new(fixture(root.path(), "clean-exit")).unwrap();
        let events = collect(
            provider
                .stream(&messages(), &[], &StreamOptions::default())
                .await
                .unwrap(),
        )
        .await;
        assert!(
            events.contains(&AgentEvent::TextDone("answer".into())),
            "{events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::ResponseDone { clean: true, .. })),
            "{events:?}"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn native_settings_factory_uses_calling_session_registry_and_approval() {
        use codescribe_core::agent::{
            AgentSession, AgentUiEvent, ToolApprovalFuture, ToolApprovalRequest,
        };
        use codescribe_core::config::{AcpLaunchSettings, Config, UserSettings};
        use codescribe_core::test_isolation::EnvGuard;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        let _data = EnvGuard::set("CODESCRIBE_DATA_DIR", root_path.to_str().unwrap());
        let launch = fixture(&root_path, "denied-read").command;
        std::fs::write(root_path.join(".env"), "FIXTURE_SECRET=keep-private").unwrap();
        let settings = UserSettings {
            agent_acp: Some(AcpLaunchSettings {
                program: launch.program,
                args: launch.args,
                cwd: root_path.display().to_string(),
            }),
            agent_workspace_roots: Some(vec![root_path.display().to_string()]),
            ..Default::default()
        };
        settings.save().unwrap();
        let snapshot = Config::load_runtime_snapshot_without_keychain().unwrap();
        assert!(
            snapshot
                .llm_lanes()
                .assistive()
                .acp_configuration()
                .is_some()
        );
        let provider = super::super::create_agent_provider(&snapshot).unwrap();
        assert_eq!(provider.name(), "acp");
        assert!(
            provider
                .stream(&messages(), &[], &StreamOptions::default())
                .await
                .is_err(),
            "unbound native provider must not launch"
        );
        assert!(!root_path.join("receipt").exists());
        let calls = Arc::new(AtomicUsize::new(0));
        let approvals = calls.clone();
        let handler = Arc::new(move |request: ToolApprovalRequest| {
            assert_eq!(request.tool, "read_file");
            assert_eq!(request.thread_id, "native-thread");
            approvals.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { false }) as ToolApprovalFuture
        });
        let (tx, mut rx) = mpsc::channel(32);
        let tools = Arc::new(super::super::tools::configured_registry());
        let mut session =
            AgentSession::new(provider, tools, tx).with_tool_approval("native-thread", handler);
        tokio::time::timeout(
            Duration::from_secs(10),
            session.send("hello".into(), Vec::new(), &StreamOptions::default()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let mut approval_seen = false;
        while let Ok(event) = rx.try_recv() {
            approval_seen |= matches!(event, AgentUiEvent::ToolApprovalRequested(_));
            assert!(!format!("{event:?}").contains("keep-private"));
        }
        assert!(approval_seen);
        assert!(
            std::fs::read_to_string(root_path.join("receipt"))
                .unwrap()
                .contains("secret-refused")
        );
    }

    #[tokio::test]
    async fn acp_cancel_notifies_and_reaps_child() {
        let root = tempfile::tempdir().unwrap();
        let provider = AcpProvider::new(fixture(root.path(), "cancel")).unwrap();
        let rx = provider
            .stream(&messages(), &[], &StreamOptions::default())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let receipt =
                    std::fs::read_to_string(root.path().join("receipt")).unwrap_or_default();
                if receipt.contains("session/prompt") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        provider.cancel();
        let events = collect(rx).await;
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::ResponseDone { clean: false, .. }))
        );
        let receipt = std::fs::read_to_string(root.path().join("receipt")).unwrap();
        assert!(receipt.contains("cancel-seen"), "{receipt}");
        let pid = receipt
            .lines()
            .next()
            .unwrap()
            .strip_prefix("pid:")
            .unwrap();
        assert!(
            !std::process::Command::new("/bin/kill")
                .args(["-0", pid])
                .output()
                .unwrap()
                .status
                .success()
        );
    }
}
