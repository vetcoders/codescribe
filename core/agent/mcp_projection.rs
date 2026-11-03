//! Compact MCP tool projection, on-demand schema search, and lazy invocation gateway.
//!
//! Sending 70–90 full MCP JSON Schemas on every LLM turn burns ~40,000 tokens
//! before the conversation even starts. This module separates **registry truth**
//! (every MCP tool stays registered in [`super::ToolRegistry`] with its
//! [`super::ToolOrigin::Mcp`], [`super::ToolRisk`], validator, and permission
//! rules) from **wire projection** ([`super::ToolRegistry::wire_definitions`]):
//!
//! 1. Raw `ToolOrigin::Mcp` tool definitions are omitted from the wire `tools`
//!    list sent to the model on each turn.
//! 2. In their place, two compact gateway tools are advertised whenever at
//!    least one MCP tool is registered:
//!    - [`MCP_SEARCH_TOOL_NAME`] (`mcp_search_tools`): embeds a token-budgeted,
//!      round-robin compact catalog (`tool(arg: type, opt?: type) — summary`)
//!      grouped by MCP server, and returns full JSON Schemas on demand.
//!    - [`MCP_CALL_TOOL_NAME`] (`call_mcp_tool`): resolves `(server, tool)` or
//!      `mcp__<server>__<tool>` / `<server>/<tool>` to the canonical registered
//!      MCP tool, validates arguments against its `input_schema` (returning
//!      self-correcting schema docs on `"?"` or validation errors), and hands
//!      execution back to the registry's single permission + validator gate.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::registry::{ToolDefinition, ToolRisk};

/// Wire tool name for searching and inspecting full MCP tool JSON schemas.
pub const MCP_SEARCH_TOOL_NAME: &str = "mcp_search_tools";

/// Wire tool name for invoking a registered MCP tool through the lazy gateway.
pub const MCP_CALL_TOOL_NAME: &str = "call_mcp_tool";

/// Maximum UTF-8 byte budget for the embedded compact MCP catalog inside
/// [`MCP_SEARCH_TOOL_NAME`]'s description (~1,500 tokens).
pub const MCP_CATALOG_BUDGET_BYTES: usize = 6_000;

/// Maximum UTF-8 bytes for a single tool's one-line summary in the compact catalog.
const MCP_SUMMARY_MAX_BYTES: usize = 160;

/// Maximum parameters rendered inline in a compact signature before `, ...`.
const MCP_SIGNATURE_MAX_PARAMS: usize = 8;

/// Default number of full tool schemas returned by [`MCP_SEARCH_TOOL_NAME`].
const DEFAULT_SEARCH_LIMIT: usize = 10;

/// Hard ceiling on tool schemas returned by a single [`MCP_SEARCH_TOOL_NAME`] call.
const MAX_SEARCH_LIMIT: usize = 25;

/// Borrowed view of one registered MCP tool used for catalog projection,
/// schema search, and call resolution.
#[derive(Debug, Clone, Copy)]
pub struct McpToolEntry<'a> {
    /// Canonical registry key (typically `mcp__<server>__<upstream_tool>`).
    pub registered_name: &'a str,
    /// Configured MCP server identifier (e.g. `loctree-mcp`).
    pub server: &'a str,
    /// Upstream tool name on that server (e.g. `context`).
    pub upstream_tool: &'a str,
    /// Tool description stored in the registry.
    pub description: &'a str,
    /// Full JSON Schema for the tool's input arguments.
    pub input_schema: &'a Value,
    /// Classified risk tier used by the permission gate.
    pub risk: ToolRisk,
}

/// Outcome of resolving a [`MCP_CALL_TOOL_NAME`] invocation before the
/// permission gate runs.
#[derive(Debug, Clone, PartialEq)]
pub enum McpCallResolution {
    /// Target MCP tool was resolved and its arguments satisfied schema pre-checks.
    /// The caller must now run `decide_with_targets` and `dispatch_in_thread`
    /// against `canonical_name` and `arguments`.
    Resolved {
        canonical_name: String,
        arguments: Value,
    },
    /// Caller requested on-demand help (`arguments: "?"` / `"help"` or `help: true`).
    /// Returns full documentation and JSON Schema without executing the tool.
    Help {
        canonical_name: String,
        documentation: String,
    },
    /// Target tool could not be resolved or `arguments` failed schema validation.
    /// Carries self-correcting error text (including full JSON Schema when a
    /// target tool was matched) so the model can fix the call on the next turn.
    Invalid {
        canonical_name: Option<String>,
        error: String,
    },
}

/// Build the wire [`ToolDefinition`] for [`MCP_SEARCH_TOOL_NAME`], embedding the
/// round-robin compact catalog of currently registered MCP tools.
pub fn build_mcp_search_tool_definition(entries: &[McpToolEntry<'_>]) -> ToolDefinition {
    let preamble = format!(
        "Search or inspect full JSON schemas for external MCP tools on demand.\n\
         MCP tools are lazy-loaded to keep context compact. When a tool's parameters \
         are clear from its signature below, invoke it directly with `{MCP_CALL_TOOL_NAME}`. \
         When you need full parameter descriptions, nested object shapes, or tools not \
         shown below, call `{MCP_SEARCH_TOOL_NAME}` first.\n\n\
         Available MCP tools (compact catalog):\n"
    );
    let catalog = render_mcp_catalog(
        entries,
        MCP_CATALOG_BUDGET_BYTES.saturating_sub(preamble.len()),
    );
    let description = format!("{preamble}{catalog}");

    ToolDefinition {
        name: MCP_SEARCH_TOOL_NAME.to_string(),
        description,
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Keyword or phrase to search across MCP tool names, servers, descriptions, and parameter names."
                },
                "server": {
                    "type": "string",
                    "description": "Optional MCP server filter (e.g. 'loctree-mcp', 'aicx-mcp', 'vibecrafted-mcp', 'desktop-commander')."
                },
                "tool": {
                    "type": "string",
                    "description": "Exact MCP tool name ('<tool>', '<server>/<tool>', or 'mcp__<server>__<tool>') to inspect its full JSON schema."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of matching tool schemas to return (default 10, max 25)."
                }
            },
            "additionalProperties": false
        }),
    }
}

/// Build the wire [`ToolDefinition`] for [`MCP_CALL_TOOL_NAME`].
pub fn build_mcp_call_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: MCP_CALL_TOOL_NAME.to_string(),
        description: "Invoke an external MCP tool by server and tool name (or canonical \
             `mcp__<server>__<tool>` / `<server>/<tool>` identifier). \
             Every call goes through the operator permission gate for the target MCP tool. \
             Pass `arguments: \"?\"` (or `help: true`) to inspect the tool's full JSON schema \
             without executing it. If required arguments are missing or have the wrong type, \
             the error response includes the tool's full schema so you can retry immediately."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "MCP server name (e.g. 'loctree-mcp', 'aicx-mcp', 'vibecrafted-mcp', 'desktop-commander'). Optional if `tool` is unambiguous or already includes the server."
                },
                "tool": {
                    "type": "string",
                    "description": "Target MCP tool name: upstream name (e.g. 'context'), '<server>/<tool>', '<server>:<tool>', or 'mcp__<server>__<tool>'."
                },
                "arguments": {
                    "description": "JSON object of arguments for the target MCP tool (defaults to {}), or '?' / 'help' to inspect its full schema."
                },
                "help": {
                    "type": "boolean",
                    "description": "When true, returns the target tool's full documentation and JSON schema without executing it."
                }
            },
            "required": ["tool"],
            "additionalProperties": false
        }),
    }
}

