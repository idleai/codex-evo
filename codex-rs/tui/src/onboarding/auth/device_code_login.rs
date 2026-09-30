use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::LoginAccountParams;
use codex_app_server_protocol::LoginAccountResponse;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;
use uuid::Uuid;

use crate::motion::MotionMode;
use crate::motion::shimmer_text;

use super::AuthModeWidget;
use super::ContinueWithDeviceCodeState;
use super::SignInState;
use super::cancel_login_attempt;
use super::mark_url_hyperlink;
use super::onboarding_request_id;

/// Device-code providers share attempt tracking, cancellation, and copyable UI.
#[derive(Clone, Copy)]
pub(super) enum DeviceCodeProvider {
    ChatGpt,
    GitHubCopilot,
}

impl DeviceCodeProvider {
    fn sign_in_state(self, state: ContinueWithDeviceCodeState) -> SignInState {
        match self {
            Self::ChatGpt => SignInState::ChatGptDeviceCode(state),
            Self::GitHubCopilot => SignInState::GitHubCopilotDeviceCode(state),
        }
    }
}

pub(super) fn start_device_code_login(widget: &mut AuthModeWidget, provider: DeviceCodeProvider) {
    let request_id = Uuid::new_v4().to_string();
    *widget.sign_in_state.write().unwrap() =
        provider.sign_in_state(ContinueWithDeviceCodeState::pending(request_id.clone()));
    widget.request_frame.schedule_frame();

    let request_handle = widget.app_server_request_handle.clone();
    let sign_in_state = widget.sign_in_state.clone();
    let request_frame = widget.request_frame.clone();
    let error = widget.error.clone();
    tokio::spawn(async move {
        let response = request_handle
            .request_typed::<LoginAccountResponse>(ClientRequest::LoginAccount {
                request_id: onboarding_request_id(),
                params: match provider {
                    DeviceCodeProvider::ChatGpt => LoginAccountParams::ChatgptDeviceCode,
                    DeviceCodeProvider::GitHubCopilot => {
                        LoginAccountParams::GitHubCopilot { client_id: None }
                    }
                },
            })
            .await;
        match (provider, response) {
            (
                DeviceCodeProvider::ChatGpt,
                Ok(LoginAccountResponse::ChatgptDeviceCode {
                    login_id,
                    verification_url,
                    user_code,
                }),
            )
            | (
                DeviceCodeProvider::GitHubCopilot,
                Ok(LoginAccountResponse::GitHubCopilot {
                    login_id,
                    verification_url,
                    user_code,
                }),
            ) => {
                let updated = set_device_code_state_for_active_attempt(
                    &sign_in_state,
                    &request_frame,
                    &request_id,
                    ContinueWithDeviceCodeState::ready(
                        request_id.clone(),
                        login_id.clone(),
                        verification_url,
                        user_code,
                    ),
                );
                if updated {
                    *error.write().unwrap() = None;
                } else {
                    cancel_login_attempt(&request_handle, login_id).await;
                }
            }
            (_, Ok(other)) => {
                let _updated = set_device_code_error_for_active_attempt(
                    &sign_in_state,
                    &request_frame,
                    &error,
                    &request_id,
                    format!("Unexpected account/login/start response: {other:?}"),
                );
            }
            (_, Err(err)) => {
                let _updated = set_device_code_error_for_active_attempt(
                    &sign_in_state,
                    &request_frame,
                    &error,
                    &request_id,
                    err.to_string(),
                );
            }
        }
    });
}

pub(super) fn render_device_code_login(
    widget: &AuthModeWidget,
    area: Rect,
    buf: &mut Buffer,
    state: &ContinueWithDeviceCodeState,
    provider: DeviceCodeProvider,
) {
    let banner = match (provider, state.is_showing_copyable_auth()) {
        (DeviceCodeProvider::ChatGpt, true) => "Finish signing in via your browser",
        (DeviceCodeProvider::ChatGpt, false) => "Preparing device code login",
        (DeviceCodeProvider::GitHubCopilot, true) => "Finish signing in with GitHub Copilot",
        (DeviceCodeProvider::GitHubCopilot, false) => "Preparing GitHub Copilot login",
    };

    let mut spans = vec!["  ".into()];
    if widget.animations_enabled && !widget.animations_suppressed.get() {
        widget
            .request_frame
            .schedule_frame_in(std::time::Duration::from_millis(100));
        spans.extend(shimmer_text(banner, MotionMode::Animated));
    } else {
        spans.push(banner.into());
    }

    let mut lines = vec![spans.into(), "".into()];

    let verification_url = if let (Some(verification_url), Some(user_code)) =
        (&state.verification_url, &state.user_code)
    {
        lines.push("  1. Open this link in your browser and sign in".into());
        lines.push("".into());
        lines.push(Line::from(vec![
            "  ".into(),
            verification_url.as_str().cyan().underlined(),
        ]));
        lines.push("".into());
        lines.push(match provider {
            DeviceCodeProvider::ChatGpt => {
                "  2. Enter this one-time code after you are signed in (expires in 15 minutes)"
                    .into()
            }
            DeviceCodeProvider::GitHubCopilot => {
                "  2. Enter this one-time code and authorize GitHub Copilot".into()
            }
        });
        lines.push("".into());
        lines.push(Line::from(vec![
            "  ".into(),
            user_code.as_str().cyan().bold(),
        ]));
        lines.push("".into());
        lines.push(
            "  Continue only if you started this login in Codex. If a website or another person gave you this code, cancel."
                .dim()
                .into(),
        );
        lines.push("".into());
        Some(verification_url.clone())
    } else {
        lines.push("  Requesting a one-time code...".dim().into());
        lines.push("".into());
        None
    };

    lines.push(Line::from(vec![
        "  Press ".dim(),
        widget.cancel_binding().into(),
        " to cancel".dim(),
    ]));
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .render(area, buf);

    if let Some(url) = &verification_url {
        mark_url_hyperlink(buf, area, url);
    }
}

