//! Bounded process execution (canonical: process.run / observe / stop).
//!
//! No shell string concatenation — argv only. Terminal path policy from
//! [`super::path_policy`] is applied to the synthetic command line for program
//! and path gates. Mutating/process tools default to Ask. What the approval
//! card shows of that command line passes through the shared
//! [`redact_command_line`] first, so a secret in the argv never becomes
//! operator-visible text.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use codescribe_core::agent::{
    ToolCallPreview, ToolDefinition, ToolExecutionPolicy, ToolInputValidator, ToolRegistry,
    ToolResultContent, ToolRisk, redact_command_line,
};
use serde_json::{Value, json};

use super::{output_guard, path_policy, workspace};

/// Wall-clock timeout applied when the caller does not specify one.
const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// Ceiling for a caller-supplied timeout, so no single tool call can pin a
/// process indefinitely.
const MAX_TIMEOUT_SECS: u64 = 300;
/// Git configuration that names a program to run, pinned to an inert value
/// before the model's own arguments. Hooks stay: a commit through this tool
/// meets the repository's gates like any other commit.
const GIT_CONFIG_PINS: &[&str] = &[
    "--no-pager",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.sshCommand=ssh",
    "-c",
    "core.askPass=",
    "-c",
    "core.editor=true",
    "-c",
    "sequence.editor=true",
    "-c",
    "gpg.program=gpg",
    "-c",
    "gpg.ssh.program=ssh-keygen",
    "-c",
    "log.showSignature=false",
    "-c",
    "protocol.ext.allow=never",
];
/// Subcommands that take diff options: external diff drivers and textconv
/// filters are programs from configuration and attributes, switched off here.
const GIT_DIFF_COMMANDS: &[&str] = &[
    "diff",
    "log",
    "show",
    "diff-tree",
    "diff-files",
    "diff-index",
    "whatchanged",
    "format-patch",
];

/// Environment keys re-exported into agent-spawned processes after `env_clear`.
///
/// Explicit allowlist only — never inherit the parent Codescribe process env,
/// which holds LLM keys, account tokens, and other secrets (PR-68 S-P0).
const CHILD_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "TERM",
    "TMPDIR",
    "TMP",
    "TEMP",
    "SHELL",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "CARGO_TARGET_DIR",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_RUNTIME_DIR",
];

/// A background child retained so `observe_process` / `stop_process` can reach
/// it after `run_process` returned.
struct TrackedProcess {
    /// Live handle; owning it is what keeps the pid meaningful.
    child: Child,
    /// Spawn instant, reported as `elapsed_ms` while running.
    started: Instant,
    /// Synthetic command line, echoed back for identification.
    command: String,
    /// Validated working directory the child was spawned in.
    cwd: PathBuf,
}

/// Process-wide table of background children, keyed by pid.
///
/// Only pids started through `run_process` are addressable — observe and stop
/// refuse anything else, so the agent cannot signal arbitrary processes.
fn process_table() -> &'static Mutex<HashMap<u32, TrackedProcess>> {
    /// OnceLock process table: only agent-spawned pids are trackable.
    static TABLE: OnceLock<Mutex<HashMap<u32, TrackedProcess>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register the process tool family. Everything that can start or signal a
/// process is [`ToolRisk::ProcessControl`]; only observation is read-only.
///
/// Every tool here carries a validator, so the approval card names the program,
/// the argument vector and the resolved working directory before the operator
/// answers — a process call is never only a tool name and a risk tier.
pub fn register(registry: &mut ToolRegistry) {
    registry
        .register_with_policy(
            run_process_definition(),
            Box::new(|input| Box::pin(async move { handle_run_process(input) })),
            ToolExecutionPolicy::native(ToolRisk::ProcessControl),
            Some(run_process_validator()),
        )
        .expect("register run_process");
    registry
        .register_with_policy(
            observe_process_definition(),
            Box::new(|input| Box::pin(async move { handle_observe_process(input) })),
            ToolExecutionPolicy::native(ToolRisk::ReadOnly),
            Some(tracked_pid_validator("Check")),
        )
        .expect("register observe_process");
    registry
        .register_with_policy(
            stop_process_definition(),
            Box::new(|input| Box::pin(async move { handle_stop_process(input) })),
            ToolExecutionPolicy::native(ToolRisk::ProcessControl),
            Some(tracked_pid_validator("Stop")),
        )
        .expect("register stop_process");
    registry
        .register_with_policy(
            project_build_definition(),
            Box::new(|input| Box::pin(async move { handle_project_build(input) })),
            ToolExecutionPolicy::native(ToolRisk::ProcessControl),
            Some(project_validator("build")),
        )
        .expect("register project_build");
    registry
        .register_with_policy(
            project_test_definition(),
            Box::new(|input| Box::pin(async move { handle_project_test(input) })),
            ToolExecutionPolicy::native(ToolRisk::ProcessControl),
            Some(project_validator("test")),
        )
        .expect("register project_test");
}

