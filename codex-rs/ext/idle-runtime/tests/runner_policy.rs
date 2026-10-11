use anyhow::Result;
use codex_idle_runtime::RunnerPolicy;
use codex_idle_runtime::WorkspaceBinding;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

fn binding(checkout: &TempDir, chain: &TempDir) -> Result<WorkspaceBinding> {
    Ok(WorkspaceBinding {
        workspace_id: "workspace".into(),
        repository_id: "repository".into(),
        checkout_id: "checkout".into(),
        chain_id: "chain".into(),
        checkout_root: checkout.path().canonicalize()?,
        chain_directory: chain.path().canonicalize()?,
    })
}

#[test]
fn authored_rules_intersect_private_roots_and_cannot_restore_writes() -> Result<()> {
    let checkout = TempDir::new()?;
    let chain = TempDir::new()?;
    let binding = binding(&checkout, &chain)?;
    let source = binding.checkout_root.join("src");
    let build = binding.checkout_root.join("build");
    std::fs::create_dir(&source)?;
    std::fs::create_dir(&build)?;
    let grant = RunnerPolicy::new(&binding, vec![binding.checkout_root.clone()], vec![build])?;
    let narrow = grant.restrict(
        &binding,
        Some(
            &json!({
                "schema_version": 1, "runners": {"version": 1, "readRoots": ["src"]}
            })
            .to_string(),
        ),
    )?;
    assert_eq!(
        narrow,
        RunnerPolicy {
            read_roots: vec![source],
            write_roots: vec![]
        }
    );
    let broad =
        json!({"schema_version":1,"runners":{"version":1,"readRoots":["."],"writeRoots":["."]}})
            .to_string();
    assert_eq!(grant.restrict(&binding, Some(&broad))?, grant);
    assert_eq!(narrow.restrict(&binding, Some(&broad))?, narrow);
    assert_eq!(grant.intersect(&narrow), narrow);
    let readonly = grant.restrict(
        &binding,
        Some(
            &json!({
                "schema_version": 1, "runners": {"version": 1, "writeRoots": []}
            })
            .to_string(),
        ),
    )?;
    assert_eq!(readonly.write_roots, Vec::<std::path::PathBuf>::new());
    Ok(())
}

#[test]
fn invalid_or_unsupported_rules_deny_admission() -> Result<()> {
    let checkout = TempDir::new()?;
    let chain = TempDir::new()?;
    let binding = binding(&checkout, &chain)?;
    let grant = RunnerPolicy::new(&binding, vec![binding.checkout_root.clone()], vec![])?;
    for runners in [
        json!({"version":2}),
        json!({"version":1,"enabled":false}),
        json!({"version":1,"network":"enabled"}),
        json!({"version":1,"readRoots":[chain.path()]}),
        json!({"version":1,"writeRoots":["../"]}),
        json!({"version":1,"readRoots":["missing"]}),
        json!({"version":1,"readRoots":vec![".";33]}),
    ] {
        assert!(
            grant
                .restrict(
                    &binding,
                    Some(
                        &json!({
                            "schema_version":1,"runners":runners
                        })
                        .to_string()
                    )
                )
                .is_err()
        );
    }
    assert!(grant.restrict(&binding, Some("{")).is_err());
    assert!(grant.restrict(&binding, Some("{}")).is_err());
    assert_eq!(grant.restrict(&binding, None)?, grant);
    assert_eq!(
        grant.restrict(&binding, Some("{\"schema_version\":1,\"other\":{}}"))?,
        grant
    );
    assert!(RunnerPolicy::new(&binding, vec![chain.path().into()], vec![]).is_err());
    assert!(RunnerPolicy::new(&binding, vec![], vec![binding.checkout_root.clone()]).is_err());
    Ok(())
}

#[cfg(unix)]
#[test]
fn replaced_directories_and_symlink_escapes_are_rejected() -> Result<()> {
    let checkout = TempDir::new()?;
    let chain = TempDir::new()?;
    let binding = binding(&checkout, &chain)?;
    let root = binding.checkout_root.join("allowed");
    std::fs::create_dir(&root)?;
    let grant = RunnerPolicy::new(&binding, vec![root.clone()], vec![root.clone()])?;
    std::fs::remove_dir(&root)?;
    std::os::unix::fs::symlink(chain.path(), &root)?;
    assert!(grant.validate(&binding).is_err());
    assert!(RunnerPolicy::new(&binding, vec![root.clone()], vec![]).is_err());
    std::fs::remove_file(&root)?;
    std::fs::create_dir(&root)?;
    grant.validate(&binding)?;
    Ok(())
}
