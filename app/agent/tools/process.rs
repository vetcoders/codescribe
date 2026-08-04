//! Bounded process execution (canonical: process.run / observe / stop).
//!
//! No shell string concatenation — argv only. Terminal path policy from
//! [`super::path_policy`] is applied to the synthetic command line for program
//! and path gates. Mutating/process tools default to Ask.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use codescribe_core::agent::{ToolDefinition, ToolRegistry, ToolResultContent, ToolRisk};
use serde_json::{Value, json};

use super::{path_policy, workspace};

const MAX_OUTPUT_CHARS: usize = 40_000;
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MAX_TIMEOUT_SECS: u64 = 300;

struct TrackedProcess {
    child: Child,
    started: Instant,
    command: String,
    cwd: PathBuf,
}

fn process_table() -> &'static Mutex<HashMap<u32, TrackedProcess>> {
    static TABLE: OnceLock<Mutex<HashMap<u32, TrackedProcess>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

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

fn handle_run_process(input: Value) -> Vec<ToolResultContent> {
    match run_process_from_input(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

fn handle_observe_process(input: Value) -> Vec<ToolResultContent> {
    match observe_from_input(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

fn handle_stop_process(input: Value) -> Vec<ToolResultContent> {
    match stop_from_input(&input) {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

fn handle_project_build(input: Value) -> Vec<ToolResultContent> {
    match project_action(&input, "build") {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

fn handle_project_test(input: Value) -> Vec<ToolResultContent> {
    match project_action(&input, "test") {
        Ok(text) => vec![ToolResultContent::Text(text)],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

fn roots() -> Vec<PathBuf> {
    path_policy::canonical_roots(&workspace::resolved_roots())
}

fn resolve_cwd(cwd: &str) -> Result<PathBuf> {
    let cwd = path_policy::validate_existing(cwd, &roots())?;
    if !cwd.is_dir() {
        bail!("cwd is not a directory: {}", cwd.display());
    }
    Ok(cwd)
}

fn synthetic_command_line(program: &str, args: &[String]) -> String {
    let mut parts = vec![program.to_string()];
    parts.extend(args.iter().cloned());
    parts.join(" ")
}

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

    let mut command = Command::new(program);
    command
        .args(&args)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

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
    let text = truncate(&text);
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

fn truncate(text: &str) -> String {
    if text.chars().count() <= MAX_OUTPUT_CHARS {
        return text.to_string();
    }
    let mut out: String = text.chars().take(MAX_OUTPUT_CHARS).collect();
    out.push_str("\n…[truncated]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn terminal_policy_blocks_shell_operators_via_synthetic_line() {
        let tmp = TempDir::new().unwrap();
        let roots = path_policy::canonical_roots(&[tmp.path().to_path_buf()]);
        let cwd = tmp.path().to_str().unwrap();
        assert!(path_policy::validate_terminal("printf ok", cwd, &roots).is_ok());
        assert!(path_policy::validate_terminal("printf a | bash", cwd, &roots).is_err());
    }

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
