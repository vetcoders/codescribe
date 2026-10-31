//! ACP client methods use the ordinary tool gate before touching the host.
//!
//! Files are opened without reading or truncating before consent and retained
//! across the approval wait. Consent therefore applies to the opened object,
//! rather than to a later interpretation of an agent-supplied path.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::Duration;

use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc, watch};
use tokio::task::JoinHandle;

use crate::agent::redaction::{CommandRedactor, REDACTED, redact_command_line, redacted_arguments};
use crate::agent::secret_paths::find_secret_path;
use crate::agent::{
    AgentPermissions, AgentUiEvent, ToolApprovalHandler, ToolCallPreview, ToolDecision,
    ToolDefinition, ToolExecutionPolicy, ToolRegistry, ToolRisk,
};

use super::{AcpError, kill_process_group};

/// Canonical registry identities of the host operations exposed over ACP.
pub const READ_TOOL: &str = "acp_read_text_file";
/// Canonical ACP file-write identity.
pub const WRITE_TOOL: &str = "acp_write_text_file";
/// Canonical ACP process-creation identity.
pub const TERMINAL_TOOL: &str = "acp_terminal_create";

const FILE_BYTE_LIMIT: u64 = 4 * 1024 * 1024;
const TERMINAL_BYTE_LIMIT: usize = 1024 * 1024;

/// Host-owned configuration, copied for each ACP session. No approval callback
/// means refusal, including when a user interface sender happens to exist.
#[derive(Clone)]
pub struct AcpClientBridgeConfig {
    pub registry: Arc<ToolRegistry>,
    pub workspace_roots: Vec<PathBuf>,
    pub approval_handler: Option<ToolApprovalHandler>,
    pub ui_tx: Option<mpsc::Sender<AgentUiEvent>>,
    pub approval_timeout: Duration,
    pub thread_id: String,
}

impl AcpClientBridgeConfig {
    /// Build the ACP host-operation catalog using the supplied policy snapshot.
    /// Callers with a larger existing registry may register the same operations
    /// with [`register_tools`] and supply that registry in the public fields.
    pub fn for_workspace(
        workspace_roots: Vec<PathBuf>,
        policy: AgentPermissions,
    ) -> Result<Self, AcpError> {
        let roots = canonical_roots(&workspace_roots)?;
        let mut registry = ToolRegistry::new();
        registry.set_policy(policy);
        register_tools(&mut registry, &roots)?;
        Ok(Self {
            registry: Arc::new(registry),
            workspace_roots: roots,
            approval_handler: None,
            ui_tx: None,
            approval_timeout: Duration::from_secs(90),
            thread_id: uuid::Uuid::new_v4().to_string(),
        })
    }
}

/// Register the three concrete host capabilities. Their handlers intentionally
/// do nothing: ACP dispatch is performed by this bridge after the same gate and
/// target confirmation the regular session uses. Names cannot shadow existing
/// tools because `register_with_policy` refuses duplicate registrations.
pub fn register_tools(registry: &mut ToolRegistry, roots: &[PathBuf]) -> Result<(), AcpError> {
    let roots = canonical_roots(roots)?;
    for (name, risk, field, writing) in [
        (READ_TOOL, ToolRisk::ReadOnly, "path", false),
        (WRITE_TOOL, ToolRisk::Mutating, "path", true),
        (TERMINAL_TOOL, ToolRisk::ProcessControl, "cwd", false),
    ] {
        let roots = roots.clone();
        let validator = Arc::new(move |input: &Value| {
            let raw = input[field]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("ACP operation requires an absolute {field}"))?;
            let checked = checked_path(Path::new(raw), &roots, writing)
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            Ok(ToolCallPreview {
                summary: format!("ACP {name}"),
                command: input["command"].as_str().map(str::to_owned),
                cwd: (field == "cwd").then(|| checked.canonical.display().to_string()),
                paths: vec![checked.canonical.display().to_string()],
            })
        });
        registry
            .register_with_policy(
                ToolDefinition {
                    name: name.to_string(),
                    description: format!("ACP host capability {name}"),
                    input_schema: json!({"type":"object"}),
                },
                Box::new(|_| Box::pin(async { Vec::new() })),
                ToolExecutionPolicy::native(risk),
                Some(validator),
            )
            .map_err(|error| AcpError::Protocol(error.to_string()))?;
    }
    Ok(())
}

/// One ACP session's host resources. `handle` borrows shared state so a
/// `terminal/wait_for_exit` call can run alongside `terminal/kill` and cancel.
pub struct AcpClientBridge {
    config: AcpClientBridgeConfig,
    session_id: Option<String>,
    terminals: Mutex<HashMap<String, Arc<Terminal>>>,
    closed: AtomicBool,
    root_dirs: HashMap<PathBuf, Dir>,
    native_session_id: Option<String>,
}

