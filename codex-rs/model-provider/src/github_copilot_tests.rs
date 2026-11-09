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
use codex_model_provider_info::WireApi;
use codex_model_provider_info::create_oss_provider_with_base_url;
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
async fn copilot_auth_preserves_explicit_local_routes_and_catalogs_without_credentials() {
    let home = tempfile::tempdir().expect("temporary Codex home");
    let auth_manager = AuthManager::from_auth_for_testing_with_home(
        CodexAuth::from_api_key("unrelated-openai-key"),
        home.path().to_path_buf(),
    );
    let local_catalog = ModelsResponse {
        models: vec![ModelInfo {
            display_name: "DeepSeek V4.1 Flash (local)".to_string(),
            ..model_info_from_slug("deepseek-v41-flash")
        }],
    };
    let copilot_auth = GitHubCopilotAuth::new(
        "github-token".to_string(),
        "https://api.individual.githubcopilot.com".to_string(),
        Some("octocat".to_string()),
        None,
        vec!["gpt-5.6-sol".to_string(), "new-openai-model".to_string()],
    )
    .expect("valid Copilot auth");

    for endpoint in [
        "http://127.0.0.1:8000/v1",
        "http://localhost:8000/v1",
        "http://[::1]:8000/v1",
    ] {
        let provider = create_model_provider(
            create_oss_provider_with_base_url(endpoint, WireApi::Responses),
            Some(Arc::clone(&auth_manager)),
        );
        let catalog = provider.models_manager_without_cache(Some(local_catalog.clone()));
        codex_login::login_with_github_copilot(
            home.path(),
            copilot_auth.clone(),
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .expect("persist Copilot auth");
        auth_manager.reload().await;

        assert_eq!(provider.api_provider().await.unwrap().base_url, endpoint);
        assert!(provider.auth().await.is_none());
        assert!(provider.auth_manager().is_none());
        assert!(
            provider
                .api_auth()
                .await
                .unwrap()
                .to_auth_headers()
                .unwrap()
                .is_empty()
        );
        provider.validate_model("deepseek-v41-flash").unwrap();
        assert_eq!(
            catalog
                .raw_model_catalog(
                    RefreshStrategy::Online,
                    HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
                )
                .await,
            local_catalog
        );
    }

    let copilot = create_model_provider(
        ModelProviderInfo::create_openai_provider(None),
        Some(auth_manager),
    );
    assert_eq!(
        copilot
            .models_manager_without_cache(Some(local_catalog))
            .raw_model_catalog(
                RefreshStrategy::Online,
                HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
            )
            .await
            .models
            .into_iter()
            .map(|model| model.slug)
            .collect::<Vec<_>>(),
        copilot_auth.models()
    );
}

#[tokio::test]
async fn copilot_auth_still_overrides_remote_and_account_authenticated_routes() {
    let auth = GitHubCopilotAuth::new(
        "github-token".to_string(),
        "https://api.individual.githubcopilot.com".to_string(),
        None,
        None,
        vec!["gpt-5.6-sol".to_string()],
    )
    .expect("valid Copilot auth");
    let local = create_oss_provider_with_base_url("http://127.0.0.1:8000/v1", WireApi::Responses);
    let mut providers = vec![
        ModelProviderInfo::create_openai_provider(local.base_url.clone()),
        ModelProviderInfo {
            model_catalog_url: Some("https://api.openai.com/v1/models".into()),
            ..local.clone()
        },
        ModelProviderInfo {
            experimental_bearer_token: Some("provider-token".into()),
            ..local
        },
    ];
    providers.extend(
        [
            "https://example.test/v1",
            "https://localhost.example.test/v1",
            "http://localhost@remote.example.test/v1",
            "http://user@localhost:8000/v1",
        ]
        .into_iter()
        .map(|endpoint| create_oss_provider_with_base_url(endpoint, WireApi::Responses)),
    );
    for info in providers {
        let provider = create_model_provider(
            info,
            Some(AuthManager::from_auth_for_testing(
                CodexAuth::from_github_copilot(auth.clone()),
            )),
        );
        assert!(provider.info().is_github_copilot());
        assert_eq!(
            provider.api_provider().await.unwrap().base_url,
            auth.api_endpoint()
        );
    }
}

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
        let resolved = manager
            .get_model_info(unknown, &ModelsManagerConfig::default())
            .await;
        assert_eq!(resolved.model.as_ref(), &model_info_from_slug(unknown));
        assert_eq!(
            resolved.origin,
            codex_models_manager::ModelOrigin::Synthetic
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
    let first = model_info_from_slug("first-catalog-model");
    let second = model_info_from_slug("second-catalog-model");
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
    assert_eq!(
        resolved.redirect_policy,
        codex_login::default_client::ClientRedirectPolicy::Reject
    );
    let auth = provider.auth().await.expect("Copilot credentials");
    assert!(
        provider
            .api_provider_for_auth(Some(&CodexAuth::from_api_key("other-account")))
            .is_err()
    );
    let frozen_provider = provider
        .api_provider_for_auth(Some(&auth))
        .expect("frozen Copilot provider");
    let resolved = provider
        .route_responses_api_provider(frozen_provider, Some(&auth), &routing_context)
        .await
        .expect("route frozen Copilot credentials");
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

#[tokio::test]
async fn existing_gcp_provider_rejects_requests_after_copilot_login() {
    let home = tempfile::tempdir().expect("temporary Codex home");
    let auth_manager = AuthManager::from_auth_for_testing_with_home(
        CodexAuth::from_api_key("initial-key"),
        home.path().to_path_buf(),
    );
    let provider = create_model_provider(
        ModelProviderInfo {
            base_url: Some("https://vertex.example.test/v1".to_string()),
            ..ModelProviderInfo::create_gcp_provider()
        },
        Some(Arc::clone(&auth_manager)),
    );
    let catalog = provider.models_manager_without_cache(None);
    provider.api_provider().await.expect("initial provider");
    let auth = GitHubCopilotAuth::new(
        "github-token".to_string(),
        "https://api.individual.githubcopilot.com".to_string(),
        Some("octocat".to_string()),
        None,
        vec!["copilot-account-model".to_string()],
    )
    .expect("valid Copilot auth");
    codex_login::login_with_github_copilot(
        home.path(),
        auth,
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )
    .expect("persist Copilot auth");
    assert!(auth_manager.reload().await);
    assert!(provider.api_provider().await.is_err());
    assert!(provider.api_auth().await.is_err());
    assert!(provider.validate_model("gpt-6-astra").is_err());
    assert!(!provider.capabilities().namespace_tools);
    let models = catalog
        .raw_model_catalog(
            RefreshStrategy::Online,
            HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        )
        .await;
    assert_eq!(
        models
            .models
            .iter()
            .map(|model| model.slug.as_str())
            .collect::<Vec<_>>(),
        vec!["copilot-account-model"]
    );
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
