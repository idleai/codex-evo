use codex_idle_runtime::Secret;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::io;
use std::path::PathBuf;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;

const MAX_FRAME: usize = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Descriptor {
    pub(super) endpoint: Value,
    pub(super) connect_token: Secret,
    pub(super) expires_at: u64,
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(super) enum Command {
    Start {
        version: u32,
        state_directory: PathBuf,
        credential_program: PathBuf,
    },
    Send {
        connection_id: u64,
        message: Value,
    },
    Close {
        connection_id: u64,
    },
    Stop {
        remove: bool,
    },
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum Event {
    Hello { version: u32 },
    Ready { descriptor: Descriptor },
    Unavailable { code: String },
    Opened { connection_id: u64 },
    Message { connection_id: u64, message: Value },
    Closed { connection_id: u64 },
}

pub(super) async fn read<T: serde::de::DeserializeOwned>(
    reader: &mut (impl AsyncRead + Unpin),
) -> io::Result<Option<T>> {
    let first = match reader.read_u8().await {
        Ok(first) => first,
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut rest = [0_u8; 3];
    reader.read_exact(&mut rest).await?;
    let [a, b, c] = rest;
    let length = u32::from_be_bytes([first, a, b, c]) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(invalid());
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| invalid())
}

pub(super) async fn write<T: Serialize>(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &T,
) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|_| invalid())?;
    if bytes.len() > MAX_FRAME {
        return Err(invalid());
    }
    writer.write_u32(bytes.len() as u32).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await
}

pub(super) fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid Idle runtime transport frame",
    )
}
