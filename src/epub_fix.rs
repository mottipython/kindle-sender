use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use regex::Regex;
use thiserror::Error;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

pub const XHTML_EXT: &[&str] = &[".xhtml", ".html", ".htm", ".xht"];

#[derive(Error, Debug)]
pub enum EpubFixError {
    #[error("not a zip archive — is this really an epub? {0}")]
    NotAZip(String),
    #[error("corrupt entry in archive: {0}")]
    CorruptEntry(String),
    #[error("validation error: {0}")]
    ValidationError(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
}

/// Normalize posix paths inside zip archives (e.g. OEBPS/sub/../image.jpg -> OEBPS/image.jpg)
pub fn normalize_posix_path(path: &str) -> String {
    let mut parts = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        } else if part == ".." {
            parts.pop();
        } else {
            parts.push(part);
        }
    }
    parts.join("/")
}

/// Join posix base directory and relative path, then normalize
pub fn join_posix_path(base_dir: &str, relative: &str) -> String {
    if relative.starts_with('/') {
        normalize_posix_path(&relative[1..])
    } else if base_dir.is_empty() {
        normalize_posix_path(relative)
    } else {
        normalize_posix_path(&format!("{}/{}", base_dir, relative))
    }
}

/// Get directory component of a posix path
pub fn posix_dirname(path: &str) -> &str {
    match path.rfind('/') {
        Some(idx) => &path[..idx],
        None => "",
    }
}

/// Read all entries from an EPUB archive into memory.
pub fn read_all<P: AsRef<Path>>(path: P) -> Result<(Vec<String>, HashMap<String, Vec<u8>>), EpubFixError> {
    let file = File::open(path.as_ref()).map_err(|e| EpubFixError::NotAZip(e.to_string()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| EpubFixError::NotAZip(e.to_string()))?;

    let mut names = Vec::new();
    let mut files = HashMap::new();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| EpubFixError::CorruptEntry(e.to_string()))?;
        let name = entry.name().to_string();
        let mut data = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut data).map_err(|e| EpubFixError::CorruptEntry(format!("{}: {}", name, e)))?;
        names.push(name.clone());
        files.insert(name, data);
    }

    Ok((names, files))
}

/// Decode raw bytes to String, trying UTF-8, then latin-1 fallback.
pub fn decode_text(data: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(data) {
        return s.to_string();
    }
    // Latin-1 fallback: each byte maps to char U+0000..U+00FF
    data.iter().map(|&b| b as char).collect()
}

