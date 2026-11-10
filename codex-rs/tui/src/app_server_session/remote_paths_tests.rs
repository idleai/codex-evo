//! Exercise a Unix TUI against Windows-native app-server JSON over WebSocket.

use super::*;
use crate::legacy_core::config::ConfigBuilder;
use codex_app_server_client::AppServerEvent;
use codex_app_server_protocol::ServerRequest;
use futures::SinkExt;
use futures::StreamExt;
use serde_json::Value;
use serde_json::json;
use std::sync::Mutex;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

const THREAD_ID: &str = "01900000-0000-7000-8000-000000000001";
const FORK_ID: &str = "01900000-0000-7000-8000-000000000002";

fn response(cwd: &str, root: &str, id: &str) -> Value {
    json!({
        "thread": {
            "id": id, "sessionId": id, "preview": "", "ephemeral": true,
            "modelProvider": "openai", "createdAt": 0, "updatedAt": 0,
            "status": {"type": "idle"}, "cwd": cwd, "cliVersion": "test",
            "source": "cli", "turns": []
        },
        "model": "test-model", "modelProvider": "openai", "cwd": cwd,
        "runtimeWorkspaceRoots": [cwd, root], "approvalPolicy": "on-request",
        "approvalsReviewer": "user",
        "sandbox": {
            "type": "workspaceWrite", "writableRoots": [root],
            "networkAccess": false, "excludeTmpdirEnvVar": true, "excludeSlashTmp": true
        }
    })
}

