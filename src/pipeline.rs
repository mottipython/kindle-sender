use std::fs;
use std::path::{Path, PathBuf};
use regex::Regex;
use tempfile::tempdir;

use crate::config::Config;
use crate::ebook_tools;
use crate::epub_fix;
use crate::sender;

#[derive(Debug, Clone)]
pub struct ProcessResult {
    pub source: PathBuf,
    pub name: String,
    pub title: Option<String>,
    pub changes: Vec<String>,
    pub status: Status,
    pub error: Option<String>,
    pub output: Option<PathBuf>,
    pub sent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Pending,
    Ok,
    Skipped,
    Error,
}

impl ProcessResult {
    pub fn new<P: AsRef<Path>>(source: P) -> Self {
        let path = source.as_ref().to_path_buf();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        Self {
            source: path,
            name,
            title: None,
            changes: Vec::new(),
            status: Status::Pending,
            error: None,
            output: None,
            sent: false,
        }
    }

    pub fn summary(&self) -> String {
        if self.status == Status::Error {
            return self.error.clone().unwrap_or_else(|| "failed".to_string());
        }
        if self.changes.is_empty() {
            return if self.sent {
                "sent (no changes needed)".to_string()
            } else {
                "no changes needed".to_string()
            };
        }
        let head = format!("{} fix(es)", self.changes.len());
        if self.sent {
            format!("sent, {}", head)
        } else {
            head
        }
    }
}

pub fn safe_filename(name: &str) -> String {
    let invalid_chars = Regex::new(r#"[\\/:*?"<>|]+"#).unwrap();
    let multi_space = Regex::new(r"\s+").unwrap();

    let cleaned = invalid_chars.replace_all(name, "_");
    let compressed = multi_space.replace_all(&cleaned, " ").trim().to_string();

    let truncated: String = compressed.chars().take(150).collect();
    if truncated.is_empty() {
        "book".to_string()
    } else {
        truncated
    }
}

pub fn process_file<P, L, C>(
    path: P,
    cfg: &Config,
    log: L,
    cancelled: C,
) -> ProcessResult
where
    P: AsRef<Path>,
    L: Fn(&str),
    C: Fn() -> bool,
{
    let path_ref = path.as_ref();
    let mut result = ProcessResult::new(path_ref);

    let work_dir = match tempdir() {
        Ok(d) => d,
        Err(e) => {
            result.status = Status::Error;
            result.error = Some(format!("Failed to create temp directory: {}", e));
            return result;
        }
    };

    let ext = path_ref
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_lowercase()))
        .unwrap_or_default();

    let stem = path_ref
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("book");
    let safe_stem = safe_filename(stem);
    let staged = work_dir.path().join(format!("{}.epub", safe_stem));

    if ext == ".epub" {
        if let Err(e) = fs::copy(path_ref, &staged) {
            result.status = Status::Error;
            result.error = Some(format!("Failed to copy file: {}", e));
            return result;
        }
    } else if cfg.do_convert && ebook_tools::CONVERTIBLE.contains(&ext.as_str()) {
        log(&format!("  converting {} -> epub", ext));
        match ebook_tools::convert_book(path_ref, &staged) {
            Ok(engine) => {
                result.changes.push(format!("converted {} -> epub ({})", ext, engine));
            }
            Err(e) => {
                result.status = Status::Error;
                result.error = Some(format!("Conversion failed: {}", e));
                return result;
            }
        }
    } else {
        result.status = Status::Skipped;
        let suffix = if cfg.do_convert {
            ""
        } else {
            " and conversion is off"
        };
        result.error = Some(format!(
            "{} is not an epub{}",
            if ext.is_empty() { "no extension" } else { &ext },
            suffix
        ));
        return result;
    }

    if cancelled() {
        result.status = Status::Skipped;
        result.error = Some("cancelled".to_string());
        return result;
    }

    if cfg.do_metadata {
        match ebook_tools::fix_metadata(&staged, Some(path_ref)) {
            Ok(changes) => result.changes.extend(changes),
            Err(e) => log(&format!("  metadata skipped: {}", e)),
        }
    }

    if let Ok(meta) = ebook_tools::read_metadata(&staged) {
        if let Some(t) = meta.get("title") {
            result.title = Some(t.clone());
        }
    }

    if cfg.do_cover {
        if ebook_tools::has_real_cover(&staged, 64) {
            log("  cover looks fine, leaving it");
        } else {
            match ebook_tools::fetch_cover(&staged) {
                Ok(changes) => {
                    if changes.is_empty() {
                        log("  no cover match on Open Library");
                    } else {
                        result.changes.extend(changes);
                    }
                }
                Err(e) => log(&format!("  cover skipped: {}", e)),
            }
        }
    }

    if cfg.do_fix {
        match epub_fix::fix_epub(&staged, true, None, &cfg.default_lang) {
            Ok(changes) => result.changes.extend(changes),
            Err(e) => {
                result.status = Status::Error;
                result.error = Some(format!("epub fix failed: {}", e));
                return result;
            }
        }
    }

    if cancelled() {
        result.status = Status::Skipped;
        result.error = Some("cancelled".to_string());
        return result;
    }

    if cfg.keep_copy && !cfg.output_dir.is_empty() {
        let out_dir = Path::new(&cfg.output_dir);
        let _ = fs::create_dir_all(out_dir);
        let final_path = out_dir.join(staged.file_name().unwrap());
        if let Ok(_) = fs::copy(&staged, &final_path) {
            result.output = Some(final_path);
        }
    }

    if cfg.do_send {
        match sender::send_book(cfg, &staged, result.title.as_deref()) {
            Ok(size) => {
                result.sent = true;
                log(&format!(
                    "  emailed {:.1} MB to {}",
                    size as f64 / 1_000_000.0,
                    cfg.kindle_addr
                ));
            }
            Err(e) => {
                result.status = Status::Error;
                result.error = Some(format!("Send failed: {}", e));
                return result;
            }
        }
    }

    result.status = Status::Ok;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_filename() {
        assert_eq!(safe_filename("Normal Title"), "Normal Title");
        assert_eq!(safe_filename("Title: With / Bad * Chars?"), "Title_ With _ Bad _ Chars_");
        assert_eq!(safe_filename(""), "book");
    }

    #[test]
    fn test_result_summary() {
        let mut r = ProcessResult::new("test.epub");
        assert_eq!(r.summary(), "no changes needed");

        r.sent = true;
        assert_eq!(r.summary(), "sent (no changes needed)");

        r.changes.push("fixed encoding".to_string());
        assert_eq!(r.summary(), "sent, 1 fix(es)");

        r.status = Status::Error;
        r.error = Some("SMTP timeout".to_string());
        assert_eq!(r.summary(), "SMTP timeout");
    }
}
