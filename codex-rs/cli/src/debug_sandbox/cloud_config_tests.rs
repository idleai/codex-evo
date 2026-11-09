use anyhow::Result;
use app_test_support::ChatGptAuthFixture;
use app_test_support::write_chatgpt_auth;
use codex_config::LoaderOverrides;
use codex_config::types::AuthCredentialsStoreMode;
use codex_utils_absolute_path::AbsolutePathBuf;
use tempfile::TempDir;
use wiremock::MockServer;

use super::super::DebugSandboxConfigOptions;
use super::super::ManagedRequirementsMode;
use super::bootstrap_cloud_config_bundle;

#[tokio::test]
async fn copilot_distribution_sandbox_does_not_fetch_openai_managed_profiles() -> Result<()> {
    let server = MockServer::start().await;
    let codex_home = TempDir::new()?;
    std::fs::write(
        codex_home.path().join("config.toml"),
        format!(
            "cli_auth_credentials_store = \"file\"\nchatgpt_base_url = \"{}/backend-api\"\n",
            server.uri(),
        ),
    )?;
    write_chatgpt_auth(
        codex_home.path(),
        ChatGptAuthFixture::new("chatgpt-token")
            .account_id("workspace-123")
            .chatgpt_account_id("workspace-123")
            .chatgpt_user_id("user-123")
            .plan_type("enterprise"),
        AuthCredentialsStoreMode::File,
    )?;
    let before = std::fs::read(codex_home.path().join("auth.json"))?;
    for restrict_network in [false, true] {
        let mut options = DebugSandboxConfigOptions {
            sandbox_state: Default::default(),
            permissions_profile: None,
            cwd: Some(codex_home.path().to_path_buf()),
            managed_requirements_mode: ManagedRequirementsMode::Include,
            loader_overrides: LoaderOverrides::without_managed_config_for_tests(),
        };
        if restrict_network {
            let requirements = codex_home.path().join("requirements.toml");
            std::fs::write(&requirements, "[application.network]\nenabled = true\n")?;
            options.loader_overrides.system_requirements_path = Some(requirements);
        }
        let bundle = bootstrap_cloud_config_bundle(
            &[],
            &options,
            || AbsolutePathBuf::from_absolute_path(codex_home.path()),
            false,
        )
        .await?;
        assert!(bundle.get().await?.is_none());
    }
    assert!(server.received_requests().await.unwrap().is_empty());
    assert_eq!(std::fs::read(codex_home.path().join("auth.json"))?, before);
    assert!(
        !codex_home
            .path()
            .join("cloud-config-bundle-cache.json")
            .exists()
    );
    Ok(())
}

#[tokio::test]
async fn sandbox_state_skips_cloud_bootstrap() -> Result<()> {
    let options = DebugSandboxConfigOptions {
        sandbox_state: crate::SandboxStateArgs {
            sandbox_state_json: Some("{}".into()),
            ..Default::default()
        },
        permissions_profile: None,
        cwd: None,
        managed_requirements_mode: ManagedRequirementsMode::Include,
        loader_overrides: LoaderOverrides::without_managed_config_for_tests(),
    };
    bootstrap_cloud_config_bundle(
        &[],
        &options,
        || panic!("supplied state must skip CODEX_HOME resolution"),
        /*strict_config*/ false,
    )
    .await?;
    Ok(())
}