impl AcpClientBridge {
    pub fn new(mut config: AcpClientBridgeConfig) -> Result<Self, AcpError> {
        // The supplied grant roots are canonical, not symlink aliases whose
        // meaning may change between turns of a retained provider.
        for root in &config.workspace_roots {
            if root.canonicalize()? != *root {
                return Err(AcpError::PermissionDenied(
                    "ACP workspace root changed after selection".into(),
                ));
            }
        }
        config.workspace_roots = canonical_roots(&config.workspace_roots)?;
        let mut root_dirs = HashMap::new();
        for path in &config.workspace_roots {
            let expected = std::fs::metadata(path)?;
            let dir = Dir::open_ambient_dir(path, ambient_authority())?;
            if !same_file(&expected, &dir.try_clone()?.into_std_file().metadata()?) {
                return Err(AcpError::PermissionDenied(
                    "ACP workspace root changed while opening".into(),
                ));
            }
            root_dirs.insert(path.clone(), dir);
        }
        Ok(Self {
            config,
            session_id: None,
            terminals: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
            root_dirs,
            native_session_id: None,
        })
    }

    /// Use the exact calling AgentSession's registered native operations and
    /// approval identity. No ACP-specific permission or registry owner exists
    /// on this production route.
    pub fn bind_native_session(&mut self, session_id: String) {
        self.native_session_id = Some(session_id);
    }

    /// Pin the launch cwd through the same opened workspace capability as
    /// filesystem RPCs. A path swap after validation cannot redirect exec.
    pub fn open_launch_directory(&self, cwd: &Path) -> Result<std::fs::File, AcpError> {
        let checked = checked_path(cwd, &self.config.workspace_roots, false)?;
        let dir = self.root_dir(&checked)?.open_dir(&checked.relative)?;
        let file = dir.into_std_file();
        if !file.metadata()?.is_dir()
            || checked.metadata.as_ref().is_none_or(|expected| {
                file.metadata().map(|actual| !same_file(expected, &actual)).unwrap_or(true)
            })
        {
            return Err(AcpError::PermissionDenied("ACP launch cwd changed after selection".into()));
        }
        checked.confirm(&self.config.workspace_roots)?;
        Ok(file)
    }

    /// Bind before handling any client request. A peer cannot reuse a terminal
    /// or obtain a permission verdict for a different ACP session.
    pub fn set_session_id(&mut self, session_id: String) {
        self.session_id = Some(session_id);
    }

    pub fn session_id(&self) -> &str {
        self.session_id.as_deref().unwrap_or("")
    }

