use std::time::Duration;

use pretty_assertions::assert_eq;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::body_string_contains;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

use super::*;

#[tokio::test]
async fn device_flow_discovers_direct_responses_models() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/device"))
        .and(body_string_contains("client_id=codex-test-client"))
        .and(body_string_contains("scope=read%3Auser"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code": "device-secret",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://github.com/login/device",
            "expires_in": 900,
            "interval": 1
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("device_code=device-secret"))
        .and(body_string_contains(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "github-oauth-token",
            "token_type": "bearer",
            "scope": "read:user"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/entitlement"))
        .and(header("authorization", "Bearer github-oauth-token"))
        .and(header("x-github-api-version", GITHUB_USER_API_VERSION))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "login": "octocat",
            "access_type_sku": "copilot_enterprise",
            "endpoints": {"api": "https://api.githubcopilot.com"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", "Bearer github-oauth-token"))
        .and(header("openai-intent", "conversation"))
        .and(header("x-github-api-version", GITHUB_COPILOT_API_VERSION))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                {
                    "id": "gpt-5.1-codex",
                    "vendor": "OpenAI",
                    "supported_endpoints": ["/responses"]
                },
                {
                    "id": "claude-sonnet",
                    "vendor": "Anthropic",
                    "supported_endpoints": ["/responses"]
                },
                {
                    "id": "gpt-chat-only",
                    "vendor": "OpenAI",
                    "supported_endpoints": ["/chat/completions"]
                },
                {
                    "id": "gpt-disabled",
                    "vendor": "OpenAI",
                    "supported_endpoints": ["/responses"],
                    "policy": {"state": "disabled"}
                },
                {
                    "id": "gpt-hidden",
                    "vendor": "OpenAI",
                    "model_picker_enabled": false,
                    "supported_endpoints": ["/responses"]
                },
                {
                    "id": "gpt-5-mini",
                    "vendor": "openai",
                    "is_chat_default": true,
                    "capabilities": {
                        "supported_endpoints": ["/v1/responses"],
                        "supports": {"reasoning_effort": ["low", "medium", "high"]}
                    }
                },
                {
                    "id": "gpt-6-astra",
                    "vendor": "OpenAI",
                    "supported_endpoints": ["/responses"],
                    "capabilities": {
                        "supports": {
                            "reasoning_effort": ["low", "medium", "high", "xhigh", "max"]
                        }
                    }
                }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let route = crate::test_support::transport_default_auth_route_config();
    let options = GitHubCopilotLoginOptions::new("codex-test-client".to_string(), route)
        .expect("valid options")
        .with_test_endpoints(
            format!("{}/device", server.uri()),
            format!("{}/token", server.uri()),
            format!("{}/entitlement", server.uri()),
            format!("{}/models", server.uri()),
        );
    let mut device_code = request_github_copilot_device_code(&options)
        .await
        .expect("device code should be returned");
    assert_eq!(
        (
            device_code.verification_url.as_str(),
            device_code.user_code.as_str(),
            device_code.expires_in,
        ),
        (
            "https://github.com/login/device",
            "ABCD-EFGH",
            Duration::from_secs(900),
        )
    );
    device_code.interval = Duration::ZERO;

    let auth = complete_github_copilot_device_code_login(&options, device_code)
        .await
        .expect("Copilot auth should be validated");

    assert_eq!(auth.access_token(), "github-oauth-token");
    assert_eq!(auth.api_endpoint(), "https://api.githubcopilot.com");
    assert_eq!(auth.login(), Some("octocat"));
    assert_eq!(auth.copilot_sku(), Some("copilot_enterprise"));
    assert_eq!(
        auth.models(),
        [
            "gpt-5-mini".to_string(),
            "gpt-5.1-codex".to_string(),
            "gpt-6-astra".to_string(),
        ]
    );
    assert_eq!(
        auth.reasoning_efforts_for_model("gpt-5-mini"),
        [
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
        ]
    );
    assert_eq!(
        auth.reasoning_efforts_for_model("gpt-6-astra"),
        [
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::XHigh,
            ReasoningEffort::Max,
        ]
    );
}

#[test]
fn legacy_github_auth_without_reasoning_metadata_still_deserializes() {
    let auth = serde_json::from_value::<GitHubCopilotAuth>(json!({
        "access_token": "github-token",
        "api_endpoint": "https://api.individual.githubcopilot.com",
        "login": "octocat",
        "copilot_sku": "copilot_individual",
        "models": ["gpt-6-astra"]
    }))
    .expect("legacy GitHub Copilot auth should deserialize");

    assert_eq!(auth.models(), ["gpt-6-astra".to_string()]);
    assert!(auth.reasoning_efforts_for_model("gpt-6-astra").is_empty());
}

#[test]
fn endpoint_validation_rejects_non_copilot_hosts() {
    let error = GitHubCopilotAuth::new(
        "token".to_string(),
        "https://attacker.example/api".to_string(),
        None,
        None,
        vec!["gpt-5".to_string()],
    )
    .expect_err("non-Copilot host should be rejected");

    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
}

#[test]
fn github_auth_debug_output_redacts_access_token() {
    let auth = GitHubCopilotAuth::new(
        "github-secret-token".to_string(),
        "https://api.individual.githubcopilot.com".to_string(),
        Some("octocat".to_string()),
        None,
        vec!["gpt-5".to_string()],
    )
    .expect("valid auth");

    let debug = format!("{auth:?}");

    assert!(!debug.contains("github-secret-token"));
    assert!(debug.contains("<redacted>"));
}
