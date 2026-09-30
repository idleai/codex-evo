use super::*;
use codex_app_server_client::RemoteAppServerClient;
use codex_app_server_client::RemoteAppServerConnectArgs;
use codex_app_server_client::RemoteAppServerEndpoint;
use codex_app_server_protocol::JSONRPCMessage;
use futures::SinkExt;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;

fn render(widget: &AuthModeWidget, width: u16) -> String {
    let area = Rect::new(0, 0, width, 24);
    let mut buffer = Buffer::empty(area);
    widget.render_ref(area, &mut buffer);
    (area.top()..area.bottom())
        .map(|row| {
            let text = (area.left()..area.right())
                .map(|column| buffer[(column, row)].symbol())
                .collect::<String>();
            crate::terminal_hyperlinks::strip_osc8(&text)
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

async fn wait_until(predicate: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(/*secs*/ 5), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("login state transition");
}

#[tokio::test]
async fn github_copilot_menu_navigation_respects_login_policy() {
    let (mut widget, _tmp) = super::tests::widget_forced_chatgpt().await;
    widget.handle_sign_in_option(SignInOption::GitHubCopilot);
    assert!(matches!(
        &*widget.sign_in_state.read().unwrap(),
        SignInState::PickMode
    ));
    assert!(
        !widget
            .selectable_sign_in_options()
            .contains(&SignInOption::GitHubCopilot)
    );

    widget.auth_config.forced_login_method = None;
    widget.animations_enabled = false;
    for _ in 0..3 {
        widget.handle_key_event(KeyCode::Down.into());
    }
    assert_eq!(widget.highlighted_mode, SignInOption::GitHubCopilot);
    insta::assert_snapshot!("github_copilot_menu", render(&widget, 80));
    widget.handle_key_event(KeyCode::Down.into());
    assert_eq!(widget.highlighted_mode, SignInOption::ChatGpt);
    widget.auth_config.forced_login_method = Some(ForcedLoginMethod::Api);
    assert!(
        widget
            .selectable_sign_in_options()
            .contains(&SignInOption::GitHubCopilot)
    );
    widget.login_status = LoginStatus::AuthMode(AuthMode::GitHubCopilot);
    widget.handle_key_event(KeyCode::Char('3').into());
    assert_eq!(widget.get_step_state(), StepState::Complete);
}

#[tokio::test]
async fn github_copilot_rpc_handles_success_cancellation_and_start_errors() {
    for scenario in ["success", "cancel", "cancel_pending", "start_error"] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let websocket_url = format!("ws://{}", listener.local_addr().unwrap());
        let (requested_tx, requested_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let (cancelled_tx, cancelled_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut requested_tx = Some(requested_tx);
            let mut release_rx = Some(release_rx);
            let mut cancelled_tx = Some(cancelled_tx);
            while let Some(Ok(Message::Text(text))) = socket.next().await {
                let JSONRPCMessage::Request(request) = serde_json::from_str(&text).unwrap() else {
                    continue;
                };
                let mut response = match request.method.as_str() {
                    "initialize" => json!({"result": {"userAgent": "github-login-test"}}),
                    "account/login/start" => {
                        assert_eq!(
                            request.params,
                            Some(json!({
                                "type": "githubCopilot", "clientId": null
                            }))
                        );
                        requested_tx.take().unwrap().send(()).unwrap();
                        release_rx.take().unwrap().await.unwrap();
                        if scenario == "start_error" {
                            json!({"error": {"code": -32603, "message": "GitHub is unavailable"}})
                        } else {
                            json!({"result": {
                                "type": "githubCopilot", "loginId": "login-1",
                                "verificationUrl": "https://github.com/login/device",
                                "userCode": "ABCD-EFGH"
                            }})
                        }
                    }
                    "account/login/cancel" => {
                        assert_eq!(request.params, Some(json!({"loginId": "login-1"})));
                        cancelled_tx.take().unwrap().send(()).unwrap();
                        json!({"result": {"status": "canceled"}})
                    }
                    method => panic!("unexpected request: {method}"),
                };
                response["id"] = json!(request.id);
                socket
                    .send(Message::Text(response.to_string().into()))
                    .await
                    .unwrap();
            }
        });
        let client = RemoteAppServerClient::connect(RemoteAppServerConnectArgs {
            endpoint: RemoteAppServerEndpoint::WebSocket {
                websocket_url,
                auth_token: None,
            },
            client_name: "github-login-test".to_string(),
            client_version: "0.0.0".to_string(),
            experimental_api: true,
            mcp_server_openai_form_elicitation: false,
            opt_out_notification_methods: Vec::new(),
            channel_capacity: 8,
        })
        .await
        .unwrap();
        let (mut widget, _tmp) = super::tests::widget_forced_chatgpt().await;
        widget.app_server_request_handle = AppServerRequestHandle::Remote(client.request_handle());
        widget.auth_config.forced_login_method = None;
        widget.animations_enabled = false;
        widget.handle_key_event(KeyCode::Char('4').into());
        assert_eq!(widget.highlighted_mode, SignInOption::GitHubCopilot);
        assert!(widget.should_suppress_animations());
        tokio::time::timeout(Duration::from_secs(/*secs*/ 5), requested_rx)
            .await
            .unwrap()
            .unwrap();
        if scenario == "cancel_pending" {
            widget.handle_key_event(KeyCode::Esc.into());
        }
        if scenario == "success" {
            insta::assert_snapshot!("github_copilot_pending", render(&widget, 80));
        }
        release_tx.send(()).unwrap();
        if scenario == "start_error" {
            wait_until(|| widget.error_message().is_some()).await;
            assert!(
                widget
                    .error_message()
                    .unwrap()
                    .contains("GitHub is unavailable")
            );
            assert_eq!(widget.get_step_state(), StepState::InProgress);
        } else if scenario != "cancel_pending" {
            wait_until(|| matches!(
                &*widget.sign_in_state.read().unwrap(),
                SignInState::GitHubCopilotDeviceCode(state) if state.login_id() == Some("login-1")
            )).await;
            widget.handle_key_event(KeyCode::Char('1').into());
            assert!(matches!(
                &*widget.sign_in_state.read().unwrap(),
                SignInState::GitHubCopilotDeviceCode(_)
            ));
            if scenario == "success" {
                insta::assert_snapshot!("github_copilot_device_code", render(&widget, 80));
                insta::assert_snapshot!("github_copilot_device_code_narrow", render(&widget, 44));
                widget.on_account_login_completed(AccountLoginCompletedNotification {
                    login_id: Some("login-1".to_string()),
                    success: true,
                    error: None,
                    onboarding_entrypoint: None,
                });
                assert_eq!(widget.get_step_state(), StepState::InProgress);
                insta::assert_snapshot!("github_copilot_success", render(&widget, 80));
                widget.on_account_updated(AccountUpdatedNotification {
                    auth_mode: Some(ApiAuthMode::GitHubCopilot),
                    plan_type: None,
                });
                widget.handle_key_event(KeyCode::Enter.into());
                assert_eq!(widget.get_step_state(), StepState::Complete);
                assert_eq!(
                    widget.login_status,
                    LoginStatus::AuthMode(AuthMode::GitHubCopilot)
                );
            } else {
                widget.handle_key_event(KeyCode::Esc.into());
            }
        }
        if matches!(scenario, "cancel" | "cancel_pending") {
            tokio::time::timeout(Duration::from_secs(/*secs*/ 5), cancelled_rx)
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                &*widget.sign_in_state.read().unwrap(),
                SignInState::PickMode
            ));
            assert_eq!(widget.error_message(), None);
        }
        client.shutdown().await.unwrap();
        server.await.unwrap();
    }
}