    pub async fn handle(&self, method: &str, params: Value) -> Result<Value, AcpError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(AcpError::Cancelled);
        }
        self.check_session(&params)?;
        match method {
            "session/request_permission" => self.request_permission(&params).await,
            "fs/read_text_file" => self.read_file(params).await,
            "fs/write_text_file" => self.write_file(params).await,
            "terminal/create" => self.create_terminal(params).await,
            "terminal/output" => {
                let terminal = self.terminal(&params).await?;
                let output = terminal
                    .output
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                Ok(json!({
                    "output": String::from_utf8_lossy(&output.bytes),
                    "truncated": output.truncated,
                    "exitStatus": terminal.exit.borrow().map(exit_status),
                }))
            }
            "terminal/wait_for_exit" => {
                let terminal = self.terminal(&params).await?;
                Ok(exit_status(terminal.wait().await?))
            }
            "terminal/kill" => {
                self.terminal(&params).await?.kill().await?;
                Ok(json!({}))
            }
            "terminal/release" => {
                let id = required_string(&params, "terminalId")?;
                let terminal = self
                    .terminals
                    .lock()
                    .await
                    .remove(id)
                    .ok_or_else(|| AcpError::Protocol("Unknown ACP terminal".into()))?;
                terminal.kill().await?;
                Ok(json!({}))
            }
            _ => Err(AcpError::Protocol("Unsupported ACP client method".into())),
        }
    }

    /// Kill and reap all commands before the provider releases its session.
    pub async fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        let terminals: Vec<_> = self
            .terminals
            .lock()
            .await
            .drain()
            .map(|(_, t)| t)
            .collect();
        for terminal in terminals {
            let _ = terminal.kill().await;
        }
    }

    fn check_session(&self, params: &Value) -> Result<(), AcpError> {
        if self
            .session_id
            .as_deref()
            .is_some_and(|id| params["sessionId"].as_str() == Some(id))
        {
            Ok(())
        } else {
            Err(AcpError::PermissionDenied(
                "ACP session identity mismatch".into(),
            ))
        }
    }

    async fn authorize(&self, name: &str, input: &Value, call_id: &str) -> Result<(), AcpError> {
        let acp_session = self
            .session_id
            .as_deref()
            .ok_or_else(|| AcpError::PermissionDenied("ACP session has not been bound".into()))?;
        let mut native_input = input.clone();
        let name = if self.native_session_id.is_some() {
            match name {
                READ_TOOL => "read_file",
                WRITE_TOOL => "write_file",
                TERMINAL_TOOL => {
                    // Protocol translation into run_process's actual schema;
                    // its validator still checks argv and command shape.
                    native_input["program"] = input["command"].clone();
                    "run_process"
                }
                registered => registered,
            }
        } else {
            name
        };
        let input = &native_input;
        let session = self.native_session_id.as_deref().unwrap_or(acp_session);
        let (decision, targets) = self.config.registry.decide_with_targets(
            name,
            input,
            call_id,
            session,
            &self.config.thread_id,
        );
        match decision {
            ToolDecision::Deny(reason) => {
                return Err(AcpError::PermissionDenied(redact_command_line(&reason)));
            }
            ToolDecision::RequireApproval(request) => {
                let mut request = *request;
                // A write to a credential file must not disclose that file's
                // replacement contents on the approval card itself.
                if find_secret_path(input).is_some() {
                    let mut preview = input.clone();
                    mask_content(&mut preview);
                    request.arguments = redacted_arguments(&preview);
                }
                let Some(handler) = &self.config.approval_handler else {
                    return Err(AcpError::PermissionDenied(
                        "ACP tool requires approval".into(),
                    ));
                };
                let Some(ui) = &self.config.ui_tx else {
                    return Err(AcpError::PermissionDenied(
                        "ACP approval UI is unavailable".into(),
                    ));
                };
                let approved = tokio::time::timeout(self.config.approval_timeout, async {
                    // Backpressure is part of the bounded approval wait.
                    if ui
                        .send(AgentUiEvent::ToolApprovalRequested(request.clone()))
                        .await
                        .is_err()
                    {
                        return false;
                    }
                    handler(request).await
                })
                .await
                .unwrap_or(false);
                if !approved {
                    return Err(AcpError::PermissionDenied(
                        "ACP tool approval rejected or timed out".into(),
                    ));
                }
            }
            ToolDecision::Allow => {}
        }
        self.config
            .registry
            .confirm_targets(name, input, &targets)
            .map_err(|_| {
                AcpError::PermissionDenied("ACP target changed after authorization".into())
            })
    }

    async fn request_permission(&self, params: &Value) -> Result<Value, AcpError> {
        let tool = &params["toolCall"];
        let mut input = tool.get("rawInput").cloned().unwrap_or_else(|| json!({}));
        if !input.is_object() {
            input = json!({"input": input});
        }
        let mut locations = Vec::new();
        let mut invalid_location =
            !check_input_paths(&mut input, &self.config.workspace_roots, &mut locations);
        if let Some(paths) = tool["locations"].as_array() {
            for path in paths {
                let checked = path["path"].as_str().and_then(|raw| {
                    checked_path(Path::new(raw), &self.config.workspace_roots, true).ok()
                });
                match checked {
                    Some(path) => locations.push(path),
                    None => invalid_location = true,
                }
            }
        }
        input["_acp_targets"] = json!(
            locations
                .iter()
                .map(|p| p.canonical.display().to_string())
                .collect::<Vec<_>>()
        );
        let name = tool["name"]
            .as_str()
            .and_then(|name| self.config.registry.resolve_tool_name(name));
        let call_id = tool["toolCallId"].as_str().unwrap_or("acp-permission");
        let allowed = if let Some(name) = name.filter(|_| !invalid_location) {
            self.authorize(&name, &input, call_id).await.is_ok()
                && locations
                    .iter()
                    .all(|path| path.confirm(&self.config.workspace_roots).is_ok())
        } else {
            false
        };
        let kind = if allowed { "allow_once" } else { "reject_once" };
        let option = params["options"].as_array().and_then(|options| {
            options.iter().find(|option| {
                option["kind"].as_str() == Some(kind) && option["optionId"].as_str().is_some()
            })
        });
        Ok(match option {
            Some(option) => json!({"outcome":{"outcome":"selected","optionId":option["optionId"]}}),
            None => json!({"outcome":{"outcome":"cancelled"}}),
        })
    }

    async fn read_file(&self, mut params: Value) -> Result<Value, AcpError> {
        let checked = checked_path(
            Path::new(required_string(&params, "path")?),
            &self.config.workspace_roots,
            false,
        )?;
        let mut file = checked.open_existing(self.root_dir(&checked)?, false)?;
        params["path"] = json!(checked.canonical);
        self.authorize(READ_TOOL, &params, &uuid::Uuid::new_v4().to_string())
            .await?;
        checked.confirm(&self.config.workspace_roots)?;
        checked.confirm_file(&file)?;
        let start = params
            .get("line")
            .map(|v| {
                v.as_u64()
                    .filter(|line| *line > 0)
                    .ok_or_else(|| AcpError::Protocol("ACP line must be a positive integer".into()))
            })
            .transpose()?
            .unwrap_or(1);
        let limit = params
            .get("limit")
            .map(|v| {
                v.as_u64().ok_or_else(|| {
                    AcpError::Protocol("ACP limit must be a non-negative integer".into())
                })
            })
            .transpose()?;
        let mut content = String::new();
        Read::by_ref(&mut file)
            .take(FILE_BYTE_LIMIT + 1)
            .read_to_string(&mut content)?;
        if content.len() as u64 > FILE_BYTE_LIMIT {
            return Err(AcpError::Protocol(
                "ACP text file exceeds the size limit".into(),
            ));
        }
        let content = if start == 1 && limit.is_none() {
            content
        } else {
            content
                .split_inclusive('\n')
                .skip(usize::try_from(start - 1).unwrap_or(usize::MAX))
                .take(
                    limit
                        .and_then(|value| usize::try_from(value).ok())
                        .unwrap_or(usize::MAX),
                )
                .collect()
        };
        Ok(json!({"content":content}))
    }

    async fn write_file(&self, mut params: Value) -> Result<Value, AcpError> {
        let checked = checked_path(
            Path::new(required_string(&params, "path")?),
            &self.config.workspace_roots,
            true,
        )?;
        let content = params["content"]
            .as_str()
            .ok_or_else(|| AcpError::Protocol("ACP write requires text content".into()))?
            .to_string();
        if content.len() as u64 > FILE_BYTE_LIMIT {
            return Err(AcpError::Protocol(
                "ACP text file exceeds the size limit".into(),
            ));
        }
        let root = self.root_dir(&checked)?;
        let mut existing = if checked.metadata.is_some() {
            Some(checked.open_existing(root, true)?)
        } else {
            None
        };
        let parent = root.open_dir(checked.relative.parent().unwrap_or_else(|| Path::new("")))?;
        let expected_parent = std::fs::metadata(
            checked
                .canonical
                .parent()
                .ok_or_else(|| AcpError::Protocol("ACP write has no parent".into()))?,
        )?;
        if !same_file(
            &expected_parent,
            &parent.try_clone()?.into_std_file().metadata()?,
        ) {
            return Err(AcpError::PermissionDenied(
                "ACP write parent changed while opening".into(),
            ));
        }
        params["path"] = json!(checked.canonical);
        self.authorize(WRITE_TOOL, &params, &uuid::Uuid::new_v4().to_string())
            .await?;
        checked.confirm(&self.config.workspace_roots)?;
        let mut file = if let Some(file) = existing.take() {
            checked.confirm_file(&file)?;
            file
        } else {
            // create_new cannot follow a symlink inserted after approval and
            // never truncates an existing, unapproved object.
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            parent
                .open_with(
                    checked
                        .relative
                        .file_name()
                        .ok_or_else(|| AcpError::Protocol("ACP write has no filename".into()))?,
                    &options,
                )?
                .into_std()
        };
        file.set_len(0)?;
        file.write_all(content.as_bytes())?;
        Ok(json!({}))
    }

    async fn create_terminal(&self, mut params: Value) -> Result<Value, AcpError> {
        let raw_cwd = params["cwd"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| self.config.workspace_roots[0].display().to_string());
        let checked = checked_path(Path::new(&raw_cwd), &self.config.workspace_roots, false)?;
        if !checked.canonical.is_dir() {
            return Err(AcpError::PermissionDenied(
                "ACP terminal cwd is not a directory".into(),
            ));
        }
        params["cwd"] = json!(checked.canonical);
        let command_name = required_string(&params, "command")?.to_string();
        let args = string_array(params.get("args"))?;
        let env = terminal_env(params.get("env"))?;
        self.authorize(TERMINAL_TOOL, &params, &uuid::Uuid::new_v4().to_string())
            .await?;
        checked.confirm(&self.config.workspace_roots)?;
        if self.closed.load(Ordering::Acquire) {
            return Err(AcpError::Cancelled);
        }
        let cwd_dir = self.root_dir(&checked)?.open_dir(&checked.relative)?;
        if checked.metadata.as_ref().is_none_or(|expected| {
            cwd_dir
                .try_clone()
                .and_then(|dir| dir.into_std_file().metadata())
                .map(|actual| !same_file(expected, &actual))
                .unwrap_or(true)
        }) {
            return Err(AcpError::PermissionDenied(
                "ACP terminal cwd changed after authorization".into(),
            ));
        }
        let mut command = Command::new(command_name);
        command
            .args(args)
            .current_dir(&checked.canonical)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .envs(env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(target_os = "macos")]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: fchdir is async-signal-safe, the closure owns the open
            // directory capability, and performs no allocation on success.
            unsafe {
                command.pre_exec(move || {
                    if libc::fchdir(cwd_dir.as_raw_fd()) == -1 {
                        Err(std::io::Error::last_os_error())
                    } else {
                        Ok(())
                    }
                });
            }
        }
        let child = command.spawn()?;
        let limit = params["outputByteLimit"]
            .as_u64()
            .and_then(|limit| usize::try_from(limit).ok())
            .unwrap_or(TERMINAL_BYTE_LIMIT)
            .min(TERMINAL_BYTE_LIMIT);
        let terminal = Arc::new(Terminal::new(child, limit));
        let id = uuid::Uuid::new_v4().to_string();
        let mut terminals = self.terminals.lock().await;
        if self.closed.load(Ordering::Acquire) {
            drop(terminals);
            terminal.kill().await?;
            return Err(AcpError::Cancelled);
        }
        terminals.insert(id.clone(), terminal);
        Ok(json!({"terminalId":id}))
    }

    async fn terminal(&self, params: &Value) -> Result<Arc<Terminal>, AcpError> {
        self.terminals
            .lock()
            .await
            .get(required_string(params, "terminalId")?)
            .cloned()
            .ok_or_else(|| AcpError::Protocol("Unknown ACP terminal".into()))
    }

    fn root_dir(&self, path: &CheckedPath) -> Result<&Dir, AcpError> {
        self.root_dirs.get(&path.root).ok_or_else(|| {
            AcpError::PermissionDenied("ACP workspace capability is unavailable".into())
        })
    }
}

