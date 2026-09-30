#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StatusAccountDisplay {
    ChatGpt {
        email: Option<String>,
        plan: Option<String>,
    },
    ApiKey,
    GitHubCopilot {
        login: Option<String>,
        copilot_sku: Option<String>,
    },
}
