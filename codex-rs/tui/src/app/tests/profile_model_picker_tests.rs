use super::*;
use codex_models_manager::model_info::model_info_from_slug;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::ModelsResponse;
use pretty_assertions::assert_eq;

const LOCAL_MODEL: &str = "deepseek-v4-flash";

async fn configure_picker(app: &mut App) -> Result<()> {
    let home = app.config.codex_home.to_path_buf();
    std::fs::write(
        home.join("config.toml"),
        r#"model = "gpt-5.6-sol"
model_reasoning_effort = "medium"
cli_auth_credentials_store = "file"
[model_picker_profiles]
deepseek-v4-flash = "dsv4"
[model_providers.sglang_dsv4]
name = "Local DeepSeek"
base_url = "http://127.0.0.1:1/v1"
wire_api = "responses"
requires_openai_auth = false
supports_namespace_tools = false
supports_codex_agent_messages = false
"#,
    )?;
    std::fs::write(
        home.join("dsv4.config.toml"),
        r#"model = "deepseek-v4-flash"
model_provider = "sglang_dsv4"
model_reasoning_effort = "high"
model_catalog_json = "dsv4-models.json"
service_tier = "default"
"#,
    )?;
    let mut model = model_info_from_slug(LOCAL_MODEL);
    model.visibility = ModelVisibility::List;
    model.supported_in_api = true;
    std::fs::write(
        home.join("dsv4-models.json"),
        serde_json::to_vec(&ModelsResponse {
            models: vec![model],
        })?,
    )?;
    std::fs::write(
        home.join("auth.json"),
        serde_json::to_vec(&serde_json::json!({
            "auth_mode": "githubCopilot",
            "github_copilot": {
                "access_token": "synthetic-picker-test-token",
                "api_endpoint": "https://api.individual.githubcopilot.com",
                "login": "picker-test",
                "models": ["gpt-5.6-sol", "gpt-6.1-sol"]
            }
        }))?,
    )?;
    app.config = ConfigBuilder::default()
        .codex_home(home)
        .loader_overrides(LoaderOverrides::without_managed_config_for_tests())
        .harness_overrides(ConfigOverrides {
            cwd: Some(app.config.cwd.to_path_buf()),
            ..Default::default()
        })
        .build()
        .await?;
    app.local_settings = crate::local_settings::LocalSettings::from(&app.config);
    Ok(())
}

#[tokio::test]
async fn profile_model_picker_switches_both_directions_and_keeps_base_defaults() -> Result<()> {
    let (mut app, mut events, _) = make_test_app_with_channels().await;
    configure_picker(&mut app).await?;
    let base_path = app.config.codex_home.join("config.toml");
    let original = std::fs::read(&base_path)?;
    let mut server = Box::pin(crate::start_embedded_app_server_for_picker(&app.config)).await?;
    let started = server.start_thread(&app.config).await?;
    let initial = started.session.thread_id;
    app.enqueue_primary_thread_session(started.session, started.turns)
        .await?;
    let mut tui = crate::tui::test_support::make_test_tui()?;
    tui.pause_events();

    for (model, effort, provider) in [
        (LOCAL_MODEL, ReasoningEffortConfig::Low, "sglang_dsv4"),
        ("gpt-6.1-sol", ReasoningEffortConfig::High, "openai"),
    ] {
        let previous = app.chat_widget.thread_id().unwrap();
        Box::pin(app.handle_event(
            &mut tui,
            &mut server,
            AppEvent::ApplyModelSelection {
                model: model.into(),
                effort: Some(effort.clone()),
            },
        ))
        .await?;
        let mut messages = Vec::new();
        while let Ok(event) = events.try_recv() {
            if let AppEvent::InsertHistoryCell(cell) = &event {
                messages.push(lines_to_single_string(
                    &cell.transcript_lines(/*width*/ 120),
                ));
            }
            Box::pin(app.handle_event(&mut tui, &mut server, event)).await?;
        }
        let selected = app.chat_widget.thread_id().unwrap();
        assert_ne!(selected, previous, "{}", messages.join("\n"));
        let thread = server
            .thread_read(selected, /*include_turns*/ false)
            .await?;
        assert_eq!(thread.model_provider, provider);
        assert_eq!(thread.model.as_deref(), Some(model));
        assert_eq!(app.chat_widget.current_model(), model);
        assert_eq!(app.chat_widget.current_reasoning_effort(), Some(effort));
        assert_eq!(std::fs::read(&base_path)?, original);
        assert_eq!(
            server
                .thread_read(previous, /*include_turns*/ false)
                .await?
                .model_provider,
            if previous == initial {
                "openai"
            } else {
                "sglang_dsv4"
            }
        );
        if model == LOCAL_MODEL {
            Box::pin(app.handle_event(
                &mut tui,
                &mut server,
                AppEvent::ApplyModelSelection {
                    model: model.into(),
                    effort: Some(ReasoningEffortConfig::Medium),
                },
            ))
            .await?;
            assert_eq!(app.chat_widget.thread_id(), Some(selected));
            assert_eq!(
                app.chat_widget.current_reasoning_effort(),
                Some(ReasoningEffortConfig::Medium)
            );
            assert_eq!(std::fs::read(&base_path)?, original);
        }
    }
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn profile_model_picker_failure_preserves_active_thread_and_defaults() -> Result<()> {
    let (mut app, mut events, _) = make_test_app_with_channels().await;
    configure_picker(&mut app).await?;
    let base_path = app.config.codex_home.join("config.toml");
    let original = std::fs::read(&base_path)?;
    let mut server = Box::pin(crate::start_embedded_app_server_for_picker(&app.config)).await?;
    let started = server.start_thread(&app.config).await?;
    let initial = started.session.thread_id;
    app.enqueue_primary_thread_session(started.session, started.turns)
        .await?;
    std::fs::write(app.config.codex_home.join("dsv4.config.toml"), "[invalid")?;
    let mut tui = crate::tui::test_support::make_test_tui()?;
    Box::pin(app.handle_event(
        &mut tui,
        &mut server,
        AppEvent::SelectSessionModel {
            model: LOCAL_MODEL.into(),
            effort: Some(ReasoningEffortConfig::Low),
            service_tier: None,
        },
    ))
    .await?;
    let messages = std::iter::from_fn(|| events.try_recv().ok())
        .filter_map(|event| match event {
            AppEvent::InsertHistoryCell(cell) => {
                Some(lines_to_single_string(
                    &cell.transcript_lines(/*width*/ 100),
                ))
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(messages.contains("Failed to hand off"), "{messages}");
    assert_eq!(app.chat_widget.thread_id(), Some(initial));
    assert_eq!(app.chat_widget.current_model(), "gpt-5.6-sol");
    assert_eq!(std::fs::read(&base_path)?, original);
    server.shutdown().await?;
    Ok(())
}
