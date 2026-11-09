use super::*;
use serde_json::json;

#[tokio::test]
async fn copilot_distribution_ignores_saved_openai_credentials_without_modifying_them() {
    for mode in [
        AuthMode::ApiKey,
        AuthMode::Chatgpt,
        AuthMode::ChatgptAuthTokens,
        AuthMode::PersonalAccessToken,
        AuthMode::AgentIdentity,
        AuthMode::Headers,
    ] {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("auth.json");
        let data = serde_json::to_vec(&json!({
            "auth_mode": mode,
            "OPENAI_API_KEY": "unused-legacy-secret",
        }))
        .unwrap();
        std::fs::write(&path, &data).unwrap();
        let auth = load_auth(
            home.path(),
            true,
            AuthCredentialsStoreMode::File,
            None,
            None,
            None,
            AuthKeyringBackendKind::default(),
            None,
            &crate::test_support::transport_default_auth_route_config(),
            None,
            None,
        )
        .await
        .unwrap();
        assert!(auth.is_none(), "{mode:?}");
        assert_eq!(std::fs::read(path).unwrap(), data);
    }
}

#[tokio::test]
async fn copilot_distribution_rejects_openai_login_without_replacing_copilot_auth() {
    let home = tempfile::tempdir().unwrap();
    let auth = GitHubCopilotAuth::new(
        "github-test-secret".into(),
        "https://api.individual.githubcopilot.com".into(),
        Some("octocat".into()),
        None,
        vec!["gpt-5.6-sol".into()],
    )
    .unwrap();
    login_with_github_copilot(
        home.path(),
        auth,
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )
    .unwrap();
    let before = std::fs::read(home.path().join("auth.json")).unwrap();
    let route = crate::test_support::transport_default_auth_route_config();
    let results = [
        login_with_api_key(
            home.path(),
            "openai-test-secret",
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        ),
        login_with_access_token(
            home.path(),
            "openai-test-secret",
            AuthCredentialsStoreMode::File,
            None,
            None,
            AuthKeyringBackendKind::default(),
            &route,
        )
        .await,
        login_with_chatgpt_auth_tokens(home.path(), "openai-test-secret", "account", None),
    ];
    for result in results {
        let error = result.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(!error.to_string().contains("openai-test-secret"));
    }
    assert_eq!(
        std::fs::read(home.path().join("auth.json")).unwrap(),
        before
    );
}

#[tokio::test]
async fn copilot_distribution_disables_browser_device_and_refresh_even_with_local_overrides() {
    let home = tempfile::tempdir().unwrap();
    let route = crate::test_support::transport_default_auth_route_config();
    let server = wiremock::MockServer::start().await;
    let mut opts = crate::ServerOptions::new(
        home.path().to_path_buf(),
        "test-client".into(),
        None,
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
        route,
    );
    opts.issuer = server.uri();
    opts.open_browser = false;
    assert!(crate::run_login_server(opts.clone()).is_err());
    assert!(crate::request_device_code(&opts).await.is_err());
    assert!(crate::run_device_code_login(opts).await.is_err());
    let client = codex_http_client::HttpClientBuilder::new()
        .build_direct()
        .unwrap();
    let result = request_chatgpt_token_refresh(
        "unused-refresh-secret".into(),
        &server.uri(),
        "test-client",
        &client,
        RefreshReason::UnauthorizedRecovery,
    )
    .await;
    assert!(result.is_err());
    assert!(server.received_requests().await.unwrap().is_empty());
}