impl Drop for AcpClientBridge {
    fn drop(&mut self) {
        // Drop holds exclusive access: aborting a pending method drops its Arc
        // before the provider drops the bridge. Terminal Drop is synchronous.
        self.terminals.get_mut().clear();
    }
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, AcpError> {
    value[field]
        .as_str()
        .filter(|text| !text.is_empty())
        .ok_or_else(|| AcpError::Protocol(format!("ACP requires {field}")))
}

fn canonical_roots(roots: &[PathBuf]) -> Result<Vec<PathBuf>, AcpError> {
    if roots.is_empty() {
        return Err(AcpError::PermissionDenied(
            "ACP requires an explicit workspace root".into(),
        ));
    }
    roots
        .iter()
        .map(|root| {
            let canonical = root.canonicalize()?;
            if !canonical.is_dir() {
                return Err(AcpError::PermissionDenied(
                    "ACP workspace root is not a directory".into(),
                ));
            }
            Ok(canonical)
        })
        .collect()
}

struct CheckedPath {
    requested: PathBuf,
    canonical: PathBuf,
    root: PathBuf,
    relative: PathBuf,
    metadata: Option<std::fs::Metadata>,
}

fn checked_path(path: &Path, roots: &[PathBuf], writing: bool) -> Result<CheckedPath, AcpError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(AcpError::PermissionDenied(
            "ACP paths must be absolute without '..'".into(),
        ));
    }
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => Some(metadata),
        Err(error) if writing && error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let canonical = if metadata.is_some() {
        path.canonicalize()?
    } else {
        let parent = path
            .parent()
            .ok_or_else(|| AcpError::PermissionDenied("ACP path has no parent".into()))?;
        // Refuse dangling symlinks: a missing destination is not a new file.
        if std::fs::symlink_metadata(path).is_ok() {
            return Err(AcpError::PermissionDenied(
                "ACP refuses a dangling symlink".into(),
            ));
        }
        parent.canonicalize()?.join(
            path.file_name()
                .ok_or_else(|| AcpError::PermissionDenied("ACP path has no filename".into()))?,
        )
    };
    let root = roots
        .iter()
        .find(|root| canonical.starts_with(root))
        .ok_or_else(|| AcpError::PermissionDenied("ACP path is outside workspace roots".into()))?
        .clone();
    let relative = canonical
        .strip_prefix(&root)
        .map_err(|_| AcpError::PermissionDenied("ACP path is outside workspace roots".into()))?
        .to_path_buf();
    Ok(CheckedPath {
        requested: path.to_path_buf(),
        canonical,
        root,
        relative,
        metadata,
    })
}

