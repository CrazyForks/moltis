use thiserror::Error;

#[derive(Error, Debug)]
pub enum MoltisError {
    #[error("config error: {0}")]
    Config(String),

    #[error("channel error: {0}")]
    Channel(String),

    #[error("agent error: {0}")]
    Agent(String),

    #[error("tool error: {0}")]
    Tool(String),

    #[error("routing error: {0}")]
    Routing(String),

    #[error("session error: {0}")]
    Session(String),

    #[error("gateway error: {0}")]
    Gateway(String),

    #[error("plugin error: {0}")]
    Plugin(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
