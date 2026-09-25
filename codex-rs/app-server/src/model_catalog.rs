//! Gates access to the retained startup model catalog on current managed provider requirements.

use std::sync::Arc;

use codex_core::config::Config;
use codex_models_manager::manager::RefreshStrategy;
use codex_models_manager::manager::SharedModelsManager;
use codex_protocol::openai_models::ModelPreset;

use crate::config_manager::ConfigManager;

/// Checks the retained catalog route before model/list and background refreshes.
pub(crate) struct ModelCatalog {
    config_manager: ConfigManager,
    config: Arc<Config>,
    models_manager: SharedModelsManager,
}

impl ModelCatalog {
    pub(crate) fn new(
        config_manager: ConfigManager,
        config: Arc<Config>,
        models_manager: SharedModelsManager,
    ) -> Self {
        Self {
            config_manager,
            config,
            models_manager,
        }
    }

    pub(crate) async fn list_models(
        &self,
        refresh_strategy: RefreshStrategy,
    ) -> std::io::Result<Vec<ModelPreset>> {
        // Check before consulting even a warm cache, just as turn admission checks
        // the retained session route before using it.
        self.config_manager
            .check_thread_model_provider(&self.config)
            .await?;
        let mut models = self
            .models_manager
            .list_models(refresh_strategy, self.config.http_client_factory())
            .await;
        if crate::config_manager::configured_picker_profiles(&self.config)?.is_empty() {
            return Ok(models);
        }
        let picker_config = self.config_manager.load_non_project_config().await?;
        for (model, profile) in crate::config_manager::configured_picker_profiles(&picker_config)? {
            let model = self
                .config_manager
                .load_picker_profile(&model, &profile)
                .await?
                .model;
            if models.iter().any(|existing| existing.model == model.model) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "picker profile model `{}` conflicts with the startup catalog",
                        model.model
                    ),
                ));
            }
            models.push(model);
        }
        Ok(models)
    }
}
