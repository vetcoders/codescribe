//! P2-001 regressions: operator-tool MCP rows (Loctree / AICX / Vibecrafted)
//! follow the evidence for the server that is actually configured, not a
//! hard-coded server name. Every test owns its evidence store, so parallel
//! registration tests cannot overwrite what these assert on.

use std::fs;
use std::net::TcpListener;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use codescribe_core::agent::ToolRegistry;
use serde_json::json;

use super::{
    AgenticReadinessReport, CoreReadiness, McpEvidence, McpRowTone, McpStatusFacet, McpStatusRow,
    McpStatusState, probe_agentic_readiness_with, probe_mcp_status_with, register_mcp_tools_into,
    test_configured_server_at,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(2);

fn core(provider_access_available: bool) -> CoreReadiness {
    CoreReadiness {
        provider_label: "OpenAI (Responses)".to_string(),
        key_env_key: "LLM_OPENAI_API_KEY".to_string(),
        provider_access_available,
        native_tool_count: 10,
        configured_workspace_roots: vec!["~/Git".to_string()],
        tool_workspace_roots: vec!["~/Git".to_string()],
    }
}

/// Streamable HTTP MCP fixture advertising `serverInfo.name == advertised`
/// and serving `tool_count` tools.
async fn http_fixture(advertised: &str, tool_count: usize) -> mockito::ServerGuard {
    use mockito::Matcher;

    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/mcp")
        .match_body(Matcher::PartialJson(json!({"method": "initialize"})))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_header("Mcp-Session-Id", "p2-001-session")
        .with_body(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "protocolVersion": "2025-06-18",
                    "serverInfo": {"name": advertised, "version": "0.15.0"}
                }
            })
            .to_string(),
        )
        .create_async()
        .await;
    server
        .mock("POST", "/mcp")
        .match_body(Matcher::PartialJson(
            json!({"method": "notifications/initialized"}),
        ))
        .with_status(202)
        .create_async()
        .await;
    let tools: Vec<_> = (0..tool_count)
        .map(|index| json!({"name": format!("tool_{index}"), "inputSchema": {"type": "object"}}))
        .collect();
    server
        .mock("POST", "/mcp")
        .match_body(Matcher::PartialJson(json!({"method": "tools/list"})))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!({"jsonrpc": "2.0", "id": 2, "result": {"tools": tools}}).to_string())
        .create_async()
        .await;
    server
}

/// An `http://127.0.0.1:<port>/mcp` endpoint nothing listens on, so every
/// connection is refused (the Founder's AICX shape).
fn refused_endpoint() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().expect("local addr").port();
    drop(listener);
    format!("http://127.0.0.1:{port}/mcp")
}

fn write_config(path: &Path, servers: serde_json::Value) {
    fs::write(path, json!({ "mcpServers": servers }).to_string()).expect("write mcp.json");
}

fn remote(url: &str) -> serde_json::Value {
    json!({"url": url, "timeout_seconds": 2})
}

fn readiness(path: &Path, store: &Mutex<McpEvidence>, ready: bool) -> AgenticReadinessReport {
    let evidence = store.lock().expect("evidence").clone();
    probe_agentic_readiness_with(path, core(ready), &evidence)
}

fn row(report: &AgenticReadinessReport, facet: McpStatusFacet) -> &McpStatusRow {
    report
        .summary_rows()
        .iter()
        .find(|row| row.facet == facet)
        .unwrap_or_else(|| panic!("{facet:?} row present"))
}

fn server_state(path: &Path, store: &Mutex<McpEvidence>, name: &str) -> McpStatusState {
    let evidence = store.lock().expect("evidence").clone();
    probe_mcp_status_with(path, &evidence)
        .summary_rows()
        .iter()
        .find(|row| row.facet == McpStatusFacet::McpServer && row.subject == name)
        .unwrap_or_else(|| panic!("server row {name}"))
        .state
}

/// Blocking connection test from inside the async fixture runtime.
fn connection_test(
    store: &Mutex<McpEvidence>,
    path: &Path,
    name: &str,
) -> anyhow::Result<codescribe_core::mcp::McpProbeSummary> {
    tokio::task::block_in_place(|| test_configured_server_at(store, path, name, TEST_TIMEOUT))
}

/// MCP rows are informational: whatever they say, the verdict is the core
/// gate's, in both directions.
fn assert_core_gate_alone_decides(path: &Path, store: &Mutex<McpEvidence>) {
    assert!(readiness(path, store, true).is_ready());
    assert!(!readiness(path, store, false).is_ready());
}

