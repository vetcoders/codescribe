pub mod clipboard;
pub mod filesystem;
pub mod github;
pub mod mcp;
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
    typing::register(registry);
    github::register(registry);
    search_threads::register(registry);
    transcribe_audio::register(registry);
    workspace::register(registry);
}

#[cfg(test)]
mod tests {
    use super::*;

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
                "fetch_github_file".to_string(),
                "get_frontmost_app".to_string(),
                "get_selected_text".to_string(),
                "list_projects".to_string(),
                "read_clipboard".to_string(),
                "read_file".to_string(),
                "search_threads".to_string(),
                "take_screenshot".to_string(),
                "transcribe_audio".to_string(),
                "type_text".to_string(),
                "write_clipboard".to_string(),
            ]
        );
    }
}
