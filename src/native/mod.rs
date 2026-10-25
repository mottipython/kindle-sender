//! Built-in ebook converters, so common formats need no calibre install.
//!
//! Everything here ends in [`epub_writer::write`]. Callers fall back to calibre
//! when a converter reports [`NativeError::Unsupported`] or fails.

pub mod epub_meta;
pub mod epub_writer;
pub mod mobi;
pub mod pdf;
pub mod text;
pub mod xhtml;

use std::collections::HashMap;
use std::path::Path;

use regex::Regex;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum NativeError {
    #[error("{0}")]
    Unsupported(String),
    #[error("this book is DRM-protected and cannot be converted")]
    Drm,
    #[error("{0}")]
    Corrupt(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Extensions the built-in converters handle.
pub const NATIVE_EXT: &[&str] = &[".mobi", ".azw", ".azw3", ".prc", ".txt", ".html", ".htm", ".xhtml", ".fb2", ".pdf"];

pub fn handles(ext: &str) -> bool {
    NATIVE_EXT.contains(&ext)
}

/// Converts `src` to an EPUB at `dest`. `ext` is the lowercase extension with its dot.
pub fn convert(src: &Path, dest: &Path, ext: &str) -> Result<(), NativeError> {
    let book = match ext {
        ".mobi" | ".azw" | ".azw3" | ".prc" => mobi::convert(src)?,
        ".txt" => text::convert_txt(src)?,
        ".html" | ".htm" | ".xhtml" => text::convert_html(src)?,
        ".fb2" => text::convert_fb2(src)?,
        ".pdf" => pdf::convert(src)?,
        other => return Err(NativeError::Unsupported(format!("no built-in converter for {}", other))),
    };
    epub_writer::write(&book, dest)
}

/// Best-effort text decoding: UTF-8 (with BOM), else windows-1252.
pub fn decode_bytes(raw: &[u8]) -> String {
    if let Some(rest) = raw.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if raw.starts_with(&[0xFF, 0xFE]) {
        return encoding_rs::UTF_16LE.decode(&raw[2..]).0.into_owned();
    }
    if raw.starts_with(&[0xFE, 0xFF]) {
        return encoding_rs::UTF_16BE.decode(&raw[2..]).0.into_owned();
    }
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::WINDOWS_1252.decode(raw).0.into_owned(),
    }
}

/// Rewrites `href="#id"` so links that cross chapter files point at the right file.
/// `anchors` maps an id to the index (into `parts`/`files`) of the part defining it.
pub fn fix_links(parts: &mut [String], files: &[String], anchors: &HashMap<String, usize>) {
    let re = Regex::new(r##"href="#([^"]*)""##).unwrap();
    for (i, part) in parts.iter_mut().enumerate() {
        *part = re
            .replace_all(part, |c: &regex::Captures| {
                let id = &c[1];
                match anchors.get(id) {
                    Some(&j) if j != i && j < files.len() => format!("href=\"{}#{}\"", files[j], id),
                    Some(_) => c[0].to_string(),
                    // Dangling link: keep the text, lose the dead link.
                    None => "href=\"#\"".to_string(),
                }
            })
            .into_owned();
    }
}
