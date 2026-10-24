//! Runner sessions execute on the selected daemon, so these fixtures select its local executor.

use anyhow::Context;
use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use codex_app_server_protocol::RequestId;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tempfile::TempDir;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio::time::timeout;

async fn owner_rpc(owner: &mut TestAppServer, method: &str, params: Value) -> Result<Value> {
    let id = owner.send_raw_request(method, Some(params)).await?;
    Ok(timeout(
        Duration::from_secs(45),
        owner.read_stream_until_response_message(RequestId::Integer(id)),
    )
    .await??
    .result)
}

async fn send(stream: &mut TcpStream, value: Value) -> Result<()> {
    let bytes = serde_json::to_vec(&value)?;
    stream.write_u32(u32::try_from(bytes.len())?).await?;
    stream.write_all(&bytes).await?;
    Ok(())
}

async fn read(stream: &mut TcpStream) -> Result<Value> {
    timeout(Duration::from_secs(45), async {
        let length = stream.read_u32().await?;
        anyhow::ensure!(length < 1024 * 1024, "frame limit");
        let mut bytes = vec![0; length as usize];
        stream.read_exact(&mut bytes).await?;
        Ok(serde_json::from_slice(&bytes)?)
    })
    .await?
}

async fn rpc(stream: &mut TcpStream, method: &str, params: Value) -> Result<Value> {
    send(stream, json!({"id":100,"method":method,"params":params})).await?;
    loop {
        let message = read(stream).await?;
        if message.get("id") == Some(&json!(100))
            && (message.get("result").is_some() || message.get("error").is_some())
        {
            return Ok(message);
        }
    }
}

async fn connect(
    owner: &mut TestAppServer,
    checkout: &TempDir,
    chain: &TempDir,
) -> Result<TcpStream> {
    owner_rpc(owner, "idle/workspace/attach", json!({"protocolVersion":1,"binding":{
        "workspaceId":"workspace","repositoryId":"repository","checkoutId":"checkout","chainId":"chain",
        "checkoutRoot":checkout.path().canonicalize()?,"chainDirectory":chain.path().canonicalize()?
    }})).await?;
    let helper = codex_utils_cargo_bin::cargo_bin("codex-app-server-test-idle-helper")?;
    owner_rpc(
        owner,
        "idle/relay/configure",
        json!({"protocolVersion":1,"helperPath":helper,"credentialProgram":helper}),
    )
    .await?;
    let expires = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )? + 300_000;
    let invitation = owner_rpc(owner, "idle/connection/invite", json!({"protocolVersion":1,"checkoutId":"checkout","clientId":"owner","expiresAt":expires,"runner":true})).await?;
    remote_from_invitation(invitation).await
}

async fn remote_from_invitation(invitation: Value) -> Result<TcpStream> {
    let encoded = invitation["invitation"]
        .as_str()
        .context("invitation")?
        .strip_prefix("idle-runtime:")
        .context("invitation prefix")?;
    let invitation: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded)?)?;
    let port = u16::try_from(
        invitation["relay"]["endpoint"]["port"]
            .as_u64()
            .context("relay port")?,
    )?;
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
    send(&mut stream, json!({"kind":"authenticate","version":1,"grantId":invitation["grantId"],"token":invitation["grantToken"]})).await?;
    assert_eq!(read(&mut stream).await?["kind"], "authenticated");
    let initialized = rpc(&mut stream, "initialize", json!({"clientInfo":{"name":"idle-runner-test","version":"1"},"capabilities":{"experimentalApi":true}})).await?;
    assert!(initialized.get("result").is_some(), "{initialized}");
    send(&mut stream, json!({"method":"initialized"})).await?;
    Ok(stream)
}

#[tokio::test]
async fn runner_rpc_rejects_override_routes_and_keeps_binding_after_restart_and_fork() -> Result<()>
{
    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let chain = TempDir::new()?;
    let server = responses::start_mock_server().await;
    let requests = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_assistant_message("answer", "runner ready"),
            responses::ev_completed("finished"),
        ]),
    )
    .await;
    MockResponsesConfig::new(&server.uri())
        .with_root_config("thread_unload_delay_secs = 0")
        .write(home.path())?;
    let mut owner = TestAppServer::builder()
        .without_auto_env()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let mut remote = connect(&mut owner, &checkout, &chain).await?;
    let started = rpc(&mut remote, "thread/start", json!({})).await?;
    let id = started
        .pointer("/result/thread/id")
        .and_then(Value::as_str)
        .with_context(|| format!("runner start: {started}"))?
        .to_string();
    for (method, params) in [
        (
            "turn/start",
            json!({"threadId":id,"input":[],"cwd":chain.path()}),
        ),
        (
            "turn/start",
            json!({"threadId":id,"input":[],"environments":[]}),
        ),
        (
            "turn/start",
            json!({"threadId":id,"input":[{"type":"localImage","path":chain.path().join("private.png")}]}),
        ),
        (
            "thread/resume",
            json!({"threadId":id,"sandbox":"danger-full-access"}),
        ),
        (
            "thread/fork",
            json!({"threadId":id,"config":{"sandbox_mode":"danger-full-access"}}),
        ),
        (
            "thread/settings/update",
            json!({"threadId":id,"sandboxPolicy":{"type":"dangerFullAccess"}}),
        ),
        ("command/exec", json!({"command":["echo","denied"]})),
    ] {
        let rejected = rpc(&mut remote, method, params).await?;
        assert!(rejected.get("error").is_some(), "{method}: {rejected}");
    }
    let marker = home.path().join(format!("idle-runtime/runner-{id}.json"));
    let saved: Value = serde_json::from_slice(&std::fs::read(&marker)?)?;
    let submitted = rpc(
        &mut remote,
        "turn/start",
        json!({"threadId":id,"input":[{"type":"text","text":"Say ready"}]}),
    )
    .await?;
    assert!(submitted.get("result").is_some(), "{submitted}");
    while read(&mut remote).await?["method"] != "turn/completed" {}
    assert_eq!(requests.requests().len(), 1);
    drop(remote);
    assert!(owner.shutdown_gracefully().await?.success());
    let mut owner = TestAppServer::builder()
        .without_auto_env()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    owner_rpc(
        &mut owner,
        "thread/resume",
        json!({"threadId":id,"excludeTurns":true}),
    )
    .await?;
    let fork = owner_rpc(
        &mut owner,
        "thread/fork",
        json!({"threadId":id,"excludeTurns":true}),
    )
    .await?;
    let child = fork["thread"]["id"].as_str().context("fork id")?;
    let child: Value = serde_json::from_slice(&std::fs::read(
        home.path()
            .join(format!("idle-runtime/runner-{child}.json")),
    )?)?;
    assert_eq!(child["binding"]["parent"], id);
    assert_eq!(child["binding"]["workspace"], saved["binding"]["workspace"]);
    assert_eq!(child["binding"]["grantId"], saved["binding"]["grantId"]);
    assert_eq!(child["binding"]["policy"], saved["binding"]["policy"]);
    assert!(owner.shutdown_gracefully().await?.success());
    Ok(())
}