/// Founder reproduction (audit P2-001): a remote Loctree server configured as
/// `loctree-http` answers with identity `loctree` and live tools, yet the
/// Diagnostics row claimed "not configured (optional)". Red on be9a1de98.
#[tokio::test(flavor = "multi_thread")]
async fn loctree_http_with_live_tools_is_not_reported_unconfigured() {
    let fixture = http_fixture("loctree", 12).await;
    let temp = tempfile::tempdir().expect("temp dir");
    let path = temp.path().join("mcp.json");
    write_config(
        &path,
        json!({ "loctree-http": remote(&format!("{}/mcp", fixture.url())) }),
    );
    let store = Mutex::new(McpEvidence::default());

    let mut registry = ToolRegistry::new();
    let registered = tokio::task::block_in_place(|| {
        register_mcp_tools_into(&mut registry, &path, &store).expect("register")
    });
    assert_eq!(registered, 12);

    let report = readiness(&path, &store, true);
    let loctree = row(&report, McpStatusFacet::LoctreeMcp);
    assert_eq!(loctree.state, McpStatusState::Live, "got {}", loctree.value);
    assert_eq!(loctree.count, Some(12));
    assert_eq!(loctree.subject, "loctree-http");
    assert_eq!(loctree.tone, McpRowTone::Good);
    assert!(
        loctree.value.contains("advertised identity \"loctree\""),
        "selection is explained: {}",
        loctree.value
    );
    assert_core_gate_alone_decides(&path, &store);
}

/// An arbitrary server name says nothing; the identity a connection test
/// observed does. A passed test is reachability, never registration.
#[tokio::test(flavor = "multi_thread")]
async fn connection_test_identifies_an_arbitrary_name_without_claiming_registration() {
    let fixture = http_fixture("loctree", 3).await;
    let temp = tempfile::tempdir().expect("temp dir");
    let path = temp.path().join("mcp.json");
    write_config(
        &path,
        json!({ "code-map": remote(&format!("{}/mcp", fixture.url())) }),
    );
    let store = Mutex::new(McpEvidence::default());

    // Before any evidence the name proves nothing either way: uncertainty,
    // not "not configured" and not a guess.
    let before = readiness(&path, &store, true);
    for facet in [McpStatusFacet::LoctreeMcp, McpStatusFacet::AicxMcp] {
        let unknown = row(&before, facet);
        assert_eq!(unknown.state, McpStatusState::Unverified, "{facet:?}");
        assert_eq!(unknown.detail, "code-map");
        assert_eq!(unknown.tone, McpRowTone::Neutral);
    }

    let summary = connection_test(&store, &path, "code-map").expect("test passes");
    assert_eq!(summary.server_name.as_deref(), Some("loctree"));
    assert_eq!(summary.tool_count, 3);

    let after = readiness(&path, &store, true);
    let loctree = row(&after, McpStatusFacet::LoctreeMcp);
    assert_eq!(
        loctree.state,
        McpStatusState::Reachable,
        "got {}",
        loctree.value
    );
    assert_eq!(loctree.count, Some(3));
    assert_eq!(loctree.subject, "code-map");
    assert!(loctree.value.contains("not registered by the agent yet"));
    // The server now has a known identity, and it is not AICX.
    assert_eq!(
        row(&after, McpStatusFacet::AicxMcp).state,
        McpStatusState::NotConfigured
    );
    assert_eq!(
        server_state(&path, &store, "code-map"),
        McpStatusState::Reachable
    );
    assert_core_gate_alone_decides(&path, &store);
}

/// The Founder's AICX shape: a configured server whose endpoint refuses the
/// connection is still that server, failed — never "not configured". A refused
/// server with an arbitrary name is not attributed to anything.
#[tokio::test(flavor = "multi_thread")]
async fn refused_aicx_stays_configured_and_unknown_names_stay_unattributed() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = temp.path().join("mcp.json");
    write_config(
        &path,
        json!({
            "aicx": remote(&refused_endpoint()),
            "memory-box": remote(&refused_endpoint()),
        }),
    );
    let store = Mutex::new(McpEvidence::default());

    assert!(connection_test(&store, &path, "aicx").is_err());
    assert!(connection_test(&store, &path, "memory-box").is_err());

    let report = readiness(&path, &store, true);
    let aicx = row(&report, McpStatusFacet::AicxMcp);
    assert_eq!(
        aicx.state,
        McpStatusState::Unreachable,
        "got {}",
        aicx.value
    );
    assert_eq!(aicx.subject, "aicx");
    assert!(!aicx.detail.is_empty(), "the refusal reason is carried");
    assert_eq!(aicx.tone, McpRowTone::Warn);

    let loctree = row(&report, McpStatusFacet::LoctreeMcp);
    assert_eq!(loctree.state, McpStatusState::Unverified);
    assert_eq!(
        loctree.detail, "memory-box",
        "aicx is identified, memory-box is not"
    );
    assert_eq!(
        server_state(&path, &store, "memory-box"),
        McpStatusState::Unreachable
    );
    assert_core_gate_alone_decides(&path, &store);
}

