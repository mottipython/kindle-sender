//! Plain text, HTML and FictionBook (FB2) to EPUB.

use std::collections::HashMap;
use std::path::Path;

use base64::Engine;
use regex::Regex;

use super::epub_writer::{esc, sniff_image, Book, Chapter, Resource, TocEntry};
use super::xhtml::{self, Hook, Hooks};
use super::{book_id, decode_bytes, fix_links, NativeError};

fn stem(path: &Path) -> String {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or("book").replace('_', " ")
}

fn chapter_file(i: usize) -> String {
    format!("chapter{}.xhtml", i + 1)
}

fn finish(mut book: Book, titles: Vec<Option<String>>) -> Book {
    for (i, ch) in book.chapters.iter().enumerate() {
        let title = titles.get(i).cloned().flatten().unwrap_or_else(|| ch.title.clone());
        book.toc.push(TocEntry { title, href: ch.file.clone() });
    }
    book
}

// ---------------------------------------------------------------- plain text

pub fn convert_txt(path: &Path) -> Result<Book, NativeError> {
    let raw = std::fs::read(path)?;
    let text = decode_bytes(&raw).replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = text.lines().collect();
    if lines.iter().all(|l| l.trim().is_empty()) {
        return Err(NativeError::Corrupt("the text file is empty".into()));
    }

    let blank = lines.iter().filter(|l| l.trim().is_empty()).count();
    let paragraph_mode = blank * 100 / lines.len().max(1) >= 15;

    // Group lines into paragraphs.
    let mut paras: Vec<String> = Vec::new();
    if paragraph_mode {
        let mut cur: Vec<&str> = Vec::new();
        for l in &lines {
            if l.trim().is_empty() {
                if !cur.is_empty() {
                    paras.push(cur.join(" "));
                    cur.clear();
                }
            } else {
                cur.push(l.trim());
            }
        }
        if !cur.is_empty() {
            paras.push(cur.join(" "));
        }
    } else {
        paras = lines.iter().filter(|l| !l.trim().is_empty()).map(|l| l.trim().to_string()).collect();
    }

    Ok(book_from_paragraphs(stem(path), paras))
}

/// Builds chapters from paragraphs: headings ("Chapter 3", "Part II", ...) start a new
/// chapter, and long runs without one are split every ~60 KB.
pub fn book_from_paragraphs(title: String, paras: Vec<String>) -> Book {
    let heading = Regex::new(r"(?i)^(chapter|part|book|prologue|epilogue|preface|introduction|foreword|afterword|section)\b.{0,70}$|^([IVXLCDM]{1,8}|\d{1,3})\.?$").unwrap();
    let is_heading = |p: &str| p.chars().count() <= 80 && heading.is_match(p);

    let mut book = Book {
        title,
        language: "en".into(),
        identifier: book_id(paras.join("\n").as_bytes()),
        ..Default::default()
    };
    let mut titles = Vec::new();
    let mut body = String::new();
    let mut title: Option<String> = None;
    let mut size = 0usize;

    let flush = |book: &mut Book, body: &mut String, title: &mut Option<String>, size: &mut usize, titles: &mut Vec<Option<String>>| {
        if body.trim().is_empty() {
            return;
        }
        let i = book.chapters.len();
        book.chapters.push(Chapter {
            title: title.clone().unwrap_or_else(|| format!("Part {}", i + 1)),
            file: chapter_file(i),
            body: std::mem::take(body),
            raw: false,
        });
        titles.push(title.take());
        *size = 0;
    };

    for p in paras {
        let head = is_heading(&p);
        if head || size > 60_000 {
            flush(&mut book, &mut body, &mut title, &mut size, &mut titles);
        }
        if head {
            body.push_str(&format!("<h2>{}</h2>\n", esc(&p)));
            title = Some(p);
        } else {
            size += p.len();
            body.push_str(&format!("<p>{}</p>\n", esc(&p)));
        }
    }
    flush(&mut book, &mut body, &mut title, &mut size, &mut titles);
    finish(book, titles)
}

