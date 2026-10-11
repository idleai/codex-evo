use super::wire;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_transport::ConnectionId;
use codex_app_server_transport::ConnectionOrigin;
use codex_app_server_transport::OutgoingMessage;
use codex_app_server_transport::QueuedOutgoingMessage;
use codex_app_server_transport::TransportEvent;
use codex_app_server_transport::next_connection_id;
use codex_idle_runtime::Secret;
use codex_idle_runtime::WorkspaceService;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub(super) type Grants = Arc<Mutex<HashMap<ConnectionId, String>>>;

enum Client {
    Pending(Instant),
    Authenticated {
        id: ConnectionId,
        cancel: CancellationToken,
    },
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Authentication {
    Authenticate {
        version: u32,
        grant_id: String,
        token: Secret,
    },
}

pub(super) struct Connections {
    service: Arc<WorkspaceService>,
    grants: Grants,
    events: mpsc::Sender<TransportEvent>,
    commands: mpsc::Sender<wire::Command>,
    clients: HashMap<u64, Client>,
    writers: JoinSet<()>,
    host_id: String,
}

impl Connections {
    pub(super) fn new(
        service: Arc<WorkspaceService>,
        grants: Grants,
        events: mpsc::Sender<TransportEvent>,
        commands: mpsc::Sender<wire::Command>,
        host_id: String,
    ) -> Self {
        Self {
            service,
            grants,
            events,
            commands,
            clients: HashMap::new(),
            writers: JoinSet::new(),
            host_id,
        }
    }

    pub(super) async fn opened(&mut self, id: u64) -> io::Result<()> {
        if self.clients.len() >= 8 || self.clients.contains_key(&id) {
            return self.close(id).await;
        }
        self.clients.insert(id, Client::Pending(Instant::now()));
        Ok(())
    }

    pub(super) async fn incoming(&mut self, remote: u64, value: Value) -> io::Result<()> {
        match self.clients.get(&remote) {
            Some(Client::Pending(_)) => return self.authenticate(remote, value).await,
            None => return self.close(remote).await,
            Some(Client::Authenticated { .. }) => {}
        }
        let Some(Client::Authenticated { id, .. }) = self.clients.get(&remote) else {
            return Ok(());
        };
        let id = *id;
        let grant = self
            .grants
            .lock()
            .map_err(|_| wire::invalid())?
            .get(&id)
            .cloned();
        if grant.is_none_or(|grant| self.service.grant_access(&grant).is_err()) {
            return self.close(remote).await;
        }
        let message = match serde_json::from_value::<JSONRPCMessage>(value) {
            Ok(message) => message,
            Err(_) => return self.close(remote).await,
        };
        match &message {
            JSONRPCMessage::Request(request) if !allowed_method(&request.method) => {
                return self
                    .send(
                        remote,
                        json!({"id":request.id,"error":{"code":-32600,
                    "message":"This method is unavailable on the Idle connection"}}),
                    )
                    .await;
            }
            JSONRPCMessage::Request(_) => {}
            JSONRPCMessage::Notification(notification) if notification.method == "initialized" => {}
            _ => return self.close(remote).await,
        }
        self.events
            .send(TransportEvent::IncomingMessage {
                connection_id: id,
                message,
            })
            .await
            .map_err(|_| wire::invalid())
    }

    async fn authenticate(&mut self, remote: u64, value: Value) -> io::Result<()> {
        let access = match serde_json::from_value::<Authentication>(value) {
            Ok(Authentication::Authenticate {
                version: 1,
                grant_id,
                token,
            }) => self.service.authenticate(&grant_id, &token).ok(),
            _ => None,
        };
        let Some(access) = access else {
            self.send(remote, json!({"kind":"denied"})).await?;
            return self.close(remote).await;
        };
        let id = next_connection_id();
        let cancel = CancellationToken::new();
        let (writer, mut output) = mpsc::channel::<QueuedOutgoingMessage>(16);
        self.grants
            .lock()
            .map_err(|_| wire::invalid())?
            .insert(id, access.grant_id);
        self.clients.insert(
            remote,
            Client::Authenticated {
                id,
                cancel: cancel.clone(),
            },
        );
        self.events
            .send(TransportEvent::ConnectionOpened {
                connection_id: id,
                origin: ConnectionOrigin::IdleRemote,
                auth: None,
                writer,
                disconnect_sender: Some(cancel.clone()),
            })
            .await
            .map_err(|_| wire::invalid())?;
        let commands = self.commands.clone();
        self.writers.spawn(async move {
            loop {
                let outgoing = tokio::select! { () = cancel.cancelled() => break, message = output.recv() => message };
                let Some(outgoing) = outgoing else { break; };
                // No account, thread or approval broadcasts cross this workspace-only channel.
                if !matches!(outgoing.message, OutgoingMessage::Response(_) | OutgoingMessage::Error(_)) { continue; }
                let Ok(message) = serde_json::to_value(outgoing.message) else { break; };
                let sent = tokio::select! {
                    () = cancel.cancelled() => break,
                    sent = commands.send(wire::Command::Send { connection_id: remote, message }) => sent,
                };
                if sent.is_err() { break; }
                if let Some(acknowledge) = outgoing.write_complete_tx { let _ = acknowledge.send(()); }
            }
            cancel.cancel();
        });
        self.send(
            remote,
            json!({"kind":"authenticated","hostId":self.host_id,"binding":access.binding}),
        )
        .await
    }

    async fn send(&self, connection_id: u64, message: Value) -> io::Result<()> {
        self.commands
            .send(wire::Command::Send {
                connection_id,
                message,
            })
            .await
            .map_err(|_| wire::invalid())
    }

    pub(super) async fn close(&mut self, remote: u64) -> io::Result<()> {
        let client = self.clients.remove(&remote);
        if let Some(Client::Authenticated { id, cancel }) = client {
            cancel.cancel();
            self.grants.lock().map_err(|_| wire::invalid())?.remove(&id);
            let _ = self
                .events
                .send(TransportEvent::ConnectionClosed { connection_id: id })
                .await;
        }
        let _ = self.commands.try_send(wire::Command::Close {
            connection_id: remote,
        });
        Ok(())
    }

    pub(super) async fn prune(&mut self) -> io::Result<()> {
        let invalid: Vec<_> = self
            .clients
            .iter()
            .filter_map(|(remote, client)| {
                let invalid = match client {
                    Client::Pending(since) => since.elapsed() >= Duration::from_secs(10),
                    Client::Authenticated { id, cancel } => {
                        cancel.is_cancelled()
                            || self
                                .grants
                                .lock()
                                .ok()
                                .and_then(|grants| grants.get(id).cloned())
                                .is_none_or(|grant| self.service.grant_access(&grant).is_err())
                    }
                };
                invalid.then_some(*remote)
            })
            .collect();
        for remote in invalid {
            self.close(remote).await?;
        }
        while self.writers.try_join_next().is_some() {}
        Ok(())
    }

    pub(super) async fn close_all(&mut self) {
        let clients: Vec<_> = self.clients.keys().copied().collect();
        for remote in clients {
            let _ = self.close(remote).await;
        }
        while self.writers.join_next().await.is_some() {}
    }
}

fn allowed_method(method: &str) -> bool {
    matches!(
        method,
        "initialize"
            | "idle/workspace/attach"
            | "idle/runtime/status/read"
            | "idle/coordination/call"
    )
}