/// Evidence is pinned to the entry it was gathered against: editing the
/// entry or renaming the server drops it, for tests and registration alike.
#[tokio::test(flavor = "multi_thread")]
async fn edited_or_renamed_servers_drop_stale_evidence() {
    let fixture = http_fixture("loctree", 2).await;
    let url = format!("{}/mcp", fixture.url());
    let temp = tempfile::tempdir().expect("temp dir");
    let path = temp.path().join("mcp.json");
    write_config(&path, json!({ "code-map": remote(&url) }));
    let store = Mutex::new(McpEvidence::default());

    connection_test(&store, &path, "code-map").expect("test passes");
    let mut registry = ToolRegistry::new();
    tokio::task::block_in_place(|| {
        register_mcp_tools_into(&mut registry, &path, &store).expect("register")
    });
    assert_eq!(
        row(&readiness(&path, &store, true), McpStatusFacet::LoctreeMcp).state,
        McpStatusState::Live
    );

    // Same name, different endpoint: neither the test nor the registration
    // describes this entry any more.
    write_config(&path, json!({ "code-map": remote(&refused_endpoint()) }));
    let edited = readiness(&path, &store, true);
    assert_eq!(
        row(&edited, McpStatusFacet::LoctreeMcp).state,
        McpStatusState::Unverified
    );
    assert_eq!(
        server_state(&path, &store, "code-map"),
        McpStatusState::Configured
    );

    // Same endpoint under a new name: evidence does not follow the rename.
    write_config(&path, json!({ "code-map-2": remote(&url) }));
    let renamed = readiness(&path, &store, true);
    let loctree = row(&renamed, McpStatusFacet::LoctreeMcp);
    assert_eq!(loctree.state, McpStatusState::Unverified);
    assert_eq!(loctree.detail, "code-map-2");
    assert_core_gate_alone_decides(&path, &store);
}

/// Canonical stdio entries keep their identity from configuration alone, even
/// when the process advertises a different name, and several matches name the
/// selected server and the others.
#[tokio::test(flavor = "multi_thread")]
async fn canonical_stdio_entries_stay_truthful_and_multiple_matches_are_explained() {
    let fixture = http_fixture("loctree", 4).await;
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("mock_mcp.py");
    let temp = tempfile::tempdir().expect("temp dir");
    let path = temp.path().join("mcp.json");
    write_config(
        &path,
        json!({
            "loctree-mcp": {"command": "python3", "args": [script], "timeout_seconds": 5},
            "code-map": remote(&format!("{}/mcp", fixture.url())),
            "lt": {"command": "/opt/homebrew/bin/aicx-mcp", "enabled": false},
        }),
    );
    let store = Mutex::new(McpEvidence::default());

    // Configured but unstarted canonical entry vs a tested remote one: the
    // stronger evidence (connection test passed) is selected and explained.
    connection_test(&store, &path, "code-map").expect("test passes");
    let tested = readiness(&path, &store, true);
    let loctree = row(&tested, McpStatusFacet::LoctreeMcp);
    assert_eq!(loctree.state, McpStatusState::Reachable);
    assert_eq!(loctree.subject, "code-map");
    assert!(
        loctree
            .value
            .contains("also matched: loctree-mcp (configured)"),
        "got {}",
        loctree.value
    );
    // Identified by its canonical command, disabled by config.
    let aicx = row(&tested, McpStatusFacet::AicxMcp);
    assert_eq!(aicx.state, McpStatusState::Disabled);
    assert_eq!(aicx.subject, "lt");
    assert!(
        aicx.value.contains("canonical command"),
        "got {}",
        aicx.value
    );
    // Nothing unknown remains, so absence is stated plainly.
    assert_eq!(
        row(&tested, McpStatusFacet::VibecraftedRuntime).state,
        McpStatusState::NotConfigured
    );

    // Registration: the canonical stdio entry advertises `mock-mcp`, yet its
    // key still identifies it, and a registered server outranks a tested one.
    let mut registry = ToolRegistry::new();
    tokio::task::block_in_place(|| {
        register_mcp_tools_into(&mut registry, &path, &store).expect("register")
    });
    let registered = readiness(&path, &store, true);
    let loctree = row(&registered, McpStatusFacet::LoctreeMcp);
    assert_eq!(loctree.state, McpStatusState::Live);
    assert_eq!(
        loctree.subject, "code-map",
        "both live; explicit evidence ties by name"
    );
    assert!(
        loctree.value.contains("also matched: loctree-mcp (live)"),
        "got {}",
        loctree.value
    );
    assert_core_gate_alone_decides(&path, &store);
}

