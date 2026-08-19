//! The agent's native tool surface — the hands it acts with.
//!
//! Registration is split in two on purpose: [`register_native_tools`] installs
//! the in-process tools and is what the tests assert against, while
//! [`register_all_tools`] adds the MCP bridge on top. Keeping the native set
//! separately nameable is what lets a test prove the substrate covers every
//! core capability without an MCP server running.

/// Prompt-layer Responses/streaming ground truth (measured API contracts).
pub mod api_truth;
/// Clipboard read/write tools (`read_clipboard` / `write_clipboard`).
pub mod clipboard;
/// Prompt-layer review-tool + connector-fallback doctrine (no executor changes).
pub mod doctrine;
/// Sandboxed `read_file` tool with workspace-root and size bounds.
pub mod filesystem;
/// `fetch_github_file` tool shell over the shared GitHub connector.
pub mod github;
/// MCP discovery, tool registration, and readiness probes.
pub mod mcp;
/// Registers a durable Vibecrafted run monitor on the current thread.
pub mod monitor_run;
/// List/search/write/patch/move filesystem tools (beyond `read_file`).
pub mod native_fs;
/// Spills oversized tool chunks to disk and returns a follow-up pointer.
mod output_guard;
/// Fail-closed path containment and terminal command-shape gates.
mod path_policy;
/// Argv-only process run / observe / stop tools with path policy.
pub mod process;
/// Native git status/diff/log/commit tools under workspace roots.
pub mod repo;
/// Screen capture tool with permission and payload bounds.
pub mod screenshot;
/// Local thread-index search (`search_threads`).
pub mod search_threads;
/// Frontmost-app and selected-text observation tools.
pub mod selection;
/// Path-gated local audio transcription via the Whisper singleton.
pub mod transcribe_audio;
/// Focused-app text injection via clipboard paste (then restore).
pub mod typing;
/// Workspace-root resolution and `list_projects` enumeration.
pub mod workspace;

use codescribe_core::agent::ToolRegistry;

/// Install the complete tool surface: every native tool plus the MCP bridge.
pub fn register_all_tools(registry: &mut ToolRegistry) {
    register_native_tools(registry);
    mcp::register(registry);
}

/// Install only the in-process tools — no MCP, no network dependency — so the
/// native substrate can be exercised on its own.
fn register_native_tools(registry: &mut ToolRegistry) {
    screenshot::register(registry);
    clipboard::register(registry);
    selection::register(registry);
    filesystem::register(registry);
    native_fs::register(registry);
    typing::register(registry);
    github::register(registry);
    monitor_run::register(registry);
    search_threads::register(registry);
    transcribe_audio::register(registry);
    workspace::register(registry);
    repo::register(registry);
    process::register(registry);
}

#[cfg(test)]
/// Unit tests for native tool registration and capability coverage.
mod tests {
    use super::*;
    use codescribe_core::agent::{CapabilityOp, ConnectorHealth, resolve_capability};

    /// Asserts the native registry exposes the expected sorted tool name set.
    #[test]
    fn register_all_tools_registers_expected_names() {
        let mut registry = ToolRegistry::new();
        register_native_tools(&mut registry);

        let mut names = registry
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();
        names.sort();

        assert_eq!(
            names,
            vec![
                "apply_patch".to_string(),
                "fetch_github_file".to_string(),
                "get_frontmost_app".to_string(),
                "get_selected_text".to_string(),
                "git_commit".to_string(),
                "git_diff".to_string(),
                "git_log".to_string(),
                "git_status".to_string(),
                "list_directory".to_string(),
                "list_projects".to_string(),
                "monitor_run".to_string(),
                "move_path".to_string(),
                "observe_process".to_string(),
                "project_build".to_string(),
                "project_test".to_string(),
                "read_clipboard".to_string(),
                "read_file".to_string(),
                "run_process".to_string(),
                "search_files".to_string(),
                "search_threads".to_string(),
                "stop_process".to_string(),
                "take_screenshot".to_string(),
                "transcribe_audio".to_string(),
                "type_text".to_string(),
                "write_clipboard".to_string(),
                "write_file".to_string(),
            ]
        );
    }

    /// Core capability ops must resolve to registered native tools (no IntelliJ).
    #[test]
    fn native_substrate_covers_core_capabilities_without_intellij() {
        let mut registry = ToolRegistry::new();
        register_native_tools(&mut registry);
        let names: std::collections::HashSet<_> =
            registry.definitions().into_iter().map(|d| d.name).collect();

        for op in [
            CapabilityOp::FsList,
            CapabilityOp::FsRead,
            CapabilityOp::FsSearch,
            CapabilityOp::FsWrite,
            CapabilityOp::FsPatch,
            CapabilityOp::RepoStatus,
            CapabilityOp::RepoDiff,
            CapabilityOp::RepoLog,
            CapabilityOp::RepoCommit,
            CapabilityOp::ProcessRun,
            CapabilityOp::ProjectBuild,
            CapabilityOp::ProjectTest,
        ] {
            let resolution = resolve_capability(op, &ConnectorHealth::default());
            let tool = resolution
                .native_tool
                .expect("native tool required for core op");
            assert!(
                names.contains(tool),
                "missing native tool {tool} for {}",
                op.as_str()
            );
        }
    }
}
