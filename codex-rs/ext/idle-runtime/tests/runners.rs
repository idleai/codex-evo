use anyhow::Result;
use codex_idle_runtime::GrantScope;
use codex_idle_runtime::RunnerBinding;
use codex_idle_runtime::RunnerEnvironment;
use codex_idle_runtime::RunnerPolicy;
use codex_idle_runtime::WorkspaceBinding;
use codex_idle_runtime::WorkspaceService;
use pretty_assertions::assert_eq;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tempfile::TempDir;

fn setup(
    home: &TempDir,
    checkout: &TempDir,
    chain: &TempDir,
) -> Result<(WorkspaceService, RunnerBinding)> {
    let service = WorkspaceService::new(home.path(), "host".into());
    let workspace = WorkspaceBinding {
        workspace_id: "workspace".into(),
        repository_id: "repository".into(),
        checkout_id: "checkout".into(),
        chain_id: "chain".into(),
        checkout_root: checkout.path().canonicalize()?,
        chain_directory: chain.path().canonicalize()?,
    };
    service.attach(workspace.clone())?;
    let policy = RunnerPolicy::new(
        &workspace,
        vec![workspace.checkout_root.clone()],
        vec![workspace.checkout_root.clone()],
    )?;
    let grant = service.issue_runner_grant("checkout", "owner", future()?, policy.clone())?;
    Ok((
        service,
        RunnerBinding {
            thread_id: uuid::Uuid::new_v4().to_string(),
            workspace,
            client_id: "owner".into(),
            grant_id: grant.access.grant_id,
            environment: RunnerEnvironment::Local,
            policy,
            parent: None,
        },
    ))
}

fn future() -> Result<u64> {
    Ok(u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())? + 60_000)
}

#[test]
fn durable_runner_and_fork_keep_their_binding_after_revocation() -> Result<()> {
    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let chain = TempDir::new()?;
    let (service, binding) = setup(&home, &checkout, &chain)?;
    service.bind_runner(binding.clone())?;
    service.bind_runner(binding.clone())?;
    let mut child = binding.clone();
    child.thread_id = uuid::Uuid::new_v4().to_string();
    child.parent = Some(binding.thread_id.clone());
    child.policy.write_roots.clear();
    service.bind_runner(child.clone())?;
    service.shutdown();
    let restarted = WorkspaceService::new(home.path(), "host".into());
    assert_eq!(restarted.runner(&binding.thread_id)?, Some(binding.clone()));
    assert_eq!(restarted.runner(&child.thread_id)?, Some(child.clone()));
    restarted.runner_access(&binding)?;
    restarted.runner_access(&child)?;
    let mut rebound = binding.clone();
    rebound.policy.write_roots.clear();
    assert!(restarted.bind_runner(rebound).is_err());
    restarted.revoke_grant(&binding.grant_id)?;
    assert!(restarted.runner_access(&binding).is_err());
    assert!(restarted.runner_access(&child).is_err());
    assert_eq!(restarted.runner(&binding.thread_id)?, Some(binding));
    assert_eq!(restarted.runner(&child.thread_id)?, Some(child));
    Ok(())
}

#[test]
fn attachment_and_coordination_grants_cannot_create_runner_bindings() -> Result<()> {
    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let chain = TempDir::new()?;
    let (service, binding) = setup(&home, &checkout, &chain)?;
    for scope in [GrantScope::Attachment, GrantScope::CoordinationOwner] {
        let grant = service.issue_scoped_grant("checkout", "owner", future()?, scope)?;
        let mut denied = binding.clone();
        denied.grant_id = grant.access.grant_id;
        assert!(service.bind_runner(denied).is_err());
    }
    assert!(
        service
            .issue_scoped_grant("checkout", "owner", future()?, GrantScope::Runner)
            .is_err()
    );
    let mut stranger = binding.clone();
    stranger.client_id = "stranger".into();
    assert!(service.bind_runner(stranger).is_err());
    assert_eq!(service.runner(&binding.thread_id)?, None);
    service.bind_runner(binding.clone())?;
    let mut child = binding.clone();
    child.thread_id = uuid::Uuid::new_v4().to_string();
    child.parent = Some(uuid::Uuid::new_v4().to_string());
    assert!(service.bind_runner(child.clone()).is_err());
    child.parent = Some(binding.thread_id.clone());
    child.grant_id = service
        .issue_runner_grant("checkout", "owner", future()?, binding.policy)?
        .access
        .grant_id;
    assert!(service.bind_runner(child).is_err());
    Ok(())
}

#[test]
fn corrupt_state_never_turns_a_bound_runner_into_an_unbound_thread() -> Result<()> {
    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let chain = TempDir::new()?;
    let (service, binding) = setup(&home, &checkout, &chain)?;
    service.bind_runner(binding.clone())?;
    service.shutdown();
    std::fs::write(home.path().join("idle-runtime/workspaces.json"), "{")?;
    let broken = WorkspaceService::new(home.path(), "host".into());
    assert_eq!(broken.runner(&binding.thread_id)?, Some(binding.clone()));
    assert!(broken.runner_access(&binding).is_err());
    assert_eq!(broken.runner(&uuid::Uuid::new_v4().to_string())?, None);
    let marker = home
        .path()
        .join(format!("idle-runtime/runner-{}.json", binding.thread_id));
    std::fs::write(&marker, "{")?;
    assert!(broken.runner(&binding.thread_id).is_err());
    assert!(broken.runner("../workspaces").is_err());
    assert_eq!(broken.runner(&uuid::Uuid::new_v4().to_string())?, None);
    Ok(())
}
