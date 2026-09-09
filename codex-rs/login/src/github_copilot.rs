use std::time::Duration;
use std::time::Instant;

use codex_http_client::HttpClient;
use http::StatusCode;
use serde::Deserialize;

use crate::AuthRouteConfig;
use crate::GitHubCopilotAuth;
use crate::default_client::create_raw_auth_client;

pub const GITHUB_COPILOT_CLIENT_ID_ENV_VAR: &str = "GITHUB_COPILOT_CLIENT_ID";
/// Public OAuth application ID used by LiteLLM for GitHub Copilot device authorization.
///
/// This is an application identifier, not a client secret. Distribution-specific builds can
/// override it at the CLI or through [`GITHUB_COPILOT_CLIENT_ID_ENV_VAR`].
pub const GITHUB_COPILOT_DEFAULT_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

const GITHUB_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const GITHUB_ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const GITHUB_COPILOT_ENTITLEMENT_URL: &str = "https://api.github.com/copilot_internal/user";
const GITHUB_USER_API_VERSION: &str = "2025-04-01";
const GITHUB_COPILOT_API_VERSION: &str = "2026-08-01";
const DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;
const SLOW_DOWN_INCREMENT_SECS: u64 = 5;

#[derive(Clone, Debug)]
pub struct GitHubCopilotLoginOptions {
    client_id: String,
    auth_route_config: AuthRouteConfig,
    device_code_url: String,
    access_token_url: String,
    entitlement_url: String,
    models_url_override: Option<String>,
}

impl GitHubCopilotLoginOptions {
    pub fn new(client_id: String, auth_route_config: AuthRouteConfig) -> std::io::Result<Self> {
        let client_id = client_id.trim().to_string();
        if client_id.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "GitHub OAuth client ID must not be empty",
            ));
        }
        Ok(Self {
            client_id,
            auth_route_config,
            device_code_url: GITHUB_DEVICE_CODE_URL.to_string(),
            access_token_url: GITHUB_ACCESS_TOKEN_URL.to_string(),
            entitlement_url: GITHUB_COPILOT_ENTITLEMENT_URL.to_string(),
            models_url_override: None,
        })
    }

    #[cfg(test)]
    fn with_test_endpoints(
        mut self,
        device_code_url: String,
        access_token_url: String,
        entitlement_url: String,
        models_url: String,
    ) -> Self {
        self.device_code_url = device_code_url;
        self.access_token_url = access_token_url;
        self.entitlement_url = entitlement_url;
        self.models_url_override = Some(models_url);
        self
    }
}

#[derive(Clone)]
pub struct GitHubCopilotDeviceCode {
    pub verification_url: String,
    pub user_code: String,
    pub expires_in: Duration,
    device_code: String,
    interval: Duration,
    requested_at: Instant,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    #[serde(alias = "verification_url")]
    verification_uri: String,
    expires_in: u64,
    #[serde(default = "default_poll_interval_secs")]
    interval: u64,
}

#[derive(Debug, Deserialize)]
struct AccessTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CopilotEntitlementResponse {
    login: Option<String>,
    access_type_sku: Option<String>,
    endpoints: CopilotEndpoints,
}

#[derive(Debug, Deserialize)]
struct CopilotEndpoints {
    api: String,
}

#[derive(Debug, Deserialize)]
struct CopilotModelsResponse {
    data: Vec<CopilotModel>,
}

#[derive(Debug, Deserialize)]
struct CopilotModel {
    id: String,
    vendor: Option<String>,
    is_chat_default: Option<bool>,
    model_picker_enabled: Option<bool>,
    capabilities: Option<CopilotModelCapabilities>,
    supported_endpoints: Option<Vec<String>>,
    policy: Option<CopilotModelPolicy>,
}