/// Render a deterministic, token-budgeted compact MCP catalog using round-robin
/// selection across MCP servers so one large server never starves smaller ones.
///
/// Strict invariant: the returned string never exceeds `budget_bytes` UTF-8 bytes.
pub fn render_mcp_catalog(entries: &[McpToolEntry<'_>], budget_bytes: usize) -> String {
    if budget_bytes == 0 {
        return String::new();
    }
    if entries.is_empty() {
        return truncate_utf8_to_bytes("No MCP tools registered.", budget_bytes);
    }

    let mut by_server: BTreeMap<&str, Vec<&McpToolEntry<'_>>> = BTreeMap::new();
    for entry in entries {
        by_server.entry(entry.server).or_default().push(entry);
    }
    for tools in by_server.values_mut() {
        tools.sort_by(|a, b| {
            a.upstream_tool
                .cmp(b.upstream_tool)
                .then_with(|| a.registered_name.cmp(b.registered_name))
        });
    }

    let server_names: Vec<&str> = by_server.keys().copied().collect();
    let rendered_lines: BTreeMap<&str, Vec<String>> = by_server
        .iter()
        .map(|(&server, tools)| {
            let lines = tools
                .iter()
                .map(|entry| format_catalog_entry_line(entry))
                .collect();
            (server, lines)
        })
        .collect();

    // First determine how many server sections fit within `budget_bytes`.
    let mut active_servers: Vec<&str> = Vec::with_capacity(server_names.len());
    let mut selected_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for &server in &server_names {
        let mut candidate_servers = active_servers.clone();
        candidate_servers.push(server);
        let mut candidate_counts = selected_counts.clone();
        candidate_counts.insert(server, 0);
        let rendered = format_catalog_sections(
            &candidate_servers,
            server_names.len(),
            &rendered_lines,
            &candidate_counts,
        );
        if rendered.len() <= budget_bytes {
            active_servers = candidate_servers;
            selected_counts = candidate_counts;
        } else {
            break;
        }
    }

    if active_servers.is_empty() {
        let fallback = format!(
            "({} MCP servers registered — call {MCP_SEARCH_TOOL_NAME} to list tools)",
            server_names.len()
        );
        return truncate_utf8_to_bytes(&fallback, budget_bytes);
    }

    let max_len = active_servers
        .iter()
        .filter_map(|s| rendered_lines.get(s).map(Vec::len))
        .max()
        .unwrap_or(0);

    for round in 0..max_len {
        let mut any_added = false;
        for &server in &active_servers {
            let lines = &rendered_lines[server];
            let current = selected_counts.get(server).copied().unwrap_or(0);
            if round >= lines.len() || current > round {
                continue;
            }
            let mut candidate_counts = selected_counts.clone();
            candidate_counts.insert(server, current + 1);
            let candidate = format_catalog_sections(
                &active_servers,
                server_names.len(),
                &rendered_lines,
                &candidate_counts,
            );
            if candidate.len() <= budget_bytes {
                selected_counts = candidate_counts;
                any_added = true;
            }
        }
        if !any_added {
            break;
        }
    }

    let out = format_catalog_sections(
        &active_servers,
        server_names.len(),
        &rendered_lines,
        &selected_counts,
    );
    truncate_utf8_to_bytes(&out, budget_bytes)
}

fn format_catalog_sections(
    active_servers: &[&str],
    total_servers: usize,
    rendered_lines: &BTreeMap<&str, Vec<String>>,
    selected_counts: &BTreeMap<&str, usize>,
) -> String {
    let mut out = String::new();
    for &server in active_servers {
        let lines = &rendered_lines[server];
        let total = lines.len();
        let shown = selected_counts.get(server).copied().unwrap_or(0);
        if !out.is_empty() {
            out.push('\n');
        }
        if shown < total {
            out.push_str(&format!(
                "[{server}] ({shown}/{total} tools shown — call {MCP_SEARCH_TOOL_NAME}(server=\"{server}\") for all)\n"
            ));
        } else {
            out.push_str(&format!("[{server}] ({total} tools)\n"));
        }
        for line in lines.iter().take(shown) {
            out.push_str(line);
            out.push('\n');
        }
    }
    if active_servers.len() < total_servers {
        let omitted = total_servers - active_servers.len();
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "(+{omitted} more MCP server(s) — call {MCP_SEARCH_TOOL_NAME} to inspect)"
        ));
    }
    out.trim_end().to_string()
}

fn truncate_utf8_to_bytes(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = 0usize;
    for (idx, ch) in s.char_indices() {
        let next = idx + ch.len_utf8();
        if next > max_bytes {
            break;
        }
        end = next;
    }
    s[..end].trim_end().to_string()
}

/// Format a single MCP tool as `  - tool(arg: type, opt?: type) — summary`.
fn format_catalog_entry_line(entry: &McpToolEntry<'_>) -> String {
    let sig = json_schema_to_compact_signature(entry.upstream_tool, entry.input_schema);
    let summary = sanitize_one_line_summary(entry.description, MCP_SUMMARY_MAX_BYTES);
    if summary.is_empty() {
        format!("  - {sig}")
    } else {
        format!("  - {sig} — {summary}")
    }
}

/// Compile a JSON Schema object into a compact TypeScript-style call signature:
/// `tool_name(req_arg: string, opt_arg?: integer)`.
pub fn json_schema_to_compact_signature(tool_name: &str, schema: &Value) -> String {
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return format!("{tool_name}()");
    };
    if props.is_empty() {
        return format!("{tool_name}()");
    }

    let required_set: std::collections::HashSet<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let mut param_keys: Vec<&String> = props.keys().collect();
    param_keys.sort_by(|a, b| {
        let a_req = required_set.contains(a.as_str());
        let b_req = required_set.contains(b.as_str());
        b_req.cmp(&a_req).then_with(|| a.cmp(b))
    });

    let total_params = param_keys.len();
    let mut rendered_params = Vec::with_capacity(total_params.min(MCP_SIGNATURE_MAX_PARAMS) + 1);
    for key in param_keys.into_iter().take(MCP_SIGNATURE_MAX_PARAMS) {
        let prop_schema = &props[key];
        let (mut ty, nullable) = compact_json_type(prop_schema);
        if nullable && ty != "null" && !ty.split('|').any(|part| part == "null") {
            ty.push_str("|null");
        }
        let is_required = required_set.contains(key.as_str());
        if is_required {
            rendered_params.push(format!("{key}: {ty}"));
        } else {
            rendered_params.push(format!("{key}?: {ty}"));
        }
    }
    if total_params > MCP_SIGNATURE_MAX_PARAMS {
        rendered_params.push("...".to_string());
    }

    format!("{tool_name}({})", rendered_params.join(", "))
}

