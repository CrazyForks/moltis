//! RAG / semantic search using SQLite + sqlite-vec for vector storage.
//! Hybrid search combines keyword and vector similarity.

pub mod store;
pub mod embeddings;
pub mod search;