// ---------------------------------------------------------------------- HTML

struct HtmlHooks<'a> {
    base: &'a Path,
    resources: Vec<Resource>,
    seen: HashMap<String, String>,
}

impl Hooks for HtmlHooks<'_> {
    fn attr(&mut self, tag: &str, name: &str, value: &str) -> Hook {
        if tag == "img" && name == "src" {
            if let Some(p) = self.seen.get(value) {
                return Hook::Set("src".into(), p.clone());
            }
            if value.starts_with("http:") || value.starts_with("https:") || value.starts_with("data:") {
                return Hook::Drop;
            }
            let rel = value.split(['#', '?']).next().unwrap_or("");
            let file = self.base.join(rel);
            // Stay inside the book's folder: no `..` escapes.
            if rel.split('/').any(|c| c == "..") || rel.starts_with('/') {
                return Hook::Drop;
            }
            if let Ok(data) = std::fs::read(&file) {
                if let Some((ext, media)) = sniff_image(&data) {
                    let path = format!("images/img{}.{}", self.resources.len() + 1, ext);
                    self.resources.push(Resource { path: path.clone(), media_type: media.into(), data });
                    self.seen.insert(value.to_string(), path.clone());
                    return Hook::Set("src".into(), path);
                }
            }
            return Hook::Drop;
        }
        if tag == "a" && name == "href" && (value.starts_with("http:") || value.starts_with("https:") || value.starts_with('#')) {
            return Hook::Pass;
        }
        if tag == "a" && name == "href" {
            return Hook::Drop;
        }
        Hook::Pass
    }
}

pub fn convert_html(path: &Path) -> Result<Book, NativeError> {
    let raw = std::fs::read(path)?;
    let html = decode_bytes(&raw);
    let title_re = Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap();
    let title = title_re
        .captures(&html)
        .map(|c| c[1].split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| stem(path));

    // One file per major heading keeps chapters a Kindle-friendly size.
    let html = if html.to_lowercase().contains("<h1") || html.to_lowercase().contains("<h2") {
        Regex::new(r"(?i)<h[12][\s>]")
            .unwrap()
            .replace_all(&html, |c: &regex::Captures| format!("<hr class=\"{}\"/>{}", xhtml::PAGEBREAK_CLASS, &c[0]))
            .into_owned()
    } else {
        html
    };

    let mut hooks = HtmlHooks { base: path.parent().unwrap_or(Path::new(".")), resources: Vec::new(), seen: HashMap::new() };
    let mut parsed = xhtml::tidy(&html, &mut hooks);
    if parsed.parts.is_empty() {
        return Err(NativeError::Corrupt("no readable text in the HTML".into()));
    }
    let files: Vec<String> = (0..parsed.parts.len()).map(chapter_file).collect();
    fix_links(&mut parsed.parts, &files, &parsed.anchors);

    let mut book = Book { title, language: "en".into(), identifier: book_id(&raw), resources: hooks.resources, ..Default::default() };
    for (i, body) in parsed.parts.into_iter().enumerate() {
        book.chapters.push(Chapter { title: format!("Part {}", i + 1), file: files[i].clone(), body, raw: false });
    }
    Ok(finish(book, parsed.titles))
}

// ----------------------------------------------------------------------- FB2

struct Fb2 {
    binaries: HashMap<String, Vec<u8>>,
    resources: Vec<Resource>,
    used: HashMap<String, String>,
}

impl Fb2 {
    fn image(&mut self, href: &str) -> Option<String> {
        let id = href.trim_start_matches('#');
        if let Some(p) = self.used.get(id) {
            return Some(p.clone());
        }
        let data = self.binaries.get(id)?;
        let (ext, media) = sniff_image(data)?;
        let path = format!("images/img{}.{}", self.resources.len() + 1, ext);
        self.resources.push(Resource { path: path.clone(), media_type: media.into(), data: data.clone() });
        self.used.insert(id.to_string(), path.clone());
        Some(path)
    }

