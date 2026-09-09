use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_login::GitHubCopilotAuth;
use codex_model_provider_info::ModelProviderInfo;

use super::Config;
use super::ImageGenerationExtensionConfig;
use super::install;

#[test]
fn installed_extension_hides_stale_image_tool_after_github_copilot_login() {
    let auth = GitHubCopilotAuth::new(
        "github-token".to_string(),
        "https://api.individual.githubcopilot.com".to_string(),
        None,
        None,
        vec!["gpt-5.6-sol".to_string()],
    )
    .expect("valid GitHub Copilot auth");
    let mut builder = ExtensionRegistryBuilder::<Config>::new();
    install(
        &mut builder,
        AuthManager::from_auth_for_testing(CodexAuth::from_github_copilot(auth)),
        |_| None,
    );
    let registry = builder.build();
    let session_store = ExtensionData::new("session");
    let thread_store = ExtensionData::new("11111111-1111-4111-8111-111111111111");
    thread_store.insert(ImageGenerationExtensionConfig {
        available: true,
        provider: ModelProviderInfo::create_openai_provider(/*base_url*/ None),
        save_root: None,
    });

    let tools = registry
        .tool_contributors()
        .iter()
        .flat_map(|contributor| contributor.tools(&session_store, &thread_store))
        .collect::<Vec<_>>();

    assert!(tools.is_empty());
}
