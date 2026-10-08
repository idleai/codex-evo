use super::connections::Connections;
use super::connections::Grants;
use super::wire;
use codex_app_server_protocol::IdleRelayState;
use codex_app_server_transport::ConnectionId;
use codex_app_server_transport::TransportEvent;
use codex_idle_runtime::Error;
use codex_idle_runtime::GrantAccess;
use codex_idle_runtime::RelayConfiguration;
use codex_idle_runtime::WorkspaceService;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

#[cfg(test)]
mod tests;

#[derive(Clone)]
struct Context {
    events: mpsc::Sender<TransportEvent>,
    cancel: CancellationToken,
}

struct Running {
    cancel: CancellationToken,
    commands: mpsc::Sender<wire::Command>,
    stopping: Arc<AtomicBool>,
    task: JoinHandle<io::Result<()>>,
}

#[derive(Clone)]
struct State {
    kind: IdleRelayState,
    descriptor: Option<wire::Descriptor>,
}

pub(super) struct Relay {
    service: Arc<WorkspaceService>,
    directory: PathBuf,
    context: OnceLock<Context>,
    running: Mutex<Option<Running>>,
    state: watch::Sender<State>,
    grants: Grants,
}

impl Relay {
    pub(super) fn new(service: Arc<WorkspaceService>, home: &Path) -> Self {
        let (state, _) = watch::channel(State {
            kind: IdleRelayState::Disabled,
            descriptor: None,
        });
        Self {
            service,
            directory: home.join("idle-runtime/relay"),
            context: OnceLock::new(),
            running: Mutex::new(None),
            state,
            grants: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    pub(super) async fn install(
        self: &Arc<Self>,
        events: mpsc::Sender<TransportEvent>,
        cancel: CancellationToken,
    ) {
        let _ = self.context.set(Context { events, cancel });
        if self.service.relay_configuration().ok().flatten().is_some() {
            let _ = self.start().await;
        }
    }

    pub(super) fn state(&self) -> IdleRelayState {
        self.state.borrow().kind.clone()
    }

    pub(super) fn access(&self, id: ConnectionId) -> Result<GrantAccess, Error> {
        let grant = self
            .grants
            .lock()
            .map_err(|_| Error::Unavailable)?
            .get(&id)
            .cloned()
            .ok_or(Error::Denied)?;
        self.service.grant_access(&grant)
    }

    pub(super) async fn descriptor(&self) -> io::Result<wire::Descriptor> {
        let mut state = self.state.subscribe();
        tokio::time::timeout(Duration::from_secs(45), async {
            loop {
                if let Some(descriptor) = state.borrow().descriptor.clone() {
                    return Ok(descriptor);
                }
                state.changed().await.map_err(|_| wire::invalid())?;
            }
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Idle relay did not become ready"))?
    }

    pub(super) async fn start(self: &Arc<Self>) -> Result<(), Error> {
        let configuration = self
            .service
            .relay_configuration()?
            .ok_or(Error::Unavailable)?;
        let context = self.context.get().cloned().ok_or(Error::Unavailable)?;
        if context.cancel.is_cancelled() {
            return Err(Error::Stopped);
        }
        let mut running = self.running.lock().await;
        if running.is_some() {
            return Ok(());
        }
        if !configuration.helper_path.is_absolute()
            || !configuration.credential_program.is_absolute()
            || !configuration.helper_path.is_file()
            || !configuration.credential_program.is_file()
        {
            return Err(Error::Unavailable);
        }
        let cancel = context.cancel.child_token();
        let (commands, incoming) = mpsc::channel(32);
        let stopping = Arc::new(AtomicBool::new(false));
        self.update(IdleRelayState::Starting, None);
        let owner = self.clone();
        let task_cancel = cancel.clone();
        let task_commands = commands.clone();
        let task_stopping = stopping.clone();
        let task = tokio::spawn(async move {
            owner
                .supervise(
                    configuration,
                    context,
                    task_commands,
                    incoming,
                    &task_cancel,
                    &task_stopping,
                )
                .await
        });
        *running = Some(Running {
            cancel,
            commands,
            stopping,
            task,
        });
        Ok(())
    }

    pub(super) async fn shutdown(&self) {
        let running = self.running.lock().await.take();
        if let Some(mut running) = running {
            running.cancel.cancel();
            if tokio::time::timeout(Duration::from_secs(20), &mut running.task)
                .await
                .is_err()
            {
                running.task.abort();
                let _ = running.task.await;
            }
        }
        self.update(IdleRelayState::Disabled, None);
    }

    pub(super) async fn remove(&self) -> io::Result<()> {
        if self.state() != IdleRelayState::Ready {
            return Err(io::Error::other(
                "Idle relay is not connected; cleanup will need retry",
            ));
        }
        let Some(mut running) = self.running.lock().await.take() else {
            return Err(wire::invalid());
        };
        running.stopping.store(true, Ordering::Release);
        let sent = running
            .commands
            .send(wire::Command::Stop { remove: true })
            .await;
        let result = if sent.is_ok() {
            tokio::time::timeout(Duration::from_secs(30), &mut running.task)
                .await
                .map_err(|_| wire::invalid())
                .and_then(|result| result.map_err(|_| wire::invalid()))
                .and_then(std::convert::identity)
        } else {
            Err(wire::invalid())
        };
        if result.is_err() {
            running.cancel.cancel();
            running.task.abort();
            let _ = running.task.await;
        }
        self.update(IdleRelayState::Disabled, None);
        result
    }

    fn update(&self, kind: IdleRelayState, descriptor: Option<wire::Descriptor>) {
        self.state.send_replace(State { kind, descriptor });
    }

    async fn supervise(
        &self,
        configuration: RelayConfiguration,
        context: Context,
        commands: mpsc::Sender<wire::Command>,
        mut incoming: mpsc::Receiver<wire::Command>,
        cancel: &CancellationToken,
        stopping: &AtomicBool,
    ) -> io::Result<()> {
        let mut retry = Duration::from_millis(500);
        loop {
            let result = self
                .attempt(
                    &configuration,
                    &context.events,
                    commands.clone(),
                    &mut incoming,
                    cancel,
                )
                .await;
            self.update(IdleRelayState::Unavailable, None);
            if stopping.load(Ordering::Acquire) {
                return result;
            }
            if cancel.is_cancelled() {
                return Ok(());
            }
            tokio::select! { () = cancel.cancelled() => return Ok(()), () = tokio::time::sleep(retry) => {} }
            retry = retry.saturating_mul(2).min(Duration::from_secs(10));
        }
    }

    async fn attempt(
        &self,
        configuration: &RelayConfiguration,
        events: &mpsc::Sender<TransportEvent>,
        commands: mpsc::Sender<wire::Command>,
        incoming: &mut mpsc::Receiver<wire::Command>,
        cancel: &CancellationToken,
    ) -> io::Result<()> {
        let mut child = Command::new(&configuration.helper_path)
            .arg("--runtime-relay")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut input = child.stdout.take().ok_or_else(wire::invalid)?;
        let mut output = child.stdin.take().ok_or_else(wire::invalid)?;
        let hello = tokio::time::timeout(Duration::from_secs(10), wire::read(&mut input))
            .await
            .map_err(|_| wire::invalid())??;
        if !matches!(hello, Some(wire::Event::Hello { version: 1 })) {
            return Err(wire::invalid());
        }
        wire::write(
            &mut output,
            &wire::Command::Start {
                version: 1,
                state_directory: self.directory.clone(),
                credential_program: configuration.credential_program.clone(),
            },
        )
        .await?;
        let host_id = self.service.status().map_err(|_| wire::invalid())?.host_id;
        let mut connections = Connections::new(
            self.service.clone(),
            self.grants.clone(),
            events.clone(),
            commands,
            host_id,
        );
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        let reading = async {
            loop {
                let frame = wire::read::<wire::Event>(&mut input);
                tokio::pin!(frame);
                let next = loop {
                    tokio::select! {
                        frame = &mut frame => break frame?,
                        _ = interval.tick() => connections.prune().await?,
                    }
                };
                match next {
                    Some(wire::Event::Ready { descriptor }) => {
                        self.update(IdleRelayState::Ready, Some(descriptor))
                    }
                    Some(wire::Event::Unavailable { code }) => {
                        let _ = code;
                        self.update(IdleRelayState::Unavailable, None);
                        connections.close_all().await;
                    }
                    Some(wire::Event::Opened { connection_id }) => {
                        connections.opened(connection_id).await?
                    }
                    Some(wire::Event::Message {
                        connection_id,
                        message,
                    }) => connections.incoming(connection_id, message).await?,
                    Some(wire::Event::Closed { connection_id }) => {
                        connections.close(connection_id).await?
                    }
                    Some(wire::Event::Hello { .. }) => return Err(wire::invalid()),
                    None => return Ok::<_, io::Error>(()),
                }
            }
        };
        let writing = async {
            while let Some(command) = incoming.recv().await {
                wire::write(&mut output, &command).await?;
            }
            Ok::<_, io::Error>(())
        };
        let result = tokio::select! { () = cancel.cancelled() => Ok(()), result = reading => result, result = writing => result };
        drop(output);
        connections.close_all().await;
        let status = tokio::time::timeout(Duration::from_secs(15), child.wait())
            .await
            .map_err(|_| wire::invalid())??;
        if !status.success() {
            return Err(io::Error::other("Idle runtime helper exited"));
        }
        result
    }
}
