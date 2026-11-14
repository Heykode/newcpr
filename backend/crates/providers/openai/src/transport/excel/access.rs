use std::time::Duration;

use futures::StreamExt;

use crate::credential::excel::{EXCEL_ACCESS_URL, ExcelModel, ExcelModelCatalog};
use crate::transport::{
    CodexBackendClient, CodexBackendTransport, CodexClientError, CodexClientResult,
    CodexRequestContext, CodexUpstreamSendPhase,
};

impl CodexBackendClient {
    pub(crate) async fn fetch_excel_models_with_context(
        &self,
        context: CodexRequestContext<'_>,
        responses_endpoint: Option<&str>,
    ) -> CodexClientResult<Vec<ExcelModel>> {
        let endpoint = if let Some(endpoint) = responses_endpoint {
            format!(
                "{}/access?include_models=true",
                endpoint.trim_end_matches('/')
            )
        } else if reqwest::Url::parse(&self.base_url).ok().is_some_and(|url| {
            matches!(
                url.host_str(),
                Some("chatgpt.com" | "chat.openai.com" | "bps.openai.com")
            )
        }) {
            EXCEL_ACCESS_URL.to_owned()
        } else {
            super::super::endpoints::endpoint_url(
                &self.base_url,
                "basispoints/api/responses/access?include_models=true",
            )
        };
        let identity = context
            .authorization
            .strip_prefix("Bearer ")
            .and_then(crate::credential::excel::identity_from_access_token);
        let headers = crate::credential::excel::request_headers(
            context.authorization,
            context.account_id.unwrap_or(""),
            identity.as_ref().map(|value| value.user_id.as_str()),
            "application/json",
        )?;
        let profile = self.profile.snapshot();
        let response = self
            .send_profiled(&profile, true, |client| {
                client
                    .get(endpoint)
                    .headers(headers)
                    .timeout(Duration::from_secs(5))
            })
            .await?;
        let status = response.status();
        let diagnostics =
            super::super::response_meta::diagnostics(Some(status.as_u16()), response.headers());
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if body.len().saturating_add(chunk.len()) > 2 * 1024 * 1024 {
                return Err(CodexClientError::ExcelCatalog);
            }
            body.extend_from_slice(&chunk);
        }
        if status != reqwest::StatusCode::OK {
            return Err(CodexClientError::Upstream {
                status,
                body: String::from_utf8_lossy(&body).into_owned(),
                client_response: None,
                retry_after_seconds: None,
                diagnostics: Box::new(diagnostics),
                set_cookie_headers: Vec::new(),
                rate_limit_headers: Vec::new(),
                transport: CodexBackendTransport::HttpSse,
                transport_metrics: Box::default(),
                send_phase: CodexUpstreamSendPhase::BeforePayload,
            });
        }
        ExcelModelCatalog::parse(&body).map_err(|_| CodexClientError::ExcelCatalog)
    }

    pub(crate) async fn validate_excel_model(
        &self,
        context: CodexRequestContext<'_>,
        prepared: &super::ExcelPreparedRequest,
    ) -> CodexClientResult<()> {
        let models = self
            .fetch_excel_models_with_context(context, Some(&prepared.endpoint))
            .await?;
        let requested = prepared
            .body
            .get("model")
            .and_then(serde_json::Value::as_str);
        if models
            .iter()
            .any(|model| Some(model.id.as_str()) == requested)
        {
            Ok(())
        } else {
            Err(CodexClientError::ExcelModelUnsupported)
        }
    }
}
