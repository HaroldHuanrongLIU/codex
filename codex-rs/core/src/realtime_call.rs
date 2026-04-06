use std::sync::Arc;

use codex_api::RealtimeCallClient;
use codex_api::ReqwestTransport;
use codex_api::api_bridge::map_api_error;
use codex_api::session_update_session_json;
use codex_app_server_protocol::AuthMode;
use codex_backend_client::Client as BackendClient;
use codex_login::CodexAuth;
use codex_login::api_bridge::auth_provider_from_auth;
use codex_login::default_client::build_reqwest_client;
use codex_model_provider_info::ModelProviderInfo;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use serde_json::Value as JsonValue;

use crate::codex::Session;
use crate::realtime_conversation::build_realtime_session_config;

pub(crate) async fn create_realtime_call(
    sess: &Arc<Session>,
    sdp: String,
    prompt: String,
    session_id: Option<String>,
) -> CodexResult<String> {
    let provider = sess.provider().await;
    let auth_manager = sess
        .services
        .model_client
        .auth_manager()
        .unwrap_or_else(|| Arc::clone(&sess.services.auth_manager));
    let auth = auth_manager.auth().await.ok_or_else(|| {
        CodexErr::InvalidRequest(
            "codex account authentication required to create realtime call".to_string(),
        )
    })?;

    let session = realtime_session_json(sess, prompt, session_id).await?;

    match auth.auth_mode() {
        AuthMode::ApiKey => create_api_realtime_call(&provider, &auth, sdp, session).await,
        AuthMode::Chatgpt | AuthMode::ChatgptAuthTokens => {
            create_chatgpt_realtime_call(sess, &auth, sdp, session).await
        }
    }
}

async fn realtime_session_json(
    sess: &Arc<Session>,
    prompt: String,
    session_id: Option<String>,
) -> CodexResult<JsonValue> {
    let session_config = build_realtime_session_config(sess, prompt, session_id).await?;
    let model = session_config.model.clone();
    let mut session = session_update_session_json(session_config)?;
    if let Some(model) = model
        && let Some(session) = session.as_object_mut()
    {
        session.insert("model".to_string(), JsonValue::String(model));
    }
    Ok(session)
}

async fn create_chatgpt_realtime_call(
    sess: &Arc<Session>,
    auth: &CodexAuth,
    sdp: String,
    session: JsonValue,
) -> CodexResult<String> {
    let config = sess.get_config().await;
    let client = BackendClient::from_auth(config.chatgpt_base_url.clone(), auth)
        .map_err(|err| CodexErr::Fatal(format!("failed to construct backend client: {err}")))?;

    client
        .create_realtime_call(&sdp, &session)
        .await
        .map_err(|err| CodexErr::Fatal(format!("failed to create realtime call: {err}")))
}

async fn create_api_realtime_call(
    provider: &ModelProviderInfo,
    auth: &CodexAuth,
    sdp: String,
    session: JsonValue,
) -> CodexResult<String> {
    let api_provider = provider.to_api_provider(Some(AuthMode::ApiKey))?;
    let api_auth = auth_provider_from_auth(Some(auth.clone()), provider)?;
    let client = RealtimeCallClient::new(
        ReqwestTransport::new(build_reqwest_client()),
        api_provider,
        api_auth,
    );

    client
        .create_with_session(sdp, session)
        .await
        .map(|response| response.sdp)
        .map_err(map_api_error)
}
