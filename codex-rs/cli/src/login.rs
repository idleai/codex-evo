//! CLI login commands and their direct-user observability surfaces.
//!
//! The TUI path already installs a broader tracing stack with feedback, OpenTelemetry, and other
//! interactive-session layers. Direct `codex login` intentionally does less: it preserves the
//! existing stderr/browser UX and adds only a small file-backed tracing layer for login-specific
//! targets. Keeping that setup local avoids pulling the TUI's session-oriented logging machinery
//! into a one-shot CLI command while still producing a durable `codex-login.log` artifact that
//! support can request from users.

use codex_config::types::AuthCredentialsStoreMode;
use codex_core::config::Config;
use codex_core::config::edit::ConfigEdit;
use codex_core::config::edit::ConfigEditsBuilder;
use codex_login::AuthKeyringBackendKind;
use codex_login::AuthManager;
use codex_login::AuthRouteConfig;
use codex_login::CodexAuth;
use codex_login::GITHUB_COPILOT_CLIENT_ID_ENV_VAR;
use codex_login::GITHUB_COPILOT_DEFAULT_CLIENT_ID;
use codex_login::GitHubCopilotLoginOptions;
use codex_login::complete_github_copilot_device_code_login;
use codex_login::is_workload_identity_selected;
use codex_login::login_with_github_copilot;
use codex_login::logout_with_revoke;
use codex_login::request_github_copilot_device_code;
use codex_mcp::ema_auth_scope;
use codex_protocol::auth::AuthMode;
use codex_protocol::config_types::ForcedLoginMethod;
use codex_rmcp_client::EnterpriseOAuthCredentialGuard;
use codex_utils_cli::CliConfigOverrides;
use std::fs::OpenOptions;
use std::path::Path;
use tracing_appender::non_blocking;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const GITHUB_COPILOT_LOGIN_DISABLED_MESSAGE: &str =
    "GitHub Copilot login is disabled by the configured login requirements.";
const API_KEY_AUTHENTICATION_NOTICE: &str = "Authentication is checked when you send a request.";
const DEFAULT_LOGIN_LOG_FILTER: &str =
    "codex_cli=info,codex_core=info,codex_login=info,codex_otel::auth_storage=warn";

/// Installs a small file-backed tracing layer for direct `codex login` flows.
///
/// This deliberately duplicates a narrow slice of the TUI logging setup instead of reusing it
/// wholesale. The TUI stack includes session-oriented layers that are valuable for interactive
/// runs but unnecessary for a one-shot login command. Keeping the direct CLI path local lets this
/// command produce a durable `codex-login.log` artifact without coupling it to the TUI's broader
/// telemetry and feedback initialization.
fn init_login_file_logging(config: &Config) -> Option<WorkerGuard> {
    let log_dir = match codex_core::config::log_dir(config) {
        Ok(log_dir) => log_dir,
        Err(err) => {
            eprintln!("Warning: failed to resolve login log directory: {err}");
            return None;
        }
    };

    if let Err(err) = std::fs::create_dir_all(&log_dir) {
        eprintln!(
            "Warning: failed to create login log directory {}: {err}",
            log_dir.display()
        );
        return None;
    }

    let mut log_file_opts = OpenOptions::new();
    log_file_opts.create(true).append(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        log_file_opts.mode(0o600);
    }

    let log_path = log_dir.join("codex-login.log");
    let log_file = match log_file_opts.open(&log_path) {
        Ok(log_file) => log_file,
        Err(err) => {
            eprintln!(
                "Warning: failed to open login log file {}: {err}",
                log_path.display()
            );
            return None;
        }
    };

    let (non_blocking, guard) = non_blocking(log_file);
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOGIN_LOG_FILTER));
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_target(true)
        .with_ansi(false)
        .with_filter(env_filter);

    // Direct `codex login` otherwise relies on ephemeral stderr and browser output.
    // Persist the same login targets to a file so support can inspect auth failures
    // without reproducing them through TUI or app-server.
    if let Err(err) = tracing_subscriber::registry().with(file_layer).try_init() {
        eprintln!(
            "Warning: failed to initialize login log file {}: {err}",
            log_path.display()
        );
        return None;
    }

    Some(guard)
}

async fn clear_existing_auth_before_login(
    codex_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    auth_keyring_backend_kind: AuthKeyringBackendKind,
    auth_route_config: &AuthRouteConfig,
) {
    if let Err(err) = logout_with_revoke(
        codex_home,
        auth_credentials_store_mode,
        auth_keyring_backend_kind,
        auth_route_config,
    )
    .await
    {
        tracing::warn!("failed to clear existing auth before login: {err}");
    }
}

