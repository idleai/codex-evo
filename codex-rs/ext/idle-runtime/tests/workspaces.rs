use anyhow::Result;
use codex_idle_runtime::Error;
use codex_idle_runtime::WorkspaceBinding;
use codex_idle_runtime::WorkspaceService;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

fn binding(root: &TempDir, name: &str) -> Result<WorkspaceBinding> {
    let checkout_root = root.path().join(name);
    let chain_directory = root.path().join(format!("{name}-chain"));
    std::fs::create_dir_all(&checkout_root)?;
    std::fs::create_dir_all(&chain_directory)?;
    Ok(WorkspaceBinding {
        workspace_id: "workspace:one".into(),
        repository_id: "repository:one".into(),
        checkout_id: format!("checkout:{name}"),
        chain_id: "chain:one".into(),
        checkout_root: checkout_root.canonicalize()?,
        chain_directory: chain_directory.canonicalize()?,
    })
}

#[test]
fn an_unused_service_does_not_create_idle_state() -> Result<()> {
    let home = TempDir::new()?;
    let service = WorkspaceService::new(home.path(), "host:one".into());
    assert_eq!(service.status()?.workspaces, vec![]);
    assert!(!home.path().join("idle-runtime").exists());
    Ok(())
}

#[test]
fn restart_preserves_bindings_and_host_but_changes_runtime() -> Result<()> {
    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let binding = binding(&checkout, "first")?;
    let service = WorkspaceService::new(home.path(), "host:one".into());
    let first = service.attach(binding.clone())?;
    assert_eq!(service.attach(binding.clone())?, first);
    service.shutdown();
    assert!(matches!(service.status(), Err(Error::Stopped)));
    assert!(matches!(
        service.attach(binding.clone()),
        Err(Error::Stopped)
    ));
    let replacement = WorkspaceService::new(home.path(), "host:one".into());
    let restored = replacement.status()?;
    assert_eq!(restored.host_id, first.host_id);
    assert_ne!(restored.runtime_id, first.runtime_id);
    assert_eq!(restored.workspaces, first.workspaces);
    assert_eq!(restored.workspaces[0].binding, binding);
    assert!(restored.workspaces[0].available);
    Ok(())
}

#[test]
fn a_second_process_owner_cannot_take_the_registry() -> Result<()> {
    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let service = WorkspaceService::new(home.path(), "host:one".into());
    service.attach(binding(&checkout, "first")?)?;
    let contender = WorkspaceService::new(home.path(), "host:one".into());
    assert!(matches!(contender.status(), Err(Error::Busy)));
    assert_eq!(service.status()?.workspaces.len(), 1);
    service.shutdown();
    let replacement = WorkspaceService::new(home.path(), "host:one".into());
    assert_eq!(replacement.status()?.workspaces.len(), 1);
    Ok(())
}

#[test]
fn copied_checkouts_and_rebound_host_ids_require_explicit_resolution() -> Result<()> {
    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let original = binding(&checkout, "first")?;
    let service = WorkspaceService::new(home.path(), "host:one".into());
    service.attach(original.clone())?;
    let mut copied = binding(&checkout, "copied")?;
    copied.checkout_id = original.checkout_id.clone();
    assert!(matches!(service.attach(copied), Err(Error::Conflict)));
    let mut aliased = original;
    aliased.checkout_id = "checkout:another".into();
    assert!(matches!(service.attach(aliased), Err(Error::Conflict)));
    service.shutdown();
    let wrong_host = WorkspaceService::new(home.path(), "host:other".into());
    assert!(matches!(wrong_host.status(), Err(Error::Conflict)));
    Ok(())
}

#[test]
fn unavailable_saved_paths_are_reported_without_discarding_the_binding() -> Result<()> {
    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let binding = binding(&checkout, "first")?;
    let service = WorkspaceService::new(home.path(), "host:one".into());
    service.attach(binding.clone())?;
    std::fs::remove_dir(&binding.checkout_root)?;
    let status = service.status()?;
    assert_eq!(status.workspaces[0].binding, binding);
    assert!(!status.workspaces[0].available);
    service.shutdown();
    let restored = WorkspaceService::new(home.path(), "host:one".into());
    assert!(!restored.status()?.workspaces[0].available);
    Ok(())
}

#[test]
fn invalid_ids_and_relative_paths_do_not_create_an_attachment() -> Result<()> {
    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let service = WorkspaceService::new(home.path(), "host:one".into());
    let mut invalid = binding(&checkout, "first")?;
    invalid.workspace_id.clear();
    assert!(matches!(
        service.attach(invalid),
        Err(Error::InvalidBinding)
    ));
    let mut invalid = binding(&checkout, "second")?;
    invalid.checkout_root = "relative".into();
    assert!(matches!(
        service.attach(invalid),
        Err(Error::InvalidBinding)
    ));
    assert!(!home.path().join("idle-runtime").exists());
    Ok(())
}

#[test]
fn corrupt_and_future_state_are_never_silently_replaced() -> Result<()> {
    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let binding = binding(&checkout, "first")?;
    let service = WorkspaceService::new(home.path(), "host:one".into());
    service.attach(binding.clone())?;
    service.shutdown();
    let path = home.path().join("idle-runtime/workspaces.json");
    for bytes in [
        b"not json".as_slice(),
        br#"{"version":99,"hostId":"host:one","bindings":[]}"#,
    ] {
        std::fs::write(&path, bytes)?;
        let failed = WorkspaceService::new(home.path(), "host:one".into());
        assert!(failed.attach(binding.clone()).is_err());
        assert_eq!(std::fs::read(&path)?, bytes);
    }
    Ok(())
}

#[test]
fn a_dormant_server_observes_a_later_registry_owner() -> Result<()> {
    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let dormant = WorkspaceService::new(home.path(), "host:one".into());
    let owner = WorkspaceService::new(home.path(), "host:one".into());
    owner.attach(binding(&checkout, "first")?)?;
    assert!(matches!(dormant.status(), Err(Error::Busy)));
    owner.shutdown();
    assert_eq!(dormant.status()?.workspaces.len(), 1);
    Ok(())
}

#[cfg(unix)]
#[test]
fn replaced_directory_symlinks_are_unavailable() -> Result<()> {
    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let binding = binding(&checkout, "first")?;
    let service = WorkspaceService::new(home.path(), "host:one".into());
    service.attach(binding.clone())?;
    let replacement = checkout.path().join("replacement");
    std::fs::rename(&binding.checkout_root, &replacement)?;
    std::os::unix::fs::symlink(&replacement, &binding.checkout_root)?;
    assert!(!service.status()?.workspaces[0].available);
    Ok(())
}

#[cfg(unix)]
#[test]
fn registry_files_are_private_and_public_files_are_rejected() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let home = TempDir::new()?;
    let checkout = TempDir::new()?;
    let service = WorkspaceService::new(home.path(), "host:one".into());
    service.attach(binding(&checkout, "first")?)?;
    service.shutdown();
    let path = home.path().join("idle-runtime/workspaces.json");
    assert_eq!(path.metadata()?.permissions().mode() & 0o077, 0);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;
    assert!(matches!(
        WorkspaceService::new(home.path(), "host:one".into()).status(),
        Err(Error::Unavailable)
    ));
    Ok(())
}
