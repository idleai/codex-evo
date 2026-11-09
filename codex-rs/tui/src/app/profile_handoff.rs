//! Switches the visible conversation through a portable profile fork.

use super::session_lifecycle::ThreadAttachPresentation;
use super::*;
use crate::app_server_session::HandoffModelSelection;

impl App {
    pub(super) async fn select_model_picker_profile(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        event: &AppEvent,
    ) -> bool {
        let (model, effort, service_tier) = match event {
            AppEvent::ApplyModelSelection { model, effort } => (model, effort.clone(), None),
            AppEvent::ApplyAdvancedReasoning { model, effort } => {
                (model, Some(effort.clone()), None)
            }
            AppEvent::SelectSessionModel {
                model,
                effort,
                service_tier,
            } => (model, effort.clone(), service_tier.clone()),
            _ => return false,
        };
        let effective = self.config.config_layer_stack.effective_config();
        let Some(profiles) = effective
            .get("model_picker_profiles")
            .and_then(toml::Value::as_table)
        else {
            return false;
        };
        let target = profiles.get(model).and_then(toml::Value::as_str);
        let current = profiles
            .get(self.chat_widget.current_model())
            .and_then(toml::Value::as_str);
        if target.is_none() && current.is_none() {
            return false;
        }
        if target == current {
            // A profile owns its defaults. Adjust this conversation without writing
            // its model into the base provider's configuration.
            self.select_session_model(app_server, model.clone(), effort, service_tier)
                .await;
            return true;
        }
        let profile = target.map(str::to_owned);
        if self.chat_widget.is_agent_turn_running() {
            self.chat_widget.add_info_message(
                "Wait for the current turn to finish before switching model providers.".into(),
                /*hint*/ None,
            );
            return true;
        }
        if self.chat_widget.has_misalignment_policy_violation() {
            self.chat_widget.show_misalignment_policy_precaution();
            return true;
        }
        self.handoff_session(
            tui,
            app_server,
            profile,
            Some(HandoffModelSelection {
                model: model.clone(),
                effort,
                service_tier,
            }),
        )
        .await;
        true
    }

    pub(super) async fn handoff_current_session(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        profile: Option<String>,
    ) {
        self.handoff_session(tui, app_server, profile, /*selection*/ None)
            .await;
    }

    async fn handoff_session(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        profile: Option<String>,
        selection: Option<HandoffModelSelection>,
    ) {
        self.session_telemetry.counter(
            "codex.thread.fork",
            /*inc*/ 1,
            &[(
                "source",
                if selection.is_some() {
                    "model_picker"
                } else {
                    "slash_handoff"
                },
            )],
        );
        let profile_display = profile.clone().unwrap_or_else(|| "base".to_string());
        let from_picker = selection.is_some();
        let command = if let Some(selection) = selection.as_ref() {
            format!("/model {}", selection.model)
        } else if profile.is_some() {
            format!("/handoff {profile_display}")
        } else {
            "/handoff --base".to_string()
        };
        self.chat_widget
            .add_plain_history_lines(vec![command.magenta().into()]);
        let previous_usage = self.chat_widget.token_usage();
        let previous_thread = resumable_thread(
            self.chat_widget.thread_id(),
            self.chat_widget.thread_name(),
            self.chat_widget.rollout_path().as_deref(),
        );
        if let Some(thread_id) = self.chat_widget.thread_id() {
            match app_server
                .handoff_thread(
                    &self.local_settings,
                    self.config.clone(),
                    thread_id,
                    profile,
                    selection,
                )
                .await
            {
                Ok(handoff) => {
                    let model = handoff.session.model.clone();
                    let model_provider = handoff.session.model_provider_id.clone();
                    let model_picker_catalog_available = self
                        .model_catalog
                        .try_list_models()
                        .is_ok_and(|models| models.iter().any(|preset| preset.model == model));
                    self.shutdown_current_thread(app_server).await;
                    match self
                        .replace_chat_widget_with_app_server_thread(
                            tui,
                            handoff,
                            ThreadAttachPresentation::SessionLineage,
                            /*initial_user_message*/ None,
                        )
                        .await
                    {
                        Ok(()) => {
                            self.chat_widget
                                .set_model_picker_catalog_available(model_picker_catalog_available);
                            let message = if from_picker {
                                format!(
                                    "Model changed to {model}. Continued in a linked conversation with {model_provider}."
                                )
                            } else {
                                format!(
                                    "Continued through profile '{profile_display}' with {model} ({model_provider})."
                                )
                            };
                            self.chat_widget.add_info_message(
                                message,
                                /*hint*/
                                (!from_picker && profile_display != "base").then(|| {
                                    "Use /handoff --base to return to the base config.".to_string()
                                }),
                            );
                            let mut lines: Vec<Line<'static>> = Vec::new();
                            if !previous_usage.is_zero() {
                                lines.push(previous_usage.to_string().into());
                            }
                            if let Some(command) = previous_thread.and_then(|thread| {
                                codex_utils_cli::resume_hint(
                                    thread.thread_name.as_deref(),
                                    Some(thread.thread_id),
                                )
                            }) {
                                lines.push(
                                    vec![
                                        "To continue the previous provider thread, run ".into(),
                                        command.cyan(),
                                    ]
                                    .into(),
                                );
                            }
                            if !lines.is_empty() {
                                self.chat_widget.add_plain_history_lines(lines);
                            }
                        }
                        Err(err) => {
                            self.chat_widget.add_error_message(format!(
                                "Failed to attach to handed-off app-server thread: {err}"
                            ));
                        }
                    }
                }
                Err(err) => {
                    self.chat_widget.add_error_message(format!(
                        "Failed to hand off through profile '{profile_display}': {err}"
                    ));
                }
            }
        } else {
            self.chat_widget.add_error_message(
                "The session is still starting; try /handoff again in a moment.".to_string(),
            );
        }

        self.chat_widget.maybe_send_next_queued_input();
        tui.frame_requester().schedule_frame();
    }
}
