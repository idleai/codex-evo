//! TUI summaries default to detailed on the wire, including after resume and model defaults of none.

use super::*;
use crate::legacy_core::config::ConfigBuilder;
use codex_app_server_client::AppServerEvent;
use codex_app_server_protocol::ServerNotification;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn reasoning_defaults_reach_responses() -> Result<()> {
    // Like the other embedded-server fixtures, startup and resume load full configs on this thread.
    const TEST_STACK_SIZE_BYTES: usize = 16 * 1024 * 1024;
    std::thread::Builder::new()
        .name("tui-reasoning-defaults".into())
        .stack_size(TEST_STACK_SIZE_BYTES)
        .spawn(|| -> Result<()> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            for model_default in [ReasoningSummary::None, ReasoningSummary::Concise] {
                runtime.block_on(crate::boxed_future!(reasoning_defaults_cases(
                    model_default
                )))?;
            }
            Ok(())
        })?
        .join()
        .expect("reasoning defaults test thread panicked")
}

async fn reasoning_defaults_cases(model_default: ReasoningSummary) -> Result<()> {
    for (settings, summary, stream_options, mode) in [
        (
            "",
            json!("detailed"),
            json!(null),
            ThreadParamsMode::Embedded,
        ),
        (
            "[features]\nconcurrent_reasoning_summaries = false",
            json!("detailed"),
            json!(null),
            ThreadParamsMode::Embedded,
        ),
        (
            "[features]\nconcurrent_reasoning_summaries = true",
            json!("detailed"),
            json!({"reasoning_summary_delivery": "sequential_cutoff"}),
            ThreadParamsMode::Embedded,
        ),
        (
            "model_reasoning_summary = 'detailed'",
            json!("detailed"),
            json!(null),
            ThreadParamsMode::Embedded,
        ),
        (
            "model_reasoning_summary = 'detailed'\n[features]\nconcurrent_reasoning_summaries = true",
            json!("detailed"),
            json!({"reasoning_summary_delivery": "sequential_cutoff"}),
            ThreadParamsMode::Embedded,
        ),
        (
            "model_reasoning_summary = 'none'\n[features]\nconcurrent_reasoning_summaries = true",
            json!(null),
            json!(null),
            ThreadParamsMode::Embedded,
        ),
        (
            "model_reasoning_summary = 'concise'",
            json!("concise"),
            json!(null),
            ThreadParamsMode::Embedded,
        ),
        (
            "model_reasoning_summary = 'auto'",
            json!("auto"),
            json!(null),
            ThreadParamsMode::Embedded,
        ),
        (
            "model_verbosity = 'high'\nweb_search = 'live'",
            json!("detailed"),
            json!(null),
            ThreadParamsMode::Remote,
        ),
        (
            "model_reasoning_summary = 'detailed'\nmodel_verbosity = 'high'\nweb_search = 'live'",
            json!("detailed"),
            json!(null),
            ThreadParamsMode::Remote,
        ),
    ] {
        let server = responses::start_mock_server().await;
        let home = tempfile::tempdir()?;
        let base_url = server.uri();
        let mut catalog = codex_models_manager::bundled_models_response()?;
        catalog
            .models
            .iter_mut()
            .find(|model| model.slug == "gpt-5.5")
            .expect("bundled model")
            .default_reasoning_summary = model_default;
        // Thread startup reloads config, so the fixture catalog must survive that reload.
        std::fs::write(
            home.path().join("catalog.json"),
            serde_json::to_vec(&catalog)?,
        )?;
        std::fs::write(
            home.path().join("config.toml"),
            format!(
                r#"
model = "gpt-5.5"
model_provider = "reasoning-test"
model_catalog_json = "catalog.json"
{settings}
[model_providers.reasoning-test]
name = "OpenAI"
base_url = "{base_url}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#
            ),
        )?;
        crate::legacy_core::config::set_project_trust_level(
            home.path(),
            &std::env::current_dir()?,
            codex_protocol::config_types::TrustLevel::Trusted,
        )
        .map_err(|error| color_eyre::eyre::eyre!(error.to_string()))?;
        let server_config = crate::boxed_future!(
            ConfigBuilder::default()
                .codex_home(home.path().to_path_buf())
                .build()
        )
        .await?;
        let client_home = tempfile::tempdir()?;
        let config = if mode == ThreadParamsMode::Remote {
            std::fs::write(
                client_home.path().join("config.toml"),
                "model_verbosity = 'low'\nweb_search = 'disabled'",
            )?;
            crate::boxed_future!(
                ConfigBuilder::default()
                    .codex_home(client_home.path().to_path_buf())
                    .build()
            )
            .await?
        } else {
            server_config.clone()
        };
        let mut app_server =
            crate::boxed_future!(crate::start_embedded_app_server_for_picker(&server_config))
                .await?;
        app_server.thread_params_mode = mode;
        let started = crate::boxed_future!(app_server.start_thread(&config)).await?;
        for resume in [false, true] {
            if resume {
                // Simulate an existing conversation whose summaries were disabled.
                app_server
                    .thread_settings_update(ThreadSettingsUpdateParams {
                        thread_id: started.session.thread_id.to_string(),
                        summary: Some(ReasoningSummary::None),
                        ..ThreadSettingsUpdateParams::default()
                    })
                    .await?;
                app_server.shutdown().await?;
                app_server = crate::boxed_future!(crate::start_embedded_app_server_for_picker(
                    &server_config
                ))
                .await?;
                app_server.thread_params_mode = mode;
                let local_settings = crate::local_settings::LocalSettings::from(&config);
                crate::boxed_future!(app_server.resume_thread(
                    &local_settings,
                    config.clone(),
                    started.session.thread_id,
                    ResumeModelSettings::RestoreFromThread,
                ))
                .await?;
            }
            let response = responses::mount_sse_once(
                &server,
                responses::sse(vec![
                    responses::ev_response_created("response"),
                    responses::ev_completed("response"),
                ]),
            )
            .await;
            let turn = app_server
                .turn_start(
                    started.session.thread_id,
                    "user",
                    "tui-user-message".to_string(),
                    vec![UserInput::Text {
                        text: "hello".to_string(),
                        text_elements: Vec::new(),
                    }],
                    config.cwd.to_path_buf(),
                    /*approval_policy*/ None,
                    /*approvals_reviewer*/ None,
                    TurnPermissionsOverride::Preserve,
                    &config.workspace_roots,
                    started.session.model.clone(),
                    /*effort*/ None,
                    config.model_reasoning_summary,
                    /*service_tier*/ None,
                    /*collaboration_mode*/ None,
                    /*output_schema*/ None,
                    /*cyber_access_program*/ None,
                    /*cloud_selection*/ None,
                )
                .await?;
            tokio::time::timeout(std::time::Duration::from_secs(/*secs*/ 30), async {
                while let Some(event) = app_server.next_event().await {
                    if let AppServerEvent::ServerNotification(notification) = event.event
                        && let ServerNotification::TurnCompleted(completed) = *notification
                    {
                        assert_eq!(
                            completed.turn.status,
                            codex_app_server_protocol::TurnStatus::Completed
                        );
                        return;
                    }
                }
                panic!("app-server disconnected before completing the turn");
            })
            .await?;
            let body = response.single_request().body_json();
            assert_eq!(
                json!({"summary": body["reasoning"]["summary"], "stream_options": body["stream_options"]}),
                json!({"summary": summary, "stream_options": stream_options}),
                "settings: {settings}; model default: {model_default:?}; mode: {mode:?}; resumed: {resume}"
            );
            if mode == ThreadParamsMode::Remote {
                assert_eq!(body["text"]["verbosity"], json!("high"));
                let web_search = body["tools"]
                    .as_array()
                    .expect("tools")
                    .iter()
                    .find(|tool| tool["type"] == "web_search")
                    .expect("server's web search tool");
                assert_eq!(web_search["external_web_access"], json!(true));
            }
            let metadata: serde_json::Value = serde_json::from_str(
                body["client_metadata"]["x-codex-turn-metadata"]
                    .as_str()
                    .expect("canonical turn metadata"),
            )?;
            assert_eq!(
                (
                    &metadata["thread_id"],
                    &metadata["turn_id"],
                    &metadata["turn_trigger"],
                ),
                (
                    &json!(started.session.thread_id),
                    &json!(turn.turn.id),
                    &json!("user"),
                )
            );
        }
        app_server.shutdown().await?;
    }
    Ok(())
}
