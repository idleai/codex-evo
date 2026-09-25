//! Switches the visible conversation through a portable profile fork.

use super::session_lifecycle::ThreadAttachPresentation;
use super::*;

impl App {
    pub(super) async fn handoff_current_session(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        profile: Option<String>,
    ) {
        self.session_telemetry.counter(
            "codex.thread.fork",
            /*inc*/ 1,
            &[("source", "slash_handoff")],
        );
        let profile_display = profile.clone().unwrap_or_else(|| "base".to_string());
        let command = if profile.is_some() {
            format!("/handoff {profile_display}")
        } else {
            "/handoff --base".to_string()
        };
        self.chat_widget
            .add_plain_history_lines(vec![command.magenta().into()]);
        let summary = session_summary(
            self.chat_widget.token_usage(),
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
                            self.chat_widget.add_info_message(
                                format!(
                                    "Continued through profile '{profile_display}' with {model} ({model_provider})."
                                ),
                                /*hint*/ (profile_display != "base").then(|| {
                                    "Use /handoff --base to return to the base config."
                                        .to_string()
                                }),
                            );
                            if let Some(summary) = summary {
                                let mut lines: Vec<Line<'static>> = Vec::new();
                                if let Some(usage_line) = summary.usage_line {
                                    lines.push(usage_line.into());
                                }
                                if let Some(command) = summary.resume_hint {
                                    lines.push(
                                        vec![
                                            "To continue the previous provider thread, run ".into(),
                                            command.cyan(),
                                        ]
                                        .into(),
                                    );
                                }
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