#[derive(Debug, Deserialize)]
struct CopilotModelCapabilities {
    supported_endpoints: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct CopilotModelPolicy {
    state: Option<String>,
}

fn default_poll_interval_secs() -> u64 {
    DEFAULT_POLL_INTERVAL_SECS
}

pub async fn request_github_copilot_device_code(
    options: &GitHubCopilotLoginOptions,
) -> std::io::Result<GitHubCopilotDeviceCode> {
    let client = auth_client(&options.device_code_url, &options.auth_route_config)?;
    let body = form_body(&[
        ("client_id", options.client_id.as_str()),
        ("scope", "read:user"),
    ]);
    let response = client
        .post(&options.device_code_url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("User-Agent", github_user_agent())
        .body(body)
        .send()
        .await
        .map_err(std::io::Error::other)?;
    let status = response.status();
    if !status.is_success() {
        return Err(http_error("GitHub device authorization request", status));
    }
    let response = response
        .json::<DeviceCodeResponse>()
        .await
        .map_err(|err| invalid_response("GitHub device authorization", err))?;
    if response.device_code.trim().is_empty()
        || response.user_code.trim().is_empty()
        || response.verification_uri.trim().is_empty()
        || response.expires_in == 0
    {
        return Err(invalid_response_message(
            "GitHub device authorization response is missing a required field",
        ));
    }

    Ok(GitHubCopilotDeviceCode {
        verification_url: response.verification_uri,
        user_code: response.user_code,
        expires_in: Duration::from_secs(response.expires_in),
        device_code: response.device_code,
        interval: Duration::from_secs(response.interval.max(1)),
        requested_at: Instant::now(),
    })
}

pub async fn complete_github_copilot_device_code_login(
    options: &GitHubCopilotLoginOptions,
    device_code: GitHubCopilotDeviceCode,
) -> std::io::Result<GitHubCopilotAuth> {
    let access_token = poll_for_access_token(options, &device_code).await?;
    let entitlement = discover_copilot_entitlement(options, &access_token).await?;
    validate_copilot_api_endpoint(&entitlement.endpoints.api)?;
    let models =
        fetch_openai_responses_models(options, &access_token, entitlement.endpoints.api.as_str())
            .await?;
    GitHubCopilotAuth::new(
        access_token,
        entitlement.endpoints.api,
        entitlement.login,
        entitlement.access_type_sku,
        models,
    )
}

async fn poll_for_access_token(
    options: &GitHubCopilotLoginOptions,
    device_code: &GitHubCopilotDeviceCode,
) -> std::io::Result<String> {
    let client = auth_client(&options.access_token_url, &options.auth_route_config)?;
    let mut interval = device_code.interval;
    loop {
        let elapsed = device_code.requested_at.elapsed();
        if elapsed >= device_code.expires_in {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "GitHub device authorization expired before sign-in completed",
            ));
        }
        tokio::time::sleep(interval.min(device_code.expires_in - elapsed)).await;

        let body = form_body(&[
            ("client_id", options.client_id.as_str()),
            ("device_code", device_code.device_code.as_str()),
            ("grant_type", DEVICE_CODE_GRANT_TYPE),
        ]);
        let response = client
            .post(&options.access_token_url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("User-Agent", github_user_agent())
            .body(body)
            .send()
            .await
            .map_err(std::io::Error::other)?;
        let status = response.status();
        if !status.is_success() {
            return Err(http_error("GitHub device token request", status));
        }
        let response = response
            .json::<AccessTokenResponse>()
            .await
            .map_err(|err| invalid_response("GitHub device token", err))?;
        if let Some(access_token) = response
            .access_token
            .filter(|access_token| !access_token.trim().is_empty())
        {
            return Ok(access_token);
        }

        match response.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += Duration::from_secs(SLOW_DOWN_INCREMENT_SECS),
            Some("access_denied") => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "GitHub device authorization was denied",
                ));
            }
            Some("expired_token") => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "GitHub device authorization expired before sign-in completed",
                ));
            }
            Some(error) => {
                let description = response
                    .error_description
                    .as_deref()
                    .filter(|description| !description.trim().is_empty())
                    .unwrap_or("no description provided");
                return Err(std::io::Error::other(format!(
                    "GitHub device token request failed: {error} ({description})"
                )));
            }
            None => {
                return Err(invalid_response_message(
                    "GitHub device token response contained neither a token nor an error",
                ));
            }
        }
    }
}

