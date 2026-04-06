use codex_api::RealtimeCallClient;
use codex_api::ReqwestTransport;
use codex_app_server_protocol::AuthMode;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::RealtimeCallCreateParams;
use codex_app_server_protocol::RealtimeCallCreateResponse;
use codex_backend_client::Client as BackendClient;
use codex_core::config::Config;
use codex_login::CodexAuth;
use codex_login::auth_provider_from_auth;
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
    let provider = config
        .model_provider
        .to_api_provider(Some(AuthMode::ApiKey))
        .map_err(|err| internal_error(format!("failed to build realtime call provider: {err}")))?;
    let api_auth = auth_provider_from_auth(Some(auth.clone()), &config.model_provider)
        .map_err(|err| internal_error(format!("failed to build realtime call auth: {err}")))?;
    let http = try_build_reqwest_client()
        .map_err(|err| internal_error(format!("failed to build HTTP client: {err}")))?;
    let client = RealtimeCallClient::new(ReqwestTransport::new(http), provider, api_auth);
    client
        .create(params.sdp)
        .await
        .map(|response| response.sdp)
        .map_err(|err| internal_error(format!("failed to create realtime call: {err}")))
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
