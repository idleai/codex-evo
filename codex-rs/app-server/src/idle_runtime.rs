use crate::error_code::internal_error;
use crate::error_code::invalid_params;
use crate::error_code::invalid_request;
use crate::transport::ConnectionOrigin;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::IdleConnectionInviteParams;
use codex_app_server_protocol::IdleConnectionInviteResponse;
use codex_app_server_protocol::IdleConnectionRevokeParams;
use codex_app_server_protocol::IdleConnectionRevokeResponse;
use codex_app_server_protocol::IdleCoordinationCallParams;
use codex_app_server_protocol::IdleCoordinationCallResponse;
use codex_app_server_protocol::IdleRelayConfigureParams;
use codex_app_server_protocol::IdleRelayConfigureResponse;
use codex_app_server_protocol::IdleRelayStopParams;
use codex_app_server_protocol::IdleRelayStopResponse;
use codex_app_server_protocol::IdleRuntimeCapability;
use codex_app_server_protocol::IdleRuntimeStatus;
use codex_app_server_protocol::IdleRuntimeStatusReadParams;
use codex_app_server_protocol::IdleRuntimeStatusReadResponse;
use codex_app_server_protocol::IdleWorkspaceAttachParams;
use codex_app_server_protocol::IdleWorkspaceAttachResponse;
use codex_app_server_protocol::IdleWorkspaceBinding;
use codex_app_server_protocol::IdleWorkspaceStatus;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_transport::ConnectionId;
use codex_app_server_transport::TransportEvent;
use codex_idle_runtime::Error;
use codex_idle_runtime::GrantAccess;
use codex_idle_runtime::GrantScope;
use codex_idle_runtime::RelayConfiguration;
use codex_idle_runtime::RuntimeStatus;
use codex_idle_runtime::WorkspaceBinding;
use codex_idle_runtime::WorkspaceService;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const PROTOCOL_VERSION: u32 = 1;

pub(crate) struct IdleRuntime {
    service: Arc<WorkspaceService>,
    relay: Arc<relay::Relay>,
    coordination: coordination::Coordination,
    // Admit one relay administration operation at a time, including its I/O.
    admin: Semaphore,
}

impl IdleRuntime {
    pub(crate) fn new(codex_home: &Path, installation_id: String) -> Self {
        let service = Arc::new(WorkspaceService::new(codex_home, installation_id));
        Self {
            relay: Arc::new(relay::Relay::new(service.clone(), codex_home)),
            coordination: coordination::Coordination::new(service.clone(), codex_home),
            service,
            admin: Semaphore::new(1),
        }
    }

    pub(crate) async fn install_transport(
        &self,
        events: mpsc::Sender<TransportEvent>,
        cancel: CancellationToken,
    ) {
        self.relay.install(events, cancel).await;
        self.coordination.restore().await;
    }

    pub(crate) fn authorize_request(
        &self,
        origin: ConnectionOrigin,
        id: ConnectionId,
        request: &ClientRequest,
    ) -> Result<(), JSONRPCErrorError> {
        if origin != ConnectionOrigin::IdleRemote {
            return Ok(());
        }
        let access = self.relay.access(id).map_err(rpc_error)?;
        match request {
            ClientRequest::IdleCoordinationCall { params, .. }
                if access.scope == GrantScope::CoordinationOwner
                    && params.checkout_id == access.binding.checkout_id
                    && params.client_id == access.client_id =>
            {
                Ok(())
            }
            ClientRequest::Initialize { .. } | ClientRequest::IdleRuntimeStatusRead { .. } => {
                Ok(())
            }
            ClientRequest::IdleWorkspaceAttach { params, .. }
                if binding_matches(&params.binding, &access) =>
            {
                Ok(())
            }
            _ => Err(invalid_request(
                "This operation is not permitted by the Idle connection grant",
            )),
        }
    }

