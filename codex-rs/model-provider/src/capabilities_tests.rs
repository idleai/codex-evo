use super::*;
use crate::create_model_provider;
use codex_model_provider_info::ModelProviderCapabilities;
use pretty_assertions::assert_eq;

#[test]
fn capability_overrides_preserve_unspecified_provider_defaults() {
    for info in [
        ModelProviderInfo::default(),
        ModelProviderInfo::create_openai_provider(/*base_url*/ None),
        ModelProviderInfo {
            name: "Azure".to_string(),
            ..Default::default()
        },
    ] {
        let defaults = create_model_provider(info.clone(), /*auth_manager*/ None).capabilities();
        let provider = create_model_provider(
            ModelProviderInfo {
                capabilities: Some(ModelProviderCapabilities {
                    external_web_access: Some(false),
                    ..Default::default()
                }),
                ..info
            },
            /*auth_manager*/ None,
        );
        assert_eq!(
            provider.capabilities(),
            ProviderCapabilities {
                external_web_access: false,
                ..defaults
            },
        );
    }
}

#[test]
fn response_compatibility_controls_coexist_with_capability_overrides() {
    let provider = create_model_provider(
        ModelProviderInfo {
            supports_namespace_tools: Some(false),
            supports_codex_agent_messages: Some(false),
            capabilities: Some(ModelProviderCapabilities {
                external_web_access: Some(false),
                remote_compaction: Some(RemoteCompactionSupport::V2),
            }),
            ..Default::default()
        },
        /*auth_manager*/ None,
    );

    assert_eq!(
        provider.capabilities(),
        ProviderCapabilities {
            namespace_tools: false,
            codex_agent_messages: false,
            external_web_access: false,
            remote_compaction: RemoteCompactionSupport::V2,
            ..Default::default()
        },
    );
}
