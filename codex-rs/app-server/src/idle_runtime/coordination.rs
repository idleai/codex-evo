use super::wire;
use codex_idle_runtime::GrantAccess;
use codex_idle_runtime::GrantScope;
use codex_idle_runtime::WorkspaceBinding;
use codex_idle_runtime::WorkspaceService;
use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Child;
use tokio::process::ChildStdin;
use tokio::process::ChildStdout;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

struct Work {
    client_id: String,
    grant: Option<GrantAccess>,
    request: Value,
    response: oneshot::Sender<io::Result<Value>>,
}

struct Worker {
    input: mpsc::Sender<Work>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

pub(super) struct Coordination {
    service: Arc<WorkspaceService>,
    directory: PathBuf,
    workers: Mutex<HashMap<String, Worker>>,
    cancel: CancellationToken,
}

impl Coordination {
    pub(super) fn new(service: Arc<WorkspaceService>, home: &Path) -> Self {
        Self {
            service,
            directory: home.join("idle-runtime/coordination"),
            workers: Mutex::new(HashMap::new()),
            cancel: CancellationToken::new(),
        }
    }

    pub(super) async fn restore(&self) {
        if let Ok(status) = self.service.status() {
            for workspace in status.workspaces {
                if self
                    .directory(&workspace.binding)
                    .join("authority.json")
                    .is_file()
                {
                    let _ = self.worker(workspace.binding).await;
                }
            }
        }
    }

    pub(super) async fn call(
        &self,
        binding: WorkspaceBinding,
        client_id: String,
        grant: Option<GrantAccess>,
        request: Value,
    ) -> io::Result<Value> {
        let input = self.worker(binding).await?;
        let (response, received) = oneshot::channel();
        input
            .try_send(Work {
                client_id,
                grant,
                request,
                response,
            })
            .map_err(|_| io::Error::other("Idle coordination queue is full"))?;
        tokio::time::timeout(Duration::from_secs(25), received)
            .await
            .map_err(|_| io::Error::other("Idle coordination request timed out"))?
            .map_err(|_| wire::invalid())?
    }

    async fn worker(&self, binding: WorkspaceBinding) -> io::Result<mpsc::Sender<Work>> {
        let mut workers = self.workers.lock().await;
        if self.cancel.is_cancelled() {
            return Err(wire::invalid());
        }
        if let Some(worker) = workers.get(&binding.checkout_id) {
            return Ok(worker.input.clone());
        }
        let directory = self.directory(&binding);
        let service = self.service.clone();
        let cancel = self.cancel.child_token();
        let lifetime = cancel.clone();
        let (input, requests) = mpsc::channel(/*buffer*/ 8);
        let checkout_id = binding.checkout_id.clone();
        let task = tokio::spawn(async move {
            run(service, binding, directory, requests, lifetime).await;
        });
        workers.insert(
            checkout_id,
            Worker {
                input: input.clone(),
                cancel,
                task,
            },
        );
        Ok(input)
    }

    fn directory(&self, binding: &WorkspaceBinding) -> PathBuf {
        // Every checkout for a workspace shares this lock; copied checkouts cannot
        // create a second coordinator under the same daemon installation.
        self.directory.join(format!(
            "{:x}",
            Sha256::digest(binding.workspace_id.as_bytes())
        ))
    }

    pub(super) async fn reset(&self) {
        let workers = std::mem::take(&mut *self.workers.lock().await);
        for worker in workers.values() {
            worker.cancel.cancel();
        }
        for (_, worker) in workers {
            let _ = worker.task.await;
        }
    }

