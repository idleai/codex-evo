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