// --- Registration vs later connection test (root review of P2-001) ----------

/// Self-contained stdio MCP server advertising `loctree` with two tools. It
/// exits before `initialize` while a `down` file sits next to it, so a test can
/// flip reachability without touching the `mcp.json` entry.
const TOGGLED_SERVER: &str = r#"import json, os, sys
if os.path.exists(os.path.join(os.path.dirname(os.path.abspath(__file__)), "down")):
    sys.exit(0)
for raw in sys.stdin:
    message = json.loads(raw)
    method, request_id = message.get("method"), message.get("id")
    if method == "initialize":
        result = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                  "serverInfo": {"name": "loctree", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": [{"name": "t1", "inputSchema": {"type": "object"}},
                            {"name": "t2", "inputSchema": {"type": "object"}}]}
    else:
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}), flush=True)
"#;

/// One unchanged `loctree-mcp` stdio entry plus the switch that takes it down.
struct ToggledServer {
    _temp: tempfile::TempDir,
    path: std::path::PathBuf,
    down: std::path::PathBuf,
}

impl ToggledServer {
    fn new() -> Self {
        Self::named("loctree-mcp")
    }

    fn named(name: &str) -> Self {
        let temp = tempfile::tempdir().expect("temp dir");
        let script = temp.path().join("toggled_mcp.py");
        fs::write(&script, TOGGLED_SERVER).expect("write fixture");
        let path = temp.path().join("mcp.json");
        write_config(
            &path,
            json!({ name: {"command": "python3", "args": [script], "timeout_seconds": 5} }),
        );
        let down = temp.path().join("down");
        Self {
            _temp: temp,
            path,
            down,
        }
    }

    fn set_up(&self, up: bool) {
        if up {
            let _ = fs::remove_file(&self.down);
        } else {
            fs::write(&self.down, "").expect("mark down");
        }
    }

    fn register(&self, store: &Mutex<McpEvidence>) -> usize {
        register_mcp_tools_into(&mut ToolRegistry::new(), &self.path, store).expect("register")
    }

    fn test(&self, store: &Mutex<McpEvidence>) -> bool {
        test_configured_server_at(store, &self.path, "loctree-mcp", TEST_TIMEOUT).is_ok()
    }

    fn loctree(&self, store: &Mutex<McpEvidence>) -> McpStatusRow {
        let report = readiness(&self.path, store, true);
        let row = row(&report, McpStatusFacet::LoctreeMcp);
        McpStatusRow {
            label: row.label.clone(),
            value: row.value.clone(),
            tone: row.tone,
            facet: row.facet,
            state: row.state,
            count: row.count,
            subject: row.subject.clone(),
            detail: row.detail.clone(),
        }
    }
}

#[test]
fn failed_retest_retains_handshake_identity_without_registration() {
    let server = ToggledServer::named("code-map");
    let store = Mutex::new(McpEvidence::default());
    test_configured_server_at(&store, &server.path, "code-map", TEST_TIMEOUT)
        .expect("initial handshake");
    assert_eq!(server.loctree(&store).state, McpStatusState::Reachable);

    server.set_up(false);
    assert!(test_configured_server_at(&store, &server.path, "code-map", TEST_TIMEOUT).is_err());
    let failed = server.loctree(&store);
    assert_eq!(failed.state, McpStatusState::Unreachable);
    assert_eq!(failed.subject, "code-map");
    assert_eq!(
        row(
            &readiness(&server.path, &store, true),
            McpStatusFacet::AicxMcp
        )
        .state,
        McpStatusState::NotConfigured
    );
    assert_core_gate_alone_decides(&server.path, &store);

    let config = fs::read_to_string(&server.path).expect("config");
    fs::write(
        &server.path,
        config.replace("timeout_seconds\":5", "timeout_seconds\":4"),
    )
    .expect("edit config");
    assert_eq!(server.loctree(&store).state, McpStatusState::Unverified);
}

