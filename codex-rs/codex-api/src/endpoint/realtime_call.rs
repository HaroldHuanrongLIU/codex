use crate::auth::AuthProvider;
use crate::endpoint::session::EndpointSession;
use crate::error::ApiError;
use crate::provider::Provider;
use bytes::Bytes;
use codex_client::HttpTransport;
use codex_client::Request;
use codex_client::RequestTelemetry;
use http::HeaderMap;
use http::HeaderValue;
use http::Method;
use http::header::CONTENT_TYPE;
use serde::Serialize;
use serde_json::Value;
use serde_json::to_string;
use serde_json::to_value;
use std::sync::Arc;
use tracing::instrument;
use url::Url;

const MULTIPART_BOUNDARY: &str = "codex-realtime-call-boundary";
const REALTIME_CALL_INTENT: &str = "quicksilver";

pub struct RealtimeCallClient<T: HttpTransport, A: AuthProvider> {
    session: EndpointSession<T, A>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeCallResponse {
    pub sdp: String,
}

#[derive(Serialize)]
struct BackendRealtimeCallRequest<'a> {
    sdp: &'a str,
    session: &'a Value,
}

impl<T: HttpTransport, A: AuthProvider> RealtimeCallClient<T, A> {
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
        "realtime/calls"
    }

    fn uses_backend_request_shape(&self) -> bool {
        self.session.provider().base_url.contains("/backend-api")
    }

    #[instrument(
        name = "realtime_call.create",
        level = "info",
        skip_all,
        fields(
            http.method = "POST",
            api.path = "realtime/calls"
        )
    )]
    pub async fn create(&self, sdp: String) -> Result<RealtimeCallResponse, ApiError> {
        self.create_with_headers(sdp, HeaderMap::new()).await
    }

    pub async fn create_with_session(
        &self,
        sdp: String,
        session: Value,
    ) -> Result<RealtimeCallResponse, ApiError> {
        self.create_with_session_and_headers(sdp, session, HeaderMap::new())
            .await
    }

    pub async fn create_with_headers(
        &self,
        sdp: String,
        extra_headers: HeaderMap,
    ) -> Result<RealtimeCallResponse, ApiError> {
        let resp = self
            .session
            .execute_with(
                Method::POST,
                Self::path(),
                extra_headers,
                /*body*/ None,
                |req| {
                    append_realtime_call_intent(req);
                    req.headers
                        .insert(CONTENT_TYPE, HeaderValue::from_static("application/sdp"));
                    req.raw_body = Some(Bytes::from(sdp.clone()));
                },
            )
            .await?;

        let sdp = decode_sdp_response(resp.body.as_ref())?;

        Ok(RealtimeCallResponse { sdp })
    }

    pub async fn create_with_session_and_headers(
        &self,
        sdp: String,
        session: Value,
        extra_headers: HeaderMap,
    ) -> Result<RealtimeCallResponse, ApiError> {
        if self.uses_backend_request_shape() {
            let body = to_value(BackendRealtimeCallRequest {
                sdp: &sdp,
                session: &session,
            })
            .map_err(|err| ApiError::Stream(format!("failed to encode realtime call: {err}")))?;
            let resp = self
                .session
                .execute_with(
                    Method::POST,
                    Self::path(),
                    extra_headers,
                    Some(body),
                    |req| {
                        append_realtime_call_intent(req);
                    },
                )
                .await?;
            let sdp = decode_sdp_response(resp.body.as_ref())?;
            return Ok(RealtimeCallResponse { sdp });
        }

        let session = to_string(&session).map_err(|err| ApiError::InvalidRequest {
            message: err.to_string(),
        })?;
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{MULTIPART_BOUNDARY}\r\n").as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"sdp\"; filename=\"offer.sdp\"\r\n",
        );
        body.extend_from_slice(b"Content-Type: application/sdp\r\n\r\n");
        body.extend_from_slice(sdp.as_bytes());
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(format!("--{MULTIPART_BOUNDARY}\r\n").as_bytes());
        body.extend_from_slice(b"Content-Disposition: form-data; name=\"session\"\r\n");
        body.extend_from_slice(b"Content-Type: application/json\r\n\r\n");
        body.extend_from_slice(session.as_bytes());
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(format!("--{MULTIPART_BOUNDARY}--\r\n").as_bytes());

        let resp = self
            .session
            .execute_with(
                Method::POST,
                Self::path(),
                extra_headers,
                /*body*/ None,
                |req| {
                    append_realtime_call_intent(req);
                    req.headers.insert(
                        CONTENT_TYPE,
                        HeaderValue::from_static(
                            "multipart/form-data; boundary=codex-realtime-call-boundary",
                        ),
                    );
                    req.raw_body = Some(Bytes::from(body.clone()));
                },
            )
            .await?;

        let sdp = decode_sdp_response(resp.body.as_ref())?;

        Ok(RealtimeCallResponse { sdp })
    }
}

fn append_realtime_call_intent(req: &mut Request) {
    let mut url = Url::parse(&req.url).expect("endpoint session should build valid URLs");
    if !url.query_pairs().any(|(key, _)| key == "intent") {
        url.query_pairs_mut()
            .append_pair("intent", REALTIME_CALL_INTENT);
    }
    req.url = url.to_string();
}

