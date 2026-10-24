use super::IdleRuntime;
use super::coordination::RunnerRequest;
use super::rpc_error;
use crate::error_code::invalid_request;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ToolAdmissionContributor;
use codex_extension_api::ToolAdmissionInput;
use codex_extension_api::ToolPayload;
use codex_idle_runtime::RunnerBinding;
use serde_json::Value;

impl IdleRuntime {
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
