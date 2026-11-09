use super::IdleRuntime;
use super::coordination::RunnerRequest;
use super::rpc_error;
use crate::error_code::invalid_request;
use crate::transport::ConnectionOrigin;
use codex_app_server_protocol::IdleSessionCallParams;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_transport::ConnectionId;
use codex_core::CodexThread;
use codex_extension_api::ThreadIdleCause;
use codex_idle_runtime::GrantAccess;
use codex_idle_runtime::GrantScope;
use codex_idle_runtime::RunnerBinding;
use codex_protocol::turn_input::StartIfIdleSubmission;
use codex_protocol::turn_input::TurnInput;
use codex_protocol::turn_input::TurnInputRequest;
use codex_protocol::user_input::UserInput;
use serde_json::Value;
use serde_json::json;

#[derive(Clone)]
pub(super) struct QueuedInput {
    pub reference: Value,
    pub author: String,
    pub grant_id: String,
    pending: Option<Value>,
}

impl IdleRuntime {
    pub(crate) fn session_access(
        &self,
        origin: ConnectionOrigin,
        connection: ConnectionId,
        params: &IdleSessionCallParams,
    ) -> Result<GrantAccess, JSONRPCErrorError> {
        if origin != ConnectionOrigin::IdleRemote
            || params.protocol_version != 1
            || params.request.len() > 128 * 1024
        {
            return Err(invalid_request(
                "Live sessions require an authenticated runner invitation",
            ));
        }
        let access = self.relay.access(connection).map_err(rpc_error)?;
        if access.scope != GrantScope::Runner {
            return Err(invalid_request(
                "This invitation does not allow live sessions",
            ));
        }
        Ok(access)
    }

    pub(crate) async fn session_operation(
        &self,
        access: GrantAccess,
        operation: Value,
    ) -> Result<Value, JSONRPCErrorError> {
        match self.coordination.session(access, operation).await {
            Ok(result) => result
                .get("Ok")
                .cloned()
                .or_else(|| {
                    result
                        .get("Err")
                        .and_then(Value::as_str)
                        .map(|code| json!({"error":{"code":code}}))
                })
                .ok_or_else(|| invalid_request("Invalid session authority response")),
            Err(error) => Ok(
                json!({"error":{"code":if error.kind() == std::io::ErrorKind::Unsupported {"version"} else {"transport"}}}),
            ),
        }
    }

    pub(crate) async fn session_created(
        &self,
        thread_id: &str,
    ) -> Result<Value, JSONRPCErrorError> {
        let binding = self
            .service
            .runner(thread_id)
            .map_err(rpc_error)?
            .ok_or_else(|| invalid_request("Runner binding is unavailable"))?;
        self.coordination
            .runner(
                &binding,
                RunnerRequest::Work(json!({"kind":"created","session_id":thread_id})),
            )
            .await
            .map_err(|_| {
                invalid_request(
                    "Runner creation acknowledgement is unavailable; retain the request",
                )
            })
    }

    pub(super) fn input_author(&self, binding: &RunnerBinding) -> Option<String> {
        self.runners.active.lock().ok().and_then(|active| {
            active
                .get(&binding.thread_id)
                .and_then(|runner| runner.input.as_ref().map(|input| input.author.clone()))
        })
    }

    pub(super) fn check_input_invitation(
        &self,
        binding: &RunnerBinding,
    ) -> Result<(), JSONRPCErrorError> {
        let input = self
            .runners
            .active
            .lock()
            .ok()
            .and_then(|active| active.get(&binding.thread_id)?.input.clone());
        if let Some(input) = input {
            let access = self
                .service
                .grant_access(&input.grant_id)
                .map_err(rpc_error)?;
            if access.scope != GrantScope::Runner
                || access.client_id != input.author
                || access.binding != binding.workspace
                || access
                    .runner_policy
                    .as_ref()
                    .is_none_or(|policy| binding.policy.intersect(policy) != binding.policy)
            {
                return Err(invalid_request(
                    "Input author no longer has permission to use this runner",
                ));
            }
        }
        Ok(())
    }

