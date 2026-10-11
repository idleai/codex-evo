//! Private pipe and loopback relay fixture for app-server runner integration tests.
use anyhow::Result;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;

#[tokio::main]
async fn main() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("--runtime-authority") => authority().await,
        Some("--runtime-relay") => relay().await,
        _ => anyhow::bail!("expected fixture helper mode"),
    }
}

async fn read(input: &mut (impl AsyncRead + Unpin)) -> Result<Option<Value>> {
    let length = match input.read_u32().await {
        Ok(length) => length,
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!((1..=1024 * 1024).contains(&length), "invalid fixture frame");
    let mut bytes = vec![0; length as usize];
    input.read_exact(&mut bytes).await?;
    Ok(Some(serde_json::from_slice(&bytes)?))
}

async fn write(output: &mut (impl AsyncWrite + Unpin), value: Value) -> Result<()> {
    let bytes = serde_json::to_vec(&value)?;
    output.write_u32(u32::try_from(bytes.len())?).await?;
    output.write_all(&bytes).await?;
    output.flush().await?;
    Ok(())
}

async fn authority() -> Result<()> {
    let (mut input, mut output) = (tokio::io::stdin(), tokio::io::stdout());
    write(&mut output, json!({"version":1})).await?;
    let start = read(&mut input)
        .await?
        .ok_or_else(|| anyhow::anyhow!("missing start"))?;
    let root = PathBuf::from(
        start["destination"]["checkout_root"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing checkout"))?,
    );
    anyhow::ensure!(start["version"] == 2, "runner protocol required");
    write(&mut output, json!({"ready":true,"runner_authorization":1})).await?;
    while let Some(call) = read(&mut input).await? {
        let request = &call["request"];
        let result = if call["source"] != "daemon"
            || call["client_id"] != "owner"
            || root.join("deny-runner").exists()
        {
            json!({"Err":"forbidden"})
        } else {
            match request["kind"].as_str() {
                Some("register_runner") => json!({"Ok":null}),
                Some("authorize_runner") => json!({"Ok":{
                    "contributor_id":"owner","session":{"id":request["session_id"]},
                    "configuration_revision":"fixture",
                    "agent_rules":std::fs::read_to_string(root.join(".idle/workspace/agent-rules.json")).ok()
                }}),
                _ => json!({"Err":"invalid"}),
            }
        };
        write(&mut output, result).await?;
    }
    Ok(())
}

async fn relay() -> Result<()> {
    let (mut input, mut output) = (tokio::io::stdin(), tokio::io::stdout());
    write(&mut output, json!({"kind":"hello","version":1})).await?;
    let _start = read(&mut input)
        .await?
        .ok_or_else(|| anyhow::anyhow!("missing start"))?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    write(&mut output, json!({"kind":"ready","descriptor":{"endpoint":{"port":listener.local_addr()?.port()},"connectToken":"fixture","expiresAt":u64::MAX}})).await?;
    let (events, mut received) = tokio::sync::mpsc::channel(32);
    let (commands, mut requested) = tokio::sync::mpsc::channel(32);
    let mut writers = HashMap::new();
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(async move {
        while let Ok(Some(command)) = read(&mut input).await {
            if commands.send(command).await.is_err() {
                break;
            }
        }
    });
    let mut next_id = 0_u64;
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                next_id += 1;
                let id = next_id;
                let (mut reader, writer) = stream.into_split();
                writers.insert(id, writer);
                write(&mut output, json!({"kind":"opened","connectionId":id})).await?;
                let events = events.clone();
                tasks.spawn(async move {
                    while let Ok(Some(message)) = read(&mut reader).await {
                        if events.send(json!({"kind":"message","connectionId":id,"message":message})).await.is_err() { return; }
                    }
                    let _ = events.send(json!({"kind":"closed","connectionId":id})).await;
                });
            }
            command = requested.recv() => {
                let Some(command) = command else { break; };
                let id = command["connectionId"].as_u64().unwrap_or_default();
                match command["kind"].as_str() {
                    Some("send") => { if let Some(writer) = writers.get_mut(&id) { let _ = write(writer, command["message"].clone()).await; } }
                    Some("close") => { writers.remove(&id); }
                    Some("stop") => break,
                    _ => anyhow::bail!("invalid relay command"),
                }
            }
            Some(event) = received.recv() => { write(&mut output, event).await?; }
        }
    }
    tasks.shutdown().await;
    Ok(())
}
