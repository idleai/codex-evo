//! A native Unix terminal client must preserve paths returned by a Windows server.

use super::focus_palette::PtyCodex;
use super::focus_palette::write_test_config;
use anyhow::Result;
use anyhow::ensure;
use codex_app_server_protocol::JSONRPCMessage;
use futures::SinkExt;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::path::Path;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_unix_cli_uses_windows_remote_workspace_end_to_end() -> Result<()> {
    core_test_support::skip_if_sandbox!(Ok(()));
    let workspace = tempfile::tempdir()?;
    let home = tempfile::tempdir()?;
    write_test_config(home.path(), workspace.path())?;
    let cwd = r"C:\Users\alice\project with spaces";
    let extra_root = r"\\build-server\shared\assets";
    let thread_id = "01900000-0000-7000-8000-000000000001";
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("ws://{}", listener.local_addr()?);
    let (submitted_tx, submitted_rx) = tokio::sync::oneshot::channel();
    let fixture = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let mut submitted_tx = Some(submitted_tx);
        let thread = json!({
            "id": thread_id, "sessionId": thread_id, "preview": "", "ephemeral": false,
            "modelProvider": "openai", "createdAt": 1, "updatedAt": 2,
            "status": {"type": "idle"}, "cwd": cwd, "cliVersion": "test",
            "source": "cli", "turns": []
        });
        while let Some(Ok(Message::Text(text))) = socket.next().await {
            let JSONRPCMessage::Request(request) = serde_json::from_str(&text)? else {
                continue;
            };
            let result = match request.method.as_str() {
                "initialize" => json!({
                    "userAgent": "windows-paths-pty", "codexHome": r"C:\Users\alice\.codex",
                    "platformFamily": "windows", "platformOs": "windows"
                }),
                "account/read" => {
                    json!({"account": {"type": "apiKey"}, "requiresOpenaiAuth": false})
                }
                "model/list" => json!({"data": [], "nextCursor": null}),
                "experimentalFeature/list" => json!({"data": [], "nextCursor": null}),
                "configRequirements/read" => json!({"requirements": null}),
                "config/read" => json!({
                    "config": {"model": "gpt-5.6-terra", "model_provider": "openai",
                        "tui": {"status_line": ["thread-id"]},
                        "projects": {cwd: {"trust_level": "trusted"}}},
                    "origins": {}, "layers": [{
                        "name": {"type": "user", "file": r"C:\Users\alice\.codex\config.toml"},
                        "version": "test", "config": {}
                    }]
                }),
                "thread/start" | "thread/resume" => json!({
                    "thread": thread, "model": "gpt-5.6-terra", "modelProvider": "openai",
                    "cwd": cwd, "runtimeWorkspaceRoots": [cwd, extra_root],
                    "approvalPolicy": "on-request", "approvalsReviewer": "user",
                    "sandbox": {"type": "workspaceWrite", "writableRoots": [extra_root],
                        "networkAccess": false, "excludeTmpdirEnvVar": true, "excludeSlashTmp": true}
                }),
                "thread/read" => json!({"thread": thread}),
                "thread/goal/get" => json!({"goal": null}),
                "skills/list" | "hooks/list" => json!({"data": []}),
                "plugin/installed" | "plugin/list" => json!({
                    "marketplaces": [{"name": "test", "path": r"C:\Users\alice\.codex\marketplace.json", "plugins": []}],
                    "marketplaceLoadErrors": [], "featuredPluginIds": []
                }),
                "turn/start" => {
                    let params = request.params.as_ref().expect("turn params");
                    assert_eq!(params["cwd"], cwd);
                    assert_eq!(params["runtimeWorkspaceRoots"], json!([cwd, extra_root]));
                    assert_eq!(params["input"][0]["text"], "remote path probe");
                    ensure!(
                        params["sandboxPolicy"].is_null(),
                        "unexpected permission rewrite: {params}"
                    );
                    json!({"turn": {"id": "turn-1", "items": [], "status": "inProgress"}})
                }
                _ => {
                    socket
                        .send(Message::Text(
                            json!({"id": request.id, "error": {
                                "code": -32601, "message": "method not found"
                            }})
                            .to_string()
                            .into(),
                        ))
                        .await?;
                    continue;
                }
            };
            socket
                .send(Message::Text(
                    json!({"id": request.id, "result": result})
                        .to_string()
                        .into(),
                ))
                .await?;
            if request.method == "turn/start" {
                let command = json!({
                    "type": "commandExecution", "id": "command-1", "command": "Write-Output remote-path-ok",
                    "cwd": cwd, "status": "completed", "commandActions": [],
                    "aggregatedOutput": "remote-path-ok\n", "exitCode": 0, "durationMs": 1
                });
                let assistant = json!({"type": "agentMessage", "id": "assistant-1", "text": "Windows workspace completed."});
                for event in [
                    json!({"method": "turn/started", "params": {"threadId": thread_id, "turn": {"id": "turn-1", "status": "inProgress", "items": []}}}),
                    json!({"method": "item/completed", "params": {"threadId": thread_id, "turnId": "turn-1", "completedAtMs": 1, "item": command}}),
                    json!({"method": "item/completed", "params": {"threadId": thread_id, "turnId": "turn-1", "completedAtMs": 2, "item": assistant}}),
                    json!({"method": "turn/completed", "params": {"threadId": thread_id, "turn": {"id": "turn-1", "status": "completed", "items": []}}}),
                ] {
                    let _: codex_app_server_protocol::ServerNotification =
                        serde_json::from_value(event.clone())?;
                    socket.send(Message::Text(event.to_string().into())).await?;
                }
                submitted_tx
                    .take()
                    .expect("one turn")
                    .send(())
                    .expect("test waiting");
            }
        }
        anyhow::Ok(())
    });
    // The normal CLI parser receives a foreign -C value as well as --remote.
    let mut terminal = PtyCodex::start_cli(
        Path::new(cwd),
        home,
        &[
            "--remote",
            &url,
            "--no-alt-screen",
            "-c",
            r#"tui.status_line=["thread-id"]"#,
        ],
    )?;
    terminal.wait_for_startup()?;
    terminal.wait_for_screen(thread_id)?;
    terminal.write_input(b"remote path probe")?;
    // Keep literal typing separate from Enter so paste-burst handling is exercised normally.
    tokio::time::sleep(Duration::from_millis(250)).await;
    terminal.read_output(Duration::from_millis(100))?;
    terminal.write_input(b"\r")?;
    terminal.wait_for_screen("Windows workspace completed.")?;
    tokio::time::timeout(Duration::from_secs(5), submitted_rx).await??;
    ensure!(
        !terminal.screen_contains("AbsolutePathBuf"),
        "path decode failure reached the terminal"
    );
    drop(terminal);
    tokio::time::timeout(Duration::from_secs(5), fixture).await???;
    Ok(())
}
