use super::IdleRuntime;
use super::authorize;
use super::rpc_error;
use crate::error_code::invalid_request;
use crate::transport::ConnectionOrigin;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_transport::ConnectionId;
use codex_idle_runtime::GrantAccess;
use codex_idle_runtime::GrantScope;
use serde_json::Value;

pub(super) fn allowed(method: &str) -> bool {
    matches!(
        method,
        "thread/start"
            | "thread/resume"
            | "thread/fork"
            | "thread/read"
            | "thread/unsubscribe"
            | "thread/turns/list"
            | "thread/items/list"
            | "turn/start"
            | "turn/steer"
            | "turn/interrupt"
    )
}

pub(crate) fn validate(method: &str, params: &Value) -> Result<(), JSONRPCErrorError> {
    let keys: &[&str] = match method {
        "thread/start" => &[
            "model",
            "modelProvider",
            "serviceTier",
            "historyMode",
            "experimentalRawEvents",
        ],
        "thread/resume" => &[
            "threadId",
            "model",
            "modelProvider",
            "serviceTier",
            "excludeTurns",
            "initialTurnsPage",
        ],
        "thread/fork" => &["threadId", "excludeTurns", "lastTurnId", "beforeTurnId"],
        "thread/read" => &["threadId", "includeTurns"],
        "thread/unsubscribe" => &["threadId"],
        "thread/turns/list" | "thread/items/list" => {
            &["threadId", "turnId", "cursor", "limit", "sortDirection"]
        }
        "turn/start" => &[
            "threadId",
            "input",
            "model",
            "effort",
            "summary",
            "serviceTier",
            "clientUserMessageId",
        ],
        "turn/steer" => &["threadId", "input", "expectedTurnId"],
        "turn/interrupt" => &["threadId", "turnId"],
        _ => {
            return Err(invalid_request(
                "This operation is outside the runner grant",
            ));
        }
    };
    let params = params
        .as_object()
        .ok_or_else(|| invalid_request("Invalid runner request"))?;
    if params
        .iter()
        .any(|(key, value)| !value.is_null() && !keys.contains(&key.as_str()))
    {
        return Err(invalid_request(
            "Runner requests cannot replace their saved paths, environment, tools or approval policy",
        ));
    }
    if let Some(input) = params.get("input") {
        let input = input
            .as_array()
            .ok_or_else(|| invalid_request("Runner input must be text"))?;
        if input.iter().any(|item| {
            item.get("type").and_then(Value::as_str) != Some("text")
                || item.get("textElements").is_some_and(|elements| {
                    !elements.is_null()
                        && elements
                            .as_array()
                            .is_none_or(|elements| !elements.is_empty())
                })
        }) {
            return Err(invalid_request("Runner input must be plain text"));
        }
    }
    Ok(())
}

impl IdleRuntime {
    pub(crate) async fn runner_response(
        &self,
        origin: ConnectionOrigin,
        connection: ConnectionId,
        thread_id: Option<codex_protocol::ThreadId>,
        result: &Value,
    ) -> Result<(), JSONRPCErrorError> {
        let binding = thread_id
            .map(|id| self.service.runner(&id.to_string()))
            .transpose()
            .map_err(rpc_error)?
            .flatten();
        let Some(binding) = binding else {
            return if origin == ConnectionOrigin::IdleRemote {
                Err(invalid_request("Runner approval target is unavailable"))
            } else {
                Ok(())
            };
        };
        if origin == ConnectionOrigin::IdleRemote {
            let access = self.relay.access(connection).map_err(rpc_error)?;
            if access.scope != GrantScope::Runner
                || access.grant_id != binding.grant_id
                || access.client_id != binding.client_id
                || access.binding != binding.workspace
            {
                return Err(invalid_request(
                    "This connection cannot approve runner work",
                ));
            }
        } else {
            authorize(origin, /*version*/ 1)?;
        }
        if !matches!(
            result.get("decision").and_then(Value::as_str),
            Some("accept" | "acceptForSession" | "decline" | "cancel")
        ) {
            return Err(invalid_request(
                "Runner approval cannot change execution or network policy",
            ));
        }
        self.check_runner(&self.loaded_runner(&binding)).await
    }
    pub(crate) async fn runner_request(
        &self,
        origin: ConnectionOrigin,
        connection: ConnectionId,
        request: &ClientRequest,
    ) -> Result<Option<GrantAccess>, JSONRPCErrorError> {
        let access = if origin == ConnectionOrigin::IdleRemote {
            Some(self.relay.access(connection).map_err(rpc_error)?)
        } else {
            None
        };
        let method = request.method_name();
        let request =
            serde_json::to_value(request).map_err(|_| invalid_request("Invalid runner request"))?;
        let params = &request["params"];
        if method == "thread/start"
            && access
                .as_ref()
                .is_some_and(|access| access.scope == GrantScope::Runner)
        {
            validate(method, params)?;
            return Ok(access);
        }
        let thread_id = params
            .get("threadId")
            .and_then(Value::as_str)
            .and_then(|id| codex_protocol::ThreadId::from_string(id).ok());
        let binding = thread_id
            .map(|id| self.service.runner(&id.to_string()))
            .transpose()
            .map_err(rpc_error)?
            .flatten();
        if let Some(binding) = binding {
            if let Some(access) = &access {
                if access.scope != GrantScope::Runner
                    || access.binding != binding.workspace
                    || access.client_id != binding.client_id
                    || access.grant_id != binding.grant_id
                {
                    return Err(invalid_request("This connection cannot access the runner"));
                }
            } else {
                authorize(origin, /*version*/ 1)?;
            }
            validate(method, params)?;
            if matches!(method, "thread/resume" | "thread/fork") {
                self.coordination
                    .runner(&binding, super::coordination::RunnerRequest::Register)
                    .await
                    .map_err(|_| invalid_request("Native helper cannot restore this runner"))?;
                let rules = self.current_rules(&binding).await?;
                binding
                    .policy
                    .restrict(&binding.workspace, rules.as_deref())
                    .map_err(rpc_error)?;
            } else {
                self.check_runner(&self.loaded_runner(&binding)).await?;
            }
        } else if access
            .as_ref()
            .is_some_and(|access| access.scope == GrantScope::Runner)
            && allowed(method)
        {
            return Err(invalid_request("Runner binding is unavailable"));
        }
        Ok(None)
    }
}