/// Extract a concise type label and whether the schema explicitly permits `null`.
fn compact_json_type(prop_schema: &Value) -> (String, bool) {
    let explicit_nullable = prop_schema
        .get("nullable")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if let Some(enum_vals) = prop_schema.get("enum").and_then(Value::as_array)
        && !enum_vals.is_empty()
        && enum_vals.len() <= 4
        && enum_vals.iter().all(|v| v.is_string() || v.is_null())
    {
        let has_null = enum_vals.iter().any(Value::is_null) || explicit_nullable;
        let literals: Vec<String> = enum_vals
            .iter()
            .filter_map(Value::as_str)
            .map(|s| format!("\"{s}\""))
            .collect();
        if literals.is_empty() {
            return ("null".to_string(), true);
        }
        return (literals.join("|"), has_null);
    }

    if let Some(variants) = prop_schema
        .get("anyOf")
        .or_else(|| prop_schema.get("oneOf"))
        .and_then(Value::as_array)
        && !variants.is_empty()
    {
        let mut nullable = explicit_nullable;
        let mut labels = Vec::new();
        for variant in variants {
            let (label, var_null) = compact_json_type(variant);
            if var_null || label == "null" {
                nullable = true;
            }
            if label != "null" && label != "any" && !labels.contains(&label) {
                labels.push(label);
            }
        }
        if labels.is_empty() {
            return (
                if nullable {
                    "null".to_string()
                } else {
                    "any".to_string()
                },
                nullable,
            );
        }
        return (labels.join("|"), nullable);
    }

    match prop_schema.get("type") {
        Some(Value::String(t)) => {
            if t == "array" {
                let item_ty = prop_schema
                    .get("items")
                    .map(|items| compact_json_type(items).0)
                    .unwrap_or_else(|| "any".to_string());
                (format!("{item_ty}[]"), explicit_nullable)
            } else {
                (t.clone(), explicit_nullable || t == "null")
            }
        }
        Some(Value::Array(types)) => {
            let mut nullable = explicit_nullable;
            let mut non_null = Vec::new();
            for item in types {
                if let Some(s) = item.as_str() {
                    if s == "null" {
                        nullable = true;
                    } else {
                        non_null.push(s.to_string());
                    }
                }
            }
            if non_null.is_empty() {
                (
                    if nullable {
                        "null".to_string()
                    } else {
                        "any".to_string()
                    },
                    nullable,
                )
            } else {
                (non_null.join("|"), nullable)
            }
        }
        _ => ("any".to_string(), explicit_nullable),
    }
}

/// True when a property's JSON Schema explicitly allows `null` (or is an unconstrained schema).
fn schema_allows_null(prop_schema: &Value) -> bool {
    let Some(obj) = prop_schema.as_object() else {
        return true;
    };
    if obj.get("nullable").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    if obj.get("const").is_some_and(Value::is_null) {
        return true;
    }
    if let Some(enum_vals) = obj.get("enum").and_then(Value::as_array) {
        return enum_vals.iter().any(Value::is_null);
    }
    if let Some(variants) = obj
        .get("anyOf")
        .or_else(|| obj.get("oneOf"))
        .and_then(Value::as_array)
    {
        return variants.iter().any(schema_allows_null);
    }
    match obj.get("type") {
        Some(Value::String(t)) => t == "null",
        Some(Value::Array(types)) => types.iter().any(|v| v.as_str() == Some("null")),
        Some(_) => false,
        // Unconstrained schema object (no `type`, `enum`, `const`, `anyOf`, `oneOf`) accepts any JSON value.
        None => !obj.contains_key("const"),
    }
}

/// Extract the first informative line of a description, stripping redundant
/// `[MCP:<server>]` prefixes, control characters, and truncating to `max_bytes`.
pub fn sanitize_one_line_summary(description: &str, max_bytes: usize) -> String {
    let trimmed = description.trim();
    let without_prefix = if let Some(rest) = trimmed.strip_prefix("[MCP:") {
        rest.split_once(']')
            .map(|(_, after)| after.trim())
            .unwrap_or(trimmed)
    } else {
        trimmed
    };

    let first_line = without_prefix
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");

    let mut collapsed = String::with_capacity(first_line.len());
    let mut prev_space = false;
    for ch in first_line.chars() {
        if ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}') {
            if !prev_space && !collapsed.is_empty() {
                collapsed.push(' ');
                prev_space = true;
            }
            continue;
        }
        if ch.is_whitespace() {
            if !prev_space && !collapsed.is_empty() {
                collapsed.push(' ');
                prev_space = true;
            }
        } else {
            collapsed.push(ch);
            prev_space = false;
        }
    }
    let collapsed = collapsed.trim();
    if collapsed.len() <= max_bytes {
        return collapsed.to_string();
    }

    let target = max_bytes.saturating_sub("…".len());
    let mut end = 0usize;
    for (idx, ch) in collapsed.char_indices() {
        let next = idx + ch.len_utf8();
        if next > target {
            break;
        }
        end = next;
    }
    format!("{}…", collapsed[..end].trim_end())
}