pub async fn run_login_with_github_copilot(
    cli_config_overrides: CliConfigOverrides,
    client_id: Option<String>,
) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;
    let _login_log_guard = init_login_file_logging(&config);
    tracing::info!("starting GitHub Copilot device login flow");

    if !config
        .auth_config()
        .is_login_method_allowed(ForcedLoginMethod::Api)
    {
        eprintln!("{GITHUB_COPILOT_LOGIN_DISABLED_MESSAGE}");
        std::process::exit(1);
    }

    let client_id = client_id
        .filter(|client_id| !client_id.trim().is_empty())
        .or_else(|| std::env::var(GITHUB_COPILOT_CLIENT_ID_ENV_VAR).ok())
        .filter(|client_id| !client_id.trim().is_empty())
        .unwrap_or_else(|| GITHUB_COPILOT_DEFAULT_CLIENT_ID.to_string());
    let auth_route_config = config.auth_route_config();
    let options = match GitHubCopilotLoginOptions::new(client_id, auth_route_config.clone()) {
        Ok(options) => options,
        Err(err) => {
            eprintln!("Error starting GitHub Copilot login: {err}");
            std::process::exit(1);
        }
    };
    let device_code = match request_github_copilot_device_code(&options).await {
        Ok(device_code) => device_code,
        Err(err) => {
            eprintln!("Error starting GitHub Copilot login: {err}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "Sign in to GitHub Copilot:\n\n1. Open {}\n2. Enter code {}\n\nThe code expires in {} minutes. Continue only if you started this login in Codex.",
        device_code.verification_url,
        device_code.user_code,
        device_code.expires_in.as_secs().div_ceil(60),
    );

    let auth = match complete_github_copilot_device_code_login(&options, device_code).await {
        Ok(auth) => auth,
        Err(err) => {
            eprintln!("Error logging in to GitHub Copilot: {err}");
            std::process::exit(1);
        }
    };

    clear_existing_auth_before_login(
        &config.codex_home,
        config.cli_auth_credentials_store_mode,
        config.auth_keyring_backend_kind(),
        &auth_route_config,
    )
    .await;
    let login = auth.login().map(str::to_string);
    if let Err(err) = login_with_github_copilot(
        &config.codex_home,
        auth,
        config.cli_auth_credentials_store_mode,
        config.auth_keyring_backend_kind(),
    ) {
        eprintln!("Error saving GitHub Copilot login: {err}");
        std::process::exit(1);
    }

    match login {
        Some(login) => eprintln!("Successfully logged in to GitHub Copilot as {login}"),
        None => eprintln!("Successfully logged in to GitHub Copilot"),
    }
    std::process::exit(0);
}

pub async fn run_login_status(cli_config_overrides: CliConfigOverrides) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;

    if is_workload_identity_selected() {
        match AuthManager::shared_from_config(&config, /*enable_codex_api_key_env*/ false).await {
            Ok(_) => {
                eprintln!("Logged in using workload identity");
                std::process::exit(0);
            }
            Err(err) => {
                eprintln!("Error checking login status: {err}");
                std::process::exit(1);
            }
        }
    }

    let auth_config = config.auth_config();
    match auth_config
        .load_auth(/*enable_codex_api_key_env*/ false)
        .await
    {
        Ok(Some(auth)) => match auth.auth_mode() {
            AuthMode::ApiKey => match auth.get_token() {
                Ok(api_key) => {
                    eprintln!("API key configured - {}", safe_format_key(&api_key));
                    eprintln!("{API_KEY_AUTHENTICATION_NOTICE}");
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("Unexpected error retrieving API key: {e}");
                    std::process::exit(1);
                }
            },
            AuthMode::Chatgpt | AuthMode::ChatgptAuthTokens => {
                eprintln!("Logged in using ChatGPT");
                std::process::exit(0);
            }
            AuthMode::Headers => {
                unreachable!("header auth cannot be loaded from auth storage")
            }
            AuthMode::AgentIdentity => {
                eprintln!("Logged in using access token");
                std::process::exit(0);
            }
            AuthMode::PersonalAccessToken => {
                eprintln!("Logged in using personal access token");
                std::process::exit(0);
            }
            AuthMode::BedrockApiKey => {
                eprintln!("Logged in using Amazon Bedrock API key");
                std::process::exit(0);
            }
            AuthMode::BedrockAccessKeys => {
                eprintln!("Logged in using Amazon Bedrock AWS access keys");
                std::process::exit(0);
            }
            AuthMode::GitHubCopilot => {
                let account = match &auth {
                    CodexAuth::GitHubCopilot(auth) => auth.login(),
                    _ => None,
                }
                .map(|login| format!(" as {login}"))
                .unwrap_or_default();
                eprintln!("Logged in using GitHub Copilot{account}");
                std::process::exit(0);
            }
        },
        Ok(None) => {
            eprintln!("Not logged in");
            std::process::exit(1);
        }
        Err(err) => {
            eprintln!("Error checking login status: {err}");
            std::process::exit(1);
        }
    }
}

async fn logout_config_server(config: &Config) -> std::io::Result<bool> {
    match codex_cloud_config::managed_config_auth_manager(
        config.auth_config(),
        config.config_layer_stack.requirements_toml(),
    )? {
        Some(manager) => manager.logout().await,
        None => Ok(false),
    }
}

