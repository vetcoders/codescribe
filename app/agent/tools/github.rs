use codescribe_core::agent::{ToolDefinition, ToolRegistry, ToolResultContent};
use serde_json::{Value, json};

pub fn register(registry: &mut ToolRegistry) {
    registry
        .register(
            fetch_github_file_definition(),
            Box::new(|input| Box::pin(handle_fetch_github_file(input))),
        )
        .expect("register fetch_github_file tool");
}

/// Tool: fetch_github_file
/// Allows the agent to fetch raw file content from a public or private GitHub repository.
/// This gives the agent real "hands" to attach code/docs from GitHub during reasoning
/// (especially useful in voice chat / assistive / AoT contexts).
fn fetch_github_file_definition() -> ToolDefinition {
    ToolDefinition {
        name: "fetch_github_file".to_string(),
        description: "Fetch the raw content of a file from a GitHub repository. Supports public repos and private repos with a configured GITHUB_TOKEN. Input can be a full GitHub URL or owner/repo@ref:path format. Returns the file content as text plus metadata.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "url_or_spec": {
                    "type": "string",
                    "description": "GitHub file reference. Examples: 'https://github.com/owner/repo/blob/main/path/to/file.rs' or 'owner/repo:main/path/to/file.rs'"
                }
            },
            "required": ["url_or_spec"]
        }),
    }
}

async fn handle_fetch_github_file(input: Value) -> Vec<ToolResultContent> {
    let url_or_spec = input
        .get("url_or_spec")
        .and_then(Value::as_str)
        .unwrap_or("");

    if url_or_spec.trim().is_empty() {
        return vec![ToolResultContent::Error(
            "url_or_spec is required".to_string(),
        )];
    }

    // Reuse the existing robust parser and fetcher from the core connectors
    let Some(gh_ref) = codescribe_core::connectors::github::parse_github_ref(url_or_spec) else {
        return vec![ToolResultContent::Error(format!(
            "Could not parse GitHub reference: {}",
            url_or_spec
        ))];
    };

    let token = codescribe_core::connectors::github::load_github_token();

    match codescribe_core::connectors::github::fetch_github_blob(&gh_ref, token.as_deref()).await {
        Ok((data, filename)) => {
            let content = String::from_utf8_lossy(&data).to_string();
            let meta = format!(
                "Fetched from GitHub: {}/{}/{}@{} (filename: {}, {} bytes)",
                gh_ref.owner,
                gh_ref.repo,
                gh_ref.path,
                gh_ref.git_ref,
                filename,
                data.len()
            );

            // For code files, the agent can follow up by calling loctree analysis if needed.
            // For now we return the raw content + metadata so the agent can reason over it.
            vec![ToolResultContent::Text(format!("{}\n\n{}", meta, content))]
        }
        Err(e) => {
            vec![ToolResultContent::Error(format!(
                "Failed to fetch from GitHub ({}): {}",
                gh_ref, e
            ))]
        }
    }
}
