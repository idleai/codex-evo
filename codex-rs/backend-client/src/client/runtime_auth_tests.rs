//! The Copilot distribution must reject backend work before registering runtime auth.

use super::*;
use codex_features::RuntimeAuthMode;
use codex_http_client::OutboundProxyPolicy;
use codex_login::AuthManager;
use codex_login::GitHubCopilotAuth;
use codex_protocol::error::CodexErr;
use codex_protocol::error::CodexErrorDetails;
use codex_runtime_agent_instance::AgentHostAuthorization;
use codex_runtime_agent_instance::RuntimeAuthManager;
use codex_runtime_agent_instance::RuntimeRegistrationMetadata;
use codex_runtime_agent_instance::RuntimeRole;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use uuid::Uuid;
use wiremock::MockServer;

fn runtime_provider(
    server: &MockServer,
    credentials: Arc<AuthManager>,
    mode: RuntimeAuthMode,
) -> (Arc<RuntimeAuthManager>, RuntimeAuthMode) {
    (
        Arc::new(RuntimeAuthManager::new(
            credentials,
            AgentHostAuthorization::InstallationId {
                installation_id: Uuid::nil(),
            },
            RuntimeRegistrationMetadata {
                harness: Some("codex".into()),
                harness_version: None,
                role: RuntimeRole::Orchestrator,
            },
            format!("{}/register", server.uri()).parse().unwrap(),
        )),
        mode,
    )
}

fn credentials() -> [CodexAuth; 2] {
    [
        CodexAuth::from_api_key("key-a"),
        CodexAuth::from_github_copilot(
            GitHubCopilotAuth::new(
                "github-token".to_string(),
                "https://api.individual.githubcopilot.com".to_string(),
                None,
                None,
                vec!["gpt-6.1-sol".to_string()],
            )
            .unwrap(),
        ),
    ]
}

#[tokio::test]
async fn disabled_backend_rejects_runtime_auth_before_registration() {
    for mode in [RuntimeAuthMode::DualHeader, RuntimeAuthMode::AssertionOnly] {
        for auth in credentials() {
            let server = MockServer::start().await;
            let runtime = runtime_provider(
                &server,
                AuthManager::from_auth_for_testing(auth.clone()),
                mode,
            );
            let headers = HeaderMap::from_iter([(
                HeaderName::from_static("openai-organization"),
                HeaderValue::from_static("org-a"),
            )]);
            let factory = HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault);
            let results = [
                Client::from_auth_with_selected_runtime(
                    server.uri(),
                    &auth,
                    factory.clone(),
                    Some(&runtime),
                    &headers,
                    Client::new,
                )
                .await,
                Client::from_auth_with_runtime(
                    server.uri(),
                    &auth,
                    factory.clone(),
                    Some(&runtime),
                    &headers,
                )
                .await,
                Client::from_auth_with_runtime_without_redirects(
                    server.uri(),
                    &auth,
                    factory,
                    Some(&runtime),
                    &headers,
                )
                .await,
            ];
            for result in results {
                let error = result.unwrap_err();
                assert_eq!(
                    error.to_string(),
                    codex_http_client::OPENAI_DISABLED_MESSAGE
                );
            }
            assert!(server.received_requests().await.unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn disabled_backend_rejects_credentialed_requests_before_network() {
    for auth in credentials() {
        let server = MockServer::start().await;
        let client = Client::from_auth(
            server.uri(),
            &auth,
            HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
        );
        let error = client.get_accounts_check().await.unwrap_err();
        assert_eq!(
            error.to_string(),
            codex_http_client::OPENAI_DISABLED_MESSAGE
        );
        let error = client.get_rate_limits().await.unwrap_err();
        assert_eq!(
            error.to_string(),
            codex_http_client::OPENAI_DISABLED_MESSAGE
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn unsupported_openai_backend_rejects_runtime_auth_before_registration() {
    for mode in [RuntimeAuthMode::DualHeader, RuntimeAuthMode::AssertionOnly] {
        let server = MockServer::start().await;
        let auth = CodexAuth::from_api_key("key-a");
        let runtime = runtime_provider(
            &server,
            AuthManager::from_auth_for_testing(auth.clone()),
            mode,
        );
        let result = Client::from_auth_with_runtime(
            "https://future.openai.com/backend-api",
            &auth,
            HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
            Some(&runtime),
            &HeaderMap::new(),
        )
        .await;
        let Err(error) = result else {
            panic!("unsupported OpenAI backend accepted runtime auth");
        };
        assert!(matches!(
            error.downcast_ref::<CodexErr>().map(CodexErr::details),
            Some(CodexErrorDetails::UnsupportedOperation(_))
        ));
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