pub async fn run_logout(cli_config_overrides: CliConfigOverrides) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;
    let auth_route_config = config.auth_route_config();
    let config_server_logout = if config
        .config_layer_stack
        .requirements_toml()
        .requires_remote_config()
    {
        logout_config_server(&config).await
    } else {
        Ok(false)
    };

    // Cleanup is independent of the current feature gate. Retain the grant lock
    // through account removal so another process cannot commit a late login.
    let enterprise_guard = async {
        let Some(profile) = &config.mcp_enterprise_managed_auth else {
            return Ok::<_, anyhow::Error>(None);
        };
        let auth = config
            .auth_config()
            .load_auth(/*enable_codex_api_key_env*/ false)
            .await?;
        let Some(scope) = ema_auth_scope(auth.as_ref()) else {
            return Ok(None);
        };
        EnterpriseOAuthCredentialGuard::acquire(
            &profile.idp.credential_name(&scope),
            &profile.idp.issuer,
            config.auth_keyring_backend_kind(),
        )
        .await
        .map(Some)
    }
    .await;
    let cleanup_failed = match &enterprise_guard {
        Ok(Some(guard)) => guard.delete_tokens().is_err(),
        Ok(None) => false,
        Err(_) => true,
    };
    if cleanup_failed {
        eprintln!("Warning: failed to remove enterprise authorization; continuing account logout");
    }

    let logged_out = match logout_with_revoke(
        &config.codex_home,
        config.cli_auth_credentials_store_mode,
        config.auth_keyring_backend_kind(),
        &auth_route_config,
    )
    .await
    {
        Ok(logged_out) => logged_out,
        Err(err) => {
            eprintln!("Error logging out: {err}");
            std::process::exit(1);
        }
    };
    drop(enterprise_guard);

    let cleared_bedrock_config =
        if let Some(paths) = ConfigEditsBuilder::bedrock_provider_config_paths_to_clear(&config) {
            let edits = paths
                .into_iter()
                .map(|segments| ConfigEdit::ClearPath { segments });
            if let Err(err) = ConfigEditsBuilder::for_config(&config)
                .with_edits(edits)
                .apply()
                .await
            {
                eprintln!("Error clearing Amazon Bedrock configuration after logout: {err}");
                std::process::exit(1);
            }
            true
        } else {
            false
        };

    let config_server_logged_out = match config_server_logout {
        Ok(removed) => removed,
        Err(error) => {
            eprintln!("Error logging out: {error}");
            std::process::exit(1);
        }
    };
    if logged_out || config_server_logged_out || cleared_bedrock_config {
        eprintln!("Successfully logged out");
    } else {
        eprintln!("Not logged in");
    }
    std::process::exit(0);
}

async fn load_config_or_exit(cli_config_overrides: CliConfigOverrides) -> Config {
    let cli_overrides = match cli_config_overrides.parse_overrides() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error parsing -c overrides: {e}");
            std::process::exit(1);
        }
    };

    match Config::load_with_cli_overrides(cli_overrides).await {
        Ok(config) => match config.auth_config().validate() {
            Ok(()) => config,
            Err(e) => {
                eprintln!("Error loading configuration: {e}");
                std::process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("Error loading configuration: {e}");
            std::process::exit(1);
        }
    }
}

fn safe_format_key(key: &str) -> String {
    if key.len() <= 13 {
        return "***".to_string();
    }
    let prefix = &key[..8];
    let suffix = &key[key.len() - 5..];
    format!("{prefix}***{suffix}")
}

#[cfg(test)]
#[path = "login_storage_tests.rs"]
mod storage_tests;

#[cfg(test)]
mod tests {
    use codex_config::types::AuthCredentialsStoreMode;
    use codex_login::AuthKeyringBackendKind;
    use codex_login::GitHubCopilotAuth;
    use codex_login::load_auth_dot_json;
    use codex_login::login_with_github_copilot;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    use super::clear_existing_auth_before_login;
    use super::safe_format_key;

    #[tokio::test]
    async fn clears_existing_auth_before_login() {
        let codex_home = tempdir().expect("create temporary Codex home");
        login_with_github_copilot(
            codex_home.path(),
            GitHubCopilotAuth::new(
                "github-existing".into(),
                "https://api.individual.githubcopilot.com".into(),
                Some("octocat".into()),
                None,
                vec!["gpt-5.6-sol".into()],
            )
            .expect("valid Copilot auth"),
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .expect("save existing auth");

        clear_existing_auth_before_login(
            codex_home.path(),
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
            &codex_login::test_support::transport_default_auth_route_config(),
        )
        .await;

        let auth = load_auth_dot_json(
            codex_home.path(),
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .expect("load auth after cleanup");
        assert_eq!(auth, None);
    }

    #[test]
    fn formats_long_key() {
        let key = "sk-proj-1234567890ABCDE";
        assert_eq!(safe_format_key(key), "sk-proj-***ABCDE");
    }

    #[test]
    fn short_key_returns_stars() {
        let key = "sk-proj-12345";
        assert_eq!(safe_format_key(key), "***");
    }
}
