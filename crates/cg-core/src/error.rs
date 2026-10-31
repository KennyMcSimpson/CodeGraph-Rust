use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, CgError>;

#[derive(thiserror::Error, Debug)]
pub enum CgError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("corrupt index: {0}")]
    Corrupt(String),
    #[error("index format/engine mismatch: {0}")]
    Version(String),
    #[error("no index found at {0}")]
    NoIndex(PathBuf),
    #[error("lock error: {0}")]
    Lock(String),
    #[error("invalid configuration: {0}")]
    Config(String),
    #[error("{0}")]
    Other(String),
}

impl CgError {
    pub fn other(msg: impl Into<String>) -> Self {
        CgError::Other(msg.into())
    }
    pub fn corrupt(msg: impl Into<String>) -> Self {
        CgError::Corrupt(msg.into())
    }
}
