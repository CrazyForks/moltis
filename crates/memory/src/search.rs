/// Hybrid search: keyword + vector similarity.
pub async fn hybrid_search(_query: &str, _limit: usize) -> anyhow::Result<Vec<SearchResult>> {
    todo!("combine keyword search and vector similarity from SQLite")
}

pub struct SearchResult {
    pub content: String,
    pub score: f32,
}
