use anyhow::Context;
use clap::Args;
use codex_app_server_client::RemoteAppServerClient;
use codex_app_server_client::RemoteAppServerConnectArgs;
use codex_app_server_client::RemoteAppServerEndpoint;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::IdleConnectionInviteParams;
use codex_app_server_protocol::IdleConnectionInviteResponse;
use codex_app_server_protocol::IdleConnectionRevokeParams;
use codex_app_server_protocol::IdleConnectionRevokeResponse;
use codex_app_server_protocol::IdleRelayConfigureParams;
use codex_app_server_protocol::IdleRelayConfigureResponse;
use codex_app_server_protocol::IdleRelayStopParams;
use codex_app_server_protocol::IdleRelayStopResponse;
use codex_app_server_protocol::IdleRuntimeStatusReadParams;
use codex_app_server_protocol::IdleRuntimeStatusReadResponse;
use codex_app_server_protocol::IdleWorkspaceAttachParams;
use codex_app_server_protocol::IdleWorkspaceAttachResponse;
use codex_app_server_protocol::IdleWorkspaceBinding;
use codex_app_server_protocol::RequestId;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde::Deserialize;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// Connect an Idle workspace to the local Codex daemon.
#[derive(Debug, Args)]
pub(crate) struct IdleCommand {
    /// Local daemon control socket. Defaults to the socket for CODEX_HOME.
    #[arg(long, global = true)]
    socket_path: Option<PathBuf>,
    #[command(subcommand)]
    command: Operation,
}

#[derive(Debug, clap::Subcommand)]
enum Operation {
    /// Approve a VS Code connection request and save its private invitation.
    Host(Host),
    /// Read the daemon's saved workspace attachments.
    Status,
    /// Revoke a previously issued workspace connection grant.
    Revoke {
        #[arg(long)]
        grant_id: String,
    },
    /// Stop runtime sharing and remove its Dev Tunnel; keep the daemon running.
    Stop,
}

