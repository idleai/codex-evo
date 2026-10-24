use super::*;
use codex_app_server_client::AppServerClient;
use codex_app_server_client::CloudClient;
use pretty_assertions::assert_eq;
use std::time::Duration;

#[tokio::test]
async fn dropping_server_releases_backend_with_an_open_mcp_session() -> color_eyre::Result<()> {
    let (client, mut commands, _backend_events) = CloudClient::channel(/*capacity*/ 1);
    let client = AppServerClient::Cloud(client);
    let (events, _events_rx) = tokio::sync::mpsc::unbounded_channel();
    let (status, _status_rx) = broadcast::channel(/*capacity*/ 1);
    let server = DynamicToolMcpServer::start(
        client.request_handle(),
        ThreadStartParams::default(),
        Features::default(),
        AppEventSender::new(events),
        status,
        /*managed_requirement*/ None,
        ToolServices {
            task_tools: true,
            worktrees: None,
        },
    )
    .await?;
    drop(client);

    let http = codex_http_client::HttpClientBuilder::new().build_direct()?;
    let url = server.config["url"].as_str().unwrap().to_owned();
    let authorization = server.config["http_headers"]["Authorization"]
        .as_str()
        .unwrap()
        .to_owned();
    let response = http
        .post(&url)
        .header("Authorization", &authorization)
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "tui-lifecycle-test", "version": "1"}
            }
        }))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let session_id = response.headers()["mcp-session-id"].to_str()?.to_owned();
    let body = response.text().await?;
    let result: Value = serde_json::from_str(
        body.lines()
            .find_map(|line| line.strip_prefix("data: ").filter(|data| !data.is_empty()))
            .unwrap_or(&body),
    )?;
    assert_eq!(result["result"]["protocolVersion"], "2025-11-25");
    let initialized = http
        .post(&url)
        .header("Authorization", &authorization)
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .header("Mcp-Session-Id", &session_id)
        .json(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .send()
        .await?;
    assert_eq!(initialized.status(), StatusCode::ACCEPTED);
    let stream = http
        .get(&url)
        .header("Authorization", &authorization)
        .header("Accept", "text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .header("Mcp-Session-Id", &session_id)
        .send()
        .await?;
    assert_eq!(stream.status(), StatusCode::OK);

    // Keep the session stream open while the TUI drops its MCP server.
    drop(server);
    let command = tokio::time::timeout(Duration::from_secs(/*secs*/ 5), commands.recv())
        .await
        .expect("dropping the MCP server must release its app-server request handle");
    assert!(command.is_none());
    drop(stream);
    drop(http);
    Ok(())
}