fn device_code_attempt_matches(state: &SignInState, request_id: &str) -> bool {
    matches!(
        state,
        SignInState::ChatGptDeviceCode(state) | SignInState::GitHubCopilotDeviceCode(state)
            if state.request_id == request_id
    )
}

fn set_device_code_state_for_active_attempt(
    sign_in_state: &std::sync::Arc<std::sync::RwLock<SignInState>>,
    request_frame: &crate::tui::FrameRequester,
    request_id: &str,
    next_state: ContinueWithDeviceCodeState,
) -> bool {
    let mut guard = sign_in_state.write().unwrap();
    if !device_code_attempt_matches(&guard, request_id) {
        return false;
    }

    *guard = match &*guard {
        SignInState::GitHubCopilotDeviceCode(_) => DeviceCodeProvider::GitHubCopilot,
        _ => DeviceCodeProvider::ChatGpt,
    }
    .sign_in_state(next_state);
    drop(guard);
    request_frame.schedule_frame();
    true
}

pub(super) fn render_github_copilot_success_message(
    widget: &AuthModeWidget,
    area: Rect,
    buf: &mut Buffer,
) {
    let lines = vec![
        "✓ Signed in with GitHub Copilot".green().into(),
        "".into(),
        "  Codex will use your GitHub Copilot access and available models.".into(),
        "  Review the code it writes and commands it runs."
            .dim()
            .into(),
        "".into(),
        Line::from(vec![
            "  Press ".cyan(),
            widget.confirm_binding().into(),
            " to continue".cyan(),
        ]),
    ];
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .render(area, buf);
}

fn set_device_code_error_for_active_attempt(
    sign_in_state: &std::sync::Arc<std::sync::RwLock<SignInState>>,
    request_frame: &crate::tui::FrameRequester,
    error: &std::sync::Arc<std::sync::RwLock<Option<String>>>,
    request_id: &str,
    message: String,
) -> bool {
    let mut guard = sign_in_state.write().unwrap();
    if !device_code_attempt_matches(&guard, request_id) {
        return false;
    }

    *guard = SignInState::PickMode;
    drop(guard);
    *error.write().unwrap() = Some(message);
    request_frame.schedule_frame();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use std::sync::RwLock;

    fn pending_device_code_state(request_id: &str) -> Arc<RwLock<SignInState>> {
        Arc::new(RwLock::new(SignInState::ChatGptDeviceCode(
            ContinueWithDeviceCodeState::pending(request_id.to_string()),
        )))
    }

    #[test]
    fn device_code_attempt_matches_only_for_matching_request_id() {
        let state = SignInState::ChatGptDeviceCode(ContinueWithDeviceCodeState::pending(
            "request-1".to_string(),
        ));

        assert_eq!(device_code_attempt_matches(&state, "request-1"), true);
        assert_eq!(device_code_attempt_matches(&state, "request-2"), false);
        assert_eq!(
            device_code_attempt_matches(&SignInState::PickMode, "request-1"),
            false
        );
    }

    #[test]
    fn set_device_code_state_for_active_attempt_updates_only_when_active() {
        let request_frame = crate::tui::FrameRequester::test_dummy();
        let sign_in_state = pending_device_code_state("request-1");

        assert_eq!(
            set_device_code_state_for_active_attempt(
                &sign_in_state,
                &request_frame,
                "request-1",
                ContinueWithDeviceCodeState::ready(
                    "request-1".to_string(),
                    "login-1".to_string(),
                    "https://example.com/device".to_string(),
                    "ABCD-EFGH".to_string(),
                ),
            ),
            true
        );
        assert!(matches!(
            &*sign_in_state.read().unwrap(),
            SignInState::ChatGptDeviceCode(state) if state.login_id() == Some("login-1")
        ));

        let sign_in_state = pending_device_code_state("request-2");
        assert_eq!(
            set_device_code_state_for_active_attempt(
                &sign_in_state,
                &request_frame,
                "request-1",
                ContinueWithDeviceCodeState::ready(
                    "request-1".to_string(),
                    "login-1".to_string(),
                    "https://example.com/device".to_string(),
                    "ABCD-EFGH".to_string(),
                ),
            ),
            false
        );
        assert!(matches!(
            &*sign_in_state.read().unwrap(),
            SignInState::ChatGptDeviceCode(state) if state.login_id.is_none()
        ));
    }

    #[test]
    fn set_device_code_error_for_active_attempt_updates_only_when_active() {
        let request_frame = crate::tui::FrameRequester::test_dummy();
        let error = Arc::new(RwLock::new(None));
        let sign_in_state = pending_device_code_state("request-1");

        assert_eq!(
            set_device_code_error_for_active_attempt(
                &sign_in_state,
                &request_frame,
                &error,
                "request-1",
                "device code unavailable".to_string(),
            ),
            true
        );
        assert!(matches!(
            &*sign_in_state.read().unwrap(),
            SignInState::PickMode
        ));
        assert_eq!(
            error.read().unwrap().as_deref(),
            Some("device code unavailable")
        );

        let error = Arc::new(RwLock::new(None));
        let sign_in_state = pending_device_code_state("request-2");
        assert_eq!(
            set_device_code_error_for_active_attempt(
                &sign_in_state,
                &request_frame,
                &error,
                "request-1",
                "device code unavailable".to_string(),
            ),
            false
        );
        assert!(matches!(
            &*sign_in_state.read().unwrap(),
            SignInState::ChatGptDeviceCode(_)
        ));
        assert_eq!(*error.read().unwrap(), None);
    }
}
