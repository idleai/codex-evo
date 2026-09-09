use codex_login::GitHubCopilotAuth;
use codex_protocol::openai_models::ReasoningEffort;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::model_catalog;

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