    fn inline(&mut self, node: roxmltree::Node, out: &mut String) {
        for c in node.children() {
            if c.is_text() {
                out.push_str(&esc(c.text().unwrap_or("")));
            } else if c.is_element() {
                let name = c.tag_name().name();
                let (open, close): (&str, &str) = match name {
                    "emphasis" => ("<em>", "</em>"),
                    "strong" => ("<strong>", "</strong>"),
                    "strikethrough" => ("<s>", "</s>"),
                    "sub" => ("<sub>", "</sub>"),
                    "sup" => ("<sup>", "</sup>"),
                    "code" => ("<code>", "</code>"),
                    _ => ("", ""),
                };
                match name {
                    "a" => {
                        let href = c.attributes().find(|a| a.name() == "href").map(|a| a.value()).unwrap_or("");
                        if href.starts_with('#') || href.starts_with("http") {
                            out.push_str(&format!("<a href=\"{}\">", esc(href)));
                            self.inline(c, out);
                            out.push_str("</a>");
                        } else {
                            self.inline(c, out);
                        }
                    }
                    "image" => {
                        let href = c.attributes().find(|a| a.name() == "href").map(|a| a.value()).unwrap_or("");
                        if let Some(p) = self.image(href) {
                            out.push_str(&format!("<img src=\"{}\" alt=\"\"/>", p));
                        }
                    }
                    _ => {
                        out.push_str(open);
                        self.inline(c, out);
                        out.push_str(close);
                    }
                }
            }
        }
    }

    fn block(&mut self, node: roxmltree::Node, out: &mut String, depth: usize) {
        for c in node.children().filter(|c| c.is_element()) {
            let id = c.attribute("id").map(|i| format!(" id=\"{}\"", esc(i))).unwrap_or_default();
            match c.tag_name().name() {
                "title" => {
                    let level = (depth + 1).min(5);
                    out.push_str(&format!("<h{}{}>", level, id));
                    let mut first = true;
                    for p in c.children().filter(|p| p.is_element()) {
                        if !first {
                            out.push_str("<br/>");
                        }
                        first = false;
                        self.inline(p, out);
                    }
                    out.push_str(&format!("</h{}>\n", level));
                }
                "subtitle" => {
                    out.push_str(&format!("<h4{}>", id));
                    self.inline(c, out);
                    out.push_str("</h4>\n");
                }
                "p" | "text-author" => {
                    out.push_str(&format!("<p{}>", id));
                    self.inline(c, out);
                    out.push_str("</p>\n");
                }
                "v" => {
                    out.push_str(&format!("<p{}>", id));
                    self.inline(c, out);
                    out.push_str("</p>\n");
                }
                "empty-line" => out.push_str("<p><br/></p>\n"),
                "image" => {
                    let href = c.attributes().find(|a| a.name() == "href").map(|a| a.value()).unwrap_or("");
                    if let Some(p) = self.image(href) {
                        out.push_str(&format!("<div{}><img src=\"{}\" alt=\"\"/></div>\n", id, p));
                    }
                }
                "epigraph" | "cite" => {
                    out.push_str(&format!("<blockquote{}>\n", id));
                    self.block(c, out, depth);
                    out.push_str("</blockquote>\n");
                }
                "poem" | "stanza" | "section" => {
                    out.push_str(&format!("<div{}>\n", id));
                    self.block(c, out, depth + 1);
                    out.push_str("</div>\n");
                }
                _ => {}
            }
        }
    }

    /// Renders one `<section>` as a chapter: its own title at h1, nested sections one level down.
    fn block_single(&mut self, sec: roxmltree::Node, out: &mut String) {
        let id = sec.attribute("id").map(|i| format!(" id=\"{}\"", esc(i))).unwrap_or_default();
        out.push_str(&format!("<div{}>\n", id));
        self.block(sec, out, 0);
        out.push_str("</div>\n");
    }
}

