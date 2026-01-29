use moltis_common::types::{MsgContext, ReplyPayload};

/// Main entry point: process an inbound message and produce a reply.
pub async fn get_reply(_msg: &MsgContext) -> anyhow::Result<ReplyPayload> {
    todo!("load session → parse directives → invoke agent → chunk → return reply")
}
