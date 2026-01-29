use serde::{Deserialize, Serialize};

/// Persistent cron job storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub expression: String,
    pub agent_id: String,
    pub message: String,
    pub deliver_to: Option<DeliverTarget>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliverTarget {
    pub channel: String,
    pub account_id: String,
    pub peer_id: String,
}

pub fn load_jobs(_path: &std::path::Path) -> anyhow::Result<Vec<CronJob>> {
    todo!("read cron-jobs.json")
}

pub fn save_jobs(_path: &std::path::Path, _jobs: &[CronJob]) -> anyhow::Result<()> {
    todo!("write cron-jobs.json")
}