#[test]
fn diagnostics_count_only_successfully_registered_tools() {
    let server = ToggledServer::new();
    let script = server._temp.path().join("toggled_mcp.py");
    fs::write(&script, TOGGLED_SERVER.replace(
        "{\"name\": \"t2\", \"inputSchema\": {\"type\": \"object\"}}",
        "{\"name\": \"t2\", \"inputSchema\": {\"type\": \"object\"}}, {\"name\": \"t1\", \"inputSchema\": {\"type\": \"object\"}}, {\"name\": \"bad/name\", \"inputSchema\": {\"type\": \"object\"}}"
    )).expect("tool fixture");
    let store = Mutex::new(McpEvidence::default());
    assert_eq!(server.register(&store), 2);
    let status = server.loctree(&store);
    assert_eq!(status.state, McpStatusState::Live);
    assert_eq!(status.count, Some(2));
}

/// Registration failed, then the user's Settings Test passed on the same
/// entry: Diagnostics must show the newer passing test without claiming the
/// agent registered anything. A newer registration then supersedes both.
#[test]
fn passing_test_after_failed_registration_is_shown_without_claiming_registration() {
    let server = ToggledServer::new();
    let store = Mutex::new(McpEvidence::default());

    server.set_up(false);
    assert_eq!(server.register(&store), 0);
    assert_eq!(server.loctree(&store).state, McpStatusState::Failed);

    server.set_up(true);
    assert!(server.test(&store), "test passes once the server is up");
    let row = server.loctree(&store);
    assert!(
        row.value.contains("last connection test passed"),
        "the newer test is visible: {}",
        row.value
    );
    assert!(
        row.value.contains("registration failed"),
        "the registration fact stays visible: {}",
        row.value
    );
    assert_ne!(
        row.state,
        McpStatusState::Live,
        "a test never registers tools"
    );
    assert_eq!(row.state, McpStatusState::FailedLastTestPassed);
    assert_eq!(row.count, Some(2), "count is the tested tool count");
    assert!(
        row.detail.contains("closed stdout"),
        "detail keeps the registration failure: {}",
        row.detail
    );
    assert_eq!(row.tone, McpRowTone::Warn);
    let evidence = store.lock().expect("evidence").clone();
    let server_row = probe_mcp_status_with(&server.path, &evidence);
    assert_eq!(
        server_row.summary_rows()[0].state,
        McpStatusState::FailedLastTestPassed,
        "the MCP server table reports the same facts"
    );
    assert_core_gate_alone_decides(&server.path, &store);

    // A registration recorded after the test is now the newest fact.
    assert_eq!(server.register(&store), 2);
    let row = server.loctree(&store);
    assert_eq!(row.state, McpStatusState::Live, "got {}", row.value);
    assert!(!row.value.contains("connection test"), "got {}", row.value);
}

/// Tools were registered, then the user's Settings Test failed on the same
/// entry: Diagnostics must keep the registered tools and show the newer
/// failure. An older failed test does not override a newer registration.
#[test]
fn failing_test_after_registration_is_shown_without_hiding_registered_tools() {
    let server = ToggledServer::new();
    let store = Mutex::new(McpEvidence::default());

    // Older failed test, newer registration: registration is the latest fact.
    server.set_up(false);
    assert!(!server.test(&store));
    server.set_up(true);
    assert_eq!(server.register(&store), 2);
    let row = server.loctree(&store);
    assert_eq!(row.state, McpStatusState::Live, "got {}", row.value);
    assert!(!row.value.contains("connection test"), "got {}", row.value);

    // Newer failed test: shown next to the tools the agent still holds.
    server.set_up(false);
    assert!(!server.test(&store));
    let row = server.loctree(&store);
    assert!(
        row.value.contains("last connection test failed"),
        "the newer failure is visible: {}",
        row.value
    );
    assert_eq!(row.count, Some(2), "registered tools are not erased");
    assert_eq!(row.state, McpStatusState::LiveLastTestFailed);
    assert!(
        row.detail.contains("closed stdout"),
        "detail is the test failure: {}",
        row.detail
    );
    assert_eq!(row.tone, McpRowTone::Warn);
    assert_core_gate_alone_decides(&server.path, &store);

    // A later passing test agrees with the registration: plain Live again.
    server.set_up(true);
    assert!(server.test(&store));
    let row = server.loctree(&store);
    assert_eq!(row.state, McpStatusState::Live, "got {}", row.value);
    assert_eq!(row.count, Some(2));

    // An edited entry still drops both kinds of evidence.
    let edited = fs::read_to_string(&server.path)
        .expect("read config")
        .replace("\"timeout_seconds\":5", "\"timeout_seconds\":6");
    fs::write(&server.path, edited).expect("edit config");
    assert_eq!(server.loctree(&store).state, McpStatusState::Configured);
}
