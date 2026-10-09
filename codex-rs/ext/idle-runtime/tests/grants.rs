use anyhow::Result;
use codex_idle_runtime::Error;
use codex_idle_runtime::GrantScope;
use codex_idle_runtime::RelayConfiguration;
use codex_idle_runtime::Secret;
use codex_idle_runtime::WorkspaceBinding;
use codex_idle_runtime::WorkspaceService;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tempfile::TempDir;

fn binding(checkout: &TempDir, chain: &TempDir) -> Result<WorkspaceBinding> {
    Ok(WorkspaceBinding {
        workspace_id: "workspace:one".into(),
        repository_id: "repository:one".into(),
        checkout_id: "checkout:one".into(),
        chain_id: "chain:one".into(),
        checkout_root: checkout.path().canonicalize()?,
        chain_directory: chain.path().canonicalize()?,
    })
}

fn future() -> Result<u64> {
    Ok(u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())? + 60_000)
}

#[test]
fn grants_survive_restart_and_revocation_is_durable() -> Result<()> {
    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let chain = TempDir::new()?;
    let binding = binding(&checkout, &chain)?;
    let service = WorkspaceService::new(home.path(), "host:one".into());
    service.attach(binding.clone())?;
    let grant = service.issue_grant(&binding.checkout_id, "client:one", future()?)?;
    assert_eq!(
        service
            .authenticate(&grant.access.grant_id, &grant.token)?
            .binding,
        binding
    );
    assert!(matches!(
        service.authenticate(&grant.access.grant_id, &Secret("wrong".into())),
        Err(Error::Denied)
    ));
    let saved = std::fs::read_to_string(home.path().join("idle-runtime/workspaces.json"))?;
    assert!(!saved.contains(&grant.token.0));
    assert!(!format!("{grant:?}").contains(&grant.token.0));
    service.shutdown();
    let restored = WorkspaceService::new(home.path(), "host:one".into());
    assert_eq!(
        restored
            .authenticate(&grant.access.grant_id, &grant.token)?
            .binding,
        binding
    );
    restored.revoke_grant(&grant.access.grant_id)?;
    assert!(matches!(
        restored.grant_access(&grant.access.grant_id),
        Err(Error::Denied)
    ));
    restored.shutdown();
    let restarted = WorkspaceService::new(home.path(), "host:one".into());
    assert!(matches!(
        restarted.authenticate(&grant.access.grant_id, &grant.token),
        Err(Error::Denied)
    ));
    Ok(())
}

#[test]
fn coordination_permission_and_helper_survive_restart_without_upgrading_old_grants() -> Result<()> {
    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let chain = TempDir::new()?;
    let binding = binding(&checkout, &chain)?;
    let service = WorkspaceService::new(home.path(), "host:one".into());
    service.attach(binding.clone())?;
    let helper = std::env::current_exe()?;
    service.configure_relay(Some(RelayConfiguration {
        helper_path: helper.clone(),
        credential_program: helper.clone(),
    }))?;
    let ordinary = service.issue_grant(&binding.checkout_id, "client:one", future()?)?;
    let owner = service.issue_scoped_grant(
        &binding.checkout_id,
        "client:one",
        future()?,
        GrantScope::CoordinationOwner,
    )?;
    service.configure_relay(/*configuration*/ None)?;
    service.shutdown();
    let restored = WorkspaceService::new(home.path(), "host:one".into());
    assert_eq!(
        restored.grant_access(&ordinary.access.grant_id)?.scope,
        GrantScope::Attachment
    );
    assert_eq!(
        restored.grant_access(&owner.access.grant_id)?.scope,
        GrantScope::CoordinationOwner
    );
    assert_eq!(restored.coordination_helper()?, Some(helper));
    assert_eq!(restored.relay_configuration()?, None);
    restored.revoke_grant(&owner.access.grant_id)?;
    assert!(restored.grant_access(&owner.access.grant_id).is_err());
    assert_eq!(
        restored.grant_access(&ordinary.access.grant_id)?.scope,
        GrantScope::Attachment
    );
    Ok(())
}

#[test]
fn grants_reject_unknown_checkout_expiry_and_invalid_principal() -> Result<()> {
    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let chain = TempDir::new()?;
    let binding = binding(&checkout, &chain)?;
    let service = WorkspaceService::new(home.path(), "host:one".into());
    service.attach(binding.clone())?;
    assert!(
        service
            .issue_grant("unknown", "client:one", future()?)
            .is_err()
    );
    assert!(
        service
            .issue_grant(&binding.checkout_id, "client:one", 0)
            .is_err()
    );
    assert!(
        service
            .issue_grant(&binding.checkout_id, "client:one", u64::MAX)
            .is_err()
    );
    assert!(
        service
            .issue_grant(&binding.checkout_id, "bad\nprincipal", future()?)
            .is_err()
    );
    let grant = service.issue_grant(&binding.checkout_id, "client:one", future()?)?;
    service.shutdown();
    let path = home.path().join("idle-runtime/workspaces.json");
    let mut value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    value["grants"][0]["expiresAt"] = serde_json::json!(1);
    std::fs::write(&path, serde_json::to_vec(&value)?)?;
    let restored = WorkspaceService::new(home.path(), "host:one".into());
    assert!(matches!(
        restored.authenticate(&grant.access.grant_id, &grant.token),
        Err(Error::Expired)
    ));
    Ok(())
}

#[test]
fn local_relay_configuration_survives_restart_and_explicit_stop() -> Result<()> {
    let home = TempDir::new()?;
    let program = std::env::current_exe()?;
    let configuration = RelayConfiguration {
        helper_path: program.clone(),
        credential_program: program,
    };
    let service = WorkspaceService::new(home.path(), "host:one".into());
    service.configure_relay(Some(configuration.clone()))?;
    service.shutdown();
    let restored = WorkspaceService::new(home.path(), "host:one".into());
    assert_eq!(restored.relay_configuration()?, Some(configuration));
    restored.configure_relay(None)?;
    restored.shutdown();
    let restarted = WorkspaceService::new(home.path(), "host:one".into());
    assert_eq!(restarted.relay_configuration()?, None);
    Ok(())
}