/// Pre-flight validator for `run_process`: resolves the call exactly as
/// [`run_process_from_input`] will, and hands the gate the program, the full
/// argument vector and the canonical cwd it resolved them against.
///
/// The resolved cwd travels as the call's target, so a directory symlink
/// re-pointed while the operator was deciding invalidates the approval through
/// the registry's own `confirm_targets` check rather than being followed.
fn run_process_validator() -> ToolInputValidator {
    Arc::new(|input: &Value| {
        let program = require_program(input)?;
        let args = requested_args(input);
        let cwd = resolve_cwd(require_cwd(input)?)?;
        let cmdline = synthetic_command_line(program, &args);
        path_policy::validate_terminal(&cmdline, cwd.to_str().context("cwd utf-8")?, &roots())?;
        Ok(process_preview(
            format!("Run {program} with {} argument(s)", args.len()),
            cmdline,
            cwd,
        ))
    })
}

/// Pre-flight validator for `project_build` / `project_test`: runs the same
/// build-system detection the handler runs, so the card names the exact command
/// the operator is approving instead of "a build".
///
/// The call's targets are the resolved cwd *and* the build file the detection
/// decided on. Binding only the directory would let a `Cargo.toml` dropped
/// into it while the card was open silently promote an approved `make test`
/// to `cargo test`; with the marker in `paths`, the re-detection at dispatch
/// no longer matches what was authorized and `confirm_targets` refuses.
fn project_validator(kind: &'static str) -> ToolInputValidator {
    Arc::new(move |input: &Value| {
        let cwd = resolve_cwd(require_cwd(input)?)?;
        let (program, args, marker) = detect_project_command(&cwd, kind)?;
        let mut preview = process_preview(
            format!("Project {kind} in the workspace"),
            synthetic_command_line(program, &args),
            cwd,
        );
        preview.paths.push(marker.display().to_string());
        Ok(preview)
    })
}

/// Pre-flight validator for `observe_process` / `stop_process`: a bare pid says
/// nothing, so the card recalls the command and cwd that pid was started with.
///
/// A pid the agent never started is refused here, before any approval is asked
/// for — the same rule the handlers enforce.
fn tracked_pid_validator(verb: &'static str) -> ToolInputValidator {
    Arc::new(move |input: &Value| {
        let pid = require_pid(input)?;
        let table = process_table().lock().expect("process table");
        let Some(tracked) = table.get(&pid) else {
            bail!("Unknown process pid {pid} (not started via run_process)");
        };
        Ok(process_preview(
            format!("{verb} process {pid} started by this agent"),
            tracked.command.clone(),
            tracked.cwd.clone(),
        ))
    })
}

/// The single shape every process-tool preview takes: a one-line summary, the
/// synthetic command line, and the resolved cwd carried both as the displayed
/// directory and as the call's authorization target.
///
/// The command line goes through the shared [`redact_command_line`] before it
/// is shown: an argv like `["--token", "abc"]` joins into a line that would
/// otherwise put the secret on the card verbatim. Display only — the argv
/// actually spawned is untouched.
fn process_preview(summary: String, command: String, cwd: PathBuf) -> ToolCallPreview {
    let cwd = cwd.display().to_string();
    ToolCallPreview {
        summary,
        command: Some(redact_command_line(&command)),
        cwd: Some(cwd.clone()),
        paths: vec![cwd],
    }
}

/// The required `program` field of a `run_process` call.
fn require_program(input: &Value) -> Result<&str> {
    input
        .get("program")
        .and_then(Value::as_str)
        .context("Missing required string field 'program'")
}