fn decode_sdp_response(body: &[u8]) -> Result<String, ApiError> {
    String::from_utf8(body.to_vec()).map_err(|err| {
        ApiError::Stream(format!(
            "failed to decode realtime call SDP response: {err}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::RetryConfig;
    use async_trait::async_trait;
    use codex_client::Request;
    use codex_client::Response;
    use codex_client::StreamResponse;
    use codex_client::TransportError;
    use http::StatusCode;
    use pretty_assertions::assert_eq;
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Clone)]
    struct CapturingTransport {
        last_request: Arc<Mutex<Option<Request>>>,
    }

    impl CapturingTransport {
        fn new() -> Self {
            Self {
                last_request: Arc::new(Mutex::new(None)),
            }
        }
    }

    #[async_trait]
    impl HttpTransport for CapturingTransport {
        async fn execute(&self, req: Request) -> Result<Response, TransportError> {
            *self.last_request.lock().unwrap() = Some(req);
            Ok(Response {
                status: StatusCode::OK,
                headers: HeaderMap::new(),
                body: Bytes::from_static(b"v=0\r\n").into(),
            })
        }

        async fn stream(&self, _req: Request) -> Result<StreamResponse, TransportError> {
            Err(TransportError::Build("stream should not run".to_string()))
        }
    }

    #[derive(Clone, Default)]
    struct DummyAuth;

    impl AuthProvider for DummyAuth {
        fn bearer_token(&self) -> Option<String> {
            Some("test-token".to_string())
        }
    }

    fn provider(base_url: &str) -> Provider {
        Provider {
            name: "test".to_string(),
            base_url: base_url.to_string(),
            query_params: None,
            headers: HeaderMap::new(),
            retry: RetryConfig {
                max_attempts: 1,
                base_delay: Duration::from_millis(1),
                retry_429: false,
                retry_5xx: true,
                retry_transport: true,
            },
            stream_idle_timeout: Duration::from_secs(1),
        }
    }

    #[tokio::test]
    async fn sends_sdp_offer_as_raw_body() {
        let transport = CapturingTransport::new();
        let client = RealtimeCallClient::new(
            transport.clone(),
            provider("https://api.openai.com/v1"),
            DummyAuth,
        );

        let response = client
            .create("v=offer\r\n".to_string())
            .await
            .expect("request should succeed");

        assert_eq!(
            response,
            RealtimeCallResponse {
                sdp: "v=0\r\n".to_string()
            }
        );

        let request = transport.last_request.lock().unwrap().clone().unwrap();
        assert_eq!(request.method, Method::POST);
        assert_eq!(
            request.url,
            "https://api.openai.com/v1/realtime/calls?intent=quicksilver"
        );
        assert_eq!(
            request.headers.get(CONTENT_TYPE).unwrap(),
            HeaderValue::from_static("application/sdp")
        );
        assert_eq!(
            request
                .headers
                .get(http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer test-token")
        );
        assert_eq!(request.raw_body, Some(Bytes::from_static(b"v=offer\r\n")));
        assert_eq!(request.body, None);
    }

    #[tokio::test]
    async fn sends_api_session_call_as_multipart_body() {
        let transport = CapturingTransport::new();
        let client = RealtimeCallClient::new(
            transport.clone(),
            provider("https://api.openai.com/v1"),
            DummyAuth,
        );

        let response = client
            .create_with_session(
                "v=offer\r\n".to_string(),
                serde_json::json!({"type": "realtime", "instructions": "hi"}),
            )
            .await
            .expect("request should succeed");

        assert_eq!(
            response,
            RealtimeCallResponse {
                sdp: "v=0\r\n".to_string()
            }
        );

        let request = transport.last_request.lock().unwrap().clone().unwrap();
        assert_eq!(request.method, Method::POST);
        assert_eq!(
            request.url,
            "https://api.openai.com/v1/realtime/calls?intent=quicksilver"
        );
        assert_eq!(
            request.headers.get(CONTENT_TYPE).unwrap(),
            HeaderValue::from_static("multipart/form-data; boundary=codex-realtime-call-boundary")
        );
        let body = request.raw_body.expect("multipart body");
        let body = std::str::from_utf8(&body).expect("multipart body should be utf-8");
        assert!(body.contains("Content-Disposition: form-data; name=\"sdp\""));
        assert!(body.contains("Content-Type: application/sdp"));
        assert!(body.contains("v=offer\r\n"));
        assert!(body.contains("Content-Disposition: form-data; name=\"session\""));
        assert!(body.contains("Content-Type: application/json"));
        assert!(body.contains(r#""instructions":"hi""#));
        assert_eq!(request.body, None);
    }

    #[tokio::test]
    async fn sends_backend_session_call_as_json_body() {
        let transport = CapturingTransport::new();
        let client = RealtimeCallClient::new(
            transport.clone(),
            provider("https://chatgpt.com/backend-api/codex"),
            DummyAuth,
        );

        let response = client
            .create_with_session(
                "v=offer\r\n".to_string(),
                serde_json::json!({"type": "realtime", "instructions": "hi"}),
            )
            .await
            .expect("request should succeed");

        assert_eq!(
            response,
            RealtimeCallResponse {
                sdp: "v=0\r\n".to_string()
            }
        );

        let request = transport.last_request.lock().unwrap().clone().unwrap();
        assert_eq!(request.method, Method::POST);
        assert_eq!(
            request.url,
            "https://chatgpt.com/backend-api/codex/realtime/calls?intent=quicksilver"
        );
        assert_eq!(request.raw_body, None);
        assert_eq!(
            request.body,
            Some(serde_json::json!({
                "sdp": "v=offer\r\n",
                "session": {
                    "type": "realtime",
                    "instructions": "hi"
                }
            }))
        );
    }
}
