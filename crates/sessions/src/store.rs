use anyhow::Result;

/// Append-only JSONL session storage with file locking.
pub struct SessionStore {
    pub base_dir: std::path::PathBuf,
}

impl SessionStore {
    pub fn new(base_dir: std::path::PathBuf) -> Self {
        Self { base_dir }
    }

    pub async fn append(&self, _key: &str, _message: &serde_json::Value) -> Result<()> {
        todo!("append message line to JSONL file with lock")
    }

    pub async fn read(&self, _key: &str) -> Result<Vec<serde_json::Value>> {
        todo!("read all messages from JSONL file")
    }

    pub async fn clear(&self, _key: &str) -> Result<()> {
        todo!("delete session file")
    }
}