#[derive(Debug, Args)]
struct Host {
    /// JSON request copied with Idle: Copy Compute Connection Request.
    #[arg(long)]
    request: PathBuf,
    /// Existing checkout directory on this compute host.
    #[arg(long)]
    checkout_root: PathBuf,
    /// Existing EditChain directory on this compute host.
    #[arg(long)]
    chain_directory: PathBuf,
    /// Absolute path to the installed idle-host binary from host-tools.
    #[arg(long)]
    relay_helper: PathBuf,
    /// Absolute path to an authenticated GitHub CLI installation.
    #[arg(long)]
    github_cli: PathBuf,
    /// New private file for the invitation. Existing files are not overwritten.
    #[arg(long)]
    output: PathBuf,
    /// Grant lifetime in hours, from one to 168.
    #[arg(long, default_value_t = 24, value_parser = clap::value_parser!(u16).range(1..=168))]
    hours: u16,
    /// Allow this client to move standalone workspace coordination to the daemon.
    #[arg(long)]
    coordination_owner: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConnectionRequest {
    version: u32,
    workspace_id: String,
    repository_id: String,
    chain_id: String,
    client_id: String,
}

impl IdleCommand {
    pub(crate) async fn run(self) -> anyhow::Result<()> {
        let socket_path = match self.socket_path {
            Some(path) => AbsolutePathBuf::try_from(path)?,
            None => codex_app_server_client::app_server_control_socket_path(
                &codex_core::config::find_codex_home()?,
            )?,
        };
        let args = RemoteAppServerConnectArgs {
            endpoint: RemoteAppServerEndpoint::UnixSocket { socket_path },
            client_name: "codex-idle-owner".into(),
            client_version: env!("CARGO_PKG_VERSION").into(),
            experimental_api: true,
            mcp_server_openai_form_elicitation: false,
            opt_out_notification_methods: Vec::new(),
            channel_capacity: 16,
        };
        #[cfg(windows)]
        let client = RemoteAppServerClient::connect_local_daemon(args).await?;
        #[cfg(not(windows))]
        let client = RemoteAppServerClient::connect(args)
            .await
            .context("Start the updated local Codex app-server daemon first")?;
        let result = match self.command {
            Operation::Host(host) => host.run(&client).await,
            Operation::Status => {
                let response = read_status(&client).await?;
                println!("{}", serde_json::to_string_pretty(&response.status)?);
                Ok(())
            }
            Operation::Revoke { grant_id } => {
                let _: IdleConnectionRevokeResponse = client
                    .request_typed(ClientRequest::IdleConnectionRevoke {
                        request_id: RequestId::Integer(1),
                        params: IdleConnectionRevokeParams {
                            protocol_version: 1,
                            grant_id,
                        },
                    })
                    .await?;
                println!("Idle connection revoked.");
                Ok(())
            }
            Operation::Stop => {
                let _: IdleRelayStopResponse = client
                    .request_typed(ClientRequest::IdleRelayStop {
                        request_id: RequestId::Integer(1),
                        params: IdleRelayStopParams {
                            protocol_version: 1,
                        },
                    })
                    .await?;
                println!("Idle runtime sharing stopped. The Codex daemon remains running.");
                Ok(())
            }
        };
        let closed = client.shutdown().await;
        result?;
        closed?;
        Ok(())
    }
}

impl Host {
    async fn run(self, client: &RemoteAppServerClient) -> anyhow::Result<()> {
        let mut bytes = Vec::new();
        File::open(&self.request)?
            .take(32_769)
            .read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() <= 32_768,
            "Compute connection request is too large"
        );
        let request: ConnectionRequest = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(
            request.version == 1,
            "Unsupported compute connection request version"
        );
        let checkout_root = self.checkout_root.canonicalize()?;
        let chain_directory = self.chain_directory.canonicalize()?;
        let relay_helper = self.relay_helper.canonicalize()?;
        let github_cli = self.github_cli.canonicalize()?;
        let existing = read_status(client).await?.status.workspaces;
        let checkout_id = existing
            .iter()
            .find(|entry| std::path::Path::new(&entry.binding.checkout_root) == checkout_root)
            .map(|entry| entry.binding.checkout_id.clone())
            .unwrap_or_else(|| format!("checkout:{}", uuid::Uuid::new_v4()));
        let binding = IdleWorkspaceBinding {
            workspace_id: request.workspace_id,
            repository_id: request.repository_id,
            checkout_id: checkout_id.clone(),
            chain_id: request.chain_id,
            checkout_root: checkout_root
                .to_str()
                .context("Checkout path is not UTF-8")?
                .into(),
            chain_directory: chain_directory
                .to_str()
                .context("Chain path is not UTF-8")?
                .into(),
        };
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut output = options
            .open(&self.output)
            .context("Choose a new private invitation file")?;
        let _: IdleWorkspaceAttachResponse = client
            .request_typed(ClientRequest::IdleWorkspaceAttach {
                request_id: RequestId::Integer(2),
                params: IdleWorkspaceAttachParams {
                    protocol_version: 1,
                    binding,
                },
            })
            .await?;
        let _: IdleRelayConfigureResponse = client
            .request_typed(ClientRequest::IdleRelayConfigure {
                request_id: RequestId::Integer(3),
                params: IdleRelayConfigureParams {
                    protocol_version: 1,
                    helper_path: relay_helper
                        .to_str()
                        .context("Helper path is not UTF-8")?
                        .into(),
                    credential_program: github_cli
                        .to_str()
                        .context("GitHub CLI path is not UTF-8")?
                        .into(),
                },
            })
            .await?;
        let now = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
        let expires_at = now.saturating_add(u64::from(self.hours) * 60 * 60 * 1000);
        let response: IdleConnectionInviteResponse = client
            .request_typed(ClientRequest::IdleConnectionInvite {
                request_id: RequestId::Integer(4),
                params: IdleConnectionInviteParams {
                    protocol_version: 1,
                    checkout_id,
                    client_id: request.client_id,
                    expires_at,
                    coordination_owner: self.coordination_owner,
                },
            })
            .await?;
        output.write_all(response.invitation.as_bytes())?;
        output.write_all(b"\n")?;
        output.sync_all()?;
        println!(
            "Invitation saved to {}. Grant: {}",
            self.output.display(),
            response.grant_id
        );
        println!("Paste the invitation into Idle: Connect Compute Host in VS Code.");
        Ok(())
    }
}

async fn read_status(
    client: &RemoteAppServerClient,
) -> anyhow::Result<IdleRuntimeStatusReadResponse> {
    Ok(client
        .request_typed(ClientRequest::IdleRuntimeStatusRead {
            request_id: RequestId::Integer(1),
            params: IdleRuntimeStatusReadParams {
                protocol_version: 1,
            },
        })
        .await?)
}
