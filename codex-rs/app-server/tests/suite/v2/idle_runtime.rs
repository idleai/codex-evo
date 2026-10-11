use super::connection_handling_websocket::connect_websocket;
use super::connection_handling_websocket::read_error_for_id;
use super::connection_handling_websocket::read_response_for_id;
use super::connection_handling_websocket::send_request;
use super::connection_handling_websocket::spawn_websocket_server;
use anyhow::Result;
use app_test_support::TestAppServer;
use codex_app_server_protocol::ClientInfo;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::IdleRuntimeCapability;
use codex_app_server_protocol::IdleRuntimeStatus;
use codex_app_server_protocol::IdleRuntimeStatusReadParams;
use codex_app_server_protocol::IdleRuntimeStatusReadResponse;
use codex_app_server_protocol::IdleWorkspaceAttachParams;
use codex_app_server_protocol::IdleWorkspaceAttachResponse;
use codex_app_server_protocol::IdleWorkspaceBinding;
use codex_app_server_protocol::InitializeCapabilities;
use codex_app_server_protocol::RequestId;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

async fn status(server: &mut TestAppServer) -> Result<IdleRuntimeStatus> {
    let response: IdleRuntimeStatusReadResponse = server
        .request(|request_id| ClientRequest::IdleRuntimeStatusRead {
            request_id,
            params: IdleRuntimeStatusReadParams {
                protocol_version: 1,
            },
        })
        .await?;
    Ok(response.status)
}

fn binding(checkout: &TempDir, chain: &TempDir) -> Result<IdleWorkspaceBinding> {
    Ok(IdleWorkspaceBinding {
        workspace_id: "workspace:one".into(),
        repository_id: "repository:one".into(),
        checkout_id: "checkout:one".into(),
        chain_id: "chain:one".into(),
        checkout_root: checkout.path().canonicalize()?.display().to_string(),
        chain_directory: chain.path().canonicalize()?.display().to_string(),
    })
}

#[tokio::test]
async fn idle_workspace_attachment_survives_app_server_restart() -> Result<()> {
    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let chain = TempDir::new()?;
    let binding = binding(&checkout, &chain)?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_auto_env()
        .build_initialized()
        .await?;
    let initial = status(&mut server).await?;
    assert!(initial.workspaces.is_empty());
    assert!(!home.path().join("idle-runtime").exists());
    assert_eq!(
        initial.capabilities,
        vec![
            IdleRuntimeCapability::WorkspaceAttachment,
            IdleRuntimeCapability::WorkspaceStatus,
            IdleRuntimeCapability::WorkspaceCoordination,
        ]
    );
    for _ in 0..2 {
        let attached: IdleWorkspaceAttachResponse = server
            .request(|request_id| ClientRequest::IdleWorkspaceAttach {
                request_id,
                params: IdleWorkspaceAttachParams {
                    protocol_version: 1,
                    binding: binding.clone(),
                },
            })
            .await?;
        assert_eq!(attached.status.workspaces.len(), 1);
        assert_eq!(attached.status.workspaces[0].binding, binding);
        assert!(attached.status.workspaces[0].available);
    }
    let first = status(&mut server).await?;
    assert!(
        timeout(Duration::from_secs(20), server.shutdown_gracefully())
            .await??
            .success()
    );
    let mut replacement = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_auto_env()
        .build_initialized()
        .await?;
    let restored = status(&mut replacement).await?;
    assert_eq!(restored.host_id, first.host_id);
    assert_ne!(restored.runtime_id, first.runtime_id);
    assert_eq!(restored.workspaces, first.workspaces);
    Ok(())
}

