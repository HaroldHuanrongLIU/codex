use codex_app_server_protocol::AuthMode;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::RealtimeCallCreateParams;
use codex_app_server_protocol::RealtimeCallCreateResponse;
use codex_backend_client::Client as BackendClient;
use codex_core::config::Config;
use codex_login::CodexAuth;
use codex_login::default_client::try_build_reqwest_client;
use serde_json::Value as JsonValue;

use crate::error_code::INTERNAL_ERROR_CODE;
use crate::error_code::INVALID_REQUEST_ERROR_CODE;

pub async fn create_realtime_call(
    config: &Config,
    auth: Option<CodexAuth>,
    params: RealtimeCallCreateParams,
) -> Result<RealtimeCallCreateResponse, JSONRPCErrorError> {
    let Some(auth) = auth else {
        return Err(invalid_request_error(
            "codex account authentication required to create realtime call",
        ));
    };

    let sdp = match auth.auth_mode() {
        AuthMode::ApiKey => create_api_realtime_call(config, &auth, params).await?,
        AuthMode::Chatgpt | AuthMode::ChatgptAuthTokens => {
            create_chatgpt_realtime_call(config, &auth, params).await?
        }
    };

    Ok(RealtimeCallCreateResponse { sdp })
}

async fn create_chatgpt_realtime_call(
    config: &Config,
    auth: &CodexAuth,
    params: RealtimeCallCreateParams,
) -> Result<String, JSONRPCErrorError> {
    let session = params
        .session
        .filter(JsonValue::is_object)
        .ok_or_else(|| invalid_request_error("session must be an object for chatgpt auth"))?;
    let client = BackendClient::from_auth(config.chatgpt_base_url.clone(), auth)
        .map_err(|err| internal_error(format!("failed to construct backend client: {err}")))?;

    client
        .create_realtime_call(&params.sdp, &session)
        .await
        .map_err(|err| internal_error(format!("failed to create realtime call: {err}")))
}

async fn create_api_realtime_call(
    config: &Config,
    auth: &CodexAuth,
    params: RealtimeCallCreateParams,
) -> Result<String, JSONRPCErrorError> {
    let token = auth
        .get_token()
        .map_err(|err| internal_error(format!("failed to read api key: {err}")))?;
    let base_url = config
        .model_provider
        .base_url
        .as_deref()
        .unwrap_or("https://api.openai.com/v1")
        .trim_end_matches('/');
    let url = format!("{base_url}/realtime/calls");
    let http = try_build_reqwest_client()
        .map_err(|err| internal_error(format!("failed to build HTTP client: {err}")))?;

    let mut request = http
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/sdp")
        .body(params.sdp);

    if let Some(headers) = &config.model_provider.http_headers {
        for (name, value) in headers {
            request = request.header(name, value);
        }
    }

    if let Some(headers) = &config.model_provider.env_http_headers {
        for (name, env_var) in headers {
            if let Ok(value) = std::env::var(env_var)
                && !value.trim().is_empty()
            {
                request = request.header(name, value);
            }
        }
    }

    let response = request
        .send()
        .await
        .map_err(|err| internal_error(format!("failed to create realtime call: {err}")))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = response.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(internal_error(format!(
            "failed to create realtime call: POST {url} failed: {status}; content-type={content_type}; body={body}"
        )));
    }

    Ok(body)
}

fn invalid_request_error(message: impl Into<String>) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: INVALID_REQUEST_ERROR_CODE,
        message: message.into(),
        data: None,
    }
}

fn internal_error(message: impl Into<String>) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: INTERNAL_ERROR_CODE,
        message: message.into(),
        data: None,
    }
}