#[tokio::test]
async fn github_copilot_completion_errors_allow_retry_and_ignore_stale_notifications() {
    let (mut widget, _tmp) = super::tests::widget_forced_chatgpt().await;
    widget.auth_config.forced_login_method = None;
    widget.animations_enabled = false;
    for error in [
        "GitHub device authorization expired before sign-in completed",
        "GitHub device authorization was denied",
        "failed to save GitHub Copilot authentication",
    ] {
        *widget.sign_in_state.write().unwrap() =
            SignInState::GitHubCopilotDeviceCode(ContinueWithDeviceCodeState::ready(
                "request-1".to_string(),
                "login-1".to_string(),
                "https://github.com/login/device".to_string(),
                "ABCD-EFGH".to_string(),
            ));
        let mut completion = AccountLoginCompletedNotification {
            login_id: Some("old-login".to_string()),
            success: true,
            error: None,
            onboarding_entrypoint: None,
        };
        widget.on_account_login_completed(completion.clone());
        assert!(matches!(
            &*widget.sign_in_state.read().unwrap(),
            SignInState::GitHubCopilotDeviceCode(_)
        ));
        completion.login_id = Some("login-1".to_string());
        completion.success = false;
        completion.error = Some(error.to_string());
        widget.on_account_login_completed(completion.clone());
        assert_eq!(widget.error_message().as_deref(), Some(error));
        assert!(matches!(
            &*widget.sign_in_state.read().unwrap(),
            SignInState::PickMode
        ));
        completion.success = true;
        completion.error = None;
        widget.on_account_login_completed(completion);
        assert!(matches!(
            &*widget.sign_in_state.read().unwrap(),
            SignInState::PickMode
        ));
    }
    insta::assert_snapshot!("github_copilot_error", render(&widget, 80));
}
