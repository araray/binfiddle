use thiserror::Error;

#[derive(Error, Debug)]
pub enum BinfiddleError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Invalid range: {0}")]
    InvalidRange(String),

    #[error("Invalid chunk size: {0}")]
    InvalidChunkSize(usize),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Operation not supported: {0}")]
    UnsupportedOperation(String),
}

pub type Result<T> = std::result::Result<T, BinfiddleError>;
