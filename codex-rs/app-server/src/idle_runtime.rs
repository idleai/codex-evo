use crate::error_code::internal_error;
use crate::error_code::invalid_params;
use crate::error_code::invalid_request;
use crate::transport::ConnectionOrigin;
use codex_app_server_protocol::IdleRuntimeCapability;
use codex_app_server_protocol::IdleRuntimeStatus;
use codex_app_server_protocol::IdleRuntimeStatusReadParams;
use codex_app_server_protocol::IdleRuntimeStatusReadResponse;
use codex_app_server_protocol::IdleWorkspaceAttachParams;
use codex_app_server_protocol::IdleWorkspaceAttachResponse;
use codex_app_server_protocol::IdleWorkspaceBinding;
use codex_app_server_protocol::IdleWorkspaceStatus;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_idle_runtime::Error;
use codex_idle_runtime::RuntimeStatus;
use codex_idle_runtime::WorkspaceBinding;
use codex_idle_runtime::WorkspaceService;
use std::path::Path;
use std::sync::Arc;

const PROTOCOL_VERSION: u32 = 1;

pub(crate) struct IdleRuntime {
    service: Arc<WorkspaceService>,
}

impl IdleRuntime {
    pub(crate) fn new(codex_home: &Path, installation_id: String) -> Self {
        Self {
            service: Arc::new(WorkspaceService::new(codex_home, installation_id)),
        }
    }

    pub(crate) async fn attach(
        &self,
        origin: ConnectionOrigin,
        params: IdleWorkspaceAttachParams,
    ) -> Result<IdleWorkspaceAttachResponse, JSONRPCErrorError> {
        authorize(origin, params.protocol_version)?;
        let binding = params.binding;
        let service = Arc::clone(&self.service);
        let status = tokio::task::spawn_blocking(move || {
            service.attach(WorkspaceBinding {
                workspace_id: binding.workspace_id,
                repository_id: binding.repository_id,
                checkout_id: binding.checkout_id,
                chain_id: binding.chain_id,
                checkout_root: binding.checkout_root.into(),
                chain_directory: binding.chain_directory.into(),
            })
        })
        .await
        .map_err(|_| internal_error("Idle workspace task failed"))?
        .map_err(rpc_error)?;
        Ok(IdleWorkspaceAttachResponse {
            status: response(status),
        })
    }

    pub(crate) async fn status(
        &self,
        origin: ConnectionOrigin,
        params: IdleRuntimeStatusReadParams,
    ) -> Result<IdleRuntimeStatusReadResponse, JSONRPCErrorError> {
        authorize(origin, params.protocol_version)?;
        let service = Arc::clone(&self.service);
        let status = tokio::task::spawn_blocking(move || service.status())
            .await
            .map_err(|_| internal_error("Idle workspace task failed"))?
            .map_err(rpc_error)?;
        Ok(IdleRuntimeStatusReadResponse {
            status: response(status),
        })
    }

    pub(crate) fn shutdown(&self) {
        self.service.shutdown();
    }
}

fn authorize(origin: ConnectionOrigin, version: u32) -> Result<(), JSONRPCErrorError> {
    // TCP loopback and clientInfo names do not establish local owner authority.
    // The Dev Tunnels ingress will use separate scoped grants, not this local path.
    if !matches!(
        origin,
        ConnectionOrigin::Stdio | ConnectionOrigin::InProcess | ConnectionOrigin::LocalSocket
    ) {
        return Err(invalid_request(
            "Idle workspace access requires a local owner connection",
        ));
    }
    if version != PROTOCOL_VERSION {
        return Err(invalid_params(
            "Unsupported Idle protocol version; expected 1",
        ));
    }
    Ok(())
}

fn rpc_error(error: Error) -> JSONRPCErrorError {
    match error {
        Error::InvalidBinding | Error::Conflict | Error::Limit => invalid_params(error.to_string()),
        Error::Busy | Error::Stopped | Error::Version => invalid_request(error.to_string()),
        Error::Unavailable | Error::Io(_) | Error::Json(_) => {
            internal_error("Idle workspace registry is unavailable")
        }
    }
}

fn response(status: RuntimeStatus) -> IdleRuntimeStatus {
    IdleRuntimeStatus {
        protocol_version: PROTOCOL_VERSION,
        server_version: env!("CARGO_PKG_VERSION").to_string(),
        host_id: status.host_id,
        runtime_id: status.runtime_id,
        capabilities: vec![
            IdleRuntimeCapability::WorkspaceAttachment,
            IdleRuntimeCapability::WorkspaceStatus,
        ],
        workspaces: status
            .workspaces
            .into_iter()
            .map(|workspace| {
                let binding = workspace.binding;
                IdleWorkspaceStatus {
                    binding: IdleWorkspaceBinding {
                        workspace_id: binding.workspace_id,
                        repository_id: binding.repository_id,
                        checkout_id: binding.checkout_id,
                        chain_id: binding.chain_id,
                        checkout_root: binding.checkout_root.to_string_lossy().into_owned(),
                        chain_directory: binding.chain_directory.to_string_lossy().into_owned(),
                    },
                    available: workspace.available,
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_owner_transports_can_access_workspace_bindings() {
        for origin in [
            ConnectionOrigin::Stdio,
            ConnectionOrigin::InProcess,
            ConnectionOrigin::LocalSocket,
        ] {
            assert!(authorize(origin, 1).is_ok());
            assert!(authorize(origin, 2).is_err());
        }
        for origin in [ConnectionOrigin::WebSocket, ConnectionOrigin::RemoteControl] {
            assert!(authorize(origin, 1).is_err());
        }
    }
}
