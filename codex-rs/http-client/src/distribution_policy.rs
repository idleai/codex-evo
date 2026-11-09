//! Non-configurable service boundaries for the Copilot distribution.
//!
//! These checks also apply to unmanaged clients and redirects. They are independent
//! of account state, workspace requirements, and user configuration.

use crate::NetworkPolicyDenied;
use url::Url;

pub const OPENAI_SERVICES_ENABLED: bool = false;
pub const OPENAI_DISABLED_MESSAGE: &str = "OpenAI and ChatGPT services are disabled in this build. Use GitHub Copilot or a configured local provider.";

pub fn ensure_openai_services_enabled() -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        OPENAI_DISABLED_MESSAGE,
    ))
}

pub fn ensure_service_url_allowed(url: &Url) -> Result<(), NetworkPolicyDenied> {
    let Some(host) = url.host_str() else {
        return Ok(());
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if [
        "openai.com",
        "openai.org",
        "chatgpt.com",
        "chatgpt-staging.com",
        "oaistatic.com",
        "oaiusercontent.com",
    ]
    .iter()
    .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
    {
        return Err(NetworkPolicyDenied::Destination);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DestinationPolicy;
    use crate::NetworkPolicy;
    use crate::NetworkPolicyController;

    #[test]
    fn openai_is_denied_even_by_unmanaged_and_explicitly_unrestricted_clients() {
        let controller = NetworkPolicyController::default();
        let managed = controller.policy();
        controller.publish(managed.revision(), DestinationPolicy::Unrestricted);
        for policy in [NetworkPolicy::unmanaged(), managed] {
            for endpoint in [
                "https://api.openai.com/v1/responses",
                "https://auth.openai.com/oauth/token",
                "https://auth.api.openai.org/oauth/token",
                "wss://gov.chatgpt.com/backend-api/codex/responses",
                "http://CHATGPT.COM./backend-api",
                "https://chatgpt-staging.com/remote/control",
                "https://persistent.oaistatic.com/codex/install.ps1",
                "https://files.oaiusercontent.com/asset",
            ] {
                assert!(
                    matches!(
                        policy.acquire(&Url::parse(endpoint).unwrap()),
                        Err(NetworkPolicyDenied::Destination)
                    ),
                    "{endpoint}"
                );
            }
        }
    }

    #[test]
    fn copilot_and_local_providers_remain_available() {
        for endpoint in [
            "https://github.com/login/device/code",
            "https://api.github.com/user",
            "https://api.individual.githubcopilot.com/responses",
            "http://127.0.0.1:8000/v1/responses",
            "http://[::1]:8000/v1/models",
            "http://localhost:8000/v1/responses",
            "https://api.openai.com.example.org/v1",
        ] {
            assert!(
                NetworkPolicy::unmanaged()
                    .acquire(&Url::parse(endpoint).unwrap())
                    .is_ok()
            );
        }
    }
}
