use crate::error_code::invalid_request;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_core::StartThreadOptions;
use codex_exec_server::EnvironmentManager;
use codex_extension_api::SessionIsolation;
use codex_extension_api::ToolName;
use codex_extension_api::ToolPolicy;
use codex_features::Feature;
use codex_idle_runtime::RunnerBinding;
use codex_protocol::config_types::ShellEnvironmentPolicyInherit;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxEntry;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::FileSystemSpecialPath;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_protocol::protocol::EnvironmentConfigState;
use codex_protocol::protocol::TurnEnvironmentRequest;
use codex_utils_absolute_path::AbsolutePathBuf;

pub(super) fn apply(
    binding: &RunnerBinding,
    options: &mut StartThreadOptions,
    environments: &EnvironmentManager,
) -> Result<(), JSONRPCErrorError> {
    if environments.try_local_environment().is_none() {
        return Err(invalid_request(
            "This runner requires the selected daemon's local executor",
        ));
    }
    let config = &mut options.config;
    let cwd = AbsolutePathBuf::from_absolute_path(&binding.workspace.checkout_root)
        .map_err(|_| invalid_request("Runner checkout path is invalid"))?;
    config.cwd = cwd.clone();
    config.workspace_roots = vec![cwd.clone()];
    config.permissions.set_workspace_roots(vec![cwd.clone()]);
    let mut entries = vec![FileSystemSandboxEntry::new(
        FileSystemPath::Special {
            value: FileSystemSpecialPath::Minimal,
        },
        FileSystemAccessMode::Read,
    )];
    for (roots, access) in [
        (&binding.policy.read_roots, FileSystemAccessMode::Read),
        (&binding.policy.write_roots, FileSystemAccessMode::Write),
    ] {
        for root in roots {
            entries.push(FileSystemSandboxEntry::new(
                FileSystemPath::Path {
                    path: AbsolutePathBuf::from_absolute_path(root)
                        .map_err(|_| invalid_request("Runner path is invalid"))?
                        .into(),
                },
                access,
            ));
        }
    }
    for path in [
        config.codex_home.to_path_buf(),
        binding.workspace.chain_directory.clone(),
        binding.workspace.checkout_root.join(".idle"),
        binding.workspace.checkout_root.join(".codex"),
    ] {
        entries.push(FileSystemSandboxEntry::new(
            FileSystemPath::Path {
                path: AbsolutePathBuf::from_absolute_path(path)
                    .map_err(|_| invalid_request("Runner private path is invalid"))?
                    .into(),
            },
            FileSystemAccessMode::Deny,
        ));
    }
    // The native sandbox re-enters these trusted executables after applying
    // filesystem mounts. Keep their exact paths readable, without their parents.
    let runtime_paths = config
        .codex_self_exe
        .iter()
        .chain(config.codex_linux_sandbox_exe.iter())
        .map(AbsolutePathBuf::from_absolute_path)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid_request("Runner sandbox launcher path is invalid"))?;
    let filesystem = FileSystemSandboxPolicy::restricted(entries)
        .with_additional_readable_roots(cwd.as_path(), &runtime_paths);
    let profile =
        PermissionProfile::from_runtime_permissions(&filesystem, NetworkSandboxPolicy::Restricted);
    config
        .permissions
        .set_permission_profile(profile)
        .map_err(|_| invalid_request("Runner policy conflicts with daemon requirements"))?;
    config
        .permissions
        .freeze_permission_profile()
        .map_err(|_| invalid_request("Runner permission ceiling cannot be fixed"))?;
    config.permissions.allow_login_shell = false;
    config.permissions.network = None;
    config.permissions.shell_environment_policy.inherit = ShellEnvironmentPolicyInherit::None;
    config.permissions.shell_environment_policy.r#set.clear();
    #[cfg(unix)]
    config.permissions.shell_environment_policy.r#set.insert(
        "PATH".to_string(),
        "/usr/bin:/bin:/usr/sbin:/sbin".to_string(),
    );
    config.permissions.shell_environment_policy.use_profile = false;
    config.notify = None;
    config.mcp_servers.set(Default::default()).map_err(|_| {
        invalid_request("Runner tool restrictions conflict with required MCP servers")
    })?;
    for feature in [
        Feature::CodexHooks,
        Feature::ShellSnapshot,
        Feature::ShellSnapshotV2,
        Feature::ShellZshFork,
    ] {
        config.features.disable(feature).map_err(|_| {
            invalid_request("Runner restrictions conflict with daemon feature requirements")
        })?;
        if config.features.enabled(feature) {
            return Err(invalid_request(
                "Runner restrictions conflict with daemon feature requirements",
            ));
        }
    }
    if !config.features.enabled(Feature::UnifiedExec) {
        return Err(invalid_request(
            "Runner requires unified execution with command hooks disabled",
        ));
    }
    config.ephemeral = false;
    options.environments = Some(vec![TurnEnvironmentRequest {
        environment_id: codex_exec_server::LOCAL_ENVIRONMENT_ID.into(),
        cwd: cwd.clone().into(),
        workspace_roots: vec![cwd.into()],
        config: EnvironmentConfigState::FromThread,
    }]);
    options
        .thread_extension_init
        .insert(SessionIsolation::Isolated);
    options
        .thread_extension_init
        .insert(Vec::<codex_protocol::capabilities::SelectedCapabilityRoot>::new());
    options.thread_extension_init.insert(ToolPolicy {
        allowed_tools: Some(vec![
            ToolName::plain("exec_command"),
            ToolName::plain("write_stdin"),
            ToolName::plain("exec"),
            ToolName::plain("wait"),
        ]),
        require_managed_sandbox: true,
        require_unified_exec: true,
        expose_additional_permissions: false,
        freeze_execution_permissions: true,
    });
    Ok(())
}
