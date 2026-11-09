//! New-chat model selection must keep discovery, routing, and restored history aligned.

use anyhow::Result;
use app_test_support::ChatGptAuthFixture;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use app_test_support::write_chatgpt_auth;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ModelListParams;
use codex_app_server_protocol::ModelListResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadHistoryMode;
use codex_app_server_protocol::ThreadListResponse;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::TurnCompletedNotification;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::TurnStatus;
use codex_app_server_protocol::UserInput;
use codex_config::types::AuthCredentialsStoreMode;
use codex_login::AuthKeyringBackendKind;
use codex_login::GitHubCopilotAuth;
use codex_login::login_with_github_copilot;
use codex_models_manager::model_info::model_info_from_slug;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::ModelsResponse;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;
use test_case::test_case;
use tokio::time::timeout;
use wiremock::MockServer;

const MODEL: &str = "deepseek-v4-flash";
const REGISTRY: &str = "model_picker_profiles = { deepseek-v4-flash = 'dsv4' }";
const WAIT: Duration = Duration::from_secs(/*secs*/ 20);

fn write_profile(home: &Path, endpoint: &str) -> Result<()> {
    let mut model = model_info_from_slug(MODEL);
    model.display_name = "DeepSeek V4.1 Flash (local)".to_string();
    model.visibility = ModelVisibility::List;
    model.supported_in_api = true;
    let catalog = home.join("deepseek.json");
    std::fs::write(
        &catalog,
        serde_json::to_vec(&ModelsResponse {
            models: vec![model],
        })?,
    )?;
    let instructions = home.join("deepseek-prompt.txt");
    std::fs::write(&instructions, "DEEPSEEK_PROFILE_INSTRUCTIONS")?;
    std::fs::write(
        home.join("dsv4.config.toml"),
        format!(
            r#"
model = "{MODEL}"
model_provider = "sglang_dsv4"
model_catalog_json = {}
model_instructions_file = {}
model_context_window = 524288
model_reasoning_effort = "medium"
service_tier = "default"
[model_providers.sglang_dsv4]
name = "DeepSeek test"
base_url = "{endpoint}/v1"
wire_api = "responses"
requires_openai_auth = false
supports_namespace_tools = false
supports_codex_agent_messages = false
request_max_retries = 0
stream_max_retries = 0
"#,
            serde_json::to_string(&catalog)?,
            serde_json::to_string(&instructions)?
        ),
    )?;
    Ok(())
}

async fn complete_turn(server: &mut TestAppServer, thread_id: &str) -> Result<()> {
    let _: TurnStartResponse = server
        .request(|request_id| ClientRequest::TurnStart {
            request_id,
            params: TurnStartParams {
                thread_id: thread_id.to_string(),
                input: vec![UserInput::Text {
                    text: "Reply with the test marker.".to_string(),
                    text_elements: vec![],
                }],
                ..Default::default()
            },
        })
        .await?;
    let notification = timeout(
        WAIT,
        server.read_stream_until_notification_message("turn/completed"),
    )
    .await??;
    let completed: TurnCompletedNotification =
        serde_json::from_value(notification.params.expect("params"))?;
    assert_eq!(completed.turn.status, TurnStatus::Completed);
    Ok(())
}

