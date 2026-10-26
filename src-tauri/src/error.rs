use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Ssh(String),
    #[error("{}", sftp_message(.0))]
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

/// Readable text for SFTP status errors; OpenSSH's sftp-server only sends
/// generic messages ("Failure"), so the code is translated.
fn sftp_message(error: &openssh_sftp_client::Error) -> String {
    use openssh_sftp_client::error::SftpErrorKind;
    match error {
        openssh_sftp_client::Error::SftpError(kind, msg) => {
            let text = match kind {
                SftpErrorKind::NoSuchFile => "file o cartella inesistente",
                SftpErrorKind::PermDenied => "permesso negato",
                SftpErrorKind::OpUnsupported => "operazione non supportata dal server",
                SftpErrorKind::Failure => "operazione rifiutata dal server",
                _ => "",
            };
            let (server, _) = msg.get();
            let generic = ["", "Failure", "No such file", "Permission denied", "Operation unsupported"];
            match (text.is_empty(), generic.contains(&server)) {
                (false, true) => format!("SFTP: {text}"),
                (false, false) => format!("SFTP: {text} ({server})"),
                (true, _) => format!("SFTP: {server}"),
            }
        }
        other => format!("SFTP: {other}"),
    }
}
