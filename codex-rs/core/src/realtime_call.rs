use std::sync::Arc;

use codex_api::session_update_session_json;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::protocol::ConversationCallCreateParams;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::RealtimeConversationCallCreatedEvent;
use serde_json::Value as JsonValue;

use crate::codex::Session;
use crate::realtime_conversation::RealtimeSessionInstructions;
use crate::realtime_conversation::build_realtime_session_config;

pub(crate) async fn handle_create(
    sess: &Arc<Session>,
    sub_id: String,
    params: ConversationCallCreateParams,
) -> CodexResult<()> {
    let session = realtime_session_json(sess).await?;
    let sdp = sess
        .services
        .model_client
        .create_realtime_call(params.sdp, session)
        .await?;

    sess.send_event_raw(Event {
        id: sub_id,
        msg: EventMsg::RealtimeConversationCallCreated(RealtimeConversationCallCreatedEvent {
            sdp,
        }),
    })
    .await;
    Ok(())
}

async fn realtime_session_json(sess: &Arc<Session>) -> CodexResult<JsonValue> {
    let session_config = build_realtime_session_config(
        sess,
        RealtimeSessionInstructions::ConfigOnly,
        /*session_id*/ None,
    )
    .await?;
    let model = session_config.model.clone();
    let mut session = session_update_session_json(session_config)?;
    if let Some(model) = model
        && let Some(session) = session.as_object_mut()
    {
        session.insert("model".to_string(), JsonValue::String(model));
    }
    Ok(session)
}
