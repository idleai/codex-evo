use crate::JsonSchema;
use crate::TS;
use serde::Deserialize;
use serde::Serialize;

/// Explicit binding approved by the local daemon owner; never inferred from a manifest.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct IdleWorkspaceBinding {
    pub workspace_id: String,
    pub repository_id: String,
    pub checkout_id: String,
    pub chain_id: String,
    pub checkout_root: String,
    pub chain_directory: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct IdleWorkspaceAttachParams {
    /// Must equal the Idle protocol version advertised by status/read.
    pub protocol_version: u32,
    pub binding: IdleWorkspaceBinding,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct IdleRuntimeStatusReadParams {
    pub protocol_version: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum IdleRuntimeCapability {
    WorkspaceAttachment,
    WorkspaceStatus,
    WorkspaceCoordination,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct IdleWorkspaceStatus {
    pub binding: IdleWorkspaceBinding,
    pub available: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct IdleRuntimeStatus {
    pub protocol_version: u32,
    pub server_version: String,
    pub host_id: String,
    pub host_name: String,
    pub runtime_id: String,
    pub capabilities: Vec<IdleRuntimeCapability>,
    pub workspaces: Vec<IdleWorkspaceStatus>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct IdleWorkspaceAttachResponse {
    pub status: IdleRuntimeStatus,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct IdleRuntimeStatusReadResponse {
    pub status: IdleRuntimeStatus,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct IdleRelayConfigureParams {
    pub protocol_version: u32,
    /// Absolute path to the installed idle-host executable, selected by the owner.
    pub helper_path: String,
    /// Absolute path to GitHub CLI, which supplies the daemon's management credential.
    pub credential_program: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum IdleRelayState {
    Disabled,
    Starting,
    Ready,
    Unavailable,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct IdleRelayConfigureResponse {
    pub state: IdleRelayState,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct IdleConnectionInviteParams {
    pub protocol_version: u32,
    pub checkout_id: String,
    pub client_id: String,
    /// Grant expiry in Unix milliseconds, at most seven days from issuance.
    pub expires_at: u64,
    /// Explicitly permit transfer and administration of standalone workspace metadata.
    /// Existing attachment grants never acquire this permission automatically.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub coordination_owner: bool,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct IdleCoordinationCallParams {
    pub protocol_version: u32,
    pub checkout_id: String,
    /// Must match the authenticated runtime grant on remote connections.
    pub client_id: String,
    /// Bounded host-tools coordination request, preserving its native value encoding.
    pub request: String,
}

impl std::fmt::Debug for IdleCoordinationCallParams {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdleCoordinationCallParams")
            .field("checkout_id", &self.checkout_id)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct IdleCoordinationCallResponse {
    /// Native result envelope; mutations retain their original request identities.
    pub response: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct IdleConnectionInviteResponse {
    /// Private invitation. Store as a credential; never publish it as a resource route.
    pub invitation: String,
    pub grant_id: String,
    pub expires_at: u64,
}

impl std::fmt::Debug for IdleConnectionInviteResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdleConnectionInviteResponse")
            .field("invitation", &"[redacted]")
            .field("grant_id", &self.grant_id)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct IdleConnectionRevokeParams {
    pub protocol_version: u32,
    pub grant_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct IdleConnectionRevokeResponse {}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct IdleRelayStopParams {
    pub protocol_version: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct IdleRelayStopResponse {}
