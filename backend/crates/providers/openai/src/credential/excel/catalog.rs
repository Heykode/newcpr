use std::{collections::HashSet, time::Duration};

use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};

use super::{EXCEL_ACCESS_URL, ExcelAuthError, ExcelCredentialData, oauth::read_bounded};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExcelModel {
    pub id: String,
    pub label: String,
    pub efforts: Vec<String>,
}

pub struct ExcelModelCatalog;

impl ExcelModelCatalog {
    pub async fn fetch(
        client: &reqwest::Client,
        credential: &ExcelCredentialData,
    ) -> Result<Vec<ExcelModel>, ExcelAuthError> {
        Self::fetch_at(client, credential, EXCEL_ACCESS_URL).await
    }

    pub(crate) async fn fetch_at(
        client: &reqwest::Client,
        credential: &ExcelCredentialData,
        endpoint: &str,
    ) -> Result<Vec<ExcelModel>, ExcelAuthError> {
        credential.ensure_usable()?;
        let headers = request_headers(
            &format!("Bearer {}", credential.access_token),
            &credential.chatgpt_account_id,
            Some(&credential.user_id),
            "application/json",
        )
        .map_err(|_| ExcelAuthError::InvalidCredential)?;
        let response = client
            .get(endpoint)
            .headers(headers)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|_| ExcelAuthError::Transport)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(ExcelAuthError::AccessDenied);
        }
        let bytes = read_bounded(response, 2 * 1024 * 1024).await?;
        Self::parse(&bytes)
    }

    pub fn parse(bytes: &[u8]) -> Result<Vec<ExcelModel>, ExcelAuthError> {
        #[derive(Deserialize)]
        struct Access {
            allowed: bool,
            model_catalog: Option<Catalog>,
        }
        #[derive(Deserialize)]
        struct Catalog {
            models: Vec<serde_json::Value>,
            #[serde(default)]
            restricted_models: Vec<String>,
        }
        #[derive(Deserialize)]
        struct Model {
            id: String,
            #[serde(default)]
            label: String,
            model_picker_enabled: Option<bool>,
            #[serde(default)]
            policy: Policy,
            #[serde(default)]
            efforts: Vec<Effort>,
        }
        #[derive(Default, Deserialize)]
        struct Policy {
            #[serde(default)]
            state: String,
        }
        #[derive(Deserialize)]
        struct Effort {
            value: String,
        }
        if bytes.len() > 2 * 1024 * 1024 {
            return Err(ExcelAuthError::InvalidCatalog);
        }
        let access: Access =
            serde_json::from_slice(bytes).map_err(|_| ExcelAuthError::InvalidCatalog)?;
        if !access.allowed {
            return Ok(Vec::new());
        }
        let catalog = access.model_catalog.ok_or(ExcelAuthError::InvalidCatalog)?;
        let mut seen: HashSet<String> = catalog.restricted_models.into_iter().collect();
        let mut result = Vec::new();
        for raw in catalog.models {
            let Ok(model) = serde_json::from_value::<Model>(raw) else {
                continue;
            };
            if !valid_model_id(&model.id)
                || model.model_picker_enabled == Some(false)
                || matches!(model.policy.state.as_str(), "disabled" | "unconfigured")
                || !seen.insert(model.id.clone())
            {
                continue;
            }
            let mut efforts = Vec::new();
            for effort in model.efforts {
                if matches!(effort.value.as_str(), "low" | "medium" | "high" | "xhigh")
                    && !efforts.contains(&effort.value)
                {
                    efforts.push(effort.value);
                }
            }
            result.push(ExcelModel {
                id: model.id,
                label: model.label.trim().into(),
                efforts,
            });
        }
        Ok(result)
    }
}

fn valid_model_id(id: &str) -> bool {
    if id.len() > 128 || id.ends_with("-excel") {
        return false;
    }
    let Some(rest) = id.strip_prefix("gpt-") else {
        return false;
    };
    rest.split(['.', '-']).all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
    })
}

pub(crate) fn request_headers(
    authorization: &str,
    account_id: &str,
    user_id: Option<&str>,
    accept: &'static str,
) -> Result<HeaderMap, reqwest::header::InvalidHeaderValue> {
    let mut headers = HeaderMap::new();
    let mut auth = HeaderValue::from_str(authorization)?;
    auth.set_sensitive(true);
    headers.insert("authorization", auth);
    for (name, value) in [
        ("chatgpt-account-id", account_id),
        ("x-openai-account-id", account_id),
    ] {
        headers.insert(name, HeaderValue::from_str(value)?);
    }
    if let Some(user_id) = user_id.filter(|value| !value.is_empty()) {
        headers.insert("x-openai-account-user-id", HeaderValue::from_str(user_id)?);
    }
    for (name, value) in [
        ("user-agent", "Mozilla/5.0"),
        ("origin", "https://bps.openai.com"),
        ("x-basispoints-auth-mode", "chatgpt"),
        (
            "x-openai-internal-basispoints-client-product",
            "basispoints-excel-plugin",
        ),
        (
            "x-openai-internal-basispoints-client-agent-profile",
            "excel",
        ),
        ("accept-encoding", "identity"),
        ("accept", accept),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    Ok(headers)
}
