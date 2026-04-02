use crate::ModelProviderInfo;
use crate::error::CodexErr;
use crate::error::EnvVarError;
use crate::error::Result;
use codex_utils_azure_catalog::run_azure_cli_json;
use serde::Deserialize;

const AZURE_COGNITIVE_SERVICES_RESOURCE: &str = "https://cognitiveservices.azure.com";
const DEFAULT_AZURE_ENV_KEY: &str = "AZURE_OPENAI_API_KEY";

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct AzureCliAccessTokenResponse {
    #[serde(rename = "accessToken")]
    access_token: String,
}

pub(crate) fn azure_cli_bearer_token(provider: &ModelProviderInfo) -> Result<Option<String>> {
    if !provider.is_azure() {
        return Ok(None);
    }

    let response: AzureCliAccessTokenResponse = run_azure_cli_json(
        [
            "account",
            "get-access-token",
            "--resource",
            AZURE_COGNITIVE_SERVICES_RESOURCE,
            "--output",
            "json",
        ],
        "Azure access token response",
    )
    .map_err(|detail| azure_cli_auth_error(provider, detail))?;

    access_token_from_response(response)
        .map(Some)
        .map_err(|detail| azure_cli_auth_error(provider, detail))
}

fn access_token_from_response(
    response: AzureCliAccessTokenResponse,
) -> std::result::Result<String, String> {
    let access_token = response.access_token.trim();
    if access_token.is_empty() {
        return Err("Azure CLI returned an empty access token".to_string());
    }
    Ok(access_token.to_string())
}

fn azure_cli_auth_error(provider: &ModelProviderInfo, detail: String) -> CodexErr {
    let var = provider
        .env_key
        .clone()
        .unwrap_or_else(|| DEFAULT_AZURE_ENV_KEY.to_string());
    let instructions = format!(
        "Set `{var}` or sign in with Azure CLI via `az login` so Codex can obtain a Microsoft Entra access token for Azure OpenAI automatically. {detail}"
    );
    CodexErr::EnvVar(EnvVarError {
        var,
        instructions: Some(instructions),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn azure_provider() -> ModelProviderInfo {
        ModelProviderInfo {
            name: "azure".into(),
            base_url: Some("https://example.cognitiveservices.azure.com/openai/v1".into()),
            env_key: Some("AZURE_OPENAI_API_KEY".into()),
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: None,
            wire_api: crate::WireApi::Responses,
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

    #[test]
    fn parses_access_token_from_azure_cli_output() {
        let parsed = access_token_from_response(AzureCliAccessTokenResponse {
            access_token: "azure-token".to_string(),
        })
        .expect("should parse access token");
        assert_eq!(parsed, "azure-token");
    }

    #[test]
    fn azure_cli_auth_error_points_to_env_var_and_az_login() {
        let error = azure_cli_auth_error(&azure_provider(), "Azure CLI failed".to_string());
        assert_eq!(
            error.to_string(),
            "Missing environment variable: `AZURE_OPENAI_API_KEY`. Set `AZURE_OPENAI_API_KEY` or sign in with Azure CLI via `az login` so Codex can obtain a Microsoft Entra access token for Azure OpenAI automatically. Azure CLI failed"
        );
    }
}
