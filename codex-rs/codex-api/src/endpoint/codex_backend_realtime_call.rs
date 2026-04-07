use crate::auth::AuthProvider;
use crate::endpoint::session::EndpointSession;
use crate::error::ApiError;
use crate::provider::Provider;
use codex_client::HttpTransport;
use codex_client::RequestTelemetry;
use http::HeaderMap;
use http::Method;
use serde::Serialize;
use serde_json::Value;
use serde_json::to_value;
use std::sync::Arc;
use tracing::instrument;

pub struct CodexBackendRealtimeCallClient<T: HttpTransport, A: AuthProvider> {
    session: EndpointSession<T, A>,
}

#[derive(Serialize)]
struct CodexBackendRealtimeCallRequest<'a> {
    sdp: &'a str,
    session: &'a Value,
}

impl<T: HttpTransport, A: AuthProvider> CodexBackendRealtimeCallClient<T, A> {
    pub fn new(transport: T, provider: Provider, auth: A) -> Self {
        Self {
            session: EndpointSession::new(transport, provider, auth),
        }
    }

    pub fn with_telemetry(self, request: Option<Arc<dyn RequestTelemetry>>) -> Self {
        Self {
            session: self.session.with_request_telemetry(request),
        }
    }

    fn path() -> &'static str {
        "api/codex/realtime/calls"
    }

    #[instrument(
        name = "codex_backend_realtime_call.create",
        level = "info",
        skip_all,
        fields(
            http.method = "POST",
            api.path = "api/codex/realtime/calls"
        )
    )]
    pub async fn create(&self, sdp: &str, session: &Value) -> Result<String, ApiError> {
        let body = to_value(CodexBackendRealtimeCallRequest { sdp, session })
            .map_err(|err| ApiError::Stream(format!("failed to encode realtime call: {err}")))?;
        let resp = self
            .session
            .execute(Method::POST, Self::path(), HeaderMap::new(), Some(body))
            .await?;

        String::from_utf8(resp.body.to_vec())
            .map_err(|err| ApiError::Stream(format!("failed to decode realtime call SDP: {err}")))
    }
}