impl CheckedPath {
    fn confirm(&self, roots: &[PathBuf]) -> Result<(), AcpError> {
        let current = checked_path(&self.requested, roots, self.metadata.is_none())?;
        if current.canonical != self.canonical
            || current.metadata.is_some() != self.metadata.is_some()
        {
            return Err(AcpError::PermissionDenied(
                "ACP target changed after authorization".into(),
            ));
        }
        if let (Some(before), Some(after)) = (&self.metadata, &current.metadata)
            && !same_file(before, after)
        {
            return Err(AcpError::PermissionDenied(
                "ACP target changed after authorization".into(),
            ));
        }
        Ok(())
    }

    fn open_existing(&self, dir: &Dir, writing: bool) -> Result<std::fs::File, AcpError> {
        let mut options = OpenOptions::new();
        options.read(!writing).write(writing);
        let file = dir.open_with(&self.relative, &options)?.into_std();
        self.confirm_file(&file)?;
        if !file.metadata()?.is_file() {
            return Err(AcpError::PermissionDenied(
                "ACP target is not a regular file".into(),
            ));
        }
        Ok(file)
    }

    fn confirm_file(&self, file: &std::fs::File) -> Result<(), AcpError> {
        let actual = file.metadata()?;
        if self
            .metadata
            .as_ref()
            .is_none_or(|metadata| !same_file(metadata, &actual))
        {
            return Err(AcpError::PermissionDenied(
                "ACP opened object differs from approved target".into(),
            ));
        }
        Ok(())
    }
}