    pub(crate) async fn attach(
        &self,
        origin: ConnectionOrigin,
        id: ConnectionId,
        params: IdleWorkspaceAttachParams,
    ) -> Result<IdleWorkspaceAttachResponse, JSONRPCErrorError> {
        let access = self.access(origin, id, params.protocol_version)?;
        if access
            .as_ref()
            .is_some_and(|access| !binding_matches(&params.binding, access))
        {
            return Err(invalid_request(
                "Workspace does not match the Idle connection grant",
            ));
        }
        if access.is_some() {
            // A remote attachment selects an existing owner-approved binding.
            // Missing checkout directories remain visible in its status.
            return self
                .status(
                    origin,
                    id,
                    IdleRuntimeStatusReadParams {
                        protocol_version: params.protocol_version,
                    },
                )
                .await
                .map(|response| IdleWorkspaceAttachResponse {
                    status: response.status,
                });
        }
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
            status: response(scoped(status, access.as_ref()), access.as_ref()),
        })
    }

    pub(crate) async fn status(
        &self,
        origin: ConnectionOrigin,
        id: ConnectionId,
        params: IdleRuntimeStatusReadParams,
    ) -> Result<IdleRuntimeStatusReadResponse, JSONRPCErrorError> {
        let access = self.access(origin, id, params.protocol_version)?;
        let service = Arc::clone(&self.service);
        let status = tokio::task::spawn_blocking(move || service.status())
            .await
            .map_err(|_| internal_error("Idle workspace task failed"))?
            .map_err(rpc_error)?;
        Ok(IdleRuntimeStatusReadResponse {
            status: response(scoped(status, access.as_ref()), access.as_ref()),
        })
    }

    pub(crate) async fn configure(
        &self,
        origin: ConnectionOrigin,
        params: IdleRelayConfigureParams,
    ) -> Result<IdleRelayConfigureResponse, JSONRPCErrorError> {
        authorize(origin, params.protocol_version)?;
        let _admin = self
            .admin
            .acquire()
            .await
            .map_err(|_| internal_error("Idle relay administration stopped"))?;
        let configuration = RelayConfiguration {
            helper_path: params.helper_path.into(),
            credential_program: params.credential_program.into(),
        };
        let unchanged = self
            .service
            .relay_configuration()
            .map_err(rpc_error)?
            .as_ref()
            == Some(&configuration);
        let service = self.service.clone();
        tokio::task::spawn_blocking(move || service.configure_relay(Some(configuration)))
            .await
            .map_err(|_| internal_error("Idle relay setup failed"))?
            .map_err(rpc_error)?;
        if !unchanged {
            self.relay.shutdown().await;
            self.coordination.reset().await;
        }
        self.relay.start().await.map_err(rpc_error)?;
        self.coordination.restore().await;
        Ok(IdleRelayConfigureResponse {
            state: self.relay.state(),
        })
    }

    pub(crate) async fn invite(
        &self,
        origin: ConnectionOrigin,
        params: IdleConnectionInviteParams,
    ) -> Result<IdleConnectionInviteResponse, JSONRPCErrorError> {
        authorize(origin, params.protocol_version)?;
        let _admin = self
            .admin
            .acquire()
            .await
            .map_err(|_| internal_error("Idle relay administration stopped"))?;
        let descriptor = self
            .relay
            .descriptor()
            .await
            .map_err(|_| internal_error("Idle relay is unavailable"))?;
        let service = self.service.clone();
        let grant = tokio::task::spawn_blocking(move || {
            service.issue_scoped_grant(
                &params.checkout_id,
                &params.client_id,
                params.expires_at,
                if params.coordination_owner {
                    GrantScope::CoordinationOwner
                } else {
                    GrantScope::Attachment
                },
            )
        })
        .await
        .map_err(|_| internal_error("Idle connection grant failed"))?
        .map_err(rpc_error)?;
        let host_id = self.service.status().map_err(rpc_error)?.host_id;
        let access = grant.access;
        let mut invitation = json!({"version":1, "hostId":host_id,
            "workspaceId":access.binding.workspace_id, "repositoryId":access.binding.repository_id,
            "checkoutId":access.binding.checkout_id, "chainId":access.binding.chain_id,
            "clientId":access.client_id, "grantId":access.grant_id,
            "grantToken":grant.token, "relay":descriptor, "expiresAt":access.expires_at});
        if access.scope == GrantScope::CoordinationOwner {
            invitation["version"] = json!(2);
            invitation["coordinationOwner"] = json!(true);
        }
        let bytes = serde_json::to_vec(&invitation)
            .map_err(|_| internal_error("Idle invitation encoding failed"))?;
        Ok(IdleConnectionInviteResponse {
            invitation: format!("idle-runtime:{}", URL_SAFE_NO_PAD.encode(bytes)),
            grant_id: access.grant_id,
            expires_at: access.expires_at,
        })
    }

    pub(crate) async fn revoke(
        &self,
        origin: ConnectionOrigin,
        params: IdleConnectionRevokeParams,
    ) -> Result<IdleConnectionRevokeResponse, JSONRPCErrorError> {
        authorize(origin, params.protocol_version)?;
        let service = self.service.clone();
        tokio::task::spawn_blocking(move || service.revoke_grant(&params.grant_id))
            .await
            .map_err(|_| internal_error("Idle connection revocation failed"))?
            .map_err(rpc_error)?;
        Ok(IdleConnectionRevokeResponse {})
    }

    pub(crate) async fn stop(
        &self,
        origin: ConnectionOrigin,
        params: IdleRelayStopParams,
    ) -> Result<IdleRelayStopResponse, JSONRPCErrorError> {
        authorize(origin, params.protocol_version)?;
        let _admin = self
            .admin
            .acquire()
            .await
            .map_err(|_| internal_error("Idle relay administration stopped"))?;
        self.relay.remove().await.map_err(|_| {
            internal_error("Idle relay cleanup failed; retry when the relay is available")
        })?;
        let service = self.service.clone();
        tokio::task::spawn_blocking(move || service.configure_relay(None))
            .await
            .map_err(|_| internal_error("Idle relay shutdown failed"))?
            .map_err(rpc_error)?;
        Ok(IdleRelayStopResponse {})
    }

    fn access(
        &self,
        origin: ConnectionOrigin,
        id: ConnectionId,
        version: u32,
    ) -> Result<Option<GrantAccess>, JSONRPCErrorError> {
        if origin == ConnectionOrigin::IdleRemote {
            if version != PROTOCOL_VERSION {
                return Err(invalid_params(
                    "Unsupported Idle protocol version; expected 1",
                ));
            }
            return self.relay.access(id).map(Some).map_err(rpc_error);
        }
        authorize(origin, version)?;
        Ok(None)
    }

    pub(crate) async fn coordination_call(
        &self,
        origin: ConnectionOrigin,
        id: ConnectionId,
        params: IdleCoordinationCallParams,
    ) -> Result<IdleCoordinationCallResponse, JSONRPCErrorError> {
        let access = self.access(origin, id, params.protocol_version)?;
        if params.request.len() > 256 * 1024
            || params.client_id.is_empty()
            || params.client_id.len() > 256
            || access.as_ref().is_some_and(|access| {
                access.scope != GrantScope::CoordinationOwner
                    || access.client_id != params.client_id
                    || access.binding.checkout_id != params.checkout_id
            })
        {
            return Err(invalid_request(
                "This connection cannot administer workspace coordination",
            ));
        }
        let request = serde_json::from_str(&params.request)
            .map_err(|_| invalid_params("Invalid coordination request"))?;
        let binding = self
            .service
            .status()
            .map_err(rpc_error)?
            .workspaces
            .into_iter()
            .find(|workspace| {
                workspace.available && workspace.binding.checkout_id == params.checkout_id
            })
            .ok_or_else(|| invalid_request("Idle workspace is unavailable"))?
            .binding;
        let result = self.coordination.call(binding, params.client_id, access, request).await
            .map_err(|_| internal_error("Idle coordination owner is unavailable; retain the original request for recovery"))?;
        Ok(IdleCoordinationCallResponse {
            response: serde_json::to_string(&result)
                .map_err(|_| internal_error("Idle coordination response encoding failed"))?,
        })
    }

    pub(crate) async fn shutdown(&self) {
        if let Ok(_admin) = self.admin.acquire().await {
            self.relay.shutdown().await;
            self.coordination.shutdown().await;
            self.service.shutdown();
        }
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
        Error::Busy | Error::Stopped | Error::Version | Error::Denied | Error::Expired => {
            invalid_request(error.to_string())
        }
        Error::Unavailable | Error::Io(_) | Error::Json(_) => {
            internal_error("Idle workspace registry is unavailable")
        }
    }
}