    pub(super) async fn drive_input(&self, binding: &RunnerBinding, thread: &CodexThread) {
        if self.save_progress(binding).await.is_err() || self.finish_input(binding).await.is_err() {
            return;
        }
        let busy = self
            .runners
            .active
            .lock()
            .map(|active| {
                active
                    .get(&binding.thread_id)
                    .is_none_or(|runner| runner.input.is_some())
            })
            .unwrap_or(true);
        if busy {
            return;
        }
        let Ok(claim) = self
            .coordination
            .runner(
                binding,
                RunnerRequest::Work(json!({"kind":"claim","session_id":binding.thread_id})),
            )
            .await
        else {
            return;
        };
        let (Some(reference), Some(text), Some(author), Some(grant_id)) = (
            claim.get("input"),
            claim.get("text").and_then(Value::as_str),
            claim.get("contributor_id").and_then(Value::as_str),
            claim.get("grant_id").and_then(Value::as_str),
        ) else {
            return;
        };
        let input = QueuedInput {
            reference: reference.clone(),
            author: author.to_string(),
            grant_id: grant_id.to_string(),
            pending: None,
        };
        if let Ok(mut active) = self.runners.active.lock()
            && let Some(runner) = active.get_mut(&binding.thread_id)
        {
            runner.input = Some(input.clone());
            runner.last_idle = None;
        }
        if self.check_runner(binding).await.is_err() {
            let _ = self.stop_input(binding).await;
            return;
        }
        let request = TurnInputRequest::new(TurnInput::UserInput {
            content: vec![UserInput::Text {
                text: text.to_string(),
                text_elements: Vec::new(),
            }],
            client_id: Some(author.to_string()),
        });
        let result = thread.start_turn_if_idle(request).await;
        let work = match result {
            Ok(StartIfIdleSubmission::Started { turn_id, .. }) => {
                json!({"kind":"started","input":reference,"turn_id":turn_id})
            }
            Ok(StartIfIdleSubmission::NotSubmitted { .. }) => {
                json!({"kind":"release","input":reference})
            }
            Err(_) => {
                if let Ok(mut active) = self.runners.active.lock()
                    && let Some(runner) = active.get_mut(&binding.thread_id)
                {
                    runner.last_idle = Some(ThreadIdleCause::Failed);
                }
                return;
            }
        };
        if let Ok(mut active) = self.runners.active.lock()
            && let Some(input) = active
                .get_mut(&binding.thread_id)
                .and_then(|runner| runner.input.as_mut())
        {
            input.pending = Some(work);
        }
        let _ = self.save_progress(binding).await;
    }

    async fn save_progress(&self, binding: &RunnerBinding) -> std::io::Result<()> {
        let pending = self.runners.active.lock().ok().and_then(|active| {
            active
                .get(&binding.thread_id)?
                .input
                .as_ref()?
                .pending
                .clone()
        });
        let Some(pending) = pending else {
            return Ok(());
        };
        self.coordination
            .runner(binding, RunnerRequest::Work(pending.clone()))
            .await?;
        if pending.get("kind").and_then(Value::as_str) == Some("release") {
            self.clear_input(binding);
        } else if let Ok(mut active) = self.runners.active.lock()
            && let Some(input) = active
                .get_mut(&binding.thread_id)
                .and_then(|runner| runner.input.as_mut())
        {
            input.pending = None;
        }
        Ok(())
    }

    pub(super) async fn stop_input(&self, binding: &RunnerBinding) -> std::io::Result<()> {
        if let Ok(mut active) = self.runners.active.lock()
            && let Some(runner) = active.get_mut(&binding.thread_id)
        {
            runner.last_idle.get_or_insert(ThreadIdleCause::Interrupted);
        }
        self.finish_input(binding).await
    }

    fn clear_input(&self, binding: &RunnerBinding) {
        if let Ok(mut active) = self.runners.active.lock()
            && let Some(runner) = active.get_mut(&binding.thread_id)
        {
            runner.input = None;
            runner.last_idle = None;
        }
    }

    async fn finish_input(&self, binding: &RunnerBinding) -> std::io::Result<()> {
        let finished = self.runners.active.lock().ok().and_then(|active| {
            active
                .get(&binding.thread_id)
                .and_then(|runner| Some((runner.input.clone()?, runner.last_idle?)))
        });
        if let Some((input, cause)) = finished {
            let outcome = match cause {
                ThreadIdleCause::Completed => json!({"kind":"succeeded"}),
                ThreadIdleCause::Interrupted => json!({"kind":"cancelled"}),
                ThreadIdleCause::Failed => {
                    json!({"kind":"failed","data":{"code":"unavailable","message":"Runner turn failed. Review its saved output before trying a new task.","retry":{"kind":"never"}}})
                }
            };
            self.coordination
                .runner(
                    binding,
                    RunnerRequest::Work(
                        json!({"kind":"completed","input":input.reference,"outcome":outcome}),
                    ),
                )
                .await?;
            self.clear_input(binding);
        }
        Ok(())
    }
}
