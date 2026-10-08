use super::*;
use crate::idle_runtime::IdleRuntime;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::IdleRuntimeStatusReadParams;
use codex_app_server_protocol::IdleWorkspaceAttachParams;
use codex_app_server_protocol::RequestId;
use codex_app_server_transport::ConnectionOrigin;
use codex_idle_runtime::WorkspaceBinding;
use serde_json::json;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tempfile::TempDir;

struct Fixture {
    _home: TempDir,
    runtime: IdleRuntime,
    connections: Connections,
    events: mpsc::Receiver<TransportEvent>,
    sent: mpsc::Receiver<wire::Command>,
    grant: codex_idle_runtime::IssuedGrant,
}

fn setup() -> anyhow::Result<Fixture> {
    let home = TempDir::new()?;
    let runtime = IdleRuntime::new(home.path(), "host:one".into());
    for index in 1..=2 {
        let checkout = home.path().join(format!("checkout-{index}"));
        let chain = home.path().join(format!("chain-{index}"));
        std::fs::create_dir(&checkout)?;
        std::fs::create_dir(&chain)?;
        runtime.service.attach(WorkspaceBinding {
            workspace_id: format!("workspace:{index}"),
            repository_id: format!("repository:{index}"),
            checkout_id: format!("checkout:{index}"),
            chain_id: format!("chain:{index}"),
            checkout_root: checkout.canonicalize()?,
            chain_directory: chain.canonicalize()?,
        })?;
    }
    let expires =
        u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())? + 60_000;
    let grant = runtime
        .service
        .issue_grant("checkout:1", "client:one", expires)?;
    let (events, received) = mpsc::channel(16);
    let (commands, sent) = mpsc::channel(16);
    let connections = Connections::new(
        runtime.service.clone(),
        runtime.relay.grants.clone(),
        events,
        commands,
        "host:one".into(),
    );
    Ok(Fixture {
        _home: home,
        runtime,
        connections,
        events: received,
        sent,
        grant,
    })
}

#[tokio::test]
async fn idle_remote_authentication_scopes_status_and_rejects_other_methods() -> anyhow::Result<()>
{
    let Fixture {
        _home,
        runtime,
        mut connections,
        mut events,
        mut sent,
        grant,
    } = setup()?;
    connections.opened(1).await?;
    connections.incoming(1, json!({"kind":"authenticate","version":1,"grantId":grant.access.grant_id,"token":grant.token})).await?;
    let Some(TransportEvent::ConnectionOpened {
        connection_id,
        origin,
        writer: _writer,
        ..
    }) = events.recv().await
    else {
        anyhow::bail!("expected authenticated connection");
    };
    assert_eq!(origin, ConnectionOrigin::IdleRemote);
    assert!(matches!(
        sent.recv().await,
        Some(wire::Command::Send { .. })
    ));
    let status = runtime
        .status(
            origin,
            connection_id,
            IdleRuntimeStatusReadParams {
                protocol_version: 1,
            },
        )
        .await
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(status.status.workspaces.len(), 1);
    assert_eq!(
        status.status.workspaces[0].binding.workspace_id,
        "workspace:1"
    );
    let approved = status.status.workspaces[0].binding.clone();
    let request = |binding| ClientRequest::IdleWorkspaceAttach {
        request_id: RequestId::Integer(2),
        params: IdleWorkspaceAttachParams {
            protocol_version: 1,
            binding,
        },
    };
    assert!(
        runtime
            .authorize_request(origin, connection_id, &request(approved.clone()))
            .is_ok()
    );
    let mut wrong = approved.clone();
    wrong.chain_id = "chain:2".into();
    assert!(
        runtime
            .authorize_request(origin, connection_id, &request(wrong))
            .is_err()
    );
    for method in [
        "thread/start",
        "process/spawn",
        "command/exec",
        "fs/readFile",
        "idle/connection/invite",
        "account/read",
    ] {
        connections
            .incoming(1, json!({"id":3,"method":method,"params":{}}))
            .await?;
        let Some(wire::Command::Send { message, .. }) = sent.recv().await else {
            anyhow::bail!("expected denial");
        };
        assert!(message.get("error").is_some(), "{method} was not denied");
        assert!(events.try_recv().is_err());
    }
    std::fs::remove_dir(&approved.checkout_root)?;
    let unavailable = runtime
        .attach(
            origin,
            connection_id,
            IdleWorkspaceAttachParams {
                protocol_version: 1,
                binding: approved,
            },
        )
        .await
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(unavailable.status.workspaces.len(), 1);
    assert!(!unavailable.status.workspaces[0].available);
    runtime.service.revoke_grant(&grant.access.grant_id)?;
    assert!(
        runtime
            .status(
                origin,
                connection_id,
                IdleRuntimeStatusReadParams {
                    protocol_version: 1
                }
            )
            .await
            .is_err()
    );
    connections.prune().await?;
    assert!(matches!(
        events.recv().await,
        Some(TransportEvent::ConnectionClosed { .. })
    ));
    connections.close_all().await;
    Ok(())
}

#[tokio::test]
async fn idle_remote_denies_wrong_secrets_and_unauthenticated_rpc() -> anyhow::Result<()> {
    let Fixture {
        _home,
        runtime: _runtime,
        mut connections,
        mut events,
        mut sent,
        grant,
    } = setup()?;
    for (id, message) in [
        (
            1,
            json!({"kind":"authenticate","version":1,"grantId":grant.access.grant_id,"token":"wrong"}),
        ),
        (
            2,
            json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"codex-idle-owner","version":"1"}}}),
        ),
    ] {
        connections.opened(id).await?;
        connections.incoming(id, message).await?;
        assert!(matches!(
            sent.recv().await,
            Some(wire::Command::Send { .. })
        ));
        assert!(matches!(
            sent.recv().await,
            Some(wire::Command::Close { .. })
        ));
        assert!(events.try_recv().is_err());
    }
    connections.close_all().await;
    Ok(())
}
