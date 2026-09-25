use crate::config::Config;
use crate::config::ConfigOverrides;
use crate::config::deserialize_config_toml_with_base;
use crate::config::resolve_profile_v2_config_path;
use codex_config::ConfigLayerEntry;
use codex_config::ConfigLayerSource;
use codex_config::ConfigLayerStack;
use codex_config::loader::resolve_relative_paths_in_config_toml;
use codex_exec_server::LOCAL_FS;
use codex_exec_server::read_sensitive_file_to_string;
use toml::Value as TomlValue;

/// Applies the model-routing portion of a named user profile to a spawned child.
///
/// Runtime authority and tool configuration remain inherited from the parent. Model provider
/// definitions must already exist in the parent's base configuration so evicted children can be
/// resumed without reloading the spawn-only profile.
pub(crate) async fn apply_spawn_agent_profile(
    config: &mut Config,
    profile: &str,
) -> Result<(), String> {
    let selected = load_spawn_agent_profile(config, profile)
        .await
        .map_err(|err| format!("spawn_agent could not load profile `{profile}`: {err}"))?;

    let configured_provider = config
        .model_providers
        .get(&selected.model_provider_id)
        .ok_or_else(|| {
            format!(
                "spawn_agent profile `{profile}` selects model provider `{}`, which must be declared in the base user configuration",
                selected.model_provider_id
            )
        })?;
    if configured_provider != &selected.model_provider {
        return Err(format!(
            "spawn_agent profile `{profile}` overrides model provider `{}`; provider definitions must live in the base user configuration",
            selected.model_provider_id
        ));
    }

    config.model = selected.model;
    config.service_tier = selected.service_tier;
    config.model_context_window = selected.model_context_window;
    config.model_auto_compact_token_limit = selected.model_auto_compact_token_limit;
    config.model_auto_compact_token_limit_scope = selected.model_auto_compact_token_limit_scope;
    config.model_provider_id = selected.model_provider_id;
    config.model_provider = selected.model_provider;
    config.personality = selected.personality;
    config.base_instructions = selected.base_instructions;
    config.base_instructions_provenance = selected.base_instructions_provenance;
    config.compact_prompt = selected.compact_prompt;
    config.tool_output_token_limit = selected.tool_output_token_limit;
    config.model_reasoning_effort = selected.model_reasoning_effort;
    config.model_reasoning_summary = selected.model_reasoning_summary;
    config.model_catalog = selected.model_catalog;
    config.model_verbosity = selected.model_verbosity;
    config.respect_system_proxy = selected.respect_system_proxy;
    config.responses_api_metadata = selected.responses_api_metadata;

    Ok(())
}

async fn load_spawn_agent_profile(config: &Config, profile: &str) -> std::io::Result<Config> {
    let profile_name = profile
        .parse()
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    let profile_path = resolve_profile_v2_config_path(&config.codex_home, &profile_name);
    let profile_contents = read_sensitive_file_to_string(&profile_path).await?;
    let profile_toml: TomlValue = toml::from_str(&profile_contents)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let profile_base = profile_path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "spawn_agent profile path has no parent directory",
        )
    })?;
    let profile_toml = resolve_relative_paths_in_config_toml(profile_toml, &profile_base)?;
    let profile_layer = ConfigLayerEntry::new(ConfigLayerSource::SessionFlags, profile_toml);
    let mut layers = config
        .config_layer_stack
        .all_layers_low_to_high()
        .cloned()
        .collect::<Vec<_>>();
    let insertion_index = layers.partition_point(|layer| layer.name <= profile_layer.name);
    layers.insert(insertion_index, profile_layer);
    let config_layer_stack = ConfigLayerStack::new(
        layers,
        config.config_layer_stack.requirements().clone(),
        config.config_layer_stack.requirements_toml().clone(),
    )?
    .with_user_and_project_exec_policy_rules_ignored(
        config
            .config_layer_stack
            .ignore_user_and_project_exec_policy_rules(),
    );
    let config_toml = deserialize_config_toml_with_base(
        config_layer_stack.effective_config(),
        &config.codex_home,
    )?;

    Config::load_config_with_layer_stack(
        LOCAL_FS.as_ref(),
        config_toml,
        ConfigOverrides {
            cwd: Some(config.cwd.to_path_buf()),
            ..Default::default()
        },
        config.codex_home.clone(),
        config_layer_stack,
    )
    .await
}