fn check_input_paths(input: &mut Value, roots: &[PathBuf], checked: &mut Vec<CheckedPath>) -> bool {
    match input {
        Value::Object(fields) => fields.iter_mut().all(|(key, value)| {
            if matches!(key.as_str(), "path" | "file_path" | "cwd" | "directory") {
                let Some(raw) = value.as_str() else {
                    return false;
                };
                match checked_path(Path::new(raw), roots, true) {
                    Ok(path) => {
                        *value = json!(path.canonical);
                        checked.push(path);
                        true
                    }
                    Err(_) => false,
                }
            } else {
                check_input_paths(value, roots, checked)
            }
        }),
        Value::Array(values) => values
            .iter_mut()
            .all(|value| check_input_paths(value, roots, checked)),
        _ => true,
    }
}

#[cfg(unix)]
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

fn mask_content(input: &mut Value) {
    match input {
        Value::Object(fields) => {
            for (key, value) in fields {
                if matches!(key.as_str(), "content" | "text" | "data") {
                    *value = json!(REDACTED);
                } else {
                    mask_content(value);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(mask_content),
        _ => {}
    }
}

fn string_array(input: Option<&Value>) -> Result<Vec<String>, AcpError> {
    match input {
        None => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| AcpError::Protocol("ACP args must contain only strings".into()))
            })
            .collect(),
        Some(_) => Err(AcpError::Protocol("ACP args must be an array".into())),
    }
}

fn terminal_env(input: Option<&Value>) -> Result<Vec<(String, String)>, AcpError> {
    let Some(input) = input else {
        return Ok(Vec::new());
    };
    let fields = input
        .as_array()
        .ok_or_else(|| AcpError::Protocol("ACP env must be an array".into()))?;
    fields
        .iter()
        .map(|field| {
            let name = required_string(field, "name")?;
            let value = field["value"]
                .as_str()
                .ok_or_else(|| AcpError::Protocol("ACP env requires string values".into()))?;
            if name.contains(['=', '\0'])
                || value.contains('\0')
                || name.starts_with("DYLD_")
                || name.starts_with("LD_")
                || matches!(
                    name,
                    "BASH_ENV" | "ENV" | "PYTHONPATH" | "PYTHONHOME" | "PERL5OPT" | "RUBYOPT"
                )
            {
                return Err(AcpError::PermissionDenied(
                    "ACP refuses process-loader environment overrides".into(),
                ));
            }
            Ok((name.to_string(), value.to_string()))
        })
        .collect()
}

#[derive(Default)]
struct OutputBuffer {
    bytes: Vec<u8>,
    truncated: bool,
    limit: usize,
}

impl OutputBuffer {
    fn push(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
        if self.bytes.len() > self.limit {
            let mut drop = self.bytes.len() - self.limit;
            while drop < self.bytes.len() && self.bytes[drop] & 0xc0 == 0x80 {
                drop += 1;
            }
            self.bytes.drain(..drop);
            self.truncated = true;
        }
    }
}

struct Terminal {
    kill: watch::Sender<bool>,
    exit: watch::Receiver<Option<ExitStatus>>,
    output: Arc<StdMutex<OutputBuffer>>,
    task: JoinHandle<()>,
    readers: Vec<JoinHandle<()>>,
    pid: Option<u32>,
}

impl Terminal {
    fn new(mut child: Child, limit: usize) -> Self {
        let pid = child.id();
        let output = Arc::new(StdMutex::new(OutputBuffer {
            limit,
            ..Default::default()
        }));
        let mut readers = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            readers.push(read_output(stdout, output.clone()));
        }
        if let Some(stderr) = child.stderr.take() {
            readers.push(read_output(stderr, output.clone()));
        }
        let (kill, mut killed) = watch::channel(false);
        let (exit_tx, exit) = watch::channel(None);
        let task = tokio::spawn(async move {
            let status = tokio::select! {
                status = child.wait() => status,
                _ = killed.changed() => {
                    kill_process_group(pid);
                    let _ = child.start_kill();
                    child.wait().await
                }
            };
            // Reap first, then close any descendants still holding its pipes.
            kill_process_group(pid);
            if let Ok(status) = status {
                let _ = exit_tx.send(Some(status));
            }
        });
        Self {
            kill,
            exit,
            output,
            task,
            readers,
            pid,
        }
    }

    async fn wait(&self) -> Result<ExitStatus, AcpError> {
        let mut exit = self.exit.clone();
        loop {
            if let Some(status) = *exit.borrow_and_update() {
                return Ok(status);
            }
            exit.changed()
                .await
                .map_err(|_| AcpError::ProcessExited("ACP terminal monitor stopped".into()))?;
        }
    }

    async fn kill(&self) -> Result<(), AcpError> {
        if self.exit.borrow().is_none() {
            kill_process_group(self.pid);
            let _ = self.kill.send(true);
        }
        self.wait().await.map(|_| ())
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if self.exit.borrow().is_none() {
            kill_process_group(self.pid);
            let _ = self.kill.send(true);
        }
        self.task.abort();
        for reader in &self.readers {
            reader.abort();
        }
    }
}