fn binding_matches(binding: &IdleWorkspaceBinding, access: &GrantAccess) -> bool {
    let approved = &access.binding;
    binding.workspace_id == approved.workspace_id
        && binding.repository_id == approved.repository_id
        && binding.checkout_id == approved.checkout_id
        && binding.chain_id == approved.chain_id
        && Path::new(&binding.checkout_root) == approved.checkout_root
        && Path::new(&binding.chain_directory) == approved.chain_directory
}

fn scoped(mut status: RuntimeStatus, access: Option<&GrantAccess>) -> RuntimeStatus {
    if let Some(access) = access {
        status
            .workspaces
            .retain(|workspace| workspace.binding == access.binding);
    }
    status
}

fn response(status: RuntimeStatus, access: Option<&GrantAccess>) -> IdleRuntimeStatus {
    let mut capabilities = vec![
        IdleRuntimeCapability::WorkspaceAttachment,
        IdleRuntimeCapability::WorkspaceStatus,
    ];
    if access.is_none_or(|access| access.scope == GrantScope::CoordinationOwner) {
        capabilities.push(IdleRuntimeCapability::WorkspaceCoordination);
    }
    IdleRuntimeStatus {
        protocol_version: PROTOCOL_VERSION,
        server_version: env!("CARGO_PKG_VERSION").to_string(),
        host_id: status.host_id,
        host_name: gethostname::gethostname()
            .to_string_lossy()
            .chars()
            .take(256)
            .collect(),
        runtime_id: status.runtime_id,
        capabilities,
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
        for origin in [
            ConnectionOrigin::WebSocket,
            ConnectionOrigin::RemoteControl,
            ConnectionOrigin::IdleRemote,
        ] {
            assert!(authorize(origin, 1).is_err());
        }
    }
}
mod connections;
mod coordination;
mod relay;
mod wire;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
