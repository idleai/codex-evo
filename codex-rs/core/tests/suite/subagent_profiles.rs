use anyhow::Result;
use codex_features::Feature;
use codex_models_manager::model_info::model_info_from_slug;
use codex_protocol::openai_models::ModelsResponse;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::protocol::MultiAgentVersion;
use core_test_support::responses::ResponseMock;
use core_test_support::responses::ResponsesRequest;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call_with_namespace;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::mount_sse_once_match;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::fs;
use std::path::Path;
use std::time::Duration;
use test_case::test_case;
use tokio::time::Instant;
use tokio::time::sleep;

const PARENT_MODEL: &str = "gpt-5.6-sol";
const CHILD_MODEL: &str = "deepseek-v4-flash";
const CHILD_PROVIDER: &str = "sglang_test";
const PROFILE: &str = "dsv4";
const PARENT_PROMPT: &str = "delegate this work";
const CHILD_PROMPT: &str = "inspect the repository";
const PROFILE_INSTRUCTIONS: &str = "DSV4 profile instructions.";
const SPAWN_CALL_ID: &str = "spawn-profile-call";

#[derive(Clone, Copy, Debug)]
enum ProfileSelection {
    Explicit,
    ConfiguredDefault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistoryFork {
    Default,
    Full,
    Fresh,
}

#[test_case(ProfileSelection::Explicit, HistoryFork::Default; "explicit profile")]
#[test_case(ProfileSelection::ConfiguredDefault, HistoryFork::Default; "configured default profile")]
#[test_case(ProfileSelection::Explicit, HistoryFork::Fresh; "explicit profile without history")]
#[test_case(ProfileSelection::ConfiguredDefault, HistoryFork::Fresh; "default profile without history")]
#[test_case(ProfileSelection::Explicit, HistoryFork::Full; "reject explicit profile with history")]
#[test_case(ProfileSelection::ConfiguredDefault, HistoryFork::Full; "reject default profile with history")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_parent_spawns_direct_sglang_child(
    selection: ProfileSelection,
    history: HistoryFork,
) -> Result<()> {
    skip_if_no_network!(Ok(()));

    let parent_server = start_mock_server().await;
    let child_server = start_mock_server().await;
    let (spawn_namespace, mut spawn_args) = match selection {
        ProfileSelection::Explicit => (
            "collaboration",
            json!({
                "message": CHILD_PROMPT,
                "task_name": "worker",
                "profile": PROFILE,
            }),
        ),
        ProfileSelection::ConfiguredDefault => (
            "multi_agent_v1",
            json!({
                "message": CHILD_PROMPT,
            }),
        ),
    };
    if history != HistoryFork::Default {
        match selection {
            ProfileSelection::Explicit => {
                spawn_args["fork_turns"] = json!(if history == HistoryFork::Full {
                    "all"
                } else {
                    "none"
                });
            }
            ProfileSelection::ConfiguredDefault => {
                spawn_args["fork_context"] = json!(history == HistoryFork::Full);
            }
        }
    }
    let spawn_args = serde_json::to_string(&spawn_args)?;

    let parent_initial = mount_sse_once_match(
        &parent_server,
        |request: &wiremock::Request| body_contains(request, PARENT_PROMPT),
        sse(vec![
            ev_response_created("resp-parent-1"),
            ev_function_call_with_namespace(
                SPAWN_CALL_ID,
                spawn_namespace,
                "spawn_agent",
                &spawn_args,
            ),
            ev_completed("resp-parent-1"),
        ]),
    )
    .await;
    let parent_followup = mount_sse_once_match(
        &parent_server,
        |request: &wiremock::Request| body_contains(request, SPAWN_CALL_ID),
        sse(vec![
            ev_response_created("resp-parent-2"),
            ev_assistant_message("msg-parent-2", "parent done"),
            ev_completed("resp-parent-2"),
        ]),
    )
    .await;
    let child_request_log = mount_sse_once(
        &child_server,
        sse(vec![
            ev_response_created("resp-child-1"),
            ev_assistant_message("msg-child-1", "child done"),
            ev_completed("resp-child-1"),
        ]),
    )
    .await;

    let child_base_url = format!("{}/v1", child_server.uri());
    let mut builder = test_codex()
        .with_pre_build_hook(move |home| {
            write_spawn_profile(home, &child_base_url, selection)
                .expect("write cross-provider spawn profile fixtures");
        })
        .with_config(|config| {
            config
                .features
                .enable(Feature::Collab)
                .expect("test config should allow feature update");
            config
                .features
                .enable(Feature::MultiAgentV2)
                .expect("test config should allow feature update");
            config.model = Some(PARENT_MODEL.to_string());
        });
    let test = builder.build_with_auto_env(&parent_server).await?;

    test.submit_turn(PARENT_PROMPT).await?;

    let expected_parent_multi_agent_version = match selection {
        ProfileSelection::Explicit => MultiAgentVersion::V2,
        ProfileSelection::ConfiguredDefault => MultiAgentVersion::V1,
    };
    assert_eq!(
        test.codex.multi_agent_version(),
        Some(expected_parent_multi_agent_version)
    );

    if history == HistoryFork::Full {
        let output = parent_followup
            .function_call_output_text(SPAWN_CALL_ID)
            .expect("parent should receive the rejected spawn result");
        assert!(output.contains("cross-provider spawn_agent profiles cannot fork parent context"));
        assert_eq!(child_request_log.requests().len(), 0);
        assert_eq!(
            test.thread_manager.list_thread_ids().await,
            vec![test.session_configured.thread_id]
        );
        return Ok(());
    }

    let child_request = match wait_for_request(&child_request_log).await {
        Ok(request) => request,
        Err(error) => {
            anyhow::bail!(
                "{error}; parent spawn output: {:?}; parent follow-up request count: {}; thread ids: {:?}",
                parent_followup.function_call_output_text(SPAWN_CALL_ID),
                parent_followup.requests().len(),
                test.thread_manager.list_thread_ids().await,
            );
        }
    };
    let child_body = child_request.body_json();
    assert_eq!(
        (
            child_body["model"].as_str(),
            child_body["reasoning"]["effort"].as_str(),
            child_request.inputs_of_type("agent_message").is_empty(),
            child_request.body_contains_text(CHILD_PROMPT),
            child_request.body_contains_text(PROFILE_INSTRUCTIONS),
            child_request.body_contains_text(PARENT_PROMPT),
        ),
        (Some(CHILD_MODEL), Some("max"), true, true, true, false)
    );

    let child_snapshot = wait_for_child_snapshot(&test).await?;
    assert_eq!(
        (
            test.config.model_provider_id.as_str(),
            child_snapshot.model.as_str(),
            child_snapshot.model_provider_id.as_str(),
            child_snapshot.reasoning_effort,
        ),
        (
            "openai",
            CHILD_MODEL,
            CHILD_PROVIDER,
            Some(ReasoningEffort::Max),
        )
    );
    assert_eq!(parent_initial.requests().len(), 1);
    assert_eq!(parent_followup.requests().len(), 1);

    Ok(())
}