/// The required `cwd` field shared by the process and project tools.
fn require_cwd(input: &Value) -> Result<&str> {
    input
        .get("cwd")
        .and_then(Value::as_str)
        .context("Missing required string field 'cwd'")
}

/// The required `pid` field of `observe_process` / `stop_process`.
fn require_pid(input: &Value) -> Result<u32> {
    let pid = input
        .get("pid")
        .and_then(Value::as_u64)
        .context("Missing required integer field 'pid'")?;
    u32::try_from(pid).context("pid out of range")
}

/// The optional `args` array of a `run_process` call, as owned strings.
fn requested_args(input: &Value) -> Vec<String> {
    input
        .get("args")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Wire schema for `run_process` (capability `process.run`).
fn run_process_definition() -> ToolDefinition {
    ToolDefinition {
        name: "run_process".to_string(),
        description: "Run a single program with argv inside a workspace cwd (canonical: process.run). No shell. Requires approval."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "program": { "type": "string", "description": "Program name or absolute path" },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Argument vector (not shell-interpreted)"
                },
                "cwd": { "type": "string", "description": "Absolute working directory inside workspace" },
                "timeout_secs": { "type": "integer", "description": "Wall-clock timeout (default 60, max 300)" },
                "background": { "type": "boolean", "description": "When true, start and return pid for observe/stop" }
            },
            "required": ["program", "cwd"]
        }),
    }
}

/// Wire schema for `observe_process` (capability `process.observe`).
fn observe_process_definition() -> ToolDefinition {
    ToolDefinition {
        name: "observe_process".to_string(),
        description:
            "Check a background process started via run_process (canonical: process.observe)."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "pid": { "type": "integer" }
            },
            "required": ["pid"]
        }),
    }
}

/// Wire schema for `stop_process` (capability `process.stop`).
fn stop_process_definition() -> ToolDefinition {
    ToolDefinition {
        name: "stop_process".to_string(),
        description: "Stop a background process started via run_process (canonical: process.stop)."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "pid": { "type": "integer" }
            },
            "required": ["pid"]
        }),
    }
}

/// Wire schema for `project_build` (capability `project.build`).
fn project_build_definition() -> ToolDefinition {
    ToolDefinition {
        name: "project_build".to_string(),
        description:
            "Build a project under workspace roots (canonical: project.build). Detects cargo/make."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" }
            },
            "required": ["cwd"]
        }),
    }
}

/// Wire schema for `project_test` (capability `project.test`).
fn project_test_definition() -> ToolDefinition {
    ToolDefinition {
        name: "project_test".to_string(),
        description:
            "Run project tests under workspace roots (canonical: project.test). Detects cargo/make."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "cwd": { "type": "string" }
            },
            "required": ["cwd"]
        }),
    }
}

