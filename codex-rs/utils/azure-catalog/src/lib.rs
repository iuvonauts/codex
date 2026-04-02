use serde::Deserialize;
use std::collections::HashSet;
use std::process::Command;
use url::Url;

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct AzureAccountSummary {
    name: String,
    #[serde(rename = "resourceGroup")]
    resource_group: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct AzureDeploymentSummary {
    properties: AzureDeploymentProperties,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct AzureDeploymentProperties {
    model: AzureDeploymentModel,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct AzureDeploymentModel {
    format: String,
    name: String,
}

pub fn resolve_openai_deployment_model_names_for_base_url(
    base_url: &str,
) -> Result<HashSet<String>, String> {
    let resource = resource_name_from_base_url(base_url)?;
    let resource_group = find_azure_resource_group(resource.as_str())?;
    list_azure_openai_deployments(resource.as_str(), resource_group.as_str())
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

    find_resource_group(resource, &accounts)
}

fn find_resource_group(resource: &str, accounts: &[AzureAccountSummary]) -> Result<String, String> {
    let matches: Vec<&AzureAccountSummary> = accounts
        .iter()
        .filter(|account| account.name.eq_ignore_ascii_case(resource))
        .collect();

    match matches.as_slice() {
        [] => Err(format!("Azure resource `{resource}` was not found")),
        [account] => Ok(account.resource_group.clone()),
        _ => Err(format!(
            "found multiple Azure resources named `{resource}`; select one manually instead"
        )),
    }
}

fn list_azure_openai_deployments(
    resource: &str,
    resource_group: &str,
) -> Result<HashSet<String>, String> {
    let output = run_azure_cli([
        "cognitiveservices",
        "account",
        "deployment",
        "list",
        "-n",
        resource,
        "-g",
        resource_group,
        "--output",
        "json",
    ])?;
    let deployments: Vec<AzureDeploymentSummary> = serde_json::from_slice(&output)
        .map_err(|err| format!("failed to parse Azure deployment list JSON: {err}"))?;

    Ok(openai_model_names(deployments))
}

fn openai_model_names(deployments: Vec<AzureDeploymentSummary>) -> HashSet<String> {
    deployments
        .into_iter()
        .filter(|deployment| {
            deployment
                .properties
                .model
                .format
                .eq_ignore_ascii_case("OpenAI")
        })
        .map(|deployment| deployment.properties.model.name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn resource_name_is_inferred_from_cognitiveservices_url() {
        let resource =
            resource_name_from_base_url("https://example-resource.cognitiveservices.azure.com/openai")
                .expect("resource should be inferred");

        assert_eq!(resource, "example-resource");
    }

    #[test]
    fn resource_name_is_inferred_from_openai_azure_url() {
        let resource =
            resource_name_from_base_url("https://example-resource.openai.azure.com/openai")
                .expect("resource should be inferred");

        assert_eq!(resource, "example-resource");
    }

    #[test]
    fn custom_azure_front_door_url_is_not_auto_inferable() {
        let err = resource_name_from_base_url("https://foo.z01.azurefd.net/openai")
            .expect_err("resource inference should fail");

        assert_eq!(
            err,
            "Azure base_url `https://foo.z01.azurefd.net/openai` does not expose a resource name that Codex can resolve automatically"
        );
    }

    #[test]
    fn find_resource_group_matches_case_insensitively() {
        let resource_group = find_resource_group(
            "example-resource",
            &[AzureAccountSummary {
                name: "Example-Resource".to_string(),
                resource_group: "rg-agents".to_string(),
            }],
        )
        .expect("resource group should be found");

        assert_eq!(resource_group, "rg-agents");
    }

    #[test]
    fn find_resource_group_rejects_duplicates() {
        let err = find_resource_group(
            "example-resource",
            &[
                AzureAccountSummary {
                    name: "example-resource".to_string(),
                    resource_group: "rg-a".to_string(),
                },
                AzureAccountSummary {
                    name: "Example-Resource".to_string(),
                    resource_group: "rg-b".to_string(),
                },
            ],
        )
        .expect_err("duplicate resources should fail");

        assert_eq!(
            err,
            "found multiple Azure resources named `example-resource`; select one manually instead"
        );
    }

    #[test]
    fn openai_model_names_only_keeps_openai_deployments() {
        let deployments = vec![
            AzureDeploymentSummary {
                properties: AzureDeploymentProperties {
                    model: AzureDeploymentModel {
                        format: "OpenAI".to_string(),
                        name: "gpt-5.4".to_string(),
                    },
                },
            },
            AzureDeploymentSummary {
                properties: AzureDeploymentProperties {
                    model: AzureDeploymentModel {
                        format: "Anthropic".to_string(),
                        name: "claude-sonnet-4-6".to_string(),
                    },
                },
            },
            AzureDeploymentSummary {
                properties: AzureDeploymentProperties {
                    model: AzureDeploymentModel {
                        format: "openai".to_string(),
                        name: " gpt-5.3-codex ".to_string(),
                    },
                },
            },
        ];

        let models = openai_model_names(deployments);

        assert_eq!(
            models,
            HashSet::from(["gpt-5.4".to_string(), "gpt-5.3-codex".to_string(),])
        );
    }
}