fn read_output<R: AsyncRead + Unpin + Send + 'static>(
    mut reader: R,
    output: Arc<StdMutex<OutputBuffer>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buffer = [0; 4096];
        let mut token = Vec::new();
        let mut overlong = false;
        let mut redactor = CommandRedactor::new();
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    for byte in &buffer[..count] {
                        if byte.is_ascii_whitespace() {
                            let mut complete = if overlong {
                                REDACTED.to_string()
                            } else {
                                String::from_utf8_lossy(&token).into_owned()
                            };
                            complete.push(char::from(*byte));
                            let redacted = redactor.redact(&complete);
                            output
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .push(redacted.as_bytes());
                            token.clear();
                            overlong = false;
                        } else if !overlong {
                            if token.len() == 4096 {
                                token.clear();
                                overlong = true;
                            } else {
                                token.push(*byte);
                            }
                        }
                    }
                }
            }
        }
        if !token.is_empty() || overlong {
            let complete = if overlong {
                REDACTED.to_string()
            } else {
                String::from_utf8_lossy(&token).into_owned()
            };
            let redacted = redactor.redact(&complete);
            output
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(redacted.as_bytes());
        }
    })
}

fn exit_status(status: ExitStatus) -> Value {
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.signal().map(|signal| signal.to_string())
    };
    #[cfg(not(unix))]
    let signal: Option<String> = None;
    json!({"exitCode":status.code(),"signal":signal})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::PermissionLevel;

    fn bridge(root: &Path, policy: AgentPermissions) -> AcpClientBridge {
        let config =
            AcpClientBridgeConfig::for_workspace(vec![root.to_path_buf()], policy).unwrap();
        let mut bridge = AcpClientBridge::new(config).unwrap();
        bridge.set_session_id("session-1".into());
        bridge
    }

    fn params(path: &Path) -> Value {
        json!({"sessionId":"session-1","path":path})
    }

    #[tokio::test]
    async fn acp_fs_read_is_scoped_and_preserves_line_endings() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("notes.txt");
        std::fs::write(&file, "first\nsecond\nthird").unwrap();
        let bridge = bridge(root.path(), AgentPermissions::default());
        let mut input = params(&file);
        input["line"] = json!(2);
        input["limit"] = json!(1);
        assert_eq!(
            bridge.handle("fs/read_text_file", input).await.unwrap()["content"],
            "second\n"
        );
        assert!(
            bridge
                .handle("fs/read_text_file", params(&root.path().join("../outside")))
                .await
                .is_err()
        );
        let outside = tempfile::NamedTempFile::new().unwrap();
        assert!(
            bridge
                .handle("fs/read_text_file", params(outside.path()))
                .await
                .is_err()
        );
        let mut wrong_session = params(&file);
        wrong_session["sessionId"] = json!("session-2");
        assert!(
            bridge
                .handle("fs/read_text_file", wrong_session)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn acp_secret_read_rejects_and_timeout_never_returns_contents() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join(".env");
        std::fs::write(&file, "TOP_SECRET_VALUE").unwrap();
        let mut config = AcpClientBridgeConfig::for_workspace(
            vec![root.path().into()],
            AgentPermissions::default(),
        )
        .unwrap();
        let (ui_tx, mut ui_rx) = mpsc::channel(8);
        config.ui_tx = Some(ui_tx);
        config.approval_timeout = Duration::from_millis(10);
        config.approval_handler = Some(Arc::new(|_| Box::pin(std::future::pending())));
        let mut bridge = AcpClientBridge::new(config).unwrap();
        bridge.set_session_id("session-1".into());
        let error = bridge
            .handle("fs/read_text_file", params(&file))
            .await
            .unwrap_err();
        assert!(matches!(error, AcpError::PermissionDenied(_)));
        assert!(!error.to_string().contains("TOP_SECRET_VALUE"));
        let Some(AgentUiEvent::ToolApprovalRequested(request)) = ui_rx.recv().await else {
            panic!("approval event")
        };
        assert!(request.paths.iter().any(|path| path.ends_with("/.env")));
        assert_eq!(std::fs::read_to_string(file).unwrap(), "TOP_SECRET_VALUE");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn acp_symlink_escape_and_secret_alias_cannot_read_without_approval() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("outside.txt");
        std::fs::write(&outside_file, "outside secret").unwrap();
        let escaped = root.path().join("escaped.txt");
        symlink(&outside_file, &escaped).unwrap();
        let secret = root.path().join(".env");
        std::fs::write(&secret, "inside secret").unwrap();
        let alias = root.path().join("innocent.txt");
        symlink(&secret, &alias).unwrap();
        let bridge = bridge(root.path(), AgentPermissions::default());
        for path in [&escaped, &alias] {
            assert!(
                bridge
                    .handle("fs/read_text_file", params(path))
                    .await
                    .is_err()
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn acp_target_repointed_during_approval_is_refused() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("first.txt");
        let second = root.path().join("second.txt");
        let alias = root.path().join("alias.txt");
        std::fs::write(&file, "first").unwrap();
        std::fs::write(&second, "second").unwrap();
        symlink(&file, &alias).unwrap();
        let mut policy = AgentPermissions::default();
        policy.read_only_default = PermissionLevel::Ask;
        let mut config =
            AcpClientBridgeConfig::for_workspace(vec![root.path().into()], policy).unwrap();
        let (ui_tx, _ui_rx) = mpsc::channel(8);
        config.ui_tx = Some(ui_tx);
        let alias_copy = alias.clone();
        config.approval_handler = Some(Arc::new(move |_| {
            let alias = alias_copy.clone();
            let second = second.clone();
            Box::pin(async move {
                std::fs::remove_file(&alias).unwrap();
                symlink(second, alias).unwrap();
                true
            })
        }));
        let mut bridge = AcpClientBridge::new(config).unwrap();
        bridge.set_session_id("session-1".into());
        assert!(
            bridge
                .handle("fs/read_text_file", params(&alias))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn acp_denied_write_does_not_truncate_and_allowed_write_creates_empty_file() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("notes.txt");
        std::fs::write(&file, "keep this").unwrap();
        let denied = bridge(root.path(), AgentPermissions::default());
        let mut input = params(&file);
        input["content"] = json!("replacement");
        assert!(denied.handle("fs/write_text_file", input).await.is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep this");
        let policy = AgentPermissions {
            side_effect_default: PermissionLevel::Allow,
            ..Default::default()
        };
        let allowed = bridge(root.path(), policy);
        let new_file = root.path().join("new.txt");
        let mut input = params(&new_file);
        input["content"] = json!("");
        allowed.handle("fs/write_text_file", input).await.unwrap();
        assert_eq!(std::fs::read_to_string(new_file).unwrap(), "");
    }

    #[tokio::test]
    async fn acp_permission_selects_offered_ids_and_unknown_names_refuse() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("notes.txt");
        std::fs::write(&file, "hello").unwrap();
        let bridge = bridge(root.path(), AgentPermissions::default());
        let mut request = json!({"sessionId":"session-1","toolCall":{
            "toolCallId":"tool-1","name":READ_TOOL,"rawInput":{"path":file}},
            "options":[{"kind":"allow_once","optionId":"yes-id"},{"kind":"reject_once","optionId":"no-id"}]});
        assert_eq!(
            bridge
                .handle("session/request_permission", request.clone())
                .await
                .unwrap()["outcome"]["optionId"],
            "yes-id"
        );
        request["toolCall"]["name"] = json!("unknown_read_name");
        request["toolCall"]["kind"] = json!("read");
        assert_eq!(
            bridge
                .handle("session/request_permission", request.clone())
                .await
                .unwrap()["outcome"]["optionId"],
            "no-id"
        );
        request["options"] = json!([{ "kind":"allow_always", "optionId":"permanent" }]);
        assert_eq!(
            bridge
                .handle("session/request_permission", request)
                .await
                .unwrap()["outcome"]["outcome"],
            "cancelled"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn acp_terminal_wait_does_not_block_kill_and_output_is_redacted() {
        let root = tempfile::tempdir().unwrap();
        let policy = AgentPermissions {
            side_effect_default: PermissionLevel::Allow,
            ..Default::default()
        };
        let bridge = Arc::new(bridge(root.path(), policy));
        let created = bridge
            .handle(
                "terminal/create",
                json!({"sessionId":"session-1","command":"/bin/sh",
            "args":["-c", "printf 'TOKEN=supersecret visible\\n'; sleep 30"],"cwd":root.path()}),
            )
            .await
            .unwrap();
        let input = json!({"sessionId":"session-1","terminalId":created["terminalId"]});
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let output = bridge
                    .handle("terminal/output", input.clone())
                    .await
                    .unwrap();
                let output = output["output"].as_str().unwrap();
                assert!(!output.contains("supersecret"));
                if output.contains("visible") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let waiter = {
            let bridge = bridge.clone();
            let input = input.clone();
            tokio::spawn(async move { bridge.handle("terminal/wait_for_exit", input).await })
        };
        bridge.handle("terminal/kill", input.clone()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), waiter)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        bridge
            .handle("terminal/release", input.clone())
            .await
            .unwrap();
        assert!(bridge.handle("terminal/output", input).await.is_err());
        bridge.shutdown().await;
        assert!(
            bridge
                .handle(
                    "terminal/create",
                    json!({"sessionId":"session-1","command":"/bin/true"})
                )
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn acp_output_redaction_carries_split_tokens_and_never_buffers_raw_secrets() {
        let (mut writer, reader) = tokio::io::duplex(32);
        let output = Arc::new(StdMutex::new(OutputBuffer {
            limit: 64,
            ..Default::default()
        }));
        let task = read_output(reader, output.clone());
        use tokio::io::AsyncWriteExt;
        writer.write_all(b"TO").await.unwrap();
        writer.write_all(b"KEN=sec").await.unwrap();
        writer.write_all(b"ret visible\n").await.unwrap();
        drop(writer);
        task.await.unwrap();
        let output = output.lock().unwrap();
        let text = String::from_utf8_lossy(&output.bytes);
        assert!(text.contains("[REDACTED]") && text.contains("visible"));
        assert!(!text.contains("secret"));
    }
}
