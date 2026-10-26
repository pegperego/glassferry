use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Ssh(String),
    #[error("SFTP: {0}")]
    Sftp(#[from] openssh_sftp_client::Error),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("annullato")]
    Cancelled,
    #[error("session not found: {0}")]
    SessionNotFound(u64),
    #[error("{0}")]
    Other(String),
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
