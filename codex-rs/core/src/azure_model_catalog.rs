use crate::ModelProviderInfo;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::ModelsResponse;
use serde::Deserialize;
use std::collections::HashSet;
use std::process::Command;
use tracing::warn;
use url::Url;

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct AzureAccountSummary {
    name: String,
    #[serde(rename = "resourceGroup")]
    resource_group: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct AzureModelSummary {
    name: String,
}

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
    let resource = infer_azure_resource_name(provider)?;
    let resource_group = find_azure_resource_group(resource.as_str())?;
    let available_models = list_azure_models(resource.as_str(), resource_group.as_str())?;
    let bundled_catalog = bundled_catalog()?;

    Ok(apply_azure_availability(bundled_catalog, &available_models))
}

fn bundled_catalog() -> Result<ModelsResponse, String> {
    serde_json::from_str(include_str!("../models.json"))
        .map_err(|err| format!("failed to parse bundled models.json: {err}"))
}

fn infer_azure_resource_name(provider: &ModelProviderInfo) -> Result<String, String> {
    let base_url = provider
        .base_url
        .as_deref()
        .ok_or_else(|| "Azure provider is missing a base_url".to_string())?;
    resource_name_from_base_url(base_url)
}

fn resource_name_from_base_url(base_url: &str) -> Result<String, String> {
    let parsed = Url::parse(base_url)
        .map_err(|err| format!("invalid Azure base_url `{base_url}`: {err}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| format!("Azure base_url `{base_url}` is missing a host"))?;
    let lower_host = host.to_ascii_lowercase();
    let inferable_host = lower_host.contains("openai.azure.")
        || lower_host.contains("cognitiveservices.azure.")
        || lower_host.contains("aoai.azure.")
        || lower_host.contains("windows.net");
    if !inferable_host {
        return Err(format!(
            "Azure base_url `{base_url}` does not expose a resource name that Codex can resolve automatically"
        ));
    }

    host.split('.')
        .next()
        .filter(|segment| !segment.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| format!("failed to infer Azure resource name from `{base_url}`"))
}

fn find_azure_resource_group(resource: &str) -> Result<String, String> {
    let output = run_azure_cli(["cognitiveservices", "account", "list", "--output", "json"])?;
    let accounts: Vec<AzureAccountSummary> = serde_json::from_slice(&output)
        .map_err(|err| format!("failed to parse Azure resource list JSON: {err}"))?;

    let matches: Vec<&AzureAccountSummary> = accounts
        .iter()
        .filter(|account| account.name.eq_ignore_ascii_case(resource))
        .collect();

    match matches.as_slice() {
        [] => Err(format!("Azure resource `{resource}` was not found")),
        [account] => Ok(account.resource_group.clone()),
        _ => Err(format!(
            "found multiple Azure resources named `{resource}`; select one with `model_catalog_json` instead"
        )),
    }
}

fn list_azure_models(resource: &str, resource_group: &str) -> Result<HashSet<String>, String> {
    let output = run_azure_cli([
        "cognitiveservices",
        "account",
        "list-models",
        "-n",
        resource,
        "-g",
        resource_group,
        "--output",
        "json",
    ])?;
    let models: Vec<AzureModelSummary> = serde_json::from_slice(&output)
        .map_err(|err| format!("failed to parse Azure model list JSON: {err}"))?;

    Ok(models
        .into_iter()
        .map(|model| model.name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect())
}

fn run_azure_cli<const N: usize>(args: [&str; N]) -> Result<Vec<u8>, String> {
    let output = Command::new("az")
        .args(args)
        .output()
        .map_err(|err| format!("failed to run `az {}`: {err}", args.join(" ")))?;
    if output.status.success() {
        return Ok(output.stdout);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let detail = if stderr.is_empty() {
        format!("Azure CLI exited with status {}", output.status)
    } else {
        stderr
    };
    Err(format!("`az {}` failed: {detail}", args.join(" ")))
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
    use crate::WireApi;
    use codex_protocol::config_types::ReasoningSummary;
    use codex_protocol::config_types::Verbosity;
    use codex_protocol::openai_models::ConfigShellToolType;
    use codex_protocol::openai_models::InputModality;
    use codex_protocol::openai_models::ReasoningEffort;
    use codex_protocol::openai_models::ReasoningEffortPreset;
    use codex_protocol::openai_models::TruncationPolicyConfig;
    use codex_protocol::openai_models::WebSearchToolType;
    use pretty_assertions::assert_eq;

    fn azure_provider(base_url: &str) -> ModelProviderInfo {
        ModelProviderInfo {
            name: "azure".into(),
            base_url: Some(base_url.into()),
            env_key: Some("AZURE_OPENAI_API_KEY".into()),
            env_key_instructions: None,
            experimental_bearer_token: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
        }
    }

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
    fn resource_name_is_inferred_from_cognitiveservices_url() {
        let resource = infer_azure_resource_name(&azure_provider(
            "https://iuvobot.cognitiveservices.azure.com/openai",
        ))
        .expect("resource should be inferred");

        assert_eq!(resource, "iuvobot");
    }

    #[test]
    fn resource_name_is_inferred_from_openai_azure_url() {
        let resource =
            infer_azure_resource_name(&azure_provider("https://iuvobot.openai.azure.com/openai"))
                .expect("resource should be inferred");

        assert_eq!(resource, "iuvobot");
    }

    #[test]
    fn custom_azure_front_door_url_is_not_auto_inferable() {
        let err = infer_azure_resource_name(&azure_provider("https://foo.z01.azurefd.net/openai"))
            .expect_err("resource inference should fail");

        assert_eq!(
            err,
            "Azure base_url `https://foo.z01.azurefd.net/openai` does not expose a resource name that Codex can resolve automatically"
        );
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
