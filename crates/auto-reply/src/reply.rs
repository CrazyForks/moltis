use {
    moltis_common::types::{MsgContext, ReplyPayload},
    tracing::info,
};

/// Main entry point: process an inbound message and produce a reply.
///
/// TODO: load session → parse directives → invoke agent → chunk → return reply
pub async fn get_reply(msg: &MsgContext) -> anyhow::Result<ReplyPayload> {
    info!(
        channel = %msg.channel,
        account_id = %msg.account_id,
        from = %msg.from,
        sender = msg.sender_name.as_deref().unwrap_or("unknown"),
        chat_type = ?msg.chat_type,
        session_key = %msg.session_key,
        "incoming message: {}",
        msg.body,
    );

    Ok(ReplyPayload {
        text: format!(
            "Echo: {}",
            if msg.body.is_empty() {
                "(no text)"
            } else {
                &msg.body
            }
        ),
        media: None,
        reply_to_id: msg.reply_to_id.clone(),
        silent: false,
    })
}
