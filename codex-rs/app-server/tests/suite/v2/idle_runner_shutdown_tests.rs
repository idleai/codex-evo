use super::super::daemon_update_recovery;
use super::*;
use codex_app_server_protocol::InitializeCapabilities;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn managed_daemon_drains_runner_and_rejects_new_work() -> Result<()> {
    let (home, checkout, chain) = (TempDir::new()?, TempDir::new()?, TempDir::new()?);
    let mock = responses::start_mock_server().await;
    let requests = responses::mount_sse_sequence(
        &mock,
        vec![
            execution::command("drain", json!({"cmd":"printf started > started; sleep 3; printf completed > completed","yield_time_ms":10000})),
            execution::finished(),
        ],
    ).await;
    MockResponsesConfig::new(&mock.uri()).write(home.path())?;
    let socket = home.path().join("control/server.sock");
    let mut daemon = daemon_update_recovery::spawn_server(home.path(), &socket)?;
    let mut owner = daemon_update_recovery::connect_daemon_client(
        &socket,
        InitializeCapabilities {
            experimental_api: true,
            ..Default::default()
        },
    )
    .await?;
    daemon_update_recovery::request(&mut owner, /*id*/ 2, "idle/workspace/attach", json!({"protocolVersion":1,"binding":{
        "workspaceId":"workspace","repositoryId":"repository","checkoutId":"checkout","chainId":"chain",
        "checkoutRoot":checkout.path().canonicalize()?,"chainDirectory":chain.path().canonicalize()?
    }})).await?;
    let helper = codex_utils_cargo_bin::cargo_bin("codex-app-server-test-idle-helper")?;
    daemon_update_recovery::request(
        &mut owner,
        /*id*/ 3,
        "idle/relay/configure",
        json!({"protocolVersion":1,"helperPath":helper,"credentialProgram":helper}),
    )
    .await?;
    let expires = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )? + 300_000;
    let invitation = daemon_update_recovery::request(&mut owner, /*id*/ 4, "idle/connection/invite", json!({"protocolVersion":1,"checkoutId":"checkout","clientId":"owner","expiresAt":expires,"runner":true})).await?;
    let mut remote = remote_from_invitation(invitation).await?;
    let id = execution::start(&mut remote).await?;
    execution::turn(&mut remote, &id).await?;
    execution::wait_file(&checkout.path().join("started")).await?;
    daemon_update_recovery::request_shutdown(&daemon, &socket).await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        daemon.try_wait()?.is_none(),
        "daemon exited before its runner completed"
    );
    let rejected = rpc(
        &mut remote,
        "turn/start",
        json!({"threadId":id,"input":[{"type":"text","text":"Must not start during drain"}]}),
    )
    .await?;
    assert!(rejected.get("error").is_some(), "{rejected}");
    drop(remote);
    let status = timeout(Duration::from_secs(15), daemon.wait()).await??;
    assert!(status.success(), "{status}");
    assert_eq!(
        std::fs::read_to_string(checkout.path().join("completed"))?,
        "completed"
    );
    assert_eq!(requests.requests().len(), 2);
    Ok(())
}
