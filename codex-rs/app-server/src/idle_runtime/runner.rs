use super::IdleRuntime;
use super::coordination::RunnerRequest;
use super::rpc_error;
use crate::error_code::invalid_request;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_core::StartThreadOptions;
use codex_core::ThreadManager;
use codex_core::config::Config;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::IsolatedSessionExtensions;
use codex_extension_api::ThreadIdleCause;
use codex_extension_api::ThreadIdleInput;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadReadyInput;
use codex_extension_api::ThreadStopInput;
use codex_extension_api::ToolAdmissionContributor;
use codex_extension_api::ToolAdmissionInput;
use codex_extension_api::ToolPayload;
use codex_idle_runtime::GrantAccess;
use codex_idle_runtime::RunnerBinding;
use codex_idle_runtime::RunnerEnvironment;
use codex_protocol::ThreadId;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

pub(super) struct ActiveRunner {
    binding: RunnerBinding,
    last_idle: Option<ThreadIdleCause>,
}

#[derive(Default)]
pub(super) struct Runners {
    active: Mutex<HashMap<String, ActiveRunner>>,
    cancel: tokio_util::sync::CancellationToken,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl IdleRuntime {
    pub(super) fn loaded_runner(&self, binding: &RunnerBinding) -> RunnerBinding {
        self.runners
            .active
            .lock()
            .ok()
            .and_then(|active| {
                active
                    .get(&binding.thread_id)
                    .map(|runner| runner.binding.clone())
            })
            .unwrap_or_else(|| binding.clone())
    }
    pub(crate) fn has_runner(&self, thread_id: ThreadId) -> Result<bool, JSONRPCErrorError> {
        self.service
            .runner(&thread_id.to_string())
            .map(|binding| binding.is_some())
            .map_err(rpc_error)
    }
    pub(crate) async fn prepare_runner(
        self: &Arc<Self>,
        thread_id: ThreadId,
        access: Option<GrantAccess>,
        parent: Option<ThreadId>,
        options: &mut StartThreadOptions,
        manager: &ThreadManager,
    ) -> Result<(), JSONRPCErrorError> {
        let existing = self
            .service
            .runner(&thread_id.to_string())
            .map_err(rpc_error)?;
        let binding = if let Some(binding) = existing {
            Some(binding)
        } else if let Some(parent) = parent {
            self.service
                .runner(&parent.to_string())
                .map_err(rpc_error)?
                .map(|mut binding| {
                    binding.parent = Some(parent.to_string());
                    binding.thread_id = thread_id.to_string();
                    binding
                })
        } else if let Some(access) = access {
            Some(RunnerBinding {
                thread_id: thread_id.to_string(),
                workspace: access.binding,
                client_id: access.client_id,
                grant_id: access.grant_id,
                environment: RunnerEnvironment::Local,
                policy: access
                    .runner_policy
                    .ok_or_else(|| invalid_request("Runner grant has no execution policy"))?,
                parent: None,
            })
        } else {
            None
        };
        let Some(mut binding) = binding else {
            return Ok(());
        };
        self.service.runner_access(&binding).map_err(rpc_error)?;
        self.coordination
            .runner(&binding, RunnerRequest::Register)
            .await
            .map_err(|_| invalid_request("Native helper cannot register this runner"))?;
        let rules = self.current_rules(&binding).await?;
        binding.policy = binding
            .policy
            .restrict(&binding.workspace, rules.as_deref())
            .map_err(rpc_error)?;
        // The immutable marker keeps the original ceiling. A later rule can narrow
        // the loaded runtime, but cannot widen its saved grant or parent binding.
        if self
            .service
            .runner(&binding.thread_id)
            .map_err(rpc_error)?
            .is_none()
        {
            self.service
                .bind_runner(binding.clone())
                .map_err(rpc_error)?;
        }
        super::runner_policy::apply(&binding, options, &manager.environment_manager())?;
        let mut registry = ExtensionRegistryBuilder::<Config>::new();
        registry.tool_admission_contributor(self.clone());
        registry.thread_lifecycle_contributor(self.clone());
        if let Some(admission) = &self.turn_start_admission {
            registry.turn_start_admission(admission.clone());
        }
        options
            .thread_extension_init
            .insert(IsolatedSessionExtensions(Arc::new(registry.build())));
        options.thread_extension_init.insert(binding.clone());
        Ok(())
    }

    pub(super) async fn current_rules(
        &self,
        binding: &RunnerBinding,
    ) -> Result<Option<String>, JSONRPCErrorError> {
        self.service.runner_access(binding).map_err(rpc_error)?;
        let result = self
            .coordination
            .runner(binding, RunnerRequest::Authorize)
            .await
            .map_err(|_| invalid_request("Runner session or compute access was denied"))?;
        if result.get("contributor_id").and_then(Value::as_str) != Some(&binding.client_id)
            || result.pointer("/session/id").and_then(Value::as_str) != Some(&binding.thread_id)
        {
            return Err(invalid_request(
                "Native runner identity does not match its binding",
            ));
        }
        match result.get("agent_rules") {
            Some(Value::Null) => Ok(None),
            Some(Value::String(rules)) => Ok(Some(rules.clone())),
            _ => Err(invalid_request("Native runner rules are unavailable")),
        }
    }

    pub(super) async fn check_runner(
        &self,
        binding: &RunnerBinding,
    ) -> Result<(), JSONRPCErrorError> {
        let rules = self.current_rules(binding).await?;
        if binding
            .policy
            .restrict(&binding.workspace, rules.as_deref())
            .map_err(rpc_error)?
            != binding.policy
        {
            return Err(invalid_request(
                "Runner rules changed; reload the thread to apply them",
            ));
        }
        Ok(())
    }

    pub(crate) fn monitor_runners(self: &Arc<Self>, manager: std::sync::Weak<ThreadManager>) {
        let runtime = Arc::downgrade(self);
        let cancel = self.runners.cancel.clone();
        let task = tokio::spawn(async move {
            let mut heartbeat = Instant::now();
            loop {
                tokio::select! { () = cancel.cancelled() => break, () = tokio::time::sleep(Duration::from_secs(1)) => {} }
                let (Some(runtime), Some(manager)) = (runtime.upgrade(), manager.upgrade()) else {
                    break;
                };
                let refresh = heartbeat.elapsed() >= Duration::from_secs(120);
                let bindings: Vec<_> = match runtime.runners.active.lock() {
                    Ok(mut active) => active.values_mut().map(|runner| {
                        if let Some(cause) = runner.last_idle.take() {
                            tracing::debug!(thread_id = %runner.binding.thread_id, ?cause, "Idle runner turn finished");
                        }
                        runner.binding.clone()
                    }).collect(),
                    Err(_) => break,
                };
                for binding in bindings {
                    let Ok(id) = ThreadId::from_string(&binding.thread_id) else {
                        continue;
                    };
                    let thread = manager.get_thread(id).await.ok();
                    let allowed = tokio::select! { () = cancel.cancelled() => return, result = runtime.check_runner(&binding) => result.is_ok() };
                    if !allowed || thread.is_none() {
                        if let Some(thread) = thread {
                            let _ = tokio::time::timeout(
                                Duration::from_secs(10),
                                thread.shutdown_and_wait(),
                            )
                            .await;
                        }
                        if let Ok(mut active) = runtime.runners.active.lock() {
                            active.remove(&binding.thread_id);
                        }
                    } else if refresh {
                        tokio::select! {
                            () = cancel.cancelled() => return,
                            _ = runtime.coordination.runner(&binding, RunnerRequest::Register) => {}
                        }
                    }
                }
                if refresh {
                    heartbeat = Instant::now();
                }
            }
        });
        if let Ok(mut slot) = self.runners.task.lock() {
            *slot = Some(task);
        }
    }

    pub(super) async fn stop_runner_monitor(&self) {
        self.runners.cancel.cancel();
        let task = self
            .runners
            .task
            .lock()
            .ok()
            .and_then(|mut task| task.take());
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}

impl ToolAdmissionContributor for IdleRuntime {
    fn admit<'a>(
        &'a self,
        input: ToolAdmissionInput<'a>,
    ) -> ExtensionFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let binding = self
                .service
                .runner(input.thread_store.level_id())
                .map_err(|_| "Idle runner binding is unavailable".to_string())?;
            let Some(saved) = binding else {
                return Ok(());
            };
            let binding = input
                .thread_store
                .get::<RunnerBinding>()
                .ok_or_else(|| "Idle runner must reload with its saved restrictions".to_string())?;
            if binding.thread_id != saved.thread_id
                || binding.workspace != saved.workspace
                || binding.grant_id != saved.grant_id
            {
                return Err("Idle runner binding changed".into());
            }
            if input.tool_name.is_default_namespace()
                && matches!(input.tool_name.name.as_str(), "exec" | "wait")
            {
                // Code Mode exposes only the policy's native tools. Every nested
                // command still passes this gate with its own arguments.
                return self
                    .check_runner(&binding)
                    .await
                    .map_err(|error| error.message);
            }
            if !input.tool_name.is_default_namespace()
                || !matches!(
                    input.tool_name.name.as_str(),
                    "exec_command" | "write_stdin"
                )
            {
                return Err("This tool is outside the runner grant".into());
            }
            let ToolPayload::Function { arguments } = input.payload else {
                return Err("Invalid runner tool arguments".into());
            };
            let arguments: Value = serde_json::from_str(arguments)
                .map_err(|_| "Invalid runner tool arguments".to_string())?;
            let fields = arguments
                .as_object()
                .ok_or_else(|| "Invalid runner tool arguments".to_string())?;
            if fields.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "cmd"
                        | "workdir"
                        | "shell"
                        | "login"
                        | "yield_time_ms"
                        | "max_output_tokens"
                        | "session_id"
                        | "chars"
                        | "tty"
                )
            }) {
                return Err(
                    "Runner tools cannot request additional permissions or another environment"
                        .into(),
                );
            }
            self.check_runner(&binding)
                .await
                .map_err(|error| error.message)
        })
    }
}

impl ThreadLifecycleContributor<Config> for IdleRuntime {
    fn on_thread_ready<'a>(
        &'a self,
        input: ThreadReadyInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if let Some(binding) = input.thread_store.get::<RunnerBinding>()
                && let Ok(mut active) = self.runners.active.lock()
            {
                active.insert(
                    binding.thread_id.clone(),
                    ActiveRunner {
                        binding: binding.as_ref().clone(),
                        last_idle: None,
                    },
                );
            }
        })
    }
    fn on_thread_idle<'a>(&'a self, input: ThreadIdleInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if let Ok(mut active) = self.runners.active.lock()
                && let Some(runner) = active.get_mut(input.thread_store.level_id())
            {
                runner.last_idle = Some(input.cause);
            }
        })
    }
    fn on_thread_stop<'a>(&'a self, input: ThreadStopInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if let Ok(mut active) = self.runners.active.lock() {
                active.remove(input.thread_store.level_id());
            }
        })
    }
}