#[test_case(ThreadHistoryMode::Legacy; "legacy")]
#[test_case(ThreadHistoryMode::Paginated; "paginated")]
#[tokio::test]
async fn picker_profile_routes_new_threads_and_restores_the_route(
    history_mode: ThreadHistoryMode,
) -> Result<()> {
    let native = MockServer::start().await;
    let target = MockServer::start().await;
    let body = responses::sse(vec![
        responses::ev_assistant_message("marker", "OK"),
        responses::ev_completed("response"),
    ]);
    let native_mock = responses::mount_sse_once(&native, body.clone()).await;
    let target_mock = responses::mount_sse_once(&target, body).await;
    let home = TempDir::new()?;
    MockResponsesConfig::new(&native.uri())
        .with_root_config(REGISTRY)
        .write(home.path())?;
    write_profile(home.path(), &target.uri())?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let original = server
        .start_thread(ThreadStartParams {
            history_mode: Some(history_mode),
            ..Default::default()
        })
        .await?;
    complete_turn(&mut server, &original.thread.id).await?;
    let selected = server
        .start_thread(ThreadStartParams {
            model: Some(MODEL.to_string()),
            // A client may echo the default provider with its picker choice.
            model_provider: Some("mock_provider".to_string()),
            service_tier: Some(Some("priority".to_string())),
            history_mode: Some(history_mode),
            ..Default::default()
        })
        .await?;
    assert_eq!(
        (
            selected.model.as_str(),
            selected.model_provider.as_str(),
            selected.service_tier.as_deref()
        ),
        (MODEL, "sglang_dsv4", Some("default"))
    );
    complete_turn(&mut server, &selected.thread.id).await?;
    assert_eq!(
        native_mock.single_request().body_json()["model"],
        "mock-model"
    );
    let target_request = target_mock.single_request();
    assert_eq!(
        (
            target_request.body_json()["model"].clone(),
            target_request.instructions_text()
        ),
        (json!(MODEL), "DEEPSEEK_PROFILE_INSTRUCTIONS".to_string())
    );

    let request_id = server
        .send_request(
            "thread/list",
            Some(json!({"sourceKinds":["appServer", "cli", "exec", "vscode", "unknown"]})),
        )
        .await?;
    let listed: ThreadListResponse = timeout(WAIT, server.read_response(request_id)).await??;
    let mut actual: Vec<_> = listed
        .data
        .into_iter()
        .map(|thread| (thread.id, thread.model_provider))
        .collect();
    actual.sort();
    let mut expected = vec![
        (original.thread.id.clone(), "mock_provider".to_string()),
        (selected.thread.id.clone(), "sglang_dsv4".to_string()),
    ];
    expected.sort();
    assert_eq!(actual, expected);

    for (method, params) in [
        (
            "thread/settings/update",
            json!({"threadId":original.thread.id,"model":MODEL}),
        ),
        (
            "turn/start",
            json!({"threadId":original.thread.id,"input":[],"collaborationMode":{"mode":"default","settings":{"model":MODEL,"reasoning_effort":"medium","developer_instructions":null}}}),
        ),
    ] {
        let request_id = server.send_request(method, Some(params)).await?;
        let error = timeout(
            WAIT,
            server.read_stream_until_error_message(RequestId::Integer(request_id)),
        )
        .await??;
        assert!(
            error.error.message.contains("Start a new chat"),
            "{error:?}"
        );
    }
    assert!(server.shutdown_gracefully().await?.success());
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let request_id = server
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: selected.thread.id.clone(),
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let error = timeout(
        WAIT,
        server.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert!(
        error.error.message.contains("Start a new chat"),
        "{error:?}"
    );
    let resumed: ThreadResumeResponse = server
        .request(|request_id| ClientRequest::ThreadResume {
            request_id,
            params: ThreadResumeParams {
                thread_id: selected.thread.id,
                model: Some(MODEL.to_string()),
                ..Default::default()
            },
        })
        .await?;
    assert_eq!(
        (resumed.model.as_str(), resumed.model_provider.as_str()),
        (MODEL, "sglang_dsv4")
    );
    let request_id = server
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: original.thread.id,
            model: Some(MODEL.to_string()),
            ..Default::default()
        })
        .await?;
    let error = timeout(
        WAIT,
        server.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert!(
        error.error.message.contains("Start a new chat"),
        "{error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn picker_profile_stays_available_with_disabled_chatgpt_discovery() -> Result<()> {
    let native = MockServer::start().await;
    let target = MockServer::start().await;
    let home = TempDir::new()?;
    let mut native_model = model_info_from_slug("native-discovered-model");
    native_model.visibility = ModelVisibility::List;
    responses::mount_models_once(
        &native,
        ModelsResponse {
            models: vec![native_model],
        },
    )
    .await;
    MockResponsesConfig::new(&native.uri())
        .with_root_config(REGISTRY)
        .with_provider_config("requires_openai_auth = true")
        .write(home.path())?;
    write_chatgpt_auth(
        home.path(),
        ChatGptAuthFixture::new("test-token").plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;
    write_profile(home.path(), &target.uri())?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .with_env_overrides(&[("OPENAI_API_KEY", None)])
        .build_initialized()
        .await?;
    let result: ModelListResponse = server
        .request(|request_id| ClientRequest::ModelList {
            request_id,
            params: ModelListParams::default(),
        })
        .await?;
    let local = result.data.last().expect("local profile model");
    assert_eq!(local.model, MODEL);
    assert_eq!(local.display_name, "DeepSeek V4.1 Flash (local)");
    assert!(!local.is_default);
    assert!(
        result
            .data
            .iter()
            .all(|model| model.model != "native-discovered-model")
    );
    // Legacy credentials must not restore the disabled ChatGPT discovery path.
    assert!(
        native
            .received_requests()
            .await
            .expect("requests")
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn copilot_picker_keeps_all_account_models_and_routes_local_profile_without_credentials()
-> Result<()> {
    let target = MockServer::start().await;
    let body = responses::sse(vec![
        responses::ev_assistant_message("marker", "LOCAL_OK"),
        responses::ev_completed("response"),
    ]);
    let target_mock = responses::mount_sse_once(&target, body).await;
    let home = TempDir::new()?;
    std::fs::write(
        home.path().join("config.toml"),
        format!("cli_auth_credentials_store = 'file'\n{REGISTRY}\n"),
    )?;
    write_profile(home.path(), &target.uri())?;
    let models = vec!["gpt-5.6-sol".to_string(), "new-openai-model".to_string()];
    login_with_github_copilot(
        home.path(),
        GitHubCopilotAuth::new(
            "github-token-must-stay-on-copilot".to_string(),
            "https://api.individual.githubcopilot.com".to_string(),
            Some("octocat".to_string()),
            None,
            models.clone(),
        )?,
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let listed: ModelListResponse = server
        .request(|request_id| ClientRequest::ModelList {
            request_id,
            params: ModelListParams::default(),
        })
        .await?;
    let mut expected = models;
    expected.push(MODEL.to_string());
    assert_eq!(
        listed
            .data
            .iter()
            .map(|model| model.model.clone())
            .collect::<Vec<_>>(),
        expected
    );
    let selected = server
        .start_thread(ThreadStartParams {
            model: Some(MODEL.to_string()),
            model_provider: Some("openai".to_string()),
            ..Default::default()
        })
        .await?;
    assert_eq!(selected.model_provider, "sglang_dsv4");
    complete_turn(&mut server, &selected.thread.id).await?;
    let request = target_mock.single_request();
    assert_eq!(request.body_json()["model"], MODEL);
    assert_eq!(request.instructions_text(), "DEEPSEEK_PROFILE_INSTRUCTIONS");
    assert_eq!(request.header("authorization"), None);
    assert_eq!(request.header("chatgpt-account-id"), None);
    assert_eq!(request.header("openai-intent"), None);

    assert!(server.shutdown_gracefully().await?.success());
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let resumed: ThreadResumeResponse = server
        .request(|request_id| ClientRequest::ThreadResume {
            request_id,
            params: ThreadResumeParams {
                thread_id: selected.thread.id,
                model: Some(MODEL.to_string()),
                ..Default::default()
            },
        })
        .await?;
    assert_eq!(resumed.model, MODEL);
    assert_eq!(resumed.model_provider, "sglang_dsv4");
    Ok(())
}

#[tokio::test]
async fn picker_profile_respects_required_provider() -> Result<()> {
    let native = MockServer::start().await;
    let target = MockServer::start().await;
    let home = TempDir::new()?;
    MockResponsesConfig::new(&native.uri())
        .with_root_config(REGISTRY)
        .write(home.path())?;
    write_profile(home.path(), &target.uri())?;
    std::fs::write(
        home.path().join("requirements.toml"),
        "model_provider = 'mock_provider'\n",
    )?;
    let mut server = TestAppServer::builder()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let request_id = server
        .send_thread_start_request_with_auto_env(ThreadStartParams {
            model: Some(MODEL.to_string()),
            ..Default::default()
        })
        .await?;
    let error = timeout(
        WAIT,
        server.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert!(
        error
            .error
            .message
            .contains("cannot use its configured profile provider"),
        "{error:?}"
    );
    assert!(
        target
            .received_requests()
            .await
            .expect("requests")
            .is_empty()
    );
    Ok(())
}
