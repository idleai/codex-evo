use super::*;
use pretty_assertions::assert_eq;

#[cfg(unix)]
#[tokio::test]
async fn detached_runner_does_not_approve_its_pending_command() -> Result<()> {
    let (home, checkout, chain) = (TempDir::new()?, TempDir::new()?, TempDir::new()?);
    let server = responses::start_mock_server().await;
    let requests = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_function_call(
                "approval",
                "exec_command",
                r#"{"cmd":"mkdir approval-marker","yield_time_ms":1000}"#,
            ),
            responses::ev_completed("approval"),
        ]),
    )
    .await;
    MockResponsesConfig::new(&server.uri())
        .with_approval_policy("on-request")
        .write(home.path())?;
    std::fs::create_dir_all(home.path().join("rules"))?;
    std::fs::write(
        home.path().join("rules/default.rules"),
        "prefix_rule(pattern=[\"mkdir\"], decision=\"prompt\")\n",
    )?;
    let mut owner = TestAppServer::builder()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let mut remote = connect(&mut owner, &checkout, &chain).await?;
    let started = rpc(&mut remote, "thread/start", json!({})).await?;
    let id = started["result"]["thread"]["id"]
        .as_str()
        .context("thread id")?;
    let response = rpc(
        &mut remote,
        "turn/start",
        json!({"threadId":id,"input":[{"type":"text","text":"Run a command needing review"}]}),
    )
    .await?;
    anyhow::ensure!(response.get("result").is_some(), "{response}");
    loop {
        let message = read(&mut remote).await?;
        anyhow::ensure!(
            message["method"] != "turn/completed",
            "command completed without the required approval"
        );
        if message["method"] == "item/commandExecution/requestApproval" {
            break;
        }
    }
    assert!(!checkout.path().join("approval-marker").exists());
    drop(remote);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!checkout.path().join("approval-marker").exists());
    assert_eq!(requests.requests().len(), 1);
    owner_rpc(
        &mut owner,
        "turn/interrupt",
        json!({"threadId":id,"turnId":response["result"]["turn"]["id"]}),
    )
    .await?;
    assert!(owner.shutdown_gracefully().await?.success());
    Ok(())
}
