use codex_http_client::ClientRouteClass;
use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyPolicy;
use codex_login::AuthCredentialsStoreMode;
use codex_login::AuthKeyringBackendKind;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_login::GitHubCopilotAuth;
use codex_login::default_client::create_client_for_route;
use codex_model_provider_info::ModelProviderInfo;
use codex_models_manager::ModelsManagerConfig;
use codex_models_manager::manager::RefreshStrategy;
use codex_models_manager::model_info::model_info_from_slug;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelsResponse;
use codex_protocol::openai_models::ReasoningEffort;
use pretty_assertions::assert_eq;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::any;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

use super::model_catalog;
use crate::WorkspaceRoutingContext;
use crate::create_model_provider;

#[tokio::test]
async fn github_copilot_wrapper_preserves_exact_provider_catalog_metadata() {
    let server = MockServer::start().await;
    let provider = create_model_provider(
        ModelProviderInfo {
            model_catalog_url: Some(format!("{}/models", server.uri()).into()),
            ..ModelProviderInfo::create_openai_provider(Some(server.uri()))
        },
        Some(AuthManager::from_auth_for_testing(CodexAuth::from_api_key(
            "api-key",
        ))),
    );
    let manager = provider.models_manager_without_cache(None);
    manager.set_api_key_model_discovery_enabled(true);
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ModelsResponse {
            models: vec![ModelInfo {
                display_name: "Provider-owned model".to_string(),
                ..model_info_from_slug("catalog-model")
            }],
        }))
        .expect(1)
        .mount(&server)
        .await;
    manager
        .raw_model_catalog(
            RefreshStrategy::Online,
            HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        )
        .await;

    for unknown in ["catalog-model-other", "namespace/catalog-model"] {
        assert_eq!(
            manager
                .get_model_info(unknown, &ModelsManagerConfig::default())
                .await,
            model_info_from_slug(unknown)
        );
    }
}

#[tokio::test]
async fn github_copilot_wrapper_refreshes_provider_catalog_after_auth_change() {
    let server = MockServer::start().await;
    let home = tempfile::tempdir().expect("temporary Codex home");
    let auth_manager = AuthManager::from_auth_for_testing_with_home(
        CodexAuth::from_api_key("first-token"),
        home.path().to_path_buf(),
    );
    let provider = create_model_provider(
        ModelProviderInfo {
            model_catalog_url: Some(format!("{}/models", server.uri()).into()),
            ..ModelProviderInfo::create_openai_provider(Some(server.uri()))
        },
        Some(Arc::clone(&auth_manager)),
    );
    let manager = provider.models_manager_without_cache(None);
    manager.set_api_key_model_discovery_enabled(true);
    let first = ModelInfo {
        used_fallback_model_metadata: false,
        ..model_info_from_slug("first-catalog-model")
    };
    let second = ModelInfo {
        used_fallback_model_metadata: false,
        ..model_info_from_slug("second-catalog-model")
    };
    for (token, model) in [("first-token", &first), ("second-token", &second)] {
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(header("authorization", format!("Bearer {token}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(ModelsResponse {
                models: vec![model.clone()],
            }))
            .expect(1)
            .mount(&server)
            .await;
    }
    assert_eq!(
        manager
            .raw_model_catalog(
                RefreshStrategy::Online,
                HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
            )
            .await,
        ModelsResponse {
            models: vec![first]
        }
    );
    codex_login::login_with_api_key(
        home.path(),
        "second-token",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )
    .expect("persist updated API key");
    auth_manager.reload().await;
    assert_eq!(
        auth_manager
            .auth_cached()
            .as_ref()
            .and_then(CodexAuth::api_key),
        Some("second-token")
    );
    manager
        .refresh_after_auth_change(HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault))
        .await;
    assert_eq!(manager.get_remote_models().await, vec![second]);
}

#[tokio::test]
async fn github_copilot_responses_transport_rejects_cross_origin_redirects() {
    let redirect_target = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&redirect_target)
        .await;
    let auth = GitHubCopilotAuth::new(
        "github-token".to_string(),
        "https://api.individual.githubcopilot.com".to_string(),
        Some("octocat".to_string()),
        Some("copilot_individual".to_string()),
        vec!["gpt-5.6-sol".to_string()],
    )
    .expect("valid Copilot auth");
    let provider = create_model_provider(
        ModelProviderInfo::create_openai_provider(Some(redirect_target.uri())),
        Some(AuthManager::from_auth_for_testing(
            CodexAuth::from_github_copilot(auth),
        )),
    );
    let routing_context = WorkspaceRoutingContext::new(redirect_target.uri());
    let resolved = provider
        .responses_api_provider(&routing_context)
        .await
        .expect("resolve Copilot Responses provider");
    assert_eq!(
        resolved.provider.base_url,
        "https://api.individual.githubcopilot.com"
    );

    for status in [307, 308] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(
                ResponseTemplate::new(status).insert_header("location", redirect_target.uri()),
            )
            .expect(1)
            .mount(&server)
            .await;
        let request_url = format!("{}/responses", server.uri());
        let client = create_client_for_route(
            &HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
            &request_url,
            ClientRouteClass::Api,
            resolved.redirect_policy,
        )
        .expect("build Responses transport");
        let response = client
            .post(&request_url)
            .body("private Copilot request")
            .send()
            .await
            .expect("receive redirect response");
        assert_eq!(response.status().as_u16(), status);
    }
}

#[test]
fn catalog_uses_copilot_reasoning_efforts_for_unknown_models() {
    let auth = serde_json::from_value::<GitHubCopilotAuth>(json!({
        "access_token": "github-token",
        "api_endpoint": "https://api.individual.githubcopilot.com",
        "login": "octocat",
        "copilot_sku": "copilot_individual",
        "models": ["unknown-copilot-test-model"],
        "model_reasoning_efforts": {
            "unknown-copilot-test-model": ["low", "medium", "high", "xhigh", "max"]
        }
    }))
    .expect("GitHub Copilot auth fixture should deserialize");

    let catalog = model_catalog(&auth);
    let model = catalog.models.first().expect("Astra should be available");

    assert_eq!(model.slug, "unknown-copilot-test-model");
    assert_eq!(model.default_reasoning_level, Some(ReasoningEffort::Medium));
    assert_eq!(
        model
            .supported_reasoning_levels
            .iter()
            .map(|preset| preset.effort.clone())
            .collect::<Vec<_>>(),
        vec![
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::XHigh,
            ReasoningEffort::Max,
        ]
    );
}

#[test]
fn catalog_preserves_local_ultra_reasoning_for_bundled_models() {
    let auth = serde_json::from_value::<GitHubCopilotAuth>(json!({
        "access_token": "github-token",
        "api_endpoint": "https://api.individual.githubcopilot.com",
        "login": "octocat",
        "copilot_sku": "copilot_individual",
        "models": ["gpt-5.6-sol"],
        "model_reasoning_efforts": {
            "gpt-5.6-sol": ["none", "low", "medium", "high", "xhigh", "max"]
        }
    }))
    .expect("GitHub Copilot auth fixture should deserialize");

    let catalog = model_catalog(&auth);
    let model = catalog.models.first().expect("Sol should be available");

    assert_eq!(model.default_reasoning_level, Some(ReasoningEffort::Low));
    assert_eq!(
        model
            .supported_reasoning_levels
            .iter()
            .map(|preset| preset.effort.clone())
            .collect::<Vec<_>>(),
        vec![
            ReasoningEffort::None,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::XHigh,
            ReasoningEffort::Max,
            ReasoningEffort::Ultra,
        ]
    );
}
use std::sync::Arc;
