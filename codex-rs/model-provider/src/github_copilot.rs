use std::path::PathBuf;
use std::sync::Arc;

use codex_api::Provider;
use codex_api::SharedAuthProvider;
use codex_http_client::HttpClientFactory;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_login::GitHubCopilotAuth;
use codex_model_provider_info::ModelProviderInfo;
use codex_models_manager::bundled_models_response;
use codex_models_manager::cache::ModelsCache;
use codex_models_manager::manager::ModelsManager;
use codex_models_manager::manager::ModelsManagerFuture;
use codex_models_manager::manager::RefreshStrategy;
use codex_models_manager::manager::SharedModelsManager;
use codex_models_manager::manager::StaticModelsManager;
use codex_models_manager::model_info::model_info_from_slug;
use codex_protocol::account::ProviderAccount;
use codex_protocol::config_types::CollaborationModeMask;
use codex_protocol::error::CodexErr;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::ModelsResponse;
use tokio::sync::TryLockError;

use crate::auth::resolve_provider_auth;
use crate::provider::ModelProvider;
use crate::provider::ModelProviderFuture;
use crate::provider::ProviderAccountResult;
use crate::provider::ProviderAccountState;
use crate::provider::ProviderCapabilities;
use crate::provider::RemoteCompactionSupport;
use crate::provider::SharedModelProvider;

const AUTH_CHANGED_MESSAGE: &str =
    "GitHub Copilot authentication changed; start a new Codex session before retrying";

#[derive(Debug)]
pub(crate) struct GitHubCopilotModelProvider {
    info: ModelProviderInfo,
    auth_manager: Arc<AuthManager>,
    expected_auth: GitHubCopilotAuth,
    fallback_provider: SharedModelProvider,
}

impl GitHubCopilotModelProvider {
    pub(crate) fn new(
        auth_manager: Arc<AuthManager>,
        expected_auth: GitHubCopilotAuth,
        fallback_provider: SharedModelProvider,
    ) -> Self {
        let info = ModelProviderInfo::create_github_copilot_provider(Some(
            expected_auth.api_endpoint().to_string(),
        ));
        Self {
            info,
            auth_manager,
            expected_auth,
            fallback_provider,
        }
    }

    fn current_auth(&self) -> codex_protocol::error::Result<CodexAuth> {
        let Some(CodexAuth::GitHubCopilot(auth)) = self.auth_manager.auth_cached() else {
            return Err(CodexErr::UnsupportedOperation(
                AUTH_CHANGED_MESSAGE.to_string(),
            ));
        };
        if auth != self.expected_auth {
            return Err(CodexErr::UnsupportedOperation(
                AUTH_CHANGED_MESSAGE.to_string(),
            ));
        }
        auth.validate().map_err(CodexErr::from)?;
        Ok(CodexAuth::GitHubCopilot(auth))
    }
}

impl ModelProvider for GitHubCopilotModelProvider {
    fn info(&self) -> &ModelProviderInfo {
        &self.info
    }

    fn capabilities(&self) -> ProviderCapabilities {
        if self.current_auth().is_err() {
            return ProviderCapabilities {
                namespace_tools: false,
                image_generation: false,
                web_search: false,
                external_web_access: false,
                remote_compaction: RemoteCompactionSupport::Unsupported,
            };
        }
        ProviderCapabilities {
            namespace_tools: true,
            image_generation: false,
            web_search: false,
            external_web_access: false,
            remote_compaction: RemoteCompactionSupport::Unsupported,
        }
    }

    fn approval_review_preferred_model(&self) -> &str {
        self.expected_auth.default_model()
    }

    fn memory_extraction_preferred_model(&self) -> &str {
        self.expected_auth.default_model()
    }

    fn memory_consolidation_preferred_model(&self) -> &str {
        self.expected_auth.default_model()
    }

    fn validate_model(&self, model: &str) -> codex_protocol::error::Result<()> {
        self.current_auth()?;
        if self
            .expected_auth
            .models()
            .iter()
            .any(|available| available == model)
        {
            Ok(())
        } else {
            Err(CodexErr::InvalidRequest(format!(
                "model `{model}` is not an enabled OpenAI Responses model for the signed-in GitHub Copilot account"
            )))
        }
    }

    fn auth_manager(&self) -> Option<Arc<AuthManager>> {
        Some(Arc::clone(&self.auth_manager))
    }

    fn auth(&self) -> ModelProviderFuture<'_, Option<CodexAuth>> {
        Box::pin(async move { self.current_auth().ok() })
    }

    fn account_state(&self) -> ProviderAccountResult {
        let account = self
            .current_auth()
            .ok()
            .map(|_| ProviderAccount::GitHubCopilot {
                login: self.expected_auth.login().map(str::to_string),
                copilot_sku: self.expected_auth.copilot_sku().map(str::to_string),
            });
        Ok(ProviderAccountState {
            account,
            requires_openai_auth: true,
        })
    }

    fn api_provider(&self) -> ModelProviderFuture<'_, codex_protocol::error::Result<Provider>> {
        Box::pin(async move {
            self.current_auth()?;
            self.info
                .to_api_provider(Some(codex_protocol::auth::AuthMode::GitHubCopilot))
        })
    }

    fn runtime_base_url(
        &self,
    ) -> ModelProviderFuture<'_, codex_protocol::error::Result<Option<String>>> {
        Box::pin(async move {
            let auth = self.current_auth()?;
            let CodexAuth::GitHubCopilot(auth) = auth else {
                unreachable!("current_auth only returns GitHub Copilot auth")
            };
            Ok(Some(auth.api_endpoint().to_string()))
        })
    }

    fn api_auth(
        &self,
    ) -> ModelProviderFuture<'_, codex_protocol::error::Result<SharedAuthProvider>> {
        Box::pin(async move {
            let auth = self.current_auth()?;
            resolve_provider_auth(Some(&auth), &self.info)
        })
    }

    fn models_manager(
        &self,
        codex_home: PathBuf,
        config_model_catalog: Option<ModelsResponse>,
    ) -> SharedModelsManager {
        self.fallback_provider
            .models_manager(codex_home, config_model_catalog)
    }

    fn models_manager_without_cache(
        &self,
        config_model_catalog: Option<ModelsResponse>,
    ) -> SharedModelsManager {
        self.fallback_provider
            .models_manager_without_cache(config_model_catalog)
    }

    fn models_manager_with_cache(
        &self,
        config_model_catalog: Option<ModelsResponse>,
        cache: Arc<dyn ModelsCache>,
    ) -> SharedModelsManager {
        self.fallback_provider
            .models_manager_with_cache(config_model_catalog, cache)
    }
}

