pub mod clipboard;
pub mod doctrine;
pub mod filesystem;
pub mod github;
pub mod mcp;
pub mod monitor_run;
pub mod native_fs;
mod path_policy;
pub mod process;
pub mod repo;
pub mod screenshot;
pub mod search_threads;
pub mod selection;
pub mod transcribe_audio;
pub mod typing;
pub mod workspace;

use codescribe_core::agent::ToolRegistry;

pub fn register_all_tools(registry: &mut ToolRegistry) {
    register_native_tools(registry);
    mcp::register(registry);
}

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
mod tests {
    use super::*;
    use codescribe_core::agent::{CapabilityOp, ConnectorHealth, resolve_capability};

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
