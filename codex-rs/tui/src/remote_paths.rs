//! Server-path operations for the TUI. These never access the local filesystem.

use codex_app_server_protocol::SandboxPolicy;
use codex_protocol::models::PermissionProfile;
use codex_protocol::models::SandboxEnforcement;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_utils_path_uri::LegacyAppPathString;
use codex_utils_path_uri::PathUri;
use std::io;
use std::path::Path;

pub(crate) fn path_uri(path: &Path) -> io::Result<PathUri> {
    LegacyAppPathString::from_path(path)
        .to_inferred_path_uri()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid app-server path: {}", path.display()),
            )
        })
}

/// A display snapshot only. Execution policy remains on the server and is not
/// reconstructed from the client's filesystem or sent back on ordinary turns.
pub(crate) fn permission_profile(
    sandbox: &SandboxPolicy,
    cwd: &Path,
) -> io::Result<PermissionProfile> {
    let cwd_uri = path_uri(cwd)?;
    if cwd.is_absolute()
        && let Ok(native_policy) = sandbox.try_to_core()
    {
        return Ok(PermissionProfile::from_legacy_sandbox_policy_for_cwd(
            &native_policy,
            cwd,
        ));
    }
    let SandboxPolicy::WorkspaceWrite {
        writable_roots,
        network_access,
        exclude_tmpdir_env_var,
        exclude_slash_tmp,
    } = sandbox
    else {
        return Ok(PermissionProfile::from_legacy_sandbox_policy(
            &sandbox.try_to_core()?,
        ));
    };
    let mut roots = vec![cwd_uri];
    for root in writable_roots {
        let root = path_uri(root)?;
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    let policy = FileSystemSandboxPolicy::workspace_write_with_path_uris(
        &roots,
        *exclude_tmpdir_env_var,
        *exclude_slash_tmp,
    );
    Ok(
        PermissionProfile::from_runtime_permissions_with_enforcement(
            SandboxEnforcement::Managed,
            &policy,
            if *network_access {
                NetworkSandboxPolicy::Enabled
            } else {
                NetworkSandboxPolicy::Restricted
            },
        ),
    )
}