/// Tool-result adapter for [`run_process_from_input`]; policy rejections come
/// back to the model as errors rather than aborting the turn.
fn handle_run_process(input: Value) -> Vec<ToolResultContent> {
    match run_process_from_input(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// Tool-result adapter for [`observe_from_input`].
fn handle_observe_process(input: Value) -> Vec<ToolResultContent> {
    match observe_from_input(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// Tool-result adapter for [`stop_from_input`].
fn handle_stop_process(input: Value) -> Vec<ToolResultContent> {
    match stop_from_input(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// Tool-result adapter running the detected build command.
fn handle_project_build(input: Value) -> Vec<ToolResultContent> {
    match project_action(&input, "build") {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// Tool-result adapter running the detected test command.
fn handle_project_test(input: Value) -> Vec<ToolResultContent> {
    match project_action(&input, "test") {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// Canonicalized workspace roots bounding every cwd this module accepts.
fn roots() -> Vec<PathBuf> {
    path_policy::canonical_roots(&workspace::resolved_roots())
}

/// Validate a caller-supplied cwd against workspace roots and require that it
/// actually be a directory.
fn resolve_cwd(cwd: &str) -> Result<PathBuf> {
    let cwd = path_policy::validate_existing(cwd, &roots())?;
    if !cwd.is_dir() {
        bail!("cwd is not a directory: {}", cwd.display());
    }
    Ok(cwd)
}

/// Join program and argv into a display string for the terminal path policy.
///
/// Never executed — the spawn stays argv-only. This exists so the policy can
/// reason about shell operators and forbidden programs in one text form.
fn synthetic_command_line(program: &str, args: &[String]) -> String {
    let mut parts = vec![program.to_string()];
    parts.extend(args.iter().cloned());
    parts.join(" ")
}

/// Drop every inherited env var, then re-apply a small allowlist of non-secret
/// OS/build vars. Prevents `printenv` / child tools from exfiltrating
/// `LLM_*`, account tokens, and other process-seeded secrets.
fn apply_sanitized_child_env(command: &mut Command) {
    command.env_clear();
    for key in CHILD_ENV_ALLOWLIST {
        if let Ok(value) = std::env::var(key) {
            command.env(key, value);
        }
    }
}

/// Spawn one program with argv inside a validated workspace cwd.
///
/// Foreground runs return captured output bounded by the output guard;
/// background runs return a pid registered in [`process_table`]. The child env
/// is sanitized to the allowlist before either path.
fn run_process_from_input(input: &Value) -> Result<String> {
    let program = require_program(input)?;
    let args = requested_args(input);
    let cwd = resolve_cwd(require_cwd(input)?)?;
    let timeout_secs = input
        .get("timeout_secs")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(1, MAX_TIMEOUT_SECS);
    let background = input
        .get("background")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let cmdline = synthetic_command_line(program, &args);
    path_policy::validate_terminal(&cmdline, cwd.to_str().context("cwd utf-8")?, &roots())?;

    // Argv-only spawn: program is re-materialized as an OsString after terminal
    // policy rejects shell metacharacters / forbidden programs. No shell, no
    // string concatenation into sh -c.
    let program_os = sanitize_argv_program(program)?;
    let args_os: Vec<OsString> = args
        .iter()
        .map(|arg| sanitize_argv_arg(arg))
        .collect::<Result<_>>()?;
    // nosemgrep: rust.actix.command-injection.rust-actix-command-injection.rust-actix-command-injection -- argv-only spawn after path_policy::validate_terminal (blocks shell operators + forbidden programs) and sanitize_argv_program (rejects control/metacharacters / parent-dir components).
    let mut command = Command::new(&program_os);
    let git = path_policy::is_git_program(program);
    if git {
        // Git runs programs named in configuration, which the argv profile
        // never sees: pagers, SSH wrappers, editors, fsmonitor daemons,
        // signature helpers, external diff and textconv drivers, `ext::`
        // transports. Pin them before the model's arguments; the profile
        // refuses a model-supplied `-c`, so these are the last word.
        command.args(GIT_CONFIG_PINS);
        match args.iter().position(|arg| !arg.starts_with('-')) {
            Some(subcommand) => {
                command.args(&args_os[..=subcommand]);
                if GIT_DIFF_COMMANDS.contains(&args[subcommand].as_str()) {
                    command.args(["--no-ext-diff", "--no-textconv"]);
                }
                command.args(&args_os[subcommand + 1..]);
            }
            None => {
                command.args(&args_os);
            }
        }
    } else {
        command.args(&args_os);
    }
    command
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_sanitized_child_env(&mut command);
    if git {
        command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat");
    }

    if background {
        let child = command
            .spawn()
            .with_context(|| format!("Failed to spawn background process: {program}"))?;
        let pid = child.id();
        process_table().lock().expect("process table").insert(
            pid,
            TrackedProcess {
                child,
                started: Instant::now(),
                command: cmdline.clone(),
                cwd: cwd.clone(),
            },
        );
        return Ok(json!({
            "ok": true,
            "background": true,
            "pid": pid,
            "command": cmdline,
            "cwd": cwd.display().to_string(),
            "provider": "native",
            "capability": "process.run",
        })
        .to_string());
    }

    let output = run_with_timeout(command, Duration::from_secs(timeout_secs))?;
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    if !output.stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    let text = output_guard::guard_chunk(&text, &format!("process `{cmdline}`"));
    Ok(json!({
        "ok": output.status.success(),
        "exit_code": output.status.code(),
        "command": cmdline,
        "cwd": cwd.display().to_string(),
        "output": text,
        "provider": "native",
        "capability": "process.run",
    })
    .to_string())
}

/// Run a command to completion or kill it at `timeout`.
///
/// stdout and stderr are drained on dedicated threads: a child that fills a
/// pipe buffer would otherwise block forever while the poll loop waits on a
/// process that is itself waiting on the reader.
fn run_with_timeout(mut command: Command, timeout: Duration) -> Result<std::process::Output> {
    use std::io::Read;

    let mut child = command.spawn().context("Failed to spawn process")?;
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let stdout_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = stdout_pipe.take() {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = stderr_pipe.take() {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("Process exceeded timeout of {}s", timeout.as_secs());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => bail!("Failed to poll process: {error}"),
        }
    };

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// Report whether a tracked background process is still running.
///
/// An exited process is reported once with its exit code and then dropped from
/// the table, so the pid cannot be observed or stopped afterwards.
fn observe_from_input(input: &Value) -> Result<String> {
    let pid = require_pid(input)?;
    let mut table = process_table().lock().expect("process table");
    let Some(tracked) = table.get_mut(&pid) else {
        bail!("Unknown process pid {pid} (not started via run_process)");
    };
    match tracked.child.try_wait() {
        Ok(Some(status)) => {
            let command = tracked.command.clone();
            let cwd = tracked.cwd.display().to_string();
            table.remove(&pid);
            Ok(json!({
                "pid": pid,
                "running": false,
                "exit_code": status.code(),
                "command": command,
                "cwd": cwd,
                "provider": "native",
                "capability": "process.observe",
            })
            .to_string())
        }
        Ok(None) => Ok(json!({
            "pid": pid,
            "running": true,
            "elapsed_ms": tracked.started.elapsed().as_millis() as u64,
            "command": tracked.command,
            "cwd": tracked.cwd.display().to_string(),
            "provider": "native",
            "capability": "process.observe",
        })
        .to_string()),
        Err(error) => bail!("Failed to observe pid {pid}: {error}"),
    }
}

/// Kill a tracked background process and reap it, removing it from the table.
fn stop_from_input(input: &Value) -> Result<String> {
    let pid = require_pid(input)?;
    let mut table = process_table().lock().expect("process table");
    let Some(mut tracked) = table.remove(&pid) else {
        bail!("Unknown process pid {pid} (not started via run_process)");
    };
    let _ = tracked.child.kill();
    let status = tracked.child.wait().ok();
    Ok(json!({
        "ok": true,
        "pid": pid,
        "exit_code": status.and_then(|s| s.code()),
        "provider": "native",
        "capability": "process.stop",
    })
    .to_string())
}

/// The build or test command for `cwd`, detected from what is on disk, plus
/// the build file the detection rests on.
///
/// Cargo wins over Make when both are present; neither present is an error
/// rather than a guess. One throne: the approval card and the spawn read the
/// same answer, so the operator cannot be shown a command the handler then
/// replaces.
///
/// The marker travels back because detection is a function of the directory's
/// *contents*, not its path: a `Cargo.toml` appearing while the card is open
/// turns an approved `make test` into `cargo test` with the cwd unchanged.
/// [`project_validator`] carries the marker as a second authorization target,
/// so a changed detection changes the targets and `confirm_targets` refuses
/// the call instead of running the command nobody approved.
fn detect_project_command(cwd: &Path, kind: &str) -> Result<(&'static str, Vec<String>, PathBuf)> {
    let cargo = cwd.join("Cargo.toml");
    let makefile = cwd.join("Makefile");
    let lowercase_makefile = cwd.join("makefile");
    let (program, args, marker): (&'static str, Vec<&'static str>, PathBuf) = if cargo.exists() {
        match kind {
            "build" => ("cargo", vec!["build", "--all-targets"], cargo),
            "test" => (
                "cargo",
                vec!["test", "--all-targets", "--", "--quiet"],
                cargo,
            ),
            _ => bail!("unknown project action {kind}"),
        }
    } else if makefile.exists() || lowercase_makefile.exists() {
        let marker = if makefile.exists() {
            makefile
        } else {
            lowercase_makefile
        };
        match kind {
            "build" => ("make", vec!["build"], marker),
            "test" => ("make", vec!["test"], marker),
            _ => bail!("unknown project action {kind}"),
        }
    } else {
        bail!(
            "No Cargo.toml or Makefile in {}; cannot detect build system",
            cwd.display()
        );
    };
    Ok((
        program,
        args.into_iter().map(str::to_string).collect(),
        marker,
    ))
}

/// Detect the build system in `cwd` and run its build or test command.
///
/// Runs through [`run_process_from_input`] so the same policy, sandbox, and env
/// sanitation apply, then relabels the capability.
fn project_action(input: &Value, kind: &str) -> Result<String> {
    let cwd = resolve_cwd(require_cwd(input)?)?;
    let (program, args, _marker) = detect_project_command(&cwd, kind)?;

    let synthetic = json!({
        "program": program,
        "args": args,
        "cwd": cwd.display().to_string(),
        "timeout_secs": 300,
        "background": false,
    });
    let mut result = run_process_from_input(&synthetic)?;
    // Annotate capability name for matrix honesty.
    if let Ok(mut value) = serde_json::from_str::<Value>(&result) {
        value["capability"] = json!(if kind == "build" {
            "project.build"
        } else {
            "project.test"
        });
        result = value.to_string();
    }
    Ok(result)
}

/// Materialize a program path/name that cannot reintroduce shell injection.
///
/// `path_policy::validate_terminal` already checked the synthetic command line;
/// this second pass rebuilds an `OsString` from a pure lexical component stack
/// (no `Path::new(user)` / `PathBuf::from(user)` on the raw agent string).
fn sanitize_argv_program(program: &str) -> Result<OsString> {
    if program.is_empty() {
        bail!("program must not be empty");
    }
    if program.contains([
        '\0', '\n', '\r', '|', ';', '&', '<', '>', '(', ')', '$', '`', ' ',
    ]) {
        bail!("program contains forbidden characters");
    }
    if program.contains('\\') {
        bail!("program path must use '/' separators");
    }

    let absolute = program.starts_with('/');
    let body = program.trim_start_matches('/');
    let mut stack: Vec<&str> = Vec::new();
    if absolute {
        // Root marker kept as empty first segment for rejoin.
        stack.push("");
    }
    for segment in body.split('/') {
        match segment {
            "" | "." => continue,
            ".." => bail!("program path may not contain '..' components"),
            other => {
                if other.chars().any(|c| c.is_control()) {
                    bail!("program segment contains control characters");
                }
                stack.push(other);
            }
        }
    }

    let rebuilt = if absolute {
        if stack.len() == 1 {
            // Just "/"
            "/".to_string()
        } else {
            stack.join("/")
        }
    } else {
        if stack.is_empty() {
            bail!("program resolved empty");
        }
        stack.join("/")
    };
    if rebuilt.is_empty() {
        bail!("program resolved empty");
    }
    Ok(OsString::from(rebuilt))
}

/// Materialize a single argument, rejecting embedded control characters.
fn sanitize_argv_arg(arg: &str) -> Result<OsString> {
    if arg.contains(['\0', '\n', '\r']) {
        bail!("argument contains control characters");
    }
    // Rebuild as a fresh OsString from bytes so Command does not receive the
    // original agent-string allocation identity.
    Ok(OsString::from(arg.to_owned()))
}

/// Terminal policy, sanitized child env, and safe printf timeout path.
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Shell pipes/operators fail validate_terminal; plain printf is allowed.
    #[test]
    fn terminal_policy_blocks_shell_operators_via_synthetic_line() {
        let tmp = TempDir::new().unwrap();
        let roots = path_policy::canonical_roots(&[tmp.path().to_path_buf()]);
        let cwd = tmp.path().to_str().unwrap();
        assert!(path_policy::validate_terminal("printf ok", cwd, &roots).is_ok());
        assert!(path_policy::validate_terminal("printf a | bash", cwd, &roots).is_err());
    }

    /// Child env strips LLM API secrets while preserving PATH for tools.
    #[test]
    fn sanitized_child_env_drops_llm_secrets_but_keeps_path() {
        let secret_key = "LLM_OPENAI_API_KEY";
        let secret_value = "super-secret-test-value-pr68";
        let previous = std::env::var(secret_key).ok();
        // SAFETY: test-only env mutation, restored before return.
        unsafe {
            std::env::set_var(secret_key, secret_value);
        }

        let mut secret_cmd = Command::new("/usr/bin/printenv");
        secret_cmd
            .arg(secret_key)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        apply_sanitized_child_env(&mut secret_cmd);
        let secret_out = run_with_timeout(secret_cmd, Duration::from_secs(5)).unwrap();

        let mut path_cmd = Command::new("/usr/bin/printenv");
        path_cmd
            .arg("PATH")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        apply_sanitized_child_env(&mut path_cmd);
        let path_out = run_with_timeout(path_cmd, Duration::from_secs(5)).unwrap();

        match previous {
            Some(value) => unsafe { std::env::set_var(secret_key, value) },
            None => unsafe { std::env::remove_var(secret_key) },
        }

        let leaked = String::from_utf8_lossy(&secret_out.stdout);
        assert!(
            !leaked.contains(secret_value),
            "child must not inherit parent LLM secrets, got {leaked:?}"
        );
        let path = String::from_utf8_lossy(&path_out.stdout);
        assert!(
            !path.trim().is_empty(),
            "PATH must still be present for child tools"
        );
        assert!(path_out.status.success());
    }

    /// run_with_timeout succeeds for a short safe printf child.
    #[test]
    fn run_printf_when_roots_allow() {
        // Direct command without workspace settings: path_policy needs roots.
        // This test only verifies timeout helper with a safe program.
        let mut command = Command::new("printf");
        command
            .arg("hi")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = run_with_timeout(command, Duration::from_secs(5)).unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "hi");
    }

    /// CS160-R010: the process card names the program, the full argv and the
    /// resolved cwd, and that cwd is the call's authorization target.
    #[test]
    fn process_preview_names_program_argv_and_cwd() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().canonicalize().unwrap();
        let args = vec!["--release".to_string(), "--locked".to_string()];
        let preview = process_preview(
            "Run cargo with 2 argument(s)".to_string(),
            synthetic_command_line("cargo", &args),
            cwd.clone(),
        );
        let shown = cwd.display().to_string();
        assert_eq!(preview.command.as_deref(), Some("cargo --release --locked"));
        assert_eq!(preview.cwd.as_deref(), Some(shown.as_str()));
        assert_eq!(preview.paths, vec![shown]);
        assert!(preview.summary.contains("cargo"));
    }

    /// Two calls that differ only in argv cannot share a card.
    #[test]
    fn different_argv_produce_different_process_cards() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().to_path_buf();
        let card = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
            process_preview(
                String::new(),
                synthetic_command_line("git", &args),
                cwd.clone(),
            )
        };
        assert_ne!(
            card(&["status"]).command,
            card(&["push", "--force"]).command
        );
    }

    /// One throne for build detection: the card and the spawn read the same
    /// answer. Cargo wins over Make; neither present is refused, not guessed.
    /// The detection also names the build file it rests on, so the card can
    /// bind it.
    #[test]
    fn project_command_detection_is_shared_and_refuses_to_guess() {
        let tmp = TempDir::new().unwrap();
        assert!(detect_project_command(tmp.path(), "build").is_err());
        std::fs::write(tmp.path().join("Makefile"), "build:\n").unwrap();
        assert_eq!(
            detect_project_command(tmp.path(), "test").unwrap(),
            (
                "make",
                vec!["test".to_string()],
                tmp.path().join("Makefile")
            )
        );
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(
            detect_project_command(tmp.path(), "build").unwrap(),
            (
                "cargo",
                vec!["build".to_string(), "--all-targets".to_string()],
                tmp.path().join("Cargo.toml")
            )
        );
    }

    /// A pid the agent never started is refused before any approval is asked.
    #[test]
    fn stop_card_refuses_a_pid_this_agent_did_not_start() {
        let validator = tracked_pid_validator("Stop");
        assert!(validator(&json!({"pid": 1})).is_err());
        assert!(validator(&json!({"pid": u64::from(u32::MAX) + 1})).is_err());
        assert!(validator(&json!({})).is_err());
    }

    /// PR #173 review (P1): the process card's command line goes through the
    /// one redaction authority, so an argv secret never reaches the card.
    #[test]
    fn process_card_redacts_a_secret_in_the_argv() {
        let tmp = TempDir::new().unwrap();
        let args = vec!["--token".to_string(), "abc".to_string(), "-v".to_string()];
        let preview = process_preview(
            String::new(),
            synthetic_command_line("curl", &args),
            tmp.path().to_path_buf(),
        );
        assert_eq!(
            preview.command.as_deref(),
            Some("curl --token [REDACTED] -v")
        );
    }
}
