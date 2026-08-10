//! Bounded process execution (canonical: process.run / observe / stop).
//!
//! No shell string concatenation — argv only. Terminal path policy from
//! [`super::path_policy`] is applied to the synthetic command line for program
//! and path gates. Mutating/process tools default to Ask.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use codescribe_core::agent::{ToolDefinition, ToolRegistry, ToolResultContent, ToolRisk};
use serde_json::{Value, json};

use super::{output_guard, path_policy, workspace};

/// Wall-clock timeout applied when the caller does not specify one.
const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// Ceiling for a caller-supplied timeout, so no single tool call can pin a
/// process indefinitely.
const MAX_TIMEOUT_SECS: u64 = 300;

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
pub fn register(registry: &mut ToolRegistry) {
    registry
        .register_native(
            run_process_definition(),
            Box::new(|input| Box::pin(async move { handle_run_process(input) })),
            ToolRisk::ProcessControl,
        )
        .expect("register run_process");
    registry
        .register_native(
            observe_process_definition(),
            Box::new(|input| Box::pin(async move { handle_observe_process(input) })),
            ToolRisk::ReadOnly,
        )
        .expect("register observe_process");
    registry
        .register_native(
            stop_process_definition(),
            Box::new(|input| Box::pin(async move { handle_stop_process(input) })),
            ToolRisk::ProcessControl,
        )
        .expect("register stop_process");
    registry
        .register_native(
            project_build_definition(),
            Box::new(|input| Box::pin(async move { handle_project_build(input) })),
            ToolRisk::ProcessControl,
        )
        .expect("register project_build");
    registry
        .register_native(
            project_test_definition(),
            Box::new(|input| Box::pin(async move { handle_project_test(input) })),
            ToolRisk::ProcessControl,
        )
        .expect("register project_test");
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
    let program = input
        .get("program")
        .and_then(Value::as_str)
        .context("Missing required string field 'program'")?;
    let args: Vec<String> = input
        .get("args")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let cwd_str = input
        .get("cwd")
        .and_then(Value::as_str)
        .context("Missing required string field 'cwd'")?;
    let cwd = resolve_cwd(cwd_str)?;
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
    command
        .args(&args_os)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_sanitized_child_env(&mut command);

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
    let pid = input
        .get("pid")
        .and_then(Value::as_u64)
        .context("Missing required integer field 'pid'")? as u32;
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
    let pid = input
        .get("pid")
        .and_then(Value::as_u64)
        .context("Missing required integer field 'pid'")? as u32;
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

/// Detect the build system in `cwd` and run its build or test command.
///
/// Cargo wins over Make when both are present; neither present is an error
/// rather than a guess. Runs through [`run_process_from_input`] so the same
/// policy, sandbox, and env sanitation apply, then relabels the capability.
fn project_action(input: &Value, kind: &str) -> Result<String> {
    let cwd_str = input
        .get("cwd")
        .and_then(Value::as_str)
        .context("Missing required string field 'cwd'")?;
    let cwd = resolve_cwd(cwd_str)?;
    let (program, args): (&str, Vec<&str>) = if cwd.join("Cargo.toml").exists() {
        match kind {
            "build" => ("cargo", vec!["build", "--all-targets"]),
            "test" => ("cargo", vec!["test", "--all-targets", "--", "--quiet"]),
            _ => bail!("unknown project action {kind}"),
        }
    } else if cwd.join("Makefile").exists() || cwd.join("makefile").exists() {
        match kind {
            "build" => ("make", vec!["build"]),
            "test" => ("make", vec!["test"]),
            _ => bail!("unknown project action {kind}"),
        }
    } else {
        bail!(
            "No Cargo.toml or Makefile in {}; cannot detect build system",
            cwd.display()
        );
    };

    let args_owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    let synthetic = json!({
        "program": program,
        "args": args_owned,
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
        let secret_key = "LLM_ASSISTIVE_API_KEY";
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
}
