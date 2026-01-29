/// Cron job scheduler and executor.
pub struct CronService {
    // TODO: croner-equivalent scheduler
}

impl Default for CronService {
    fn default() -> Self {
        Self::new()
    }
}

impl CronService {
    pub fn new() -> Self {
        Self {}
    }

    pub async fn start(&self) -> anyhow::Result<()> {
        todo!("load jobs from store, schedule timers, run isolated agent on trigger")
    }
}
