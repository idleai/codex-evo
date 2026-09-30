use anyhow::Result;
use codex_core::TurnInputRequest;
use codex_login::CodexAuth;
use codex_login::GitHubCopilotAuth;
use codex_protocol::protocol::EventMsg;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;

#[tokio::test]
async fn github_copilot_auth_rejects_unadvertised_model_without_network_fallback() -> Result<()> {
    let server = start_mock_server().await;
    let auth = GitHubCopilotAuth::new(
        "github-token".to_string(),
        "https://api.individual.githubcopilot.com".to_string(),
        Some("octocat".to_string()),
        Some("copilot_individual".to_string()),
        vec!["gpt-5.6-sol".to_string()],
    )?;
    let test = test_codex()
        .with_auth(CodexAuth::from_github_copilot(auth))
        .with_model("not-advertised-by-copilot")
        .build_with_auto_env(&server)
        .await?;

    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "hello".to_string(),
            text_elements: Vec::new(),
        }]))
        .await?;

    let event = wait_for_event(&test.codex, |event| matches!(event, EventMsg::Error(_))).await;
    let EventMsg::Error(error) = event else {
        unreachable!("wait_for_event returned a non-error event")
    };
    assert!(
        error
            .message
            .contains("model `not-advertised-by-copilot` is not an enabled OpenAI Responses model"),
        "unexpected error: {}",
        error.message
    );
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
    Ok(())
}