/// Search registered MCP tools and format full JSON schemas for matching entries.
pub fn search_mcp_tools(entries: &[McpToolEntry<'_>], input: &Value) -> String {
    if entries.is_empty() {
        return "No MCP tools are currently registered.".to_string();
    }

    let query = input
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let server_filter = match input.get("server") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return "`server` filter must be a non-empty string when provided.".to_string();
            }
            Some(trimmed)
        }
        Some(other) => {
            return format!(
                "`server` filter must be a string, got {}.",
                json_value_kind(other)
            );
        }
    };
    let exact_tool = input
        .get("tool")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let requested_limit = input
        .get("limit")
        .and_then(Value::as_u64)
        .and_then(|v| usize::try_from(v).ok());

    if let Some(tool_spec) = exact_tool {
        match find_target_mcp_tool(entries, server_filter, tool_spec) {
            Ok(entry) => return format_full_tool_documentation(entry),
            Err(err) => {
                // When an explicit server was given (or query is empty), never fall through to
                // returning tools from other servers.
                if query.is_empty() || server_filter.is_some() {
                    return err;
                }
            }
        }
    }

    let limit = requested_limit
        .unwrap_or(if query.is_empty() {
            MAX_SEARCH_LIMIT
        } else {
            DEFAULT_SEARCH_LIMIT
        })
        .clamp(1, MAX_SEARCH_LIMIT);

    let tokens: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();

    let has_exact_server_filter =
        server_filter.is_some_and(|srv| entries.iter().any(|e| e.server.eq_ignore_ascii_case(srv)));
    let mut scored: Vec<(u32, &McpToolEntry<'_>)> = Vec::new();
    for entry in entries {
        if let Some(srv) = server_filter {
            let matches_srv = if has_exact_server_filter {
                entry.server.eq_ignore_ascii_case(srv)
            } else {
                server_matches(entry.server, srv)
            };
            if !matches_srv {
                continue;
            }
        }
        if tokens.is_empty() {
            scored.push((1, entry));
            continue;
        }
        let score = score_entry(entry, &query.to_lowercase(), &tokens);
        if score > 0 {
            scored.push((score, entry));
        }
    }

    scored.sort_by(|(score_a, a), (score_b, b)| {
        score_b
            .cmp(score_a)
            .then_with(|| a.server.cmp(b.server))
            .then_with(|| a.upstream_tool.cmp(b.upstream_tool))
    });

    if scored.is_empty() {
        let mut msg = format!(
            "No MCP tools matched query={:?} server={:?}.",
            query,
            server_filter.unwrap_or("*")
        );
        msg.push_str("\n\nAvailable MCP servers:\n");
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for e in entries {
            *counts.entry(e.server).or_default() += 1;
        }
        for (srv, count) in counts {
            msg.push_str(&format!("  - {srv} ({count} tools)\n"));
        }
        return msg.trim_end().to_string();
    }

    let total_matches = scored.len();
    let shown = scored.into_iter().take(limit).collect::<Vec<_>>();
    let mut out = format!(
        "Found {total_matches} matching MCP tool(s) (showing {}):\n",
        shown.len()
    );
    for (_, entry) in shown {
        out.push('\n');
        out.push_str(&format_full_tool_documentation(entry));
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// Field-weighted relevance score inspired by `opencode`:
/// exact tool match (20), tool name substring (8), server substring (6),
/// description substring (4), parameter name/description substring (2).
fn score_entry(entry: &McpToolEntry<'_>, full_query_lower: &str, tokens: &[String]) -> u32 {
    let upstream_lower = entry.upstream_tool.to_lowercase();
    let reg_lower = entry.registered_name.to_lowercase();
    let server_lower = entry.server.to_lowercase();
    let desc_lower = entry.description.to_lowercase();

    let mut score = 0u32;
    if upstream_lower == full_query_lower || reg_lower == full_query_lower {
        score += 20;
    }

    let prop_text = extract_schema_searchable_text(entry.input_schema);
    for token in tokens {
        let mut token_score = 0u32;
        if upstream_lower == *token {
            token_score += 20;
        } else if upstream_lower.contains(token) || reg_lower.contains(token) {
            token_score += 8;
        }
        if server_lower.contains(token) {
            token_score += 6;
        }
        if desc_lower.contains(token) {
            token_score += 4;
        }
        if prop_text.contains(token) {
            token_score += 2;
        }
        score += token_score;
    }
    score
}

/// Flatten parameter names and descriptions from a JSON Schema into lowercase text.
fn extract_schema_searchable_text(schema: &Value) -> String {
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return String::new();
    };
    let mut out = String::new();
    for (name, prop) in props {
        out.push_str(&name.to_lowercase());
        out.push(' ');
        if let Some(desc) = prop.get("description").and_then(Value::as_str) {
            out.push_str(&desc.to_lowercase());
            out.push(' ');
        }
    }
    out
}

/// Render full documentation, compact signature, and pretty-printed JSON Schema
/// for one MCP tool.
pub fn format_full_tool_documentation(entry: &McpToolEntry<'_>) -> String {
    let signature = json_schema_to_compact_signature(entry.upstream_tool, entry.input_schema);
    let schema_pretty = serde_json::to_string_pretty(entry.input_schema)
        .unwrap_or_else(|_| entry.input_schema.to_string());
    let description = entry.description.trim();
    format!(
        "### {server}/{tool} (`{registered}`)\n\
         - **Server**: `{server}`\n\
         - **Tool**: `{tool}`\n\
         - **Risk**: `{risk}`\n\
         - **Signature**: `{signature}`\n\
         - **Call via**: `{MCP_CALL_TOOL_NAME}({{\"server\": \"{server}\", \"tool\": \"{tool}\", \"arguments\": {{...}}}})`\n\
         - **Description**: {desc}\n\
         - **Input Schema**:\n```json\n{schema_pretty}\n```",
        server = entry.server,
        tool = entry.upstream_tool,
        registered = entry.registered_name,
        risk = entry.risk.as_str(),
        desc = if description.is_empty() {
            "(none)"
        } else {
            description
        },
    )
}

/// Resolve a [`MCP_CALL_TOOL_NAME`] request against the currently registered MCP tools.
pub fn resolve_mcp_call(entries: &[McpToolEntry<'_>], input: &Value) -> McpCallResolution {
    let Some(obj) = input.as_object() else {
        return McpCallResolution::Invalid {
            canonical_name: None,
            error: format!(
                "`{MCP_CALL_TOOL_NAME}` expects a JSON object with `{{\"tool\": \"...\", \"server\": \"...\", \"arguments\": {{...}}}}`."
            ),
        };
    };

    let server_hint = match obj.get("server") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return McpCallResolution::Invalid {
                    canonical_name: None,
                    error: format!(
                        "`{MCP_CALL_TOOL_NAME}` parameter `server` must be a non-empty string when provided."
                    ),
                };
            }
            Some(trimmed)
        }
        Some(other) => {
            return McpCallResolution::Invalid {
                canonical_name: None,
                error: format!(
                    "`{MCP_CALL_TOOL_NAME}` parameter `server` must be a string, got {}.",
                    json_value_kind(other)
                ),
            };
        }
    };
    let Some(tool_spec) = obj
        .get("tool")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return McpCallResolution::Invalid {
            canonical_name: None,
            error: format!(
                "`{MCP_CALL_TOOL_NAME}` requires a non-empty string `tool` parameter (e.g. `{{\"server\": \"loctree-mcp\", \"tool\": \"context\", \"arguments\": {{}}}}`)."
            ),
        };
    };

    let entry = match find_target_mcp_tool(entries, server_hint, tool_spec) {
        Ok(entry) => entry,
        Err(error) => {
            return McpCallResolution::Invalid {
                canonical_name: None,
                error,
            };
        }
    };

    let canonical_name = entry.registered_name.to_string();

    // Check for explicit help request (`help: true` or `arguments: "?"` / `"help"`).
    let wants_help = obj.get("help").and_then(Value::as_bool).unwrap_or(false)
        || matches!(
            obj.get("arguments").and_then(Value::as_str).map(str::trim),
            Some("?" | "help" | "--help" | "-h")
        );
    if wants_help {
        return McpCallResolution::Help {
            canonical_name,
            documentation: format_full_tool_documentation(entry),
        };
    }

    let inner_arguments = match obj.get("arguments") {
        None => json!({}),
        Some(Value::Object(map)) => Value::Object(map.clone()),
        Some(Value::String(raw_json)) => {
            let trimmed = raw_json.trim();
            if trimmed.is_empty() {
                json!({})
            } else {
                match serde_json::from_str::<Value>(trimmed) {
                    Ok(Value::Object(map)) => Value::Object(map),
                    _ => {
                        return McpCallResolution::Invalid {
                            canonical_name: Some(canonical_name),
                            error: format!(
                                "Invalid `arguments` for MCP tool `{}/{}`: expected a JSON object, got string `{}`.\n\n{}",
                                entry.server,
                                entry.upstream_tool,
                                trimmed,
                                format_full_tool_documentation(entry)
                            ),
                        };
                    }
                }
            }
        }
        Some(other) => {
            return McpCallResolution::Invalid {
                canonical_name: Some(canonical_name),
                error: format!(
                    "Invalid `arguments` for MCP tool `{}/{}`: expected a JSON object, got {}.\n\n{}",
                    entry.server,
                    entry.upstream_tool,
                    json_value_kind(other),
                    format_full_tool_documentation(entry)
                ),
            };
        }
    };

    if let Err(validation_error) =
        validate_arguments_against_schema(&inner_arguments, entry.input_schema)
    {
        return McpCallResolution::Invalid {
            canonical_name: Some(canonical_name),
            error: format!(
                "Invalid arguments for MCP tool `{}/{}`: {validation_error}\n\n{}",
                entry.server,
                entry.upstream_tool,
                format_full_tool_documentation(entry)
            ),
        };
    }

    McpCallResolution::Resolved {
        canonical_name,
        arguments: inner_arguments,
    }
}

