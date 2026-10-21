use std::fs;
use std::path::Path;
use std::time::Duration;
use lettre::message::header::ContentType;
use lettre::message::{Attachment, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{Message, SmtpTransport, Transport};
use thiserror::Error;

use crate::config::{clean_password, Config};

#[derive(Error, Debug)]
pub enum SendError {
    #[error("no SMTP server configured (Settings)")]
    NoSmtpServer,
    #[error("no Kindle address configured (Settings)")]
    NoKindleAddress,
    #[error("no From address configured (Settings)")]
    NoFromAddress,
    #[error("No password set — see Settings.")]
    NoPassword,
    #[error("attachment size {size_mb:.1} MB exceeds limit {limit_mb:.0} MB")]
    FileTooLarge { size_mb: f64, limit_mb: f64 },
    #[error("SMTP authentication failed:\n\nThe mail server rejected that username/password.\n\nGmail will not accept your normal account password here — you need a 16-character App Password from myaccount.google.com/apppasswords.")]
    AuthFailed,
    #[error("SMTP error: {0}")]
    Smtp(String),
    #[error("Message construction error: {0}")]
    MessageError(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub fn create_transport(cfg: &Config) -> Result<SmtpTransport, SendError> {
    if cfg.smtp_host.is_empty() {
        return Err(SendError::NoSmtpServer);
    }

    let user = cfg.smtp_user.trim();
    let password = clean_password(&cfg.smtp_password);

    if !user.is_empty() && password.is_empty() {
        return Err(SendError::NoPassword);
    }

    let tls_params = TlsParameters::builder(cfg.smtp_host.clone())
        .build()
        .map_err(|e| SendError::Smtp(e.to_string()))?;

    let mut builder = if cfg.smtp_port == 465 {
        SmtpTransport::relay(&cfg.smtp_host)
            .map_err(|e| SendError::Smtp(e.to_string()))?
            .port(465)
            .tls(Tls::Wrapper(tls_params))
    } else {
        SmtpTransport::relay(&cfg.smtp_host)
            .map_err(|e| SendError::Smtp(e.to_string()))?
            .port(cfg.smtp_port)
            .tls(Tls::Required(tls_params))
    };

    if !user.is_empty() {
        let creds = Credentials::new(user.to_string(), password);
        builder = builder.credentials(creds);
    }

    builder = builder.timeout(Some(Duration::from_secs(60)));
    Ok(builder.build())
}

pub fn test_connection(cfg: &Config) -> Result<(), SendError> {
    let transport = create_transport(cfg)?;
    match transport.test_connection() {
        Ok(true) => Ok(()),
        Ok(false) => Err(SendError::Smtp("Mail server rejected connection/NOOP".to_string())),
        Err(err) => {
            let s = err.to_string();
            if s.contains("535") || s.to_lowercase().contains("authentication") {
                Err(SendError::AuthFailed)
            } else {
                Err(SendError::Smtp(s))
            }
        }
    }
}

pub fn send_book<P: AsRef<Path>>(
    cfg: &Config,
    path: P,
    subject: Option<&str>,
) -> Result<u64, SendError> {
    let to_addr = cfg.kindle_addr.trim();
    let from_addr = if !cfg.from_addr.trim().is_empty() {
        cfg.from_addr.trim()
    } else {
        cfg.smtp_user.trim()
    };

    if to_addr.is_empty() {
        return Err(SendError::NoKindleAddress);
    }
    if from_addr.is_empty() {
        return Err(SendError::NoFromAddress);
    }

    let file_path = path.as_ref();
    let meta = fs::metadata(file_path)?;
    let size = meta.len();
    let limit_bytes = (cfg.max_attach_mb * 1024.0 * 1024.0) as u64;

    if size > limit_bytes {
        return Err(SendError::FileTooLarge {
            size_mb: size as f64 / 1_000_000.0,
            limit_mb: cfg.max_attach_mb,
        });
    }

    let default_subject = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("book");
    let subj = subject.unwrap_or(default_subject);

    let filename = file_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("book.epub");

    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let content_type = match ext.as_str() {
        "epub" => ContentType::parse("application/epub+zip").unwrap(),
        "pdf" => ContentType::parse("application/pdf").unwrap(),
        _ => ContentType::parse("application/octet-stream").unwrap(),
    };

    let file_bytes = fs::read(file_path)?;
    let attachment = Attachment::new(filename.to_string()).body(file_bytes, content_type);

    let body_text = "Sent by kindle-sender.\n\n\
                     Note: the sending address must be on your Amazon \
                     'Approved Personal Document E-mail List'.";

    let email = Message::builder()
        .from(from_addr.parse().map_err(|e| SendError::MessageError(format!("Invalid From address: {}", e)))?)
        .to(to_addr.parse().map_err(|e| SendError::MessageError(format!("Invalid To address: {}", e)))?)
        .subject(subj)
        .multipart(
            MultiPart::mixed()
                .singlepart(SinglePart::plain(body_text.to_string()))
                .singlepart(attachment),
        )
        .map_err(|e| SendError::MessageError(e.to_string()))?;

    let transport = create_transport(cfg)?;
    transport.send(&email).map_err(|err| {
        let s = err.to_string();
        if s.contains("535") || s.to_lowercase().contains("authentication") {
            SendError::AuthFailed
        } else {
            SendError::Smtp(s)
        }
    })?;

    Ok(size)
}
