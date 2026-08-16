//! Tool allow/deny filtering.
//!
//! The deny-list is a co-equal headline defense against attacker C (a
//! compromised or careless agent holding valid credentials) — the only
//! control that works against it. A denied tool is hidden from `tools/list`
//! and rejected by `tools/call`; `ToolRouter::disable_route` enforces both on
//! whichever transport the server is served over (stdio and HTTP alike).
//!
//! Semantics:
//! - Default: all 12 tools enabled.
//! - If the allow list is non-empty, ONLY listed tools are enabled.
//! - The deny list subtracts from the allow list.
//!
//! `filtered` is wired into `main.rs`'s stdio path; the HTTP transport's
//! per-session factory (`http::build_mcp_service`) deliberately keeps
//! `ComputerUseServer::new()` per the T2i integration spec.

use crate::config::Config;
use crate::server::ComputerUseServer;
use rmcp::handler::server::tool::ToolRouter;

/// The canonical tool names this server exposes.
pub const ALL_TOOL_NAMES: [&str; 12] = [
    "left_click",
    "left_double_click",
    "right_click",
    "middle_click",
    "mouse_move",
    "scroll",
    "screenshot",
    "key",
    "type",
    "start_recording",
    "stop_recording",
    "add_recording_marker",
];

/// Parse a comma-separated tool list: trim whitespace, skip empty entries.
///
/// Test-only helper in the binary (config parsing has its own `csv_list`);
/// kept `pub` so the tests can exercise the exact parsing the CLI documents.
#[allow(dead_code)]
pub fn parse_tool_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(String::from)
        .collect()
}

/// The set of tool names that must be DISABLED given allow+deny.
///
/// DISABLED = (if allow is non-empty: `{all tools} \ allow`) ∪ (deny \ allow).
/// A tool listed in BOTH allow and deny stays enabled (allow wins); the
/// deny list only subtracts tools that the allow list would have kept.
/// Returns a sorted, deduplicated list.
pub fn denied_tools(allow: &[String], deny: &[String]) -> Vec<String> {
    let allow_set: std::collections::BTreeSet<&str> = allow.iter().map(String::as_str).collect();
    let mut denied: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if !allow.is_empty() {
        for name in ALL_TOOL_NAMES {
            if !allow_set.contains(name) {
                denied.insert(name.to_string());
            }
        }
    }
    denied.extend(
        deny.iter()
            .filter(|name| !allow_set.contains(name.as_str()))
            .cloned(),
    );
    denied.into_iter().collect()
}