#[tokio::test]
async fn windows_remote_paths_survive_start_turn_resume_fork_and_reconnect() -> Result<()> {
    for (cwd, root) in [
        (r"C:\Users\alice\project with spaces", r"D:\shared\assets"),
        (
            r"\\build-server\workspaces\project",
            r"\\build-server\shared\assets",
        ),
    ] {
        let home = tempfile::tempdir()?;
        let config = ConfigBuilder::default()
            .codex_home(home.path().to_path_buf())
            .build()
            .await?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("ws://{}", listener.local_addr()?);
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let recorded = Arc::clone(&requests);
        let fixture = tokio::spawn(async move {
            // Reconnecting must decode the same server paths on a fresh transport.
            for _ in 0..2 {
                let (stream, _) = listener.accept().await?;
                let mut socket = accept_async(stream).await?;
                while let Some(frame) = socket.next().await {
                    let Message::Text(text) = frame? else { break };
                    let message: Value = serde_json::from_str(&text)?;
                    if message.get("method").is_none() || message.get("id").is_none() {
                        continue;
                    }
                    recorded
                        .lock()
                        .expect("requests lock")
                        .push(message.clone());
                    let method = message["method"].as_str().expect("method");
                    let result = match method {
                        "initialize" => json!({
                            "userAgent": "windows-path-fixture", "codexHome": r"C:\Users\alice\.codex",
                            "platformFamily": "windows", "platformOs": "windows"
                        }),
                        "thread/start" | "thread/resume" => response(cwd, root, THREAD_ID),
                        "thread/fork" => response(cwd, root, FORK_ID),
                        "thread/read" => {
                            json!({"thread": response(cwd, root, THREAD_ID)["thread"]})
                        }
                        "turn/start" => {
                            json!({"turn": {"id": "turn-1", "items": [], "status": "inProgress"}})
                        }
                        "thread/turns/list" => json!({"data": [], "nextCursor": null}),
                        other => panic!("unexpected remote request {other}: {message}"),
                    };
                    socket
                        .send(Message::Text(
                            json!({"id": message["id"], "result": result})
                                .to_string()
                                .into(),
                        ))
                        .await?;
                    if method == "turn/start" {
                        socket.send(Message::Text(json!({
                            "id": "approval-1", "method": "item/commandExecution/requestApproval",
                            "params": {
                                "threadId": THREAD_ID, "turnId": "turn-1", "itemId": "command-1",
                                "startedAtMs": 0, "command": "Write-Output cross-platform-ok", "cwd": cwd
                            }
                        }).to_string().into())).await?;
                    }
                }
            }
            anyhow::Ok(())
        });
        let client = crate::connect_remote_app_server(crate::RemoteAppServerEndpoint::WebSocket {
            websocket_url: url.clone(),
            auth_token: None,
        })
        .await?;
        let mut server = AppServerSession::new(
            crate::backend_id::BackendId::new(),
            client,
            ThreadParamsMode::Remote,
        );
        let started = server.start_thread(&config).await?;
        assert_eq!(started.session.cwd, PathBuf::from(cwd));
        assert_eq!(
            started.session.runtime_workspace_roots,
            vec![PathBuf::from(cwd), PathBuf::from(root)]
        );
        let (mut chat, _, _, _) =
            crate::chatwidget::tests::helpers::make_chatwidget_manual_with_sender().await;
        chat.handle_thread_session(started.session);
        let remote_config = chat.config_ref().clone();
        assert_eq!(remote_config.cwd, PathBuf::from(cwd));
        assert_eq!(remote_config.effective_workspace_roots().len(), 2);
        #[cfg(unix)]
        assert!(
            remote_config.native_config().is_err(),
            "foreign paths cannot become a local executor config"
        );
        let id = ThreadId::from_string(THREAD_ID)?;
        server
            .turn_start(
                id,
                "user",
                "client-message-1".into(),
                vec![UserInput::Text {
                    text: "run marker".into(),
                    text_elements: Vec::new(),
                }],
                remote_config.cwd.clone(),
                None,
                None,
                TurnPermissionsOverride::Preserve,
                &remote_config.workspace_roots,
                "test-model".into(),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await?;
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), server.next_event())
            .await?
            .expect("command approval event");
        let AppServerEvent::ServerRequest(request) = event.event else {
            panic!("expected approval")
        };
        let ServerRequest::CommandExecutionRequestApproval { request_id, params } = *request else {
            panic!("expected command approval")
        };
        assert_eq!(
            params
                .cwd
                .as_ref()
                .map(codex_utils_path_uri::LegacyAppPathString::as_str),
            Some(cwd)
        );
        server
            .resolve_server_request(request_id, json!({"decision": "accept"}))
            .await?;
        let local_settings = LocalSettings::from(&remote_config);
        let resumed = server
            .resume_thread(
                &local_settings,
                remote_config.clone(),
                id,
                ResumeModelSettings::RestoreFromThread,
            )
            .await?;
        assert_eq!(resumed.session.cwd, PathBuf::from(cwd));
        let forked = server
            .fork_thread_at(
                &local_settings,
                remote_config.clone(),
                id,
                None,
                None,
                ForkGoalContinuation::DeferUntilNextTurn,
                None,
            )
            .await?;
        assert_eq!(forked.session.cwd, PathBuf::from(cwd));
        assert_eq!(
            forked.session.runtime_workspace_roots,
            remote_config.workspace_roots
        );
        server.shutdown().await?;
        let client = crate::connect_remote_app_server(crate::RemoteAppServerEndpoint::WebSocket {
            websocket_url: url,
            auth_token: None,
        })
        .await?;
        let mut reconnected = AppServerSession::new(
            crate::backend_id::BackendId::new(),
            client,
            ThreadParamsMode::Remote,
        );
        let resumed = reconnected
            .resume_thread(
                &local_settings,
                remote_config,
                id,
                ResumeModelSettings::PreserveExistingThread,
            )
            .await?;
        assert_eq!(resumed.session.cwd, PathBuf::from(cwd));
        reconnected.shutdown().await?;
        tokio::time::timeout(std::time::Duration::from_secs(5), fixture)
            .await??
            .map_err(|err| color_eyre::eyre::eyre!(err.to_string()))?;
        let requests = requests.lock().expect("requests lock");
        for method in ["turn/start", "thread/fork"] {
            let request = requests
                .iter()
                .find(|request| request["method"] == method)
                .expect(method);
            assert_eq!(
                request["params"]["runtimeWorkspaceRoots"],
                json!([cwd, root])
            );
        }
        let turn = requests
            .iter()
            .find(|request| request["method"] == "turn/start")
            .expect("turn");
        assert_eq!(turn["params"]["cwd"], cwd);
        assert!(
            turn["params"]["sandboxPolicy"].is_null(),
            "the server retains execution permissions"
        );
    }
    Ok(())
}
