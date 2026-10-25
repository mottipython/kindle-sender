//! Reading and editing the metadata of an EPUB without calibre's `ebook-meta`.

use std::path::Path;

use regex::Regex;

use super::epub_writer::{esc, sniff_image};
use super::NativeError;
use crate::epub_fix::{decode_text, find_opf_path, read_all, repack};

#[derive(Debug, Default, Clone)]
pub struct Meta {
    pub title: Option<String>,
    pub authors: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub struct Edit {
    pub title: Option<String>,
    pub title_sort: Option<String>,
    /// Replaces every creator with this single author.
    pub author: Option<String>,
    pub author_sort: Option<String>,
}

fn load(path: &Path) -> Result<(Vec<String>, std::collections::HashMap<String, Vec<u8>>, String, String), NativeError> {
    let (names, files) = read_all(path).map_err(|e| NativeError::Corrupt(e.to_string()))?;
    let opf_path = find_opf_path(&files).ok_or_else(|| NativeError::Corrupt("EPUB has no OPF file".into()))?;
    let opf = files.get(&opf_path).map(|d| decode_text(d)).ok_or_else(|| NativeError::Corrupt("OPF missing from archive".into()))?;
    Ok((names, files, opf_path, opf))
}

fn save(path: &Path, names: &[String], mut files: std::collections::HashMap<String, Vec<u8>>, opf_path: &str, opf: String) -> Result<(), NativeError> {
    files.insert(opf_path.to_string(), opf.into_bytes());
    let tmp = path.with_extension("epub.tmp");
    repack(names, &files, &tmp).map_err(|e| NativeError::Corrupt(e.to_string()))?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn text_between(re: &Regex, s: &str) -> Vec<String> {
    re.captures_iter(s)
        .map(|c| decode_entities(c[1].trim()))
        .filter(|t| !t.is_empty())
        .collect()
}

fn decode_entities(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

pub fn read(path: &Path) -> Result<Meta, NativeError> {
    let (_, _, _, opf) = load(path)?;
    let title_re = Regex::new(r"(?is)<dc:title\b[^>]*>(.*?)</dc:title>").unwrap();
    let creator_re = Regex::new(r"(?is)<dc:creator\b[^>]*>(.*?)</dc:creator>").unwrap();
    Ok(Meta {
        title: text_between(&title_re, &opf).into_iter().next(),
        authors: text_between(&creator_re, &opf),
    })
}

fn has_opf_ns(opf: &str) -> bool {
    opf.contains("xmlns:opf")
}

fn insert_before_metadata_end(opf: &mut String, snippet: &str) {
    if let Some(i) = opf.find("</metadata>") {
        opf.insert_str(i, snippet);
    }
}

fn remove_all(opf: &mut String, pattern: &str) {
    let re = Regex::new(pattern).unwrap();
    *opf = re.replace_all(opf, "").into_owned();
}

fn set_meta(opf: &mut String, name: &str, value: Option<&str>) {
    remove_all(opf, &format!(r#"(?is)\s*<meta\s+name\s*=\s*["']{}["'][^>]*/?>(</meta>)?"#, regex::escape(name)));
    if let Some(v) = value {
        insert_before_metadata_end(opf, &format!("<meta name=\"{}\" content=\"{}\"/>\n", name, esc(v)));
    }
}

fn apply(opf: &mut String, edit: &Edit) {
    if let Some(t) = &edit.title {
        let re = Regex::new(r"(?is)<dc:title\b[^>]*>.*?</dc:title>").unwrap();
        let new = format!("<dc:title>{}</dc:title>", esc(t));
        if re.is_match(opf) {
            let mut done = false;
            *opf = re
                .replace_all(opf, |_: &regex::Captures| {
                    if done {
                        String::new()
                    } else {
                        done = true;
                        new.clone()
                    }
                })
                .into_owned();
        } else {
            insert_before_metadata_end(opf, &format!("{}\n", new));
        }
        set_meta(opf, "calibre:title_sort", edit.title_sort.as_deref());
    }
    if let Some(a) = &edit.author {
        remove_all(opf, r"(?is)\s*<dc:creator\b[^>]*?(/>|>.*?</dc:creator>)");
        let file_as = match (&edit.author_sort, has_opf_ns(opf)) {
            (Some(s), true) => format!(" opf:file-as=\"{}\"", esc(s)),
            _ => String::new(),
        };
        let role = if has_opf_ns(opf) { " opf:role=\"aut\"" } else { "" };
        insert_before_metadata_end(opf, &format!("<dc:creator{}{}>{}</dc:creator>\n", role, file_as, esc(a)));
    }
}

pub fn edit(path: &Path, e: &Edit) -> Result<(), NativeError> {
    let (names, files, opf_path, mut opf) = load(path)?;
    apply(&mut opf, e);
    save(path, &names, files, &opf_path, opf)
}

/// Replaces the epub's cover with `image` (JPEG/PNG/GIF bytes).
pub fn set_cover(path: &Path, image: &[u8]) -> Result<(), NativeError> {
    let (ext, media) = sniff_image(image).ok_or_else(|| NativeError::Corrupt("cover is not a JPEG, PNG or GIF".into()))?;
    let (mut names, mut files, opf_path, mut opf) = load(path)?;
    let dir = opf_path.rsplit_once('/').map(|x| x.0).unwrap_or("");
    let href = format!("ks-cover.{}", ext);
    let full = if dir.is_empty() { href.clone() } else { format!("{}/{}", dir, href) };

    set_meta(&mut opf, "cover", None);
    remove_all(&mut opf, r#"(?is)\s*<item\b[^>]*\bid\s*=\s*["']ks-cover-image["'][^>]*/>"#);
    set_meta(&mut opf, "cover", Some("ks-cover-image"));
    if let Some(i) = opf.find("</manifest>") {
        opf.insert_str(i, &format!("<item id=\"ks-cover-image\" href=\"{}\" media-type=\"{}\"/>\n", href, media));
    } else {
        return Err(NativeError::Corrupt("OPF has no manifest".into()));
    }
    if !files.contains_key(&full) {
        names.push(full.clone());
    }
    files.insert(full, image.to_vec());
    save(path, &names, files, &opf_path, opf)
}

/// Copies title, authors, publisher, description and series from a calibre-style `.opf` next to the source.
pub fn merge_opf(path: &Path, sidecar: &Path) -> Result<(), NativeError> {
    let side = decode_text(&std::fs::read(sidecar)?);
    let one = |tag: &str| {
        let re = Regex::new(&format!(r"(?is)<dc:{0}\b[^>]*>(.*?)</dc:{0}>", tag)).unwrap();
        text_between(&re, &side)
    };
    let meta_re = |name: &str| {
        Regex::new(&format!(r#"(?is)<meta\s+name\s*=\s*["']{}["']\s+content\s*=\s*["']([^"']*)["']"#, regex::escape(name))).unwrap()
    };

    let (names, files, opf_path, mut opf) = load(path)?;
    if let Some(t) = one("title").into_iter().next() {
        apply(&mut opf, &Edit { title: Some(t), ..Default::default() });
    }
    let authors = one("creator");
    if !authors.is_empty() {
        remove_all(&mut opf, r"(?is)\s*<dc:creator\b[^>]*?(/>|>.*?</dc:creator>)");
        let role = if has_opf_ns(&opf) { " opf:role=\"aut\"" } else { "" };
        for a in authors {
            insert_before_metadata_end(&mut opf, &format!("<dc:creator{}>{}</dc:creator>\n", role, esc(&a)));
        }
    }
    for tag in ["publisher", "description"] {
        if let Some(v) = one(tag).into_iter().next() {
            remove_all(&mut opf, &format!(r"(?is)\s*<dc:{0}\b[^>]*>.*?</dc:{0}>", tag));
            insert_before_metadata_end(&mut opf, &format!("<dc:{0}>{1}</dc:{0}>\n", tag, esc(&v)));
        }
    }
    for name in ["calibre:series", "calibre:series_index", "calibre:title_sort"] {
        if let Some(c) = meta_re(name).captures(&side) {
            set_meta(&mut opf, name, Some(&decode_entities(&c[1])));
        }
    }
    save(path, &names, files, &opf_path, opf)
}