/// Apply the filter to a router: `disable_route` every denied tool.
pub fn apply_filter(
    router: &mut ToolRouter<ComputerUseServer>,
    config: &Config,
) -> Result<(), anyhow::Error> {
    for name in denied_tools(&config.tool_allow, &config.tool_deny) {
        router.disable_route(name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CliValues, EnvMap};
    use crate::server::ComputerUseServer;
    use rmcp::ErrorData;
    use rmcp::RoleServer;
    use rmcp::model::{
        CallToolRequest, CallToolRequestParams, ClientJsonRpcMessage, ClientRequest, ErrorCode,
        JsonRpcMessage, JsonRpcRequest, JsonRpcResponse, ListToolsRequest, RequestId,
        ServerJsonRpcMessage, ServerResult,
    };
    use rmcp::service::serve_directly;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    // ------------------------------------------------------------------
    // parse_tool_list
    // ------------------------------------------------------------------

    #[test]
    fn parse_tool_list_basic() {
        assert_eq!(
            parse_tool_list("a,b, c"),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn parse_tool_list_empty() {
        assert!(parse_tool_list("").is_empty());
    }

    #[test]
    fn parse_tool_list_trims_and_skips_empty() {
        assert_eq!(
            parse_tool_list(" a ,,b "),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    // ------------------------------------------------------------------
    // denied_tools
    // ------------------------------------------------------------------

    #[test]
    fn denied_tools_deny_only() {
        assert_eq!(denied_tools(&[], &["type".to_string()]), vec!["type"]);
    }

    #[test]
    fn denied_tools_allow_mode_disables_rest() {
        let allow = vec!["screenshot".to_string(), "mouse_move".to_string()];
        let denied = denied_tools(&allow, &[]);
        assert_eq!(denied.len(), ALL_TOOL_NAMES.len() - 2);
        assert!(!denied.contains(&"screenshot".to_string()));
        assert!(!denied.contains(&"mouse_move".to_string()));
        assert!(denied.contains(&"type".to_string()));
        assert!(denied.contains(&"left_click".to_string()));
    }

    #[test]
    fn denied_tools_deny_subtracts_from_allow() {
        let allow = vec!["screenshot".to_string()];
        let deny = vec!["screenshot".to_string()];
        let denied = denied_tools(&allow, &deny);
        assert_eq!(denied.len(), ALL_TOOL_NAMES.len() - 1);
        assert!(!denied.contains(&"screenshot".to_string()));
        assert!(denied.contains(&"type".to_string()));
    }

    #[test]
    fn denied_tools_empty_lists() {
        assert!(denied_tools(&[], &[]).is_empty());
    }

    // ------------------------------------------------------------------
    // Router-level: the important one. X11-free.
    // ------------------------------------------------------------------

    fn config_with_tool_deny(deny: &[&str]) -> Config {
        let cli = CliValues {
            tool_deny: Some(deny.join(",")),
            ..Default::default()
        };
        Config::from_sources(&cli, &EnvMap::new(), None).expect("config resolves")
    }

    async fn list_tool_names(server: &ComputerUseServer) -> Vec<String> {
        let (server_stream, client_stream) = tokio::io::duplex(4096);
        let running = serve_directly::<RoleServer, _, _, _, _>(server.clone(), server_stream, None);
        let _handle = tokio::spawn(async move {
            let _ = running.waiting().await;
        });
        let (mut reader, mut writer) = tokio::io::split(client_stream);
        let mut reader = tokio::io::BufReader::new(&mut reader);

        let request = ClientJsonRpcMessage::Request(JsonRpcRequest::new(
            RequestId::Number(1),
            ClientRequest::ListToolsRequest(ListToolsRequest::default()),
        ));
        let mut line = serde_json::to_string(&request).unwrap();
        line.push('\n');
        writer.write_all(line.as_bytes()).await.unwrap();

        let mut buf = Vec::new();
        reader.read_until(b'\n', &mut buf).await.unwrap();
        let msg: ServerJsonRpcMessage = serde_json::from_slice(&buf).unwrap();
        match msg {
            JsonRpcMessage::Response(JsonRpcResponse { result, .. }) => match result {
                ServerResult::ListToolsResult(list) => {
                    list.tools.iter().map(|t| t.name.to_string()).collect()
                }
                other => panic!("unexpected list result: {other:?}"),
            },
            other => panic!("unexpected list response: {other:?}"),
        }
    }

    async fn call_tool(server: &ComputerUseServer, name: &str) -> Result<(), ErrorData> {
        let (server_stream, client_stream) = tokio::io::duplex(4096);
        let running = serve_directly::<RoleServer, _, _, _, _>(server.clone(), server_stream, None);
        let _handle = tokio::spawn(async move {
            let _ = running.waiting().await;
        });
        let (mut reader, mut writer) = tokio::io::split(client_stream);
        let mut reader = tokio::io::BufReader::new(&mut reader);

        let request = ClientJsonRpcMessage::Request(JsonRpcRequest::new(
            RequestId::Number(2),
            ClientRequest::CallToolRequest(CallToolRequest::new(CallToolRequestParams::new(
                name.to_string(),
            ))),
        ));
        let mut line = serde_json::to_string(&request).unwrap();
        line.push('\n');
        writer.write_all(line.as_bytes()).await.unwrap();

        let mut buf = Vec::new();
        reader.read_until(b'\n', &mut buf).await.unwrap();
        let msg: ServerJsonRpcMessage = serde_json::from_slice(&buf).unwrap();
        match msg {
            JsonRpcMessage::Error(e) => Err(e.error),
            JsonRpcMessage::Response(_) => Ok(()),
            other => panic!("unexpected call response: {other:?}"),
        }
    }

    #[tokio::test]
    async fn denied_tools_hidden_from_list() {
        let server = ComputerUseServer::filtered(&config_with_tool_deny(&["type", "key"]))
            .expect("filtered server");
        let names = list_tool_names(&server).await;
        assert!(!names.contains(&"type".to_string()));
        assert!(!names.contains(&"key".to_string()));
        assert!(names.contains(&"screenshot".to_string()));
        assert!(names.contains(&"stop_recording".to_string()));
        assert_eq!(names.len(), ALL_TOOL_NAMES.len() - 2);
    }

    #[tokio::test]
    async fn denied_tool_call_returns_method_not_found_error() {
        let server = ComputerUseServer::filtered(&config_with_tool_deny(&["type", "key"]))
            .expect("filtered server");
        let err = call_tool(&server, "type")
            .await
            .expect_err("denied tool must not execute");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert_eq!(err.message, "tool not found");
    }

    #[tokio::test]
    async fn allowed_tool_routes_to_handler() {
        // stop_recording with no recording in progress fails fast with a
        // handler-level error — it never touches X11 — while proving the
        // tool ROUTED to the real handler rather than being blocked.
        let server = ComputerUseServer::filtered(&config_with_tool_deny(&["type", "key"]))
            .expect("filtered server");
        let err = call_tool(&server, "stop_recording")
            .await
            .expect_err("no recording in progress");
        assert_eq!(err.code, ErrorCode::INVALID_REQUEST);
        assert_eq!(err.message, "No recording in progress");
    }

    #[tokio::test]
    async fn default_server_lists_all_tools() {
        let server = ComputerUseServer::new();
        let names = list_tool_names(&server).await;
        assert_eq!(names.len(), ALL_TOOL_NAMES.len());
        for name in ALL_TOOL_NAMES {
            assert!(names.contains(&name.to_string()), "missing {name}");
        }
    }
}