/// Resolve a direct tool name or alias (`<server>/<tool>`, `<server>:<tool>`,
/// `mcp__<server>_<tool>`, or unambiguous `<upstream_tool>`) to its canonical
/// registered name.
pub fn resolve_mcp_alias(entries: &[McpToolEntry<'_>], name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    find_target_mcp_tool(entries, None, trimmed)
        .ok()
        .map(|entry| entry.registered_name.to_string())
}

/// Locate a single MCP tool entry from `(server_hint, tool_spec)`.
///
/// Accepts:
/// - Exact `registered_name` (`mcp__<server>__<tool>`)
/// - Single-underscore variant (`mcp__<server>_<tool>`)
/// - `<server>/<tool>` or `<server>:<tool>`
/// - `(Some(server), <upstream_tool>)`
/// - Unambiguous `<upstream_tool>` across all registered MCP servers
///
/// Security / routing invariant: when a server is explicitly specified (via
/// `server_hint` or embedded in `tool_spec`), this function NEVER falls back to
/// matching a tool on a different server.
fn find_target_mcp_tool<'a>(
    entries: &'a [McpToolEntry<'a>],
    server_hint: Option<&str>,
    tool_spec: &str,
) -> Result<&'a McpToolEntry<'a>, String> {
    let server_hint = server_hint.map(str::trim).filter(|s| !s.is_empty());
    let spec = tool_spec.trim();

    // 1. Decompose `<server>/<tool>`, `<server>:<tool>`, or `mcp__<server>__<tool>` / `mcp__<server>_<tool>`.
    let (inferred_server, inferred_tool) = if let Some(rest) = spec.strip_prefix("mcp__") {
        if let Some((srv, tl)) = rest.split_once("__") {
            (Some(srv.trim()), tl.trim())
        } else {
            // Handle single-underscore `mcp__<server>_<tool>` by matching against known server prefixes
            // (preferring exact server prefix matches over normalized `-`/`_` matches, then longest server name).
            let mut best_match: Option<(bool, &str, &str)> = None;
            let rest_lower = rest.to_ascii_lowercase();
            let rest_norm = normalize_ident(rest);
            for e in entries {
                let exact_prefix = format!("{}_", e.server.to_ascii_lowercase());
                let norm_prefix = format!("{}_", normalize_ident(e.server));
                if rest_lower.starts_with(&exact_prefix) && rest.len() > exact_prefix.len() {
                    let tl = &rest[exact_prefix.len()..];
                    if best_match.is_none_or(|(prev_exact, prev_srv, _)| {
                        !prev_exact || e.server.len() > prev_srv.len()
                    }) {
                        best_match = Some((true, e.server, tl));
                    }
                } else if rest_norm.starts_with(&norm_prefix) && rest.len() > norm_prefix.len() {
                    let tl = &rest[norm_prefix.len()..];
                    if best_match.is_none_or(|(prev_exact, prev_srv, _)| {
                        !prev_exact && e.server.len() > prev_srv.len()
                    }) {
                        best_match = Some((false, e.server, tl));
                    }
                }
            }
            match best_match {
                Some((_, srv, tl)) => (Some(srv), tl),
                None => {
                    return Err(format!(
                        "MCP tool `{spec}` is not registered (unknown server prefix in `mcp__` identifier)."
                    ));
                }
            }
        }
    } else if let Some((srv, tl)) = spec.split_once('/') {
        (Some(srv.trim()), tl.trim())
    } else if let Some((srv, tl)) = spec.split_once(':') {
        (Some(srv.trim()), tl.trim())
    } else {
        (None, spec)
    };

    if inferred_tool.is_empty() {
        return Err(format!(
            "Invalid MCP tool specification `{spec}`: tool name is empty."
        ));
    }

    // 2. Reject conflicting server specifications (`server` argument vs prefix in `tool`).
    if let (Some(hint), Some(inferred)) = (server_hint, inferred_server) {
        let hint_exact = entries
            .iter()
            .find(|e| e.server.eq_ignore_ascii_case(hint))
            .map(|e| e.server);
        let inferred_exact = entries
            .iter()
            .find(|e| e.server.eq_ignore_ascii_case(inferred))
            .map(|e| e.server);
        let distinct_exact_servers = match (hint_exact, inferred_exact) {
            (Some(h), Some(i)) => !h.eq_ignore_ascii_case(i),
            _ => false,
        };
        if !server_matches(inferred, hint) || distinct_exact_servers {
            return Err(format!(
                "Conflicting MCP server specifications: `server` is `{hint}` but `tool` specifies `{inferred}`."
            ));
        }
    }

    let effective_server = server_hint.or(inferred_server);

    // 3. Exact registered name match (`mcp__<server>__<tool>`), respecting `effective_server`.
    if let Some(found) = entries
        .iter()
        .find(|e| e.registered_name.eq_ignore_ascii_case(spec))
    {
        if let Some(srv) = effective_server {
            let srv_has_exact_other = entries.iter().any(|e| {
                e.server.eq_ignore_ascii_case(srv) && !e.server.eq_ignore_ascii_case(found.server)
            });
            if !server_matches(found.server, srv) || srv_has_exact_other {
                return Err(format!(
                    "Conflicting MCP server specifications: `server` is `{srv}` but `{spec}` belongs to server `{}`.",
                    found.server
                ));
            }
        }
        return Ok(found);
    }

    // 4. Match with explicit server constraint when provided. NEVER fall back to other servers.
    //    Prefer exact case-insensitive server ID match before normalized `-`/`_` alias matching so
    //    coexisting servers like `alpha-beta` and `alpha_beta` never collide.
    if let Some(srv) = effective_server {
        let exact_server_candidates: Vec<&McpToolEntry<'a>> = entries
            .iter()
            .filter(|e| e.server.eq_ignore_ascii_case(srv))
            .collect();
        let candidates: Vec<&McpToolEntry<'a>> = if !exact_server_candidates.is_empty() {
            exact_server_candidates
        } else {
            let norm_candidates: Vec<&McpToolEntry<'a>> = entries
                .iter()
                .filter(|e| server_matches(e.server, srv))
                .collect();
            let mut distinct_servers: Vec<&str> =
                norm_candidates.iter().map(|e| e.server).collect();
            distinct_servers.sort_unstable();
            distinct_servers.dedup();
            if distinct_servers.len() > 1 {
                return Err(format!(
                    "MCP server `{srv}` is ambiguous across multiple registered servers: {}. Specify the exact server ID.",
                    distinct_servers.join(", ")
                ));
            }
            norm_candidates
        };
        if candidates.is_empty() {
            let mut servers: Vec<&str> = entries.iter().map(|e| e.server).collect();
            servers.sort_unstable();
            servers.dedup();
            return Err(format!(
                "MCP server `{srv}` is not registered. Available MCP servers: {}.",
                if servers.is_empty() {
                    "(none)".to_string()
                } else {
                    servers.join(", ")
                }
            ));
        }
        if let Some(exact) = candidates
            .iter()
            .copied()
            .find(|e| e.upstream_tool.eq_ignore_ascii_case(inferred_tool))
            .or_else(|| {
                candidates
                    .iter()
                    .copied()
                    .find(|e| normalize_ident(e.upstream_tool) == normalize_ident(inferred_tool))
            })
        {
            return Ok(exact);
        }
        let available: Vec<String> = candidates
            .iter()
            .take(10)
            .map(|e| format!("`{}`", e.upstream_tool))
            .collect();
        return Err(format!(
            "MCP tool `{inferred_tool}` is not registered on server `{srv}`. Available tools on `{srv}`: {}.",
            available.join(", ")
        ));
    }

    // 5. No server specified: match across all servers by `upstream_tool`.
    let matches: Vec<&McpToolEntry<'a>> = entries
        .iter()
        .filter(|e| {
            e.upstream_tool.eq_ignore_ascii_case(inferred_tool)
                || normalize_ident(e.upstream_tool) == normalize_ident(inferred_tool)
        })
        .collect();

    if matches.len() == 1 {
        return Ok(matches[0]);
    }
    if matches.len() > 1 {
        let options: Vec<String> = matches
            .iter()
            .map(|e| format!("{}/{} (`{}`)", e.server, e.upstream_tool, e.registered_name))
            .collect();
        return Err(format!(
            "MCP tool `{inferred_tool}` is ambiguous across multiple servers: {}. Specify `server` explicitly.",
            options.join(", ")
        ));
    }

    // 6. Not found: suggest closest tools so the model can self-correct without an extra search turn.
    let tokens: Vec<String> = inferred_tool
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let mut suggestions: Vec<(u32, &McpToolEntry<'a>)> = entries
        .iter()
        .filter_map(|e| {
            let s = score_entry(e, &inferred_tool.to_lowercase(), &tokens);
            (s > 0).then_some((s, e))
        })
        .collect();
    suggestions.sort_by(|(sa, a), (sb, b)| {
        sb.cmp(sa)
            .then_with(|| a.server.cmp(b.server))
            .then_with(|| a.upstream_tool.cmp(b.upstream_tool))
    });

    if suggestions.is_empty() {
        Err(format!(
            "MCP tool `{spec}` is not registered. Call `{MCP_SEARCH_TOOL_NAME}` to list available MCP tools."
        ))
    } else {
        let top: Vec<String> = suggestions
            .into_iter()
            .take(5)
            .map(|(_, e)| {
                format!(
                    "`{}/{}` ({})",
                    e.server,
                    e.upstream_tool,
                    json_schema_to_compact_signature(e.upstream_tool, e.input_schema)
                )
            })
            .collect();
        Err(format!(
            "MCP tool `{spec}` is not registered. Closest matches: {}.",
            top.join(", ")
        ))
    }
}