pub fn convert_fb2(path: &Path) -> Result<Book, NativeError> {
    let raw = std::fs::read(path)?;
    let xml = String::from_utf8(raw.clone()).unwrap_or_else(|_| decode_bytes(&raw));
    let xml = xml.trim_start_matches('\u{feff}');
    let opts = roxmltree::ParsingOptions { allow_dtd: false, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(xml, opts).map_err(|e| NativeError::Corrupt(format!("bad FB2 XML: {}", e)))?;
    let root = doc.root_element();
    if root.tag_name().name() != "FictionBook" {
        return Err(NativeError::Corrupt("not a FictionBook file".into()));
    }

    let mut binaries = HashMap::new();
    for b in root.children().filter(|c| c.has_tag_name("binary")) {
        let id = b.attribute("id").unwrap_or("").to_string();
        let compact: String = b.text().unwrap_or("").chars().filter(|c| !c.is_whitespace()).collect();
        if let Ok(data) = base64::engine::general_purpose::STANDARD.decode(compact.as_bytes()) {
            binaries.insert(id, data);
        }
    }

    let mut book = Book { language: "en".into(), identifier: book_id(&raw), ..Default::default() };
    if let Some(ti) = root.descendants().find(|n| n.has_tag_name("title-info")) {
        let text = |name: &str| ti.children().find(|c| c.has_tag_name(name)).map(|c| c.text().unwrap_or("").trim().to_string());
        book.title = text("book-title").filter(|t| !t.is_empty()).unwrap_or_else(|| stem(path));
        if let Some(l) = text("lang") {
            if !l.is_empty() {
                book.language = l;
            }
        }
        for a in ti.children().filter(|c| c.has_tag_name("author")) {
            let part = |n: &str| a.children().find(|c| c.has_tag_name(n)).and_then(|c| c.text()).unwrap_or("").trim().to_string();
            let name = [part("first-name"), part("middle-name"), part("last-name")]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if !name.is_empty() {
                book.authors.push(name);
            }
        }
    } else {
        book.title = stem(path);
    }

    let mut fb = Fb2 { binaries, resources: Vec::new(), used: HashMap::new() };

    // Cover first so it is resource #1.
    if let Some(cp) = root.descendants().find(|n| n.has_tag_name("coverpage")) {
        if let Some(img) = cp.children().find(|c| c.has_tag_name("image")) {
            let href = img.attributes().find(|a| a.name() == "href").map(|a| a.value()).unwrap_or("");
            book.cover = fb.image(href);
        }
    }

    let mut bodies: Vec<String> = Vec::new();
    let mut titles: Vec<Option<String>> = Vec::new();
    let mut anchors: HashMap<String, usize> = HashMap::new();
    for body in root.children().filter(|c| c.has_tag_name("body")) {
        let sections: Vec<_> = body.children().filter(|c| c.has_tag_name("section")).collect();
        if sections.is_empty() {
            let mut s = String::new();
            fb.block(body, &mut s, 0);
            if !s.trim().is_empty() {
                titles.push(None);
                bodies.push(s);
            }
            continue;
        }
        for sec in sections {
            let mut s = String::new();
            fb.block_single(sec, &mut s);
            let t = sec
                .children()
                .find(|c| c.has_tag_name("title"))
                .map(|t| t.descendants().filter(|n| n.is_text()).map(|n| n.text().unwrap_or("")).collect::<Vec<_>>().join(" "))
                .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|t| !t.is_empty());
            for n in sec.descendants().filter(|n| n.is_element()) {
                if let Some(id) = n.attribute("id") {
                    anchors.insert(id.to_string(), bodies.len());
                }
            }
            titles.push(t);
            bodies.push(s);
        }
    }
    if bodies.iter().all(|b| b.trim().is_empty()) {
        return Err(NativeError::Corrupt("the FB2 file has no text".into()));
    }

    let files: Vec<String> = (0..bodies.len()).map(chapter_file).collect();
    fix_links(&mut bodies, &files, &anchors);
    for (i, b) in bodies.into_iter().enumerate() {
        book.chapters.push(Chapter { title: format!("Part {}", i + 1), file: files[i].clone(), body: b, raw: false });
    }
    book.resources = fb.resources;
    Ok(finish(book, titles))
}
