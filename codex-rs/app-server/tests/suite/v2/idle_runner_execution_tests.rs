use super::*;
use pretty_assertions::assert_eq;

pub(super) async fn wait_file(path: &std::path::Path) -> Result<String> {
    timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(path)
                && !text.is_empty()
            {
                return Ok(text);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?
}

pub(super) fn command(call: &str, args: Value) -> String {
    responses::sse(vec![
        responses::ev_function_call(call, "exec_command", &args.to_string()),
        responses::ev_completed(call),
    ])
}

pub(super) fn finished() -> String {
    responses::sse(vec![
        responses::ev_assistant_message("answer", "runner finished"),
        responses::ev_completed("finished"),
    ])
}

pub(super) async fn start(remote: &mut TcpStream) -> Result<String> {
    let response = rpc(remote, "thread/start", json!({})).await?;
    Ok(response["result"]["thread"]["id"]
        .as_str()
        .with_context(|| format!("runner start: {response}"))?
        .to_string())
}

pub(super) async fn turn(remote: &mut TcpStream, thread: &str) -> Result<()> {
    let response = rpc(
        remote,
        "turn/start",
        json!({"threadId":thread,"input":[{"type":"text","text":"Run the requested check"}]}),
    )
    .await?;
    anyhow::ensure!(response.get("result").is_some(), "{response}");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn code_mode_runner_exposes_commands_and_rechecks_nested_permissions() -> Result<()> {
    let (home, checkout, chain) = (TempDir::new()?, TempDir::new()?, TempDir::new()?);
    let server = responses::start_mock_server().await;
    let requests = responses::mount_sse_sequence(&server, vec![
        responses::sse(vec![
            responses::ev_custom_tool_call("wrapped", "exec", "text(await tools.exec_command({cmd: 'printf allowed > wrapped.txt', yield_time_ms: 1000})); text(await tools.exec_command({cmd: 'printf widened > wrapped.txt', sandbox_permissions: 'require_escalated'}));"),
            responses::ev_completed("wrapped"),
        ]),
        finished(),
    ]).await;
    MockResponsesConfig::new(&server.uri())
        .with_model("gpt-6-astra")
        .write(home.path())?;
    let mut owner = TestAppServer::builder()
        .without_auto_env()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let mut remote = connect(&mut owner, &checkout, &chain).await?;
    let id = start(&mut remote).await?;
    turn(&mut remote, &id).await?;
    complete(&mut remote).await?;
    let calls = requests.requests();
    let prompt = calls[0].body_json().to_string();
    assert!(prompt.contains("exec_command"), "{prompt}");
    let output = calls[1].custom_tool_call_output("wrapped").to_string();
    assert_eq!(
        std::fs::read_to_string(checkout.path().join("wrapped.txt"))
            .with_context(|| format!("Code Mode output: {output}"))?,
        "allowed"
    );
    assert!(
        output.contains("cannot request additional permissions"),
        "{output}"
    );
    assert!(owner.shutdown_gracefully().await?.success());
    Ok(())
}

async fn complete(remote: &mut TcpStream) -> Result<()> {
    loop {
        if read(remote).await?["method"] == "turn/completed" {
            return Ok(());
        }
    }
}

// This fixture binds the executor to the daemon itself. The generic execution
// ceiling also runs against the selected remote executor in core integration tests.
#[cfg(unix)]
#[tokio::test]
async fn native_sandbox_blocks_paths_network_and_per_call_escalation() -> Result<()> {
    let (home, checkout, chain) = (TempDir::new()?, TempDir::new()?, TempDir::new()?);
    let secret = "PRIVATE_RUNNER_TEST_CONTENT";
    std::fs::write(chain.path().join("secret"), secret)?;
    std::os::unix::fs::symlink(chain.path().join("secret"), checkout.path().join("escape"))?;
    let network = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = network.local_addr()?.port();
    let server = responses::start_mock_server().await;
    let requests = responses::mount_sse_sequence(&server, vec![
        command("scope", json!({"shell":"/bin/bash","cmd":format!("printf allowed > allowed.txt; cat escape; if (echo ping >/dev/tcp/127.0.0.1/{port}) 2>/dev/null; then echo NETWORK_OPEN; else echo NETWORK_DENIED; fi"),"yield_time_ms":1000})),
        command("escalate", json!({"cmd":"echo widened > allowed.txt","sandbox_permissions":"require_escalated","justification":"try escalation"})),
        command("outside", json!({"cmd":"cat secret","workdir":chain.path(),"yield_time_ms":1000})),
        finished(),
    ]).await;
    MockResponsesConfig::new(&server.uri()).write(home.path())?;
    let mut owner = TestAppServer::builder()
        .without_auto_env()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let mut remote = connect(&mut owner, &checkout, &chain).await?;
    let id = start(&mut remote).await?;
    turn(&mut remote, &id).await?;
    complete(&mut remote).await?;
    let calls = requests.requests();
    let scope = calls[1].function_call_output("scope").to_string();
    assert_eq!(
        std::fs::read_to_string(checkout.path().join("allowed.txt"))
            .with_context(|| format!("allowed command output: {scope}"))?,
        "allowed"
    );
    assert!(scope.contains("NETWORK_DENIED"), "{scope}");
    assert!(!scope.contains(secret), "{scope}");
    assert!(
        calls[2]
            .function_call_output("escalate")
            .to_string()
            .contains("cannot request additional permissions")
    );
    assert!(
        !calls[3]
            .function_call_output("outside")
            .to_string()
            .contains(secret)
    );
    assert!(
        timeout(Duration::from_millis(100), network.accept())
            .await
            .is_err()
    );
    assert!(owner.shutdown_gracefully().await?.success());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn runner_finishes_after_client_detach_and_reloads_its_saved_scope() -> Result<()> {
    let (home, checkout, chain) = (TempDir::new()?, TempDir::new()?, TempDir::new()?);
    let server = responses::start_mock_server().await;
    let requests = responses::mount_sse_sequence(&server, vec![
        command("detached", json!({"cmd":"printf started > started; sleep 2; printf completed > completed","yield_time_ms":10000})),
        finished(),
    ]).await;
    MockResponsesConfig::new(&server.uri())
        .with_root_config("thread_unload_delay_secs = 0")
        .write(home.path())?;
    let mut owner = TestAppServer::builder()
        .without_auto_env()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let mut remote = connect(&mut owner, &checkout, &chain).await?;
    let id = start(&mut remote).await?;
    turn(&mut remote, &id).await?;
    assert_eq!(
        wait_file(&checkout.path().join("started")).await?,
        "started"
    );
    drop(remote);
    assert_eq!(
        wait_file(&checkout.path().join("completed")).await?,
        "completed"
    );
    timeout(Duration::from_secs(15), async {
        while requests.requests().len() < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    timeout(Duration::from_secs(15), async {
        loop {
            let loaded = owner_rpc(&mut owner, "thread/loaded/list", json!({})).await?;
            if !loaded["data"]
                .as_array()
                .context("loaded thread IDs")?
                .contains(&json!(id))
            {
                return anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    let resumed = owner_rpc(&mut owner, "thread/resume", json!({"threadId":id})).await?;
    assert!(
        resumed["thread"]["turns"]
            .to_string()
            .contains("runner finished"),
        "{resumed}"
    );
    assert!(owner.shutdown_gracefully().await?.success());
    Ok(())
}

#[cfg(unix)]
#[test_case::test_case(Revocation::PrivateGrant; "private_grant")]
#[test_case::test_case(Revocation::NativeAccess; "native_access")]
#[test_case::test_case(Revocation::WorkspaceRules; "workspace_rules")]
#[tokio::test]
async fn revoking_live_access_stops_the_running_command(revocation: Revocation) -> Result<()> {
    let (home, checkout, chain) = (TempDir::new()?, TempDir::new()?, TempDir::new()?);
    std::fs::create_dir_all(checkout.path().join(".idle/workspace"))?;
    std::fs::write(
        checkout.path().join(".idle/workspace/agent-rules.json"),
        r#"{"schema_version":1}"#,
    )?;
    let server = responses::start_mock_server().await;
    let requests = responses::mount_sse_once(&server,
        command("active", json!({"cmd":"while :; do printf tick >> heartbeat; sleep 0.1; done","yield_time_ms":10000})),
    ).await;
    MockResponsesConfig::new(&server.uri()).write(home.path())?;
    let mut owner = TestAppServer::builder()
        .without_auto_env()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let mut remote = connect(&mut owner, &checkout, &chain).await?;
    let id = start(&mut remote).await?;
    turn(&mut remote, &id).await?;
    let heartbeat = checkout.path().join("heartbeat");
    wait_file(&heartbeat).await?;
    match revocation {
        Revocation::PrivateGrant => {
            let saved: Value = serde_json::from_slice(&std::fs::read(
                home.path().join(format!("idle-runtime/runner-{id}.json")),
            )?)?;
            owner_rpc(
                &mut owner,
                "idle/connection/revoke",
                json!({"protocolVersion":1,"grantId":saved["binding"]["grantId"]}),
            )
            .await?;
        }
        Revocation::NativeAccess => std::fs::write(checkout.path().join("deny-runner"), "denied")?,
        Revocation::WorkspaceRules => {
            std::fs::write(
                checkout.path().join(".idle/workspace/agent-rules.json"),
                r#"{"schema_version":1,"runners":{"version":1,"writeRoots":[]}}"#,
            )?;
        }
    }
    timeout(Duration::from_secs(15), async {
        loop {
            let before = std::fs::read(&heartbeat)?;
            tokio::time::sleep(Duration::from_millis(1500)).await;
            if std::fs::read(&heartbeat)? == before {
                return anyhow::Ok(());
            }
        }
    })
    .await??;
    let denied = rpc(
        &mut remote,
        "thread/read",
        json!({"threadId":id,"includeTurns":true}),
    )
    .await;
    assert!(denied.is_err() || denied?.get("error").is_some());
    assert_eq!(requests.requests().len(), 1);
    assert!(owner.shutdown_gracefully().await?.success());
    Ok(())
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum Revocation {
    PrivateGrant,
    NativeAccess,
    WorkspaceRules,
}