fn write_spawn_profile(
    home: &Path,
    child_base_url: &str,
    selection: ProfileSelection,
) -> Result<()> {
    let default_profile = match selection {
        ProfileSelection::Explicit => String::new(),
        ProfileSelection::ConfiguredDefault => {
            format!("\n[agents]\ndefault_subagent_profile = {PROFILE:?}\n")
        }
    };
    fs::write(
        home.join("config.toml"),
        format!(
            r#"[model_providers.{CHILD_PROVIDER}]
name = "SGLang test"
base_url = {child_base_url:?}
wire_api = "responses"
requires_openai_auth = false
supports_namespace_tools = false
supports_codex_agent_messages = false
{default_profile}"#,
        ),
    )?;

    let instructions_path = home.join("dsv4-instructions.md");
    fs::write(&instructions_path, PROFILE_INSTRUCTIONS)?;
    let catalog_path = home.join("dsv4-models.json");
    let mut child_model = model_info_from_slug(CHILD_MODEL);
    child_model.default_reasoning_level = Some(ReasoningEffort::Max);
    child_model.supported_reasoning_levels = vec![ReasoningEffortPreset {
        effort: ReasoningEffort::Max,
        description: "Maximum reasoning effort".to_string(),
    }];
    child_model.context_window = Some(1_048_576);
    child_model.max_context_window = Some(1_048_576);
    child_model.multi_agent_version = Some(MultiAgentVersion::V2);
    fs::write(
        &catalog_path,
        serde_json::to_vec(&ModelsResponse {
            models: vec![child_model],
        })?,
    )?;
    fs::write(
        home.join("dsv4.config.toml"),
        format!(
            r#"model = {CHILD_MODEL:?}
model_provider = {CHILD_PROVIDER:?}
model_catalog_json = {:?}
model_instructions_file = {:?}
model_context_window = 1048576
model_reasoning_effort = "max"
model_reasoning_summary = "auto"
"#,
            catalog_path.to_string_lossy(),
            instructions_path.to_string_lossy(),
        ),
    )?;
    Ok(())
}

fn body_contains(request: &wiremock::Request, text: &str) -> bool {
    let is_zstd = request
        .headers
        .get("content-encoding")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|entry| entry.trim().eq_ignore_ascii_case("zstd"))
        });
    let body = if is_zstd {
        zstd::stream::decode_all(std::io::Cursor::new(&request.body)).ok()
    } else {
        Some(request.body.clone())
    };
    body.and_then(|body| String::from_utf8(body).ok())
        .is_some_and(|body| body.contains(text))
}

async fn wait_for_request(mock_server: &ResponseMock) -> Result<ResponsesRequest> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(request) = mock_server.last_request() {
            return Ok(request);
        }
        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for response request");
        }
        sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_child_snapshot(test: &TestCodex) -> Result<codex_core::ThreadConfigSnapshot> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        for thread_id in test.thread_manager.list_thread_ids().await {
            if thread_id != test.session_configured.thread_id {
                return Ok(test
                    .thread_manager
                    .get_thread(thread_id)
                    .await?
                    .config_snapshot()
                    .await);
            }
        }
        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for spawned child thread");
        }
        sleep(Duration::from_millis(10)).await;
    }
}
