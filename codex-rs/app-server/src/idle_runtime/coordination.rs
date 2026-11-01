use super::wire;
use codex_idle_runtime::GrantAccess;
use codex_idle_runtime::GrantScope;
use codex_idle_runtime::RunnerBinding;
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
    dispatch: Dispatch,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dispatch {
    Client,
    Runner,
}

pub(super) enum RunnerRequest {
    Register,
    Authorize,
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
        if !matches!(
            request.get("kind").and_then(Value::as_str),
            Some("status" | "upload" | "commit" | "call")
        ) {
            return Err(io::Error::other(
                "Operation requires a trusted daemon dispatcher",
            ));
        }
        self.enqueue(binding, client_id, grant, request, Dispatch::Client)
            .await
    }

    pub(super) async fn runner(
        &self,
        binding: &RunnerBinding,
        request: RunnerRequest,
    ) -> io::Result<Value> {
        let access = self
            .service
            .runner_access(binding)
            .map_err(|_| wire::invalid())?;
        let request = match request {
            RunnerRequest::Register => {
                json!({"kind":"register_runner","session_id":binding.thread_id,"parent":binding.parent})
            }
            RunnerRequest::Authorize => {
                json!({"kind":"authorize_runner","session_id":binding.thread_id,"write":!binding.policy.write_roots.is_empty()})
            }
        };
        let result = self
            .enqueue(
                binding.workspace.clone(),
                binding.client_id.clone(),
                Some(access),
                request,
                Dispatch::Runner,
            )
            .await?;
        result
            .get("Ok")
            .cloned()
            .ok_or_else(|| io::Error::other("Native workspace authority denied runner admission"))
    }

    async fn enqueue(
        &self,
        binding: WorkspaceBinding,
        client_id: String,
        grant: Option<GrantAccess>,
        request: Value,
        dispatch: Dispatch,
    ) -> io::Result<Value> {
        let input = self.worker(binding).await?;
        let (response, received) = oneshot::channel();
        input
            .try_send(Work {
                client_id,
                grant,
                request,
                response,
                dispatch,
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
                    && current.scope
                        == match work.dispatch {
                            Dispatch::Client => GrantScope::CoordinationOwner,
                            Dispatch::Runner => GrantScope::Runner,
                        }
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
    runner_authorization: bool,
}

impl Process {
    async fn start(
        service: &WorkspaceService,
        binding: &WorkspaceBinding,
        directory: &Path,
    ) -> io::Result<Self> {
        match Self::start_version(service, binding, directory, /*version*/ 2).await {
            Ok(process) => Ok(process),
            Err(_) => Self::start_version(service, binding, directory, /*version*/ 1).await,
        }
    }

    async fn start_version(
        service: &WorkspaceService,
        binding: &WorkspaceBinding,
        directory: &Path,
        version: u32,
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
        let mut start = json!({"version":version,"state_directory":directory,"destination":{
            "target":{"host_id":status.host_id,"checkout_id":binding.checkout_id},
            "workspace_id":binding.workspace_id,"repository_id":binding.repository_id,"chain_id":binding.chain_id,"checkout_root":binding.checkout_root
        }});
        if version == 2 {
            start["runtime_id"] = json!(status.runtime_id);
        }
        wire::write(&mut output, &start).await?;
        let ready = if version == 2 {
            json!({"ready":true,"runner_authorization":1})
        } else {
            json!({"ready":true})
        };
        if wire::read::<Value>(&mut input).await? != Some(ready) {
            return Err(wire::invalid());
        }
        Ok(Self {
            child,
            input,
            output,
            runner_authorization: version == 2,
        })
    }

    async fn call(&mut self, work: &Work) -> io::Result<Value> {
        if work.dispatch == Dispatch::Runner && !self.runner_authorization {
            return Err(io::Error::other(
                "Idle helper does not support runner authorization",
            ));
        }
        let mut frame = json!({"client_id":work.client_id,"request":work.request});
        if work.dispatch == Dispatch::Runner {
            frame["source"] = json!("daemon");
        }
        wire::write(&mut self.output, &frame).await?;
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