/// Wraps a provider's normal model catalog so the process-wide GitHub Copilot
/// authentication state remains authoritative across live account switches.
pub(crate) fn github_copilot_aware_models_manager(
    inner: SharedModelsManager,
    auth_manager: Option<Arc<AuthManager>>,
) -> SharedModelsManager {
    match auth_manager {
        Some(auth_manager) => {
            let fallback_to_copilot_default_on_auth_switch = !auth_manager
                .auth_cached()
                .is_some_and(|auth| auth.is_github_copilot_auth());
            Arc::new(GitHubCopilotAwareModelsManager {
                inner,
                auth_manager,
                fallback_to_copilot_default_on_auth_switch,
            })
        }
        None => inner,
    }
}

#[derive(Debug)]
struct GitHubCopilotAwareModelsManager {
    inner: SharedModelsManager,
    auth_manager: Arc<AuthManager>,
    fallback_to_copilot_default_on_auth_switch: bool,
}

impl GitHubCopilotAwareModelsManager {
    fn github_copilot_catalog(&self) -> Option<ModelsResponse> {
        let CodexAuth::GitHubCopilot(auth) = self.auth_manager.auth_cached()? else {
            return None;
        };
        Some(model_catalog(&auth))
    }
}

impl ModelsManager for GitHubCopilotAwareModelsManager {
    fn get_default_model<'a>(
        &'a self,
        model: &'a Option<String>,
        allow_provider_model_fallback: bool,
        refresh_strategy: RefreshStrategy,
        http_client_factory: HttpClientFactory,
    ) -> ModelsManagerFuture<'a, String> {
        let Some(catalog) = self.github_copilot_catalog() else {
            return self.inner.get_default_model(
                model,
                allow_provider_model_fallback,
                refresh_strategy,
                http_client_factory,
            );
        };
        let manager = StaticModelsManager::new(Some(Arc::clone(&self.auth_manager)), catalog);
        let allow_provider_model_fallback =
            allow_provider_model_fallback || self.fallback_to_copilot_default_on_auth_switch;
        Box::pin(async move {
            manager
                .get_default_model(
                    model,
                    allow_provider_model_fallback,
                    refresh_strategy,
                    http_client_factory,
                )
                .await
        })
    }

    fn raw_model_catalog(
        &self,
        refresh_strategy: RefreshStrategy,
        http_client_factory: HttpClientFactory,
    ) -> ModelsManagerFuture<'_, ModelsResponse> {
        match self.github_copilot_catalog() {
            Some(catalog) => Box::pin(async move { catalog }),
            None => self
                .inner
                .raw_model_catalog(refresh_strategy, http_client_factory),
        }
    }

    fn get_remote_models(&self) -> ModelsManagerFuture<'_, Vec<ModelInfo>> {
        match self.github_copilot_catalog() {
            Some(catalog) => Box::pin(async move { catalog.models }),
            None => self.inner.get_remote_models(),
        }
    }

    fn try_get_remote_models(&self) -> Result<Vec<ModelInfo>, TryLockError> {
        match self.github_copilot_catalog() {
            Some(catalog) => Ok(catalog.models),
            None => self.inner.try_get_remote_models(),
        }
    }

    fn auth_manager(&self) -> Option<&AuthManager> {
        if self.github_copilot_catalog().is_some() {
            Some(&self.auth_manager)
        } else {
            self.inner.auth_manager()
        }
    }

    fn list_collaboration_modes(&self) -> Vec<CollaborationModeMask> {
        self.inner.list_collaboration_modes()
    }

    fn refresh_if_new_etag(
        &self,
        etag: String,
        http_client_factory: HttpClientFactory,
    ) -> ModelsManagerFuture<'_, ()> {
        if self.github_copilot_catalog().is_some() {
            Box::pin(async {})
        } else {
            self.inner.refresh_if_new_etag(etag, http_client_factory)
        }
    }
}

pub(crate) fn model_catalog(auth: &GitHubCopilotAuth) -> ModelsResponse {
    let bundled = bundled_models_response().unwrap_or_default();
    let models = auth
        .models()
        .iter()
        .enumerate()
        .map(|(priority, slug)| {
            let mut model = bundled
                .models
                .iter()
                .find(|model| model.slug == *slug)
                .cloned()
                .unwrap_or_else(|| model_info_from_slug(slug));
            model.priority = i32::try_from(priority).unwrap_or(i32::MAX);
            model.visibility = ModelVisibility::List;
            model.supported_in_api = true;
            model.additional_speed_tiers.clear();
            model.service_tiers.clear();
            model.default_service_tier = None;
            model.availability_nux = None;
            model.upgrade = None;
            model.use_responses_lite = false;
            model
        })
        .collect::<Vec<ModelInfo>>();
    ModelsResponse { models }
}