    pub(super) async fn shutdown(&self) {
        self.cancel.cancel();
        self.reset().await;
    }
}

async fn run(
    service: Arc<WorkspaceService>,
    binding: WorkspaceBinding,
    directory: PathBuf,
    mut requests: mpsc::Receiver<Work>,
    cancel: CancellationToken,
) {
    let mut process = None;
    loop {
        if cancel.is_cancelled() {
            break;
        }
        if process.is_none() {
            let opening = Process::start(&service, &binding, &directory);
            process = tokio::select! { () = cancel.cancelled() => break, result = tokio::time::timeout(Duration::from_secs(10), opening) => result.ok().and_then(Result::ok) };
            if process.is_none() {
                while let Ok(work) = requests.try_recv() {
                    let _ = work.response.send(Err(io::Error::other(
                        "Idle coordination helper is unavailable",
                    )));
                }
                tokio::select! { () = cancel.cancelled() => break, () = tokio::time::sleep(Duration::from_secs(1)) => {} }
                continue;
            }
        }
        let Some(running) = process.as_mut() else {
            continue;
        };
        let next = tokio::select! {
            () = cancel.cancelled() => break,
            _ = running.child.wait() => { process = None; continue; },
            work = requests.recv() => work,
        };
        let Some(work) = next else {
            break;
        };
        if work.response.is_closed() {
            continue;
        }
        let allowed = work.grant.as_ref().is_none_or(|grant| {
            service.grant_access(&grant.grant_id).is_ok_and(|current| {
                current.client_id == work.client_id
                    && current.binding == binding
                    && current.scope == GrantScope::CoordinationOwner
            })
        });
        let available = service.status().is_ok_and(|status| {
            status
                .workspaces
                .iter()
                .any(|workspace| workspace.binding == binding && workspace.available)
        });
        if !allowed || !available {
            let _ = work.response.send(Err(io::Error::other(
                "Idle coordination access is unavailable",
            )));
            continue;
        }
        let result = tokio::select! {
            () = cancel.cancelled() => break,
            result = tokio::time::timeout(Duration::from_secs(20), running.call(&work)) => result.unwrap_or_else(|_| Err(io::Error::other("Idle coordination helper timed out"))),
        };
        if result.is_err()
            && let Some(running) = process.take()
        {
            running.shutdown().await;
        }
        let _ = work.response.send(result);
    }
    if let Some(running) = process {
        running.shutdown().await;
    }
}

struct Process {
    child: Child,
    input: ChildStdout,
    output: ChildStdin,
}

impl Process {
    async fn start(
        service: &WorkspaceService,
        binding: &WorkspaceBinding,
        directory: &Path,
    ) -> io::Result<Self> {
        let helper_path = service
            .coordination_helper()
            .map_err(|_| wire::invalid())?
            .ok_or_else(wire::invalid)?;
        let status = service.status().map_err(|_| wire::invalid())?;
        if !helper_path.is_absolute()
            || !status
                .workspaces
                .iter()
                .any(|workspace| workspace.binding == *binding && workspace.available)
        {
            return Err(wire::invalid());
        }
        let mut child = Command::new(&helper_path)
            .arg("--runtime-authority")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut input = child.stdout.take().ok_or_else(wire::invalid)?;
        let mut output = child.stdin.take().ok_or_else(wire::invalid)?;
        if wire::read::<Value>(&mut input).await? != Some(json!({"version":1})) {
            return Err(wire::invalid());
        }
        wire::write(&mut output, &json!({"version":1,"state_directory":directory,"destination":{
            "target":{"host_id":status.host_id,"checkout_id":binding.checkout_id},
            "workspace_id":binding.workspace_id,"repository_id":binding.repository_id,"chain_id":binding.chain_id,"checkout_root":binding.checkout_root
        }})).await?;
        if wire::read::<Value>(&mut input).await? != Some(json!({"ready":true})) {
            return Err(wire::invalid());
        }
        Ok(Self {
            child,
            input,
            output,
        })
    }

    async fn call(&mut self, work: &Work) -> io::Result<Value> {
        wire::write(
            &mut self.output,
            &json!({"client_id":work.client_id,"request":work.request}),
        )
        .await?;
        wire::read(&mut self.input).await?.ok_or_else(wire::invalid)
    }

    async fn shutdown(mut self) {
        drop(self.output);
        if tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.kill().await;
            let _ = self.child.wait().await;
        }
    }
}
