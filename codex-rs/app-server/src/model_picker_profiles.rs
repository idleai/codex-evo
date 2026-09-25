//! Opt-in picker entries backed by complete, host-owned user profiles.

use super::ConfigManager;
use codex_core::config::Config;
use codex_core::config::ConfigOverrides;
use codex_core::config::resolve_profile_v2_config_path;
use codex_protocol::openai_models::ModelPreset;
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::io;

pub(crate) fn configured_picker_profiles(config: &Config) -> io::Result<BTreeMap<String, String>> {
    let profiles: BTreeMap<String, String> = config
        .config_layer_stack
        .effective_config()
        .get("model_picker_profiles")
        .cloned()
        .map(toml::Value::try_into)
        .transpose()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?
        .unwrap_or_default();
    if profiles.len() > 8 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "model_picker_profiles supports at most 8 additional models",
        ));
    }
    Ok(profiles)
}

/// A picker entry and the profile that owns its provider and model configuration.
pub(crate) struct PickerProfile {
    pub(crate) model: ModelPreset,
    pub(crate) config: Config,
    manager: ConfigManager,
}

impl PickerProfile {
    pub(crate) fn apply(
        self,
        request_overrides: &mut Option<HashMap<String, Value>>,
        typesafe_overrides: &mut ConfigOverrides,
    ) -> ConfigManager {
        // Clients can echo the startup provider alongside a model selection. The explicit
        // host mapping owns the entire route; do not combine it with those stale settings.
        if let Some(overrides) = request_overrides.as_mut() {
            for key in [
                "model_provider",
                "model_catalog_json",
                "model_instructions_file",
                "model_context_window",
                "model_auto_compact_token_limit",
            ] {
                overrides.remove(key);
            }
            if let Some(effort) = overrides.get("model_reasoning_effort")
                && !self
                    .model
                    .supported_reasoning_efforts
                    .iter()
                    .any(|supported| {
                        serde_json::to_value(&supported.effort).ok().as_ref() == Some(effort)
                    })
            {
                overrides.remove("model_reasoning_effort");
            }
        }
        typesafe_overrides.model_provider = Some(self.config.model_provider_id);
        let requested_tier = typesafe_overrides
            .service_tier
            .take()
            .unwrap_or(self.config.service_tier);
        typesafe_overrides.service_tier = Some(requested_tier.filter(|tier| {
            tier == "default"
                || self
                    .model
                    .service_tiers
                    .iter()
                    .any(|supported| &supported.id == tier)
        }));
        self.manager
    }
}

impl ConfigManager {
    pub(crate) async fn picker_profile_for_request(
        &self,
        request_overrides: Option<&HashMap<String, Value>>,
        typesafe_overrides: &ConfigOverrides,
    ) -> io::Result<Option<PickerProfile>> {
        let model = typesafe_overrides.model.as_deref().or_else(|| {
            request_overrides
                .and_then(|overrides| overrides.get("model"))
                .and_then(Value::as_str)
        });
        let Some(model) = model else {
            return Ok(None);
        };
        let base = self.load_non_project_config().await?;
        let profiles = configured_picker_profiles(&base)?;
        match profiles.get(model) {
            Some(profile) => self.load_picker_profile(model, profile).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn load_picker_profile(
        &self,
        model: &str,
        profile: &str,
    ) -> io::Result<PickerProfile> {
        let profile = profile
            .parse()
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
        let mut manager = self.clone();
        manager.loader_overrides.user_config_path =
            Some(resolve_profile_v2_config_path(self.codex_home(), &profile));
        manager.loader_overrides.user_config_profile = Some(profile);
        let config = manager.load_non_project_config().await?;
        let effective = config.config_layer_stack.effective_config();
        if effective
            .get("model_provider")
            .and_then(toml::Value::as_str)
            != Some(config.model_provider_id.as_str())
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("picker model `{model}` cannot use its configured profile provider"),
            ));
        }
        self.check_thread_model_provider(&config).await?;
        let mut preset: ModelPreset = config
            .model_catalog
            .as_ref()
            .and_then(|catalog| catalog.models.iter().find(|info| info.slug == model))
            .cloned()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "picker model `{model}` must be present in its profile's model_catalog_json"
                    ),
                )
            })?
            .into();
        preset.show_in_picker = true;
        preset.is_default = false;
        Ok(PickerProfile {
            model: preset,
            config,
            manager,
        })
    }

    pub(crate) async fn check_picker_model_selection(
        &self,
        current: &Config,
        model: &str,
    ) -> io::Result<()> {
        if configured_picker_profiles(current)?.is_empty() {
            return Ok(());
        }
        let base = self.load_non_project_config().await?;
        let profiles = configured_picker_profiles(&base)?;
        if profiles.is_empty() {
            return Ok(());
        }
        let target_provider = match profiles.get(model) {
            Some(profile) => {
                self.load_picker_profile(model, profile)
                    .await?
                    .config
                    .model_provider_id
            }
            None if current
                .model
                .as_ref()
                .is_some_and(|model| profiles.contains_key(model)) =>
            {
                base.model_provider_id
            }
            None => return Ok(()),
        };
        if target_provider != current.model_provider_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "Start a new chat with `{model}` to use its provider. Existing chats keep their provider; use a portable profile handoff to continue elsewhere."
                ),
            ));
        }
        Ok(())
    }
}
