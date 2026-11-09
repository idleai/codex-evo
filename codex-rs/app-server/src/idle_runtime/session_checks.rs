use super::IdleRuntime;
use super::rpc_error;
use crate::error_code::invalid_request;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_idle_runtime::GrantAccess;
use serde_json::Value;

impl IdleRuntime {
    pub(crate) fn check_session_submission(
        &self,
        access: &GrantAccess,
        operation: &Value,
    ) -> Result<(), JSONRPCErrorError> {
        if operation.get("kind").and_then(Value::as_str) != Some("submit") {
            return Ok(());
        }
        let id = operation
            .pointer("/request/body/session_id")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_request("Input requires an existing session"))?;
        let binding = self
            .service
            .runner(id)
            .map_err(rpc_error)?
            .ok_or_else(|| invalid_request("Runner binding is unavailable"))?;
        let policy = access
            .runner_policy
            .as_ref()
            .ok_or_else(|| invalid_request("Runner permission ceiling is unavailable"))?;
        if binding.workspace != access.binding || binding.policy.intersect(policy) != binding.policy
        {
            return Err(invalid_request(
                "This runner needs compute permissions outside this invitation",
            ));
        }
        self.service.runner_access(&binding).map_err(rpc_error)?;
        Ok(())
    }
}
