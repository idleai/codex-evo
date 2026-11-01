use super::*;
use crate::transport::ConnectionOrigin;
use codex_app_server_protocol::IdleSessionCallParams;
use codex_app_server_protocol::IdleSessionCallResponse;
use codex_idle_runtime::GrantAccess;

impl ThreadRequestProcessor {
    pub(crate) async fn idle_session_call(
        &self,
        origin: ConnectionOrigin,
        connection: ConnectionId,
        params: IdleSessionCallParams,
    ) -> Result<IdleSessionCallResponse, JSONRPCErrorError> {
        let access = self
            .idle_runtime
            .session_access(origin, connection, &params)?;
        let operation: serde_json::Value = serde_json::from_str(&params.request)
            .map_err(|_| invalid_request("Invalid live session request"))?;
        // Allocate and recover one native session at a time.
        let _permit = self
            .idle_runtime
            .session_requests
            .acquire()
            .await
            .map_err(|_| internal_error("Session service stopped"))?;
        let action = operation
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let snapshot = action.as_deref() == Some("snapshot");
        if matches!(action.as_deref(), Some("ready" | "create" | "submit")) {
            self.idle_runtime
                .runner_readiness(&access, &self.config, &self.thread_manager)
                .await?;
            if action.as_deref() == Some("ready") {
                return Ok(IdleSessionCallResponse {
                    response: serde_json::json!({"ready":true}).to_string(),
                });
            }
        }
        self.idle_runtime
            .check_session_submission(&access, &operation)?;
        let mut result = self
            .idle_runtime
            .session_operation(access.clone(), operation)
            .await?;
        if action.as_deref() == Some("create")
            && result.get("kind").and_then(serde_json::Value::as_str) == Some("created")
        {
            let id = result
                .get("session_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| internal_error("Invalid session allocation"))?;
            self.load_idle_session(id, connection, Some(access.clone()))
                .await?;
            result = self.idle_runtime.session_created(id).await?;
        } else if action.as_deref() == Some("submit")
            && let Some(id) = result
                .pointer("/update/input/session_id")
                .and_then(serde_json::Value::as_str)
        {
            self.load_idle_session(id, connection, None).await?;
        }
        if snapshot {
            let readiness = self
                .idle_runtime
                .runner_readiness(&access, &self.config, &self.thread_manager)
                .await
                .err()
                .map(|error| error.message);
            let ids: Vec<String> = result
                .pointer("/snapshot/sessions")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .take(64)
                .filter_map(|session| {
                    session
                        .pointer("/value/id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .collect();
            let mut transcripts = Vec::new();
            for id in ids {
                if transcripts.len() >= 64 {
                    break;
                }
                if self.load_idle_session(&id, connection, None).await.is_err() {
                    continue;
                }
                let Ok(thread_id) = ThreadId::from_string(&id) else {
                    continue;
                };
                let Ok(thread) = self.read_thread_view(thread_id, true).await else {
                    continue;
                };
                let Ok(thread) = serde_json::to_value(thread) else {
                    continue;
                };
                for turn in thread
                    .get("turns")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .rev()
                    .take(16)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                {
                    if transcripts.len() >= 64 {
                        break;
                    }
                    let text = turn
                        .get("items")
                        .and_then(serde_json::Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter(|item| {
                            item.get("type").and_then(serde_json::Value::as_str)
                                != Some("userMessage")
                        })
                        .filter_map(|item| {
                            item.get("text")
                                .or_else(|| item.get("aggregatedOutput"))
                                .and_then(serde_json::Value::as_str)
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let truncated = text.chars().count() > 4096;
                    transcripts.push(serde_json::json!({"session_id":id,"turn_id":turn["id"],"status":turn["status"],"text":text.chars().take(4096).collect::<String>(),"truncated":truncated}));
                }
            }
            result = self
                .idle_runtime
                .session_operation(access, serde_json::json!({"kind":"snapshot"}))
                .await?;
            result["transcripts"] = serde_json::json!(transcripts);
            result["readiness"] = serde_json::json!(readiness);
        }
        Ok(IdleSessionCallResponse {
            response: serde_json::to_string(&result)
                .map_err(|_| internal_error("Invalid live session response"))?,
        })
    }

    async fn load_idle_session(
        &self,
        id: &str,
        connection: ConnectionId,
        allocation: Option<GrantAccess>,
    ) -> Result<(), JSONRPCErrorError> {
        let thread_id =
            ThreadId::from_string(id).map_err(|_| invalid_request("Invalid session identity"))?;
        if self.thread_manager.get_thread(thread_id).await.is_err() {
            let persisted = self
                .load_persisted_thread_for_read(thread_id, false)
                .await
                .map_err(|_| internal_error("Session storage is unavailable"))?;
            if persisted.is_some() {
                let params: ThreadResumeParams =
                    serde_json::from_value(serde_json::json!({"threadId":id}))
                        .map_err(|_| internal_error("Invalid session resume"))?;
                self.thread_resume(
                    ThreadResumeTarget::DaemonRecovery(None),
                    params,
                    None,
                    None,
                    ClientMcpExtensions::default(),
                )
                .await?;
            } else {
                let access = allocation.ok_or_else(|| {
                    invalid_request("This live session has no saved native thread")
                })?;
                let mut options = StartThreadOptions::new(self.config.as_ref().clone());
                options.reserved_thread_id = Some(thread_id);
                options.history_mode = Some(ThreadHistoryMode::Paginated);
                self.idle_runtime
                    .prepare_runner(
                        thread_id,
                        Some(access),
                        None,
                        &mut options,
                        &self.thread_manager,
                    )
                    .await?;
                self.thread_manager
                    .start_thread(options)
                    .await
                    .map_err(|_| {
                        internal_error("Runner could not start; retain the creation request")
                    })?;
                self.thread_store
                    .persist_thread(thread_id, PersistContext::Standard)
                    .await
                    .map_err(|_| {
                        internal_error("Runner could not be saved; retain the creation request")
                    })?;
            }
        }
        self.ensure_conversation_listener(thread_id, connection, false)
            .await?;
        Ok(())
    }
}
