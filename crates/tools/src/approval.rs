/// Exec approval manager: gateway prompts user before running dangerous commands.
pub enum ApprovalDecision {
    Approved,
    Denied,
    Timeout,
}

pub async fn request_approval(_command: &str) -> anyhow::Result<ApprovalDecision> {
    todo!("send approval request to gateway, wait for user response via WS")
}
