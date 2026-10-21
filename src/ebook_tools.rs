use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use image::GenericImageView;
use regex::Regex;
use serde::Deserialize;
use thiserror::Error;

pub const CONVERTIBLE: &[&str] = &[
    ".mobi", ".azw", ".azw3", ".prc", ".lit", ".pdf", ".fb2", ".rtf",
    ".txt", ".doc", ".docx", ".html", ".htm", ".cbz", ".cbr", ".pdb",
];

pub const METADATA_POOR: &[&str] = &[
    ".txt", ".rtf", ".html", ".htm", ".pdf", ".cbz", ".cbr",
    ".doc", ".docx", ".pdb",
];

pub const OPEN_LIBRARY: &str = "https://openlibrary.org/search.json";
pub const COVER_URL: &str = "https://covers.openlibrary.org/b/id/{cover_id}-L.jpg";
pub const USER_AGENT: &str = "kindle-sender/1.0 (personal ebook tool)";

#[derive(Error, Debug)]
pub enum ToolError {
    #[error("tool error: {0}")]
    Failed(String),
    #[error("tool not found: {0}")]
    NotFound(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Network error: {0}")]
    Network(String),
}

pub fn which(name: &str) -> Result<PathBuf, ToolError> {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(meta) = candidate.metadata() {
                        if meta.permissions().mode() & 0o111 != 0 {
                            return Ok(candidate);
                        }
                    }
                }
                #[cfg(not(unix))]
                return Ok(candidate);
            }
        }
    }
    Err(ToolError::NotFound(format!(
        "{} not found — install calibre (e.g. sudo dnf install calibre)",
        name
    )))
}

pub fn tools_available() -> HashMap<String, bool> {
    let mut map = HashMap::new();
    map.insert("ebook-convert".to_string(), which("ebook-convert").is_ok());
    map.insert("ebook-meta".to_string(), which("ebook-meta").is_ok());
    map
}