/// Locate OPF path from container.xml, or fallback to scanning for .opf
pub fn find_opf_path(files: &HashMap<String, Vec<u8>>) -> Option<String> {
    if let Some(container_data) = files.get("META-INF/container.xml") {
        let text = decode_text(container_data);
        let re = Regex::new(r#"full-path\s*=\s*["']([^"']+)["']"#).unwrap();
        if let Some(caps) = re.captures(&text) {
            return Some(caps.get(1).unwrap().as_str().to_string());
        }
    }
    for name in files.keys() {
        if name.to_lowercase().ends_with(".opf") {
            return Some(name.clone());
        }
    }
    None
}

/// Fix 1: XML declaration and encoding
pub fn fix_encoding(files: &mut HashMap<String, Vec<u8>>, changes: &mut Vec<String>) {
    let xml_decl_re = Regex::new(r"(?i)^\s*<\?xml[^>]*\?>").unwrap();
    let encoding_re = Regex::new(r#"(?i)encoding\s*=\s*["'][^"']*["']"#).unwrap();

    let names: Vec<String> = files.keys().cloned().collect();
    for name in names {
        let lower = name.to_lowercase();
        if !XHTML_EXT.iter().any(|ext| lower.ends_with(ext)) {
            continue;
        }

        let text = decode_text(&files[&name]);
        if let Some(mat) = xml_decl_re.find(&text) {
            let decl = mat.as_str();
            if !encoding_re.is_match(decl) {
                let trimmed = decl.trim_end();
                let without_end = if trimmed.ends_with("?>") {
                    &trimmed[..trimmed.len() - 2].trim_end()
                } else {
                    trimmed
                };
                let fixed_decl = format!(r#"{} encoding="utf-8"?>"#, without_end);
                let new_text = format!("{}{}", fixed_decl, &text[mat.end()..]);
                files.insert(name.clone(), new_text.into_bytes());
                changes.push(format!("encoding: added utf-8 to declaration in {}", name));
            }
        } else {
            let new_text = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n{}", text);
            files.insert(name.clone(), new_text.into_bytes());
            changes.push(format!("encoding: added XML declaration to {}", name));
        }
    }
}

/// Fix 2: Language code in OPF
pub fn fix_language(files: &mut HashMap<String, Vec<u8>>, changes: &mut Vec<String>, default_lang: &str) {
    let opf_path = match find_opf_path(files) {
        Some(p) => p,
        None => return,
    };

    let data = match files.get(&opf_path) {
        Some(d) => d,
        None => return,
    };

    let text = decode_text(data);
    let dc_lang_re = Regex::new(r"(?is)<(dc:language|language)\b([^>]*)>(.*?)</(dc:language|language)>").unwrap();
    let metadata_open_re = Regex::new(r"(?i)<(\w*:?metadata)\b[^>]*>").unwrap();
    let lang_code_re = Regex::new(r"(?i)^[a-z]{2,3}(-[a-z0-9]{2,8})*$").unwrap();
    let bad_langs: HashSet<&str> = ["", "und", "unknown", "none", "null", "xx"].iter().cloned().collect();

    if let Some(caps) = dc_lang_re.captures(&text) {
        let full_match = caps.get(0).unwrap();
        let tag_name = caps.get(1).unwrap().as_str();
        let attrs = caps.get(2).unwrap().as_str();
        let current_val = caps.get(3).unwrap().as_str().trim();
        let close_tag = caps.get(4).unwrap().as_str();
        if !tag_name.eq_ignore_ascii_case(close_tag) {
            return;
        }

        if bad_langs.contains(current_val.to_lowercase().as_str()) || !lang_code_re.is_match(current_val) {
            let new_tag = format!("<{}{}>{}</{}>", tag_name, attrs, default_lang, tag_name);
            let mut new_text = String::with_capacity(text.len());
            new_text.push_str(&text[..full_match.start()]);
            new_text.push_str(&new_tag);
            new_text.push_str(&text[full_match.end()..]);

            files.insert(opf_path, new_text.into_bytes());
            changes.push(format!("language: {:?} -> {:?}", current_val, default_lang));
        }
        return;
    }

    // No language tag found — insert right after <metadata>
    if let Some(mat) = metadata_open_re.find(&text) {
        let mut new_text = String::with_capacity(text.len() + 60);
        new_text.push_str(&text[..mat.end()]);
        new_text.push_str(&format!("\n    <dc:language>{}</dc:language>", default_lang));
        new_text.push_str(&text[mat.end()..]);

        files.insert(opf_path, new_text.into_bytes());
        changes.push(format!("language: missing -> {:?}", default_lang));
    }
}

/// Fix 3: Body ID links that dead-end on Kindle
pub fn fix_body_id_links(files: &mut HashMap<String, Vec<u8>>, changes: &mut Vec<String>) {
    let body_id_re = Regex::new(r#"(?i)<body\b[^>]*\bid\s*=\s*["']([^"']+)["']"#).unwrap();
    let href_re = Regex::new(r#"(?i)(href\s*=\s*)(["'])([^"']+)(["'])"#).unwrap();

    let mut body_ids: HashMap<String, String> = HashMap::new();
    for (name, data) in files.iter() {
        let lower = name.to_lowercase();
        if !XHTML_EXT.iter().any(|ext| lower.ends_with(ext)) {
            continue;
        }
        let text = decode_text(data);
        if let Some(caps) = body_id_re.captures(&text) {
            body_ids.insert(name.clone(), caps.get(1).unwrap().as_str().to_string());
        }
    }

    if body_ids.is_empty() {
        return;
    }

    let names: Vec<String> = files.keys().cloned().collect();
    for name in names {
        let lower = name.to_lowercase();
        if !XHTML_EXT.iter().any(|ext| lower.ends_with(ext)) {
            continue;
        }

        let text = decode_text(&files[&name]);
        let base_dir = posix_dirname(&name);
        let mut hit_count = 0;

        let new_text = href_re.replace_all(&text, |caps: &regex::Captures| -> String {
            let prefix = caps.get(1).unwrap().as_str();
            let quote1 = caps.get(2).unwrap().as_str();
            let href = caps.get(3).unwrap().as_str();
            let quote2 = caps.get(4).unwrap().as_str();
            if quote1 != quote2 {
                return caps.get(0).unwrap().as_str().to_string();
            }

            if !href.contains('#') || href.starts_with("http:") || href.starts_with("https:") || href.starts_with("mailto:") {
                return caps.get(0).unwrap().as_str().to_string();
            }

            let mut parts = href.splitn(2, '#');
            let target = parts.next().unwrap_or("");
            let frag = parts.next().unwrap_or("");

            let resolved = if target.is_empty() {
                name.clone()
            } else {
                join_posix_path(base_dir, target)
            };

            if let Some(expected_id) = body_ids.get(&resolved) {
                if expected_id == frag {
                    hit_count += 1;
                    return format!("{}{}{}{}", prefix, quote1, target, quote2);
                }
            }

            caps.get(0).unwrap().as_str().to_string()
        });

        if hit_count > 0 {
            files.insert(name.clone(), new_text.into_owned().into_bytes());
            changes.push(format!("body-id: stripped {} link fragment(s) in {}", hit_count, name));
        }
    }
}

/// Fix 4: Stray images that crash Kindle's renderer
pub fn fix_stray_images(files: &mut HashMap<String, Vec<u8>>, changes: &mut Vec<String>) {
    let present: HashSet<String> = files.keys().cloned().collect();
    let img_tag_re = Regex::new(r"(?is)<img\b[^>]*?/?>").unwrap();
    let img_src_re = Regex::new(r#"(?i)\bsrc\s*=\s*["']([^"']*)["']"#).unwrap();

    let names: Vec<String> = files.keys().cloned().collect();
    for name in names {
        let lower = name.to_lowercase();
        if !XHTML_EXT.iter().any(|ext| lower.ends_with(ext)) {
            continue;
        }

        let text = decode_text(&files[&name]);
        let base_dir = posix_dirname(&name);
        let mut removed_count = 0;

        let new_text = img_tag_re.replace_all(&text, |caps: &regex::Captures| -> String {
            let tag = caps.get(0).unwrap().as_str();
            let src = match img_src_re.captures(tag) {
                Some(c) => c.get(1).unwrap().as_str().trim(),
                None => "",
            };

            if src.starts_with("http:") || src.starts_with("https:") || src.starts_with("data:") {
                return tag.to_string();
            }

            if !src.is_empty() {
                let clean_src = src.split('#').next().unwrap_or("");
                let resolved = join_posix_path(base_dir, clean_src);
                if present.contains(&resolved) {
                    return tag.to_string();
                }
            }

            removed_count += 1;
            String::new()
        });

        if removed_count > 0 {
            files.insert(name.clone(), new_text.into_owned().into_bytes());
            changes.push(format!("stray-img: removed {} broken <img> in {}", removed_count, name));
        }
    }
}

/// Repack the EPUB according to the spec:
/// mimetype must be first and uncompressed (Stored),
/// while other files are Deflated.
pub fn repack<P: AsRef<Path>>(
    names: &[String],
    files: &HashMap<String, Vec<u8>>,
    dest_path: P,
) -> Result<(), EpubFixError> {
    let file = File::create(dest_path.as_ref())?;
    let mut zip = zip::ZipWriter::new(file);

    let mut order = Vec::new();
    for n in names {
        if files.contains_key(n) {
            order.push(n.clone());
        }
    }
    for n in files.keys() {
        if !order.contains(n) {
            order.push(n.clone());
        }
    }

    if let Some(pos) = order.iter().position(|x| x == "mimetype") {
        order.remove(pos);
    }
    order.insert(0, "mimetype".to_string());

    let mimetype_content = files
        .get("mimetype")
        .map(|v| v.as_slice())
        .unwrap_or(b"application/epub+zip");

    // Write mimetype stored (uncompressed)
    let stored_options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", stored_options)?;
    zip.write_all(mimetype_content)?;

    // Write all other entries deflated
    let deflated_options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for name in &order[1..] {
        if let Some(data) = files.get(name) {
            zip.start_file(name, deflated_options)?;
            zip.write_all(data)?;
        }
    }

    zip.finish()?;
    Ok(())
}

/// Validate that repacked EPUB matches all requirements.
pub fn validate_epub<P: AsRef<Path>>(path: P) -> Result<(), EpubFixError> {
    let file = File::open(path.as_ref())?;
    let mut archive = zip::ZipArchive::new(file)?;

    if archive.is_empty() {
        return Err(EpubFixError::ValidationError("archive is empty".to_string()));
    }

    // Verify first entry is mimetype
    {
        let mut first_entry = archive.by_index(0)?;
        if first_entry.name() != "mimetype" {
            return Err(EpubFixError::ValidationError("mimetype is not the first entry".to_string()));
        }
        let mut mimetype_data = Vec::new();
        first_entry.read_to_end(&mut mimetype_data)?;
        if mimetype_data.trim_ascii() != b"application/epub+zip" {
            return Err(EpubFixError::ValidationError("mimetype content is wrong".to_string()));
        }
    }

    // Read and verify all entries
    let mut names = Vec::new();
    let mut container_content = None;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        if name == "META-INF/container.xml" {
            container_content = Some(buf);
        }
        names.push(name);
    }

    let container_data = container_content
        .ok_or_else(|| EpubFixError::ValidationError("META-INF/container.xml missing".to_string()))?;

    let mut temp_files = HashMap::new();
    temp_files.insert("META-INF/container.xml".to_string(), container_data);
    if let Some(opf) = find_opf_path(&temp_files) {
        if !names.contains(&opf) {
            return Err(EpubFixError::ValidationError(format!("OPF {} referenced but missing", opf)));
        }
    }

    Ok(())
}

/// Apply all Kindle fixes to `path`.
pub fn fix_epub<P: AsRef<Path>>(
    path: P,
    in_place: bool,
    dest: Option<&Path>,
    default_lang: &str,
) -> Result<Vec<String>, EpubFixError> {
    let path_ref = path.as_ref();
    let (names, mut files) = read_all(path_ref)?;

    let mut changes = Vec::new();
    fix_encoding(&mut files, &mut changes);
    fix_language(&mut files, &mut changes, default_lang);
    fix_body_id_links(&mut files, &mut changes);
    fix_stray_images(&mut files, &mut changes);

    let target_path = dest.unwrap_or(path_ref);
    if changes.is_empty() && in_place && target_path == path_ref {
        return Ok(changes);
    }

    let target_dir = target_path.parent().unwrap_or_else(|| Path::new("."));
    let temp_file = tempfile::Builder::new()
        .suffix(".epub")
        .tempfile_in(target_dir)?;

    let temp_path = temp_file.path().to_path_buf();
    // Close temp_file handle so repack can overwrite it
    drop(temp_file);

    let repack_result = repack(&names, &files, &temp_path);
    if let Err(err) = repack_result {
        let _ = std::fs::remove_file(&temp_path);
        return Err(err);
    }

    let val_result = validate_epub(&temp_path);
    if let Err(err) = val_result {
        let _ = std::fs::remove_file(&temp_path);
        return Err(err);
    }

    std::fs::rename(&temp_path, target_path)?;
    Ok(changes)
}
