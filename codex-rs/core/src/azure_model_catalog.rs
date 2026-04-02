use crate::ModelProviderInfo;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::ModelsResponse;
use codex_utils_azure_catalog::resolve_openai_deployment_model_names_for_base_url;
use std::collections::HashSet;
use tracing::warn;

pub(crate) fn catalog_for_provider(
    configured_catalog: Option<ModelsResponse>,
    provider: &ModelProviderInfo,
) -> Option<ModelsResponse> {
    if configured_catalog.is_some() {
        return configured_catalog;
    }

    if !provider.is_azure() {
        return None;
    }

    match resolve_azure_catalog(provider) {
        Ok(catalog) => Some(catalog),
        Err(err) => {
            warn!(
                provider = %provider.name,
                error = %err,
                "failed to resolve Azure model catalog; using bundled defaults"
            );
            None
        }
    }
}

fn resolve_azure_catalog(provider: &ModelProviderInfo) -> Result<ModelsResponse, String> {
    let base_url = provider
        .base_url
        .as_deref()
        .ok_or_else(|| "Azure provider is missing a base_url".to_string())?;
    let deployed_models = resolve_openai_deployment_model_names_for_base_url(base_url)?;
    let bundled_catalog = bundled_catalog()?;

    Ok(apply_azure_availability(bundled_catalog, &deployed_models))
}

fn bundled_catalog() -> Result<ModelsResponse, String> {
    serde_json::from_str(include_str!("../models.json"))
        .map_err(|err| format!("failed to parse bundled models.json: {err}"))
}

fn apply_azure_availability(
    mut catalog: ModelsResponse,
    available_models: &HashSet<String>,
) -> ModelsResponse {
    for model in &mut catalog.models {
        if available_models.contains(&model.slug.to_ascii_lowercase()) {
            continue;
        }

        model.visibility = ModelVisibility::None;
        model.supported_in_api = false;
    }

    catalog
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::config_types::ReasoningSummary;
    use codex_protocol::config_types::Verbosity;
    use codex_protocol::openai_models::ConfigShellToolType;
    use codex_protocol::openai_models::InputModality;
    use codex_protocol::openai_models::ReasoningEffort;
    use codex_protocol::openai_models::ReasoningEffortPreset;
    use codex_protocol::openai_models::TruncationPolicyConfig;
    use codex_protocol::openai_models::WebSearchToolType;
    use pretty_assertions::assert_eq;

    fn test_catalog() -> ModelsResponse {
        ModelsResponse {
            models: vec![test_model("gpt-5.4"), test_model("gpt-5.3-codex")],
        }
    }

    fn test_model(slug: &str) -> codex_protocol::openai_models::ModelInfo {
        codex_protocol::openai_models::ModelInfo {
            slug: slug.to_string(),
            display_name: slug.to_string(),
            description: Some(slug.to_string()),
            default_reasoning_level: Some(ReasoningEffort::Medium),
            supported_reasoning_levels: vec![ReasoningEffortPreset {
                effort: ReasoningEffort::Medium,
                description: "default".to_string(),
            }],
            shell_type: ConfigShellToolType::ShellCommand,
            visibility: ModelVisibility::List,
            supported_in_api: true,
            priority: 0,
            availability_nux: None,
            upgrade: None,
            base_instructions: "instructions".to_string(),
            model_messages: None,
            supports_reasoning_summaries: true,
            default_reasoning_summary: ReasoningSummary::None,
            support_verbosity: true,
            default_verbosity: Some(Verbosity::Low),
            apply_patch_tool_type: None,
            web_search_tool_type: WebSearchToolType::Text,
            truncation_policy: TruncationPolicyConfig::tokens(1000),
            supports_parallel_tool_calls: true,
            supports_image_detail_original: false,
            context_window: Some(128000),
            auto_compact_token_limit: Some(64000),
            effective_context_window_percent: 95,
            experimental_supported_tools: Vec::new(),
            input_modalities: vec![InputModality::Text],
            used_fallback_model_metadata: false,
            supports_search_tool: false,
        }
    }

    #[test]
    fn unavailable_models_are_hidden_and_removed_from_api_support() {
        let available_models = ["gpt-5.4".to_string(), "irrelevant-model".to_string()]
            .into_iter()
            .collect::<HashSet<_>>();
        let filtered = apply_azure_availability(test_catalog(), &available_models);

        assert_eq!(filtered.models[0].slug, "gpt-5.4");
        assert_eq!(filtered.models[0].visibility, ModelVisibility::List);
        assert!(filtered.models[0].supported_in_api);

        assert_eq!(filtered.models[1].slug, "gpt-5.3-codex");
        assert_eq!(filtered.models[1].visibility, ModelVisibility::None);
        assert!(!filtered.models[1].supported_in_api);
        assert_eq!(filtered.models[1].base_instructions, "instructions");
    }
}