pub fn run_tool(args: &[&str]) -> Result<String, ToolError> {
    if args.is_empty() {
        return Err(ToolError::Failed("empty command".to_string()));
    }
    let prog = which(args[0])?;
    let output = Command::new(prog)
        .args(&args[1..])
        .output()
        .map_err(|e| ToolError::Failed(e.to_string()))?;

    if !output.status.success() {
        let err_text = String::from_utf8_lossy(&output.stderr);
        let out_text = String::from_utf8_lossy(&output.stdout);
        let text = if !err_text.trim().is_empty() {
            err_text
        } else {
            out_text
        };
        let last_line = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("failed");
        return Err(ToolError::Failed(last_line.to_string()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn convert_to_epub<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dest: Q) -> Result<PathBuf, ToolError> {
    let src_str = src.as_ref().to_string_lossy();
    let dest_str = dest.as_ref().to_string_lossy();

    run_tool(&["ebook-convert", &src_str, &dest_str])?;
    if !dest.as_ref().exists() {
        return Err(ToolError::Failed("ebook-convert produced no output".to_string()));
    }
    Ok(dest.as_ref().to_path_buf())
}

pub fn read_metadata<P: AsRef<Path>>(path: P) -> Result<HashMap<String, String>, ToolError> {
    let path_str = path.as_ref().to_string_lossy();
    let out = run_tool(&["ebook-meta", &path_str])?;

    let meta_re = Regex::new(r"^([^:]+?)\s*:\s*(.*)$").unwrap();
    let mut meta = HashMap::new();

    for line in out.lines() {
        if let Some(caps) = meta_re.captures(line) {
            let key = caps.get(1).unwrap().as_str().trim().to_lowercase();
            let val = caps.get(2).unwrap().as_str().trim().to_string();
            meta.insert(key, val);
        }
    }

    Ok(meta)
}

pub fn guess_from_filename<P: AsRef<Path>>(path: P) -> (Option<String>, Option<String>) {
    let stem = match path.as_ref().file_stem().and_then(|s| s.to_str()) {
        Some(s) => s.replace('_', " ").trim().to_string(),
        None => return (None, None),
    };

    let parts_re = Regex::new(r"\s+-\s+").unwrap();
    let parts: Vec<&str> = parts_re.split(&stem).map(|s| s.trim()).filter(|s| !s.is_empty()).collect();

    if parts.len() >= 2 {
        let first = parts[0];
        let last = parts[parts.len() - 1];
        // "Author - Title" if first chunk looks like a person's name (<= 4 words, not starting with digit)
        if first.split_whitespace().count() <= 4 && !first.starts_with(|c: char| c.is_ascii_digit()) {
            return (Some(last.to_string()), Some(first.to_string()));
        }
        return (Some(first.to_string()), None);
    }

    if !stem.is_empty() {
        (Some(stem), None)
    } else {
        (None, None)
    }
}

pub fn sidecar_opf<P: AsRef<Path>>(src_path: P) -> Option<PathBuf> {
    let parent = src_path.as_ref().parent()?;
    let opf = parent.join("metadata.opf");
    if opf.is_file() {
        Some(opf)
    } else {
        None
    }
}

pub fn sort_name(author: &str) -> String {
    let parts: Vec<&str> = author.split_whitespace().collect();
    if parts.len() < 2 {
        return author.to_string();
    }
    let last = parts[parts.len() - 1];
    let rest = parts[..parts.len() - 1].join(" ");
    format!("{}, {}", last, rest)
}

fn placeholder_title(title: Option<&str>, path: &Path) -> bool {
    let title = match title {
        Some(t) => t.trim().to_lowercase(),
        None => return true,
    };
    if title.is_empty() || title == "unknown" {
        return true;
    }
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        if title == stem.to_lowercase() {
            return true;
        }
    }
    false
}

fn placeholder_author(author: Option<&str>, title: Option<&str>, stem: Option<&str>) -> bool {
    let val = match author {
        Some(a) => a.trim().to_lowercase(),
        None => return true,
    };
    if val.is_empty() || val == "unknown" || val == "unknown author" || val == "anonymous" || val == "calibre" {
        return true;
    }
    if let Some(t) = title {
        if val == t.trim().to_lowercase() {
            return true;
        }
    }
    if let Some(s) = stem {
        let s_lower = s.trim().to_lowercase();
        if val == s_lower || val == s_lower.replace('_', " ") {
            return true;
        }
    }
    false
}

pub fn fix_metadata<P: AsRef<Path>>(epub_path: P, source_path: Option<&Path>) -> Result<Vec<String>, ToolError> {
    let epub_ref = epub_path.as_ref();
    let src_ref = source_path.unwrap_or(epub_ref);
    let mut changes = Vec::new();

    if let Some(opf) = sidecar_opf(src_ref) {
        let opf_str = opf.to_string_lossy();
        let epub_str = epub_ref.to_string_lossy();
        run_tool(&["ebook-meta", &epub_str, "--from-opf", &opf_str])?;
        changes.push(format!("metadata: embedded from {}", opf.file_name().unwrap().to_string_lossy()));
        return Ok(changes);
    }

    let meta = read_metadata(epub_ref)?;
    let title = meta.get("title").map(|s| s.as_str());
    let mut author = meta.get("author(s)").map(|s| s.as_str());

    // Strip trailing brackets [tags]
    let bracket_re = Regex::new(r"\s*\[[^\]]*\]\s*$").unwrap();
    let cleaned_author;
    if let Some(a) = author {
        cleaned_author = bracket_re.replace(a, "").trim().to_string();
        author = Some(&cleaned_author);
    }

    let stem = src_ref.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let (guess_title, guess_author) = guess_from_filename(src_ref);

    let ext = src_ref.extension().and_then(|e| e.to_str()).map(|e| format!(".{}", e.to_lowercase())).unwrap_or_default();
    let trust_filename = METADATA_POOR.contains(&ext.as_str());

    let mut args: Vec<String> = vec!["ebook-meta".to_string(), epub_ref.to_string_lossy().to_string()];

    if let Some(ref gt) = guess_title {
        if trust_filename || placeholder_title(title, src_ref) {
            args.push("-t".to_string());
            args.push(gt.clone());
            args.push("--title-sort".to_string());
            args.push(gt.clone());
            changes.push(format!("metadata: title -> {:?}", gt));
        }
    }

    if let Some(ref ga) = guess_author {
        if trust_filename || placeholder_author(author, title, Some(stem)) {
            args.push("-a".to_string());
            args.push(ga.clone());
            args.push("--author-sort".to_string());
            args.push(sort_name(ga));
            changes.push(format!("metadata: author -> {:?}", ga));
        }
    } else if placeholder_author(author, title, Some(stem)) {
        if let Some(a) = author {
            if a.to_lowercase() != "unknown" && !a.is_empty() {
                args.push("-a".to_string());
                args.push("Unknown".to_string());
                changes.push(format!("metadata: cleared bogus author {:?}", a));
            }
        }
    }

    if args.len() > 2 {
        let args_slice: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        run_tool(&args_slice)?;
    }

    Ok(changes)
}

pub fn has_real_cover<P: AsRef<Path>>(epub_path: P, max_colors: usize) -> bool {
    let file = match File::open(epub_path.as_ref()) {
        Ok(f) => f,
        Err(_) => return true,
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(_) => return true,
    };

    let cover_re1 = Regex::new(r"(?i)cover.*\.(jpe?g|png)$").unwrap();
    let cover_re2 = Regex::new(r"(?i)\.(jpe?g|png)$").unwrap();

    let mut candidates = Vec::new();
    for i in 0..archive.len() {
        if let Ok(entry) = archive.by_index(i) {
            let name = entry.name().to_string();
            if cover_re1.is_match(&name) || (cover_re2.is_match(&name) && name.to_lowercase().contains("cover")) {
                candidates.push(name);
            }
        }
    }

    if candidates.is_empty() {
        return false;
    }

    candidates.sort_by_key(|n| n.len());
    let best_name = &candidates[0];

    let mut entry = match archive.by_name(best_name) {
        Ok(e) => e,
        Err(_) => return true,
    };

    let mut data = Vec::new();
    if entry.read_to_end(&mut data).is_err() {
        return true;
    }

    let img = match image::load_from_memory(&data) {
        Ok(im) => im,
        Err(_) => return true,
    };

    let thumb = img.thumbnail(256, 256);
    let mut colors = HashSet::new();
    for (_x, _y, pixel) in thumb.pixels() {
        colors.insert(pixel.0);
        if colors.len() > max_colors {
            return true;
        }
    }

    false
}

#[derive(Deserialize)]
struct OpenLibraryDoc {
    title: Option<String>,
    author_name: Option<Vec<String>>,
    cover_i: Option<i64>,
}

#[derive(Deserialize)]
struct OpenLibraryResponse {
    docs: Option<Vec<OpenLibraryDoc>>,
}

pub fn search_openlibrary_cover(title: &str, author: Option<&str>) -> Option<String> {
    let mut query = vec![("title", title), ("limit", "5")];
    if let Some(a) = author {
        query.push(("author", a));
    }

    let resp = ureq::get(OPEN_LIBRARY)
        .set("User-Agent", USER_AGENT)
        .query_pairs(query)
        .timeout(std::time::Duration::from_secs(15))
        .call()
        .ok()?;

    let ol_data: OpenLibraryResponse = resp.into_json().ok()?;
    let docs = ol_data.docs?;

    let word_re = Regex::new(r"\w+").unwrap();
    let want: HashSet<String> = word_re
        .find_iter(&title.to_lowercase())
        .map(|m| m.as_str().to_string())
        .filter(|w| w.len() > 2)
        .collect();

    let surname = author.and_then(|a| a.split_whitespace().last()).map(|s| s.to_lowercase());

    for doc in docs {
        let cover_id = match doc.cover_i {
            Some(id) if id > 0 => id,
            _ => continue,
        };

        let doc_title = doc.title.unwrap_or_default().to_lowercase();
        let got: HashSet<String> = word_re
            .find_iter(&doc_title)
            .map(|m| m.as_str().to_string())
            .filter(|w| w.len() > 2)
            .collect();

        if !want.is_empty() && want.is_disjoint(&got) {
            continue;
        }

        if let Some(ref sur) = surname {
            let authors = doc.author_name.unwrap_or_default().join(" ").to_lowercase();
            if !authors.contains(sur) {
                continue;
            }
        }

        return Some(COVER_URL.replace("{cover_id}", &cover_id.to_string()));
    }

    None
}

pub fn fetch_cover<P: AsRef<Path>, Q: AsRef<Path>>(
    epub_path: P,
    work_dir: Q,
) -> Result<Vec<String>, ToolError> {
    let meta = read_metadata(epub_path.as_ref())?;
    let title = meta.get("title").map(|s| s.as_str()).unwrap_or("");
    let bracket_re = Regex::new(r"\s*\[[^\]]*\]\s*$").unwrap();
    let author = meta.get("author(s)").map(|s| bracket_re.replace(s, "").trim().to_string());

    if title.is_empty() || title.eq_ignore_ascii_case("unknown") {
        return Ok(Vec::new());
    }

    let url = match search_openlibrary_cover(title, author.as_deref()) {
        Some(u) => u,
        None => return Ok(Vec::new()),
    };

    let resp = ureq::get(&url)
        .set("User-Agent", USER_AGENT)
        .timeout(std::time::Duration::from_secs(15))
        .call()
        .map_err(|e| ToolError::Network(e.to_string()))?;

    let mut data = Vec::new();
    resp.into_reader()
        .read_to_end(&mut data)
        .map_err(|e| ToolError::Network(e.to_string()))?;

    if data.len() < 3000 {
        return Ok(Vec::new());
    }

    let cover_path = work_dir.as_ref().join("cover.jpg");
    let mut f = File::create(&cover_path)?;
    f.write_all(&data)?;

    let epub_str = epub_path.as_ref().to_string_lossy();
    let cover_str = cover_path.to_string_lossy();
    run_tool(&["ebook-meta", &epub_str, "--cover", &cover_str])?;

    Ok(vec![format!("cover: fetched from Open Library for {:?}", title)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guess_from_filename() {
        let (title, author) = guess_from_filename("Terry Pratchett - Mort.epub");
        assert_eq!(title, Some("Mort".to_string()));
        assert_eq!(author, Some("Terry Pratchett".to_string()));

        let (title, author) = guess_from_filename("01 - First Chapter.epub");
        assert_eq!(title, Some("01".to_string()));
        assert_eq!(author, None);

        let (title, author) = guess_from_filename("JustTitle.mobi");
        assert_eq!(title, Some("JustTitle".to_string()));
        assert_eq!(author, None);
    }

    #[test]
    fn test_sort_name() {
        assert_eq!(sort_name("Terry Pratchett"), "Pratchett, Terry");
        assert_eq!(sort_name("H. G. Wells"), "Wells, H. G.");
        assert_eq!(sort_name("Plato"), "Plato");
    }
}