async fn discover_copilot_entitlement(
    options: &GitHubCopilotLoginOptions,
    access_token: &str,
) -> std::io::Result<CopilotEntitlementResponse> {
    let client = auth_client(&options.entitlement_url, &options.auth_route_config)?;
    let response = client
        .get(&options.entitlement_url)
        .header("Accept", "application/vnd.github+json")
        .header("Authorization", format!("Bearer {access_token}"))
        .header("X-GitHub-Api-Version", GITHUB_USER_API_VERSION)
        .header("User-Agent", github_user_agent())
        .send()
        .await
        .map_err(std::io::Error::other)?;
    let status = response.status();
    if !status.is_success() {
        let message = if status == StatusCode::NOT_FOUND || status == StatusCode::FORBIDDEN {
            "GitHub account does not have an active Copilot entitlement".to_string()
        } else {
            format!("GitHub Copilot entitlement request failed with status {status}")
        };
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            message,
        ));
    }
    let entitlement = response
        .json::<CopilotEntitlementResponse>()
        .await
        .map_err(|err| invalid_response("GitHub Copilot entitlement", err))?;
    if entitlement.endpoints.api.trim().is_empty() {
        return Err(invalid_response_message(
            "GitHub Copilot entitlement response is missing the API endpoint",
        ));
    }
    Ok(entitlement)
}

async fn fetch_openai_responses_models(
    options: &GitHubCopilotLoginOptions,
    access_token: &str,
    api_endpoint: &str,
) -> std::io::Result<Vec<String>> {
    let models_url = options
        .models_url_override
        .clone()
        .unwrap_or_else(|| format!("{}/models", api_endpoint.trim_end_matches('/')));
    let client = auth_client(&models_url, &options.auth_route_config)?;
    let response = client
        .get(&models_url)
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {access_token}"))
        .header("OpenAI-Intent", "conversation")
        .header("X-GitHub-Api-Version", GITHUB_COPILOT_API_VERSION)
        .header("User-Agent", github_user_agent())
        .send()
        .await
        .map_err(std::io::Error::other)?;
    let status = response.status();
    if !status.is_success() {
        return Err(http_error("GitHub Copilot model catalog request", status));
    }
    let catalog = response
        .json::<CopilotModelsResponse>()
        .await
        .map_err(|err| invalid_response("GitHub Copilot model catalog", err))?;
    let models = openai_responses_model_ids(catalog.data);
    if models.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "GitHub Copilot did not advertise an enabled OpenAI model with Responses API support",
        ));
    }
    Ok(models)
}

fn openai_responses_model_ids(models: Vec<CopilotModel>) -> Vec<String> {
    let mut models = models
        .into_iter()
        .filter(|model| {
            model
                .vendor
                .as_deref()
                .is_some_and(|vendor| vendor.eq_ignore_ascii_case("openai"))
                && model.model_picker_enabled.unwrap_or(true)
                && !model
                    .policy
                    .as_ref()
                    .and_then(|policy| policy.state.as_deref())
                    .is_some_and(|state| state.eq_ignore_ascii_case("disabled"))
                && model.supports_responses()
        })
        .collect::<Vec<_>>();
    models.sort_by_key(|model| !model.is_chat_default.unwrap_or(false));
    models.into_iter().map(|model| model.id).collect()
}

impl CopilotModel {
    fn supports_responses(&self) -> bool {
        self.supported_endpoints
            .as_ref()
            .or_else(|| {
                self.capabilities
                    .as_ref()
                    .and_then(|capabilities| capabilities.supported_endpoints.as_ref())
            })
            .is_some_and(|endpoints| {
                endpoints.iter().any(|endpoint| {
                    matches!(
                        endpoint.trim_end_matches('/'),
                        "/responses" | "/v1/responses"
                    )
                })
            })
    }
}

fn validate_copilot_api_endpoint(api_endpoint: &str) -> std::io::Result<()> {
    let auth = GitHubCopilotAuth::new(
        "validation-token".to_string(),
        api_endpoint.to_string(),
        None,
        None,
        vec!["validation-model".to_string()],
    )?;
    auth.validate_api_endpoint().map(|_| ())
}

fn auth_client(endpoint: &str, auth_route_config: &AuthRouteConfig) -> std::io::Result<HttpClient> {
    create_raw_auth_client(endpoint, auth_route_config).map_err(std::io::Error::from)
}

fn form_body(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(name, value)| {
            format!(
                "{}={}",
                urlencoding::encode(name),
                urlencoding::encode(value)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn github_user_agent() -> String {
    format!("codex/{}", env!("CARGO_PKG_VERSION"))
}

fn http_error(operation: &str, status: StatusCode) -> std::io::Error {
    std::io::Error::other(format!("{operation} failed with status {status}"))
}

fn invalid_response(operation: &str, error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("invalid {operation} response: {error}"),
    )
}

fn invalid_response_message(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
#[path = "github_copilot_tests.rs"]
mod tests;