/// Match MCP server names flexibly (`loctree-mcp` matches `loctree-mcp` or `loctree_mcp`).
fn server_matches(registered_server: &str, query_server: &str) -> bool {
    registered_server.eq_ignore_ascii_case(query_server)
        || normalize_ident(registered_server) == normalize_ident(query_server)
}

/// Normalize `-` and `_` for resilient identifier comparison.
fn normalize_ident(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '-' {
                '_'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

/// Validate top-level `required` fields and property types (including `null`ability)
/// against a tool's JSON Schema. Returns `Err(human_reason)` when validation fails
/// so the caller can attach the full schema for self-correction.
pub fn validate_arguments_against_schema(arguments: &Value, schema: &Value) -> Result<(), String> {
    let Some(arg_obj) = arguments.as_object() else {
        return Err("arguments must be a JSON object".to_string());
    };

    let props = schema.get("properties").and_then(Value::as_object);

    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        let mut missing = Vec::new();
        for req in required.iter().filter_map(Value::as_str) {
            match arg_obj.get(req) {
                None => missing.push(format!("`{req}`")),
                Some(Value::Null) => {
                    let allows_null = props
                        .and_then(|p| p.get(req))
                        .is_some_and(schema_allows_null);
                    if !allows_null {
                        missing.push(format!("`{req}`"));
                    }
                }
                _ => {}
            }
        }
        if !missing.is_empty() {
            return Err(format!(
                "missing required parameter(s): {}",
                missing.join(", ")
            ));
        }
    }

    let disallow_additional =
        schema.get("additionalProperties").and_then(Value::as_bool) == Some(false);

    if let Some(props) = props {
        for (key, value) in arg_obj {
            let Some(prop_schema) = props.get(key) else {
                if disallow_additional {
                    return Err(format!("unknown parameter `{key}`"));
                }
                continue;
            };
            if !value_matches_schema_type(value, prop_schema) {
                let (expected, _) = compact_json_type(prop_schema);
                return Err(format!(
                    "parameter `{key}` expected type `{expected}`, got {}",
                    json_value_kind(value)
                ));
            }
        }
    } else if disallow_additional && !arg_obj.is_empty() {
        let first_key = arg_obj.keys().next().expect("non-empty");
        return Err(format!("unknown parameter `{first_key}`"));
    }

    Ok(())
}

/// Check whether a JSON value (including `null`) matches a property schema's declared
/// `type`, `enum`, `const`, `nullable`, `anyOf`, or `oneOf`.
fn value_matches_schema_type(value: &Value, prop_schema: &Value) -> bool {
    if value.is_null() {
        return schema_allows_null(prop_schema);
    }

    if let Some(const_val) = prop_schema.get("const")
        && value != const_val
    {
        return false;
    }

    if let Some(enum_vals) = prop_schema.get("enum").and_then(Value::as_array)
        && !enum_vals.is_empty()
        && !enum_vals.contains(value)
    {
        return false;
    }

    if let Some(variants) = prop_schema
        .get("anyOf")
        .or_else(|| prop_schema.get("oneOf"))
        .and_then(Value::as_array)
        && !variants.is_empty()
        && !variants.iter().any(|v| value_matches_schema_type(value, v))
    {
        return false;
    }

    match prop_schema.get("type") {
        Some(Value::String(expected)) => matches_single_json_type(value, expected),
        Some(Value::Array(types)) => {
            let type_strs: Vec<&str> = types.iter().filter_map(Value::as_str).collect();
            if type_strs.is_empty() {
                true
            } else {
                type_strs
                    .iter()
                    .any(|expected| matches_single_json_type(value, expected))
            }
        }
        _ => true,
    }
}

fn matches_single_json_type(value: &Value, expected: &str) -> bool {
    match expected {
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn json_value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entries() -> Vec<(String, String, String, String, Value, ToolRisk)> {
        vec![
            (
                "mcp__loctree-mcp__context".to_string(),
                "loctree-mcp".to_string(),
                "context".to_string(),
                "[MCP:loctree-mcp] Materialize the holographic repository context atlas."
                    .to_string(),
                json!({
                    "type": "object",
                    "properties": {
                        "project": { "type": "string", "description": "Repository root path" },
                        "depth": { "type": "integer", "description": "Max traversal depth" }
                    },
                    "required": ["project"]
                }),
                ToolRisk::ReadOnly,
            ),
            (
                "mcp__loctree-mcp__find".to_string(),
                "loctree-mcp".to_string(),
                "find".to_string(),
                "[MCP:loctree-mcp] Find symbol definitions and literal occurrences.".to_string(),
                json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Symbol or literal pattern" },
                        "project": { "type": "string", "description": "Repository path" },
                        "literal": { "type": "boolean" }
                    },
                    "required": ["name", "project"]
                }),
                ToolRisk::ReadOnly,
            ),
            (
                "mcp__desktop-commander__write_file".to_string(),
                "desktop-commander".to_string(),
                "write_file".to_string(),
                "[MCP:desktop-commander] Write content to a file on disk.\nSecond line ignored."
                    .to_string(),
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" },
                        "mode": { "type": "string", "enum": ["rewrite", "append"] }
                    },
                    "required": ["path", "content"]
                }),
                ToolRisk::Mutating,
            ),
        ]
    }

    fn as_mcp_entries(
        raw: &[(String, String, String, String, Value, ToolRisk)],
    ) -> Vec<McpToolEntry<'_>> {
        raw.iter()
            .map(|(reg, srv, tool, desc, schema, risk)| McpToolEntry {
                registered_name: reg,
                server: srv,
                upstream_tool: tool,
                description: desc,
                input_schema: schema,
                risk: *risk,
            })
            .collect()
    }

    #[test]
    fn renders_compact_typescript_style_signatures() {
        let raw = sample_entries();
        let entries = as_mcp_entries(&raw);
        assert_eq!(
            json_schema_to_compact_signature(entries[0].upstream_tool, entries[0].input_schema),
            "context(project: string, depth?: integer)"
        );
        assert_eq!(
            json_schema_to_compact_signature(entries[2].upstream_tool, entries[2].input_schema),
            "write_file(content: string, path: string, mode?: \"rewrite\"|\"append\")"
        );
    }

    #[test]
    fn round_robin_catalog_never_starves_small_servers() {
        let mut raw = Vec::new();
        for i in 0..30 {
            raw.push((
                format!("mcp__huge-server__tool_{i:02}"),
                "huge-server".to_string(),
                format!("tool_{i:02}"),
                format!("Verbose tool {i} description with plenty of text to fill budget"),
                json!({
                    "type": "object",
                    "properties": { "arg": { "type": "string" } },
                    "required": ["arg"]
                }),
                ToolRisk::ReadOnly,
            ));
        }
        raw.push((
            "mcp__tiny-server__vital_probe".to_string(),
            "tiny-server".to_string(),
            "vital_probe".to_string(),
            "Critical probe on small server".to_string(),
            json!({"type": "object"}),
            ToolRisk::ReadOnly,
        ));

        let entries = as_mcp_entries(&raw);
        let catalog = render_mcp_catalog(&entries, 800);
        assert!(
            catalog.contains("[tiny-server] (1 tools)"),
            "small server must not be starved by huge-server:\n{catalog}"
        );
        assert!(catalog.contains("vital_probe()"));
        assert!(catalog.contains("[huge-server] ("));
    }

    #[test]
    fn resolves_calls_and_self_corrects_invalid_arguments_with_schema() {
        let raw = sample_entries();
        let entries = as_mcp_entries(&raw);

        // Valid call by (server, tool)
        let resolved = resolve_mcp_call(
            &entries,
            &json!({
                "server": "loctree-mcp",
                "tool": "context",
                "arguments": { "project": "/repo" }
            }),
        );
        assert_eq!(
            resolved,
            McpCallResolution::Resolved {
                canonical_name: "mcp__loctree-mcp__context".to_string(),
                arguments: json!({ "project": "/repo" }),
            }
        );

        // Help request via arguments: "?"
        let help = resolve_mcp_call(
            &entries,
            &json!({
                "tool": "loctree-mcp/find",
                "arguments": "?"
            }),
        );
        match help {
            McpCallResolution::Help {
                canonical_name,
                documentation,
            } => {
                assert_eq!(canonical_name, "mcp__loctree-mcp__find");
                assert!(documentation.contains("\"required\": ["));
            }
            other => panic!("expected Help, got {other:?}"),
        }

        // Missing required parameter `name` returns Invalid with full schema attached
        let invalid = resolve_mcp_call(
            &entries,
            &json!({
                "server": "loctree-mcp",
                "tool": "find",
                "arguments": { "project": "/repo" }
            }),
        );
        match invalid {
            McpCallResolution::Invalid {
                canonical_name,
                error,
            } => {
                assert_eq!(canonical_name.as_deref(), Some("mcp__loctree-mcp__find"));
                assert!(error.contains("missing required parameter(s): `name`"));
                assert!(error.contains("### loctree-mcp/find"));
                assert!(error.contains("Symbol or literal pattern"));
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn explicit_server_never_falls_back_to_another_server() {
        let raw = sample_entries();
        let entries = as_mcp_entries(&raw);

        // 1. Explicit `server: "loctree-mcp"` with tool `write_file` (which only exists on `desktop-commander`)
        let wrong_server = resolve_mcp_call(
            &entries,
            &json!({
                "server": "loctree-mcp",
                "tool": "write_file",
                "arguments": { "path": "/tmp/a.txt", "content": "x" }
            }),
        );
        match wrong_server {
            McpCallResolution::Invalid {
                canonical_name,
                error,
            } => {
                assert_eq!(canonical_name, None);
                assert!(error.contains("not registered on server `loctree-mcp`"));
            }
            other => panic!("must not substitute desktop-commander, got {other:?}"),
        }

        // 2. Explicit `server` conflicting with `tool` prefix (`loctree-mcp` vs `desktop-commander/write_file`)
        let conflicting = resolve_mcp_call(
            &entries,
            &json!({
                "server": "loctree-mcp",
                "tool": "desktop-commander/write_file",
                "arguments": { "path": "/tmp/a.txt", "content": "x" }
            }),
        );
        assert!(matches!(
            conflicting,
            McpCallResolution::Invalid {
                canonical_name: None,
                ..
            }
        ));

        // 3. Explicit `server` conflicting with canonical `mcp__desktop-commander__write_file`
        let conflicting_canonical = resolve_mcp_call(
            &entries,
            &json!({
                "server": "loctree-mcp",
                "tool": "mcp__desktop-commander__write_file",
                "arguments": { "path": "/tmp/a.txt", "content": "x" }
            }),
        );
        assert!(matches!(
            conflicting_canonical,
            McpCallResolution::Invalid {
                canonical_name: None,
                ..
            }
        ));

        // 4. Unregistered server in `server` or `tool` prefix never resolves to another server
        let unknown_srv = resolve_mcp_call(
            &entries,
            &json!({
                "server": "nonexistent-mcp",
                "tool": "write_file",
                "arguments": { "path": "/tmp/a.txt", "content": "x" }
            }),
        );
        assert!(matches!(
            unknown_srv,
            McpCallResolution::Invalid {
                canonical_name: None,
                ..
            }
        ));

        let unknown_prefix = resolve_mcp_call(
            &entries,
            &json!({
                "tool": "nonexistent-mcp/write_file",
                "arguments": { "path": "/tmp/a.txt", "content": "x" }
            }),
        );
        assert!(matches!(
            unknown_prefix,
            McpCallResolution::Invalid {
                canonical_name: None,
                ..
            }
        ));
    }

    #[test]
    fn catalog_and_search_definition_never_exceed_6000_byte_budget() {
        let mut raw = Vec::new();
        for s in 0..12 {
            for t in 0..20 {
                raw.push((
                    format!("mcp__server-with-long-name-{s:02}__instrument_tool_{t:02}"),
                    format!("server-with-long-name-{s:02}"),
                    format!("instrument_tool_{t:02}"),
                    format!(
                        "Detailed description for tool {t} on server {s} with enough text to test the 6000 byte budget boundary accurately."
                    ),
                    json!({
                        "type": "object",
                        "properties": {
                            "target_path": { "type": "string" },
                            "recursive_scan": { "type": "boolean" },
                            "max_depth": { "type": "integer" }
                        },
                        "required": ["target_path"]
                    }),
                    ToolRisk::ReadOnly,
                ));
            }
        }
        let entries = as_mcp_entries(&raw);
        let catalog = render_mcp_catalog(&entries, MCP_CATALOG_BUDGET_BYTES);
        assert!(
            catalog.len() <= MCP_CATALOG_BUDGET_BYTES,
            "catalog len {} exceeded {}",
            catalog.len(),
            MCP_CATALOG_BUDGET_BYTES
        );

        let search_def = build_mcp_search_tool_definition(&entries);
        assert!(
            search_def.description.len() <= MCP_CATALOG_BUDGET_BYTES,
            "search tool description len {} exceeded {}",
            search_def.description.len(),
            MCP_CATALOG_BUDGET_BYTES
        );
    }

    #[test]
    fn schema_validation_handles_null_for_optional_and_nullable_required_fields() {
        let schema = json!({
            "type": "object",
            "properties": {
                "project": { "type": "string" },
                "depth": { "type": "integer" },
                "mode": { "type": "string", "enum": ["rewrite", "append"] },
                "cursor": { "type": ["string", "null"] },
                "filter": { "anyOf": [{ "type": "string" }, { "type": "null" }] }
            },
            "required": ["project", "cursor"]
        });

        // 1. Nullable required field (`cursor: null`) is accepted when its schema allows `null`.
        assert!(
            validate_arguments_against_schema(
                &json!({ "project": "/repo", "cursor": null, "filter": null }),
                &schema
            )
            .is_ok()
        );

        // 2. Non-nullable optional field (`depth: null` or `mode: null`) is rejected.
        let err_depth = validate_arguments_against_schema(
            &json!({ "project": "/repo", "cursor": null, "depth": null }),
            &schema,
        )
        .expect_err("null for non-nullable integer must fail");
        assert!(err_depth.contains("parameter `depth` expected type `integer`, got null"));

        let err_mode = validate_arguments_against_schema(
            &json!({ "project": "/repo", "cursor": null, "mode": null }),
            &schema,
        )
        .expect_err("null for non-nullable enum must fail");
        assert!(err_mode.contains("parameter `mode`"));

        // 3. Non-nullable required field (`project: null`) is rejected.
        let err_req = validate_arguments_against_schema(
            &json!({ "project": null, "cursor": "abc" }),
            &schema,
        )
        .expect_err("null for non-nullable required string must fail");
        assert!(err_req.contains("missing required parameter(s): `project`"));

        // 4. Compact signature keeps required nullable fields as `name: type|null` (not `name?: type`).
        let nullable_req_schema = json!({
            "type": "object",
            "properties": {
                "cursor": { "type": ["string", "null"] },
                "filter": { "anyOf": [{ "type": "string" }, { "type": "null" }] }
            },
            "required": ["cursor"]
        });
        assert_eq!(
            json_schema_to_compact_signature("next", &nullable_req_schema),
            "next(cursor: string|null, filter?: string|null)"
        );
    }

    #[test]
    fn distinct_servers_differing_only_by_dash_vs_underscore_never_collide() {
        let raw = vec![
            (
                "mcp__alpha-beta__status".to_string(),
                "alpha-beta".to_string(),
                "status".to_string(),
                "Alpha-beta status".to_string(),
                json!({ "type": "object" }),
                ToolRisk::ReadOnly,
            ),
            (
                "mcp__alpha_beta__status".to_string(),
                "alpha_beta".to_string(),
                "status".to_string(),
                "Alpha_beta status".to_string(),
                json!({ "type": "object" }),
                ToolRisk::ReadOnly,
            ),
        ];
        let entries = as_mcp_entries(&raw);

        assert_eq!(
            resolve_mcp_call(
                &entries,
                &json!({
                    "server": "alpha_beta",
                    "tool": "status",
                    "arguments": {}
                })
            ),
            McpCallResolution::Resolved {
                canonical_name: "mcp__alpha_beta__status".to_string(),
                arguments: json!({}),
            }
        );

        assert_eq!(
            resolve_mcp_call(
                &entries,
                &json!({
                    "server": "alpha-beta",
                    "tool": "status",
                    "arguments": {}
                })
            ),
            McpCallResolution::Resolved {
                canonical_name: "mcp__alpha-beta__status".to_string(),
                arguments: json!({}),
            }
        );

        assert_eq!(
            resolve_mcp_call(
                &entries,
                &json!({
                    "tool": "mcp__alpha_beta_status",
                    "arguments": {}
                })
            ),
            McpCallResolution::Resolved {
                canonical_name: "mcp__alpha_beta__status".to_string(),
                arguments: json!({}),
            }
        );

        let cross_conflict = resolve_mcp_call(
            &entries,
            &json!({
                "server": "alpha_beta",
                "tool": "mcp__alpha-beta__status",
                "arguments": {}
            }),
        );
        assert!(matches!(
            cross_conflict,
            McpCallResolution::Invalid {
                canonical_name: None,
                ..
            }
        ));
    }
}