#[tokio::test]
async fn idle_workspace_attachment_survives_local_client_disconnect() -> Result<()> {
    use super::daemon_update_recovery::connect_daemon_client;
    use super::daemon_update_recovery::request;
    use super::daemon_update_recovery::request_shutdown;
    use super::daemon_update_recovery::spawn_server;

    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let chain = TempDir::new()?;
    let socket_path = codex_app_server_transport::app_server_control_socket_path(home.path())?;
    let mut server = spawn_server(home.path(), &socket_path)?;
    let capabilities = InitializeCapabilities {
        experimental_api: true,
        ..Default::default()
    };
    let mut first = connect_daemon_client(&socket_path, capabilities.clone()).await?;
    let attached = request(
        &mut first,
        /*id*/ 2,
        "idle/workspace/attach",
        json!({
            "protocolVersion": 1,
            "binding": binding(&checkout, &chain)?
        }),
    )
    .await?;
    first.close(None).await?;
    drop(first);
    assert!(server.try_wait()?.is_none());

    let mut second = connect_daemon_client(&socket_path, capabilities).await?;
    let current = request(
        &mut second,
        /*id*/ 2,
        "idle/runtime/status/read",
        json!({"protocolVersion": 1}),
    )
    .await?;
    assert_eq!(current, attached);
    second.close(None).await?;
    request_shutdown(&server, &socket_path).await?;
    assert!(
        timeout(Duration::from_secs(20), server.wait())
            .await??
            .success()
    );
    Ok(())
}

#[tokio::test]
async fn idle_workspace_requires_protocol_and_experimental_negotiation() -> Result<()> {
    let home = TempDir::new()?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_auto_env()
        .build()
        .await?;
    server
        .initialize_with_capabilities(
            ClientInfo {
                name: "idle-test".into(),
                title: None,
                version: "1".into(),
            },
            Some(InitializeCapabilities {
                experimental_api: false,
                ..Default::default()
            }),
        )
        .await?;
    let id = server
        .send_raw_request(
            "idle/runtime/status/read",
            Some(json!({"protocolVersion": 1})),
        )
        .await?;
    let error = timeout(
        Duration::from_secs(20),
        server.read_stream_until_error_message(RequestId::Integer(id)),
    )
    .await??;
    assert_eq!(error.error.code, -32600);
    assert_eq!(
        error.error.message,
        "idle/runtime/status/read requires experimentalApi capability"
    );
    assert!(
        timeout(Duration::from_secs(20), server.shutdown_gracefully())
            .await??
            .success()
    );
    let mut negotiated = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_auto_env()
        .build_initialized()
        .await?;
    let id = negotiated
        .send_raw_request(
            "idle/runtime/status/read",
            Some(json!({"protocolVersion": 99})),
        )
        .await?;
    let error = timeout(
        Duration::from_secs(20),
        negotiated.read_stream_until_error_message(RequestId::Integer(id)),
    )
    .await??;
    assert_eq!(error.error.code, -32602);
    assert!(!home.path().join("idle-runtime").exists());
    Ok(())
}

#[tokio::test]
async fn idle_workspace_rejects_tcp_clients_claiming_a_local_client_name() -> Result<()> {
    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let chain = TempDir::new()?;
    let (mut process, address) = spawn_websocket_server(home.path()).await?;
    let mut client = connect_websocket(address).await?;
    send_request(
        &mut client,
        "initialize",
        /*id*/ 1,
        Some(json!({
            "clientInfo": {"name": "Codex Desktop", "version": "1"},
            "capabilities": {"experimentalApi": true}
        })),
    )
    .await?;
    read_response_for_id(&mut client, /*id*/ 1).await?;
    for (method, params) in [
        ("idle/runtime/status/read", json!({"protocolVersion": 1})),
        (
            "idle/workspace/attach",
            json!({"protocolVersion": 1, "binding": binding(&checkout, &chain)?}),
        ),
        (
            "idle/coordination/call",
            json!({"protocolVersion":1,"checkoutId":"checkout:one","clientId":"owner","request":"{\"kind\":\"status\"}"}),
        ),
    ] {
        send_request(&mut client, method, /*id*/ 2, Some(params)).await?;
        let error = read_error_for_id(&mut client, /*id*/ 2).await?;
        assert_eq!(error.error.code, -32600);
        assert_eq!(
            error.error.message,
            "Idle workspace access requires a local owner connection"
        );
    }
    assert!(!home.path().join("idle-runtime").exists());
    process.kill().await?;
    Ok(())
}
