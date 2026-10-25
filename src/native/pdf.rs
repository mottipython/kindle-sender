//! PDF to reflowable EPUB by extracting the text layer. Scanned (image-only)
//! PDFs have no text and are reported as unsupported.

use std::collections::HashMap;
use std::path::Path;

use regex::Regex;

use super::epub_writer::Book;
use super::text::book_from_paragraphs;
use super::NativeError;

fn is_page_number(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.len() <= 6 && t.chars().all(|c| c.is_ascii_digit() || c == '-' || c.is_ascii_alphabetic() && "ivxlcdm".contains(c.to_ascii_lowercase()))
}

/// Extracts text page by page. pdf-extract panics on some fonts (e.g. Type 3 without
/// widths), so each page is guarded and unreadable pages are skipped; if most
/// pages fail the PDF is treated as unsupported.
fn extract_pages(data: &[u8]) -> Result<Vec<String>, NativeError> {
    let mut doc = pdf_extract::Document::load_mem(data)
        .map_err(|e| NativeError::Unsupported(format!("PDF read error: {}", e)))?;
    if doc.is_encrypted() && doc.decrypt("").is_err() {
        return Err(NativeError::Drm);
    }
    let count = doc.get_pages().len();
    if count == 0 {
        return Err(NativeError::Unsupported("the PDF has no pages".into()));
    }

    // Keep the panic message of a bad page off the terminal.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut pages = Vec::with_capacity(count);
    let mut failed = 0;
    for n in 1..=count as u32 {
        let page = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut s = String::new();
            let ok = {
                let mut out = pdf_extract::PlainTextOutput::new(&mut s);
                pdf_extract::output_doc_page(&doc, &mut out, n).is_ok()
            };
            ok.then_some(s)
        }));
        match page {
            Ok(Some(s)) => pages.push(s),
            _ => failed += 1,
        }
    }
    std::panic::set_hook(previous_hook);

    if failed * 2 > count {
        return Err(NativeError::Unsupported("the PDF's text could not be read".into()));
    }
    Ok(pages)
}

pub fn convert(path: &Path) -> Result<Book, NativeError> {
    let data = std::fs::read(path)?;
    if !data.starts_with(b"%PDF") && !data[..data.len().min(1024)].windows(4).any(|w| w == b"%PDF") {
        return Err(NativeError::Corrupt("not a PDF file".into()));
    }

    let pages = extract_pages(&data)?;

    let chars: usize = pages.iter().map(|p| p.chars().filter(|c| !c.is_whitespace()).count()).sum();
    if pages.is_empty() || chars < 40 * pages.len().min(5).max(1) {
        return Err(NativeError::Unsupported("the PDF has no text layer (scanned pages need OCR)".into()));
    }

    // Running headers/footers: short lines repeated on many pages.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for p in &pages {
        let mut uniq: Vec<String> = p.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty() && l.chars().count() <= 80).collect();
        uniq.sort();
        uniq.dedup();
        for l in uniq {
            *seen.entry(l).or_default() += 1;
        }
    }
    let repeated = |l: &str| pages.len() >= 6 && seen.get(l.trim()).is_some_and(|&n| n * 100 / pages.len() >= 30);

    let sentence_end = Regex::new(r#"[.!?:;"'”’)\]]$"#).unwrap();
    let mut paras: Vec<String> = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, paras: &mut Vec<String>| {
        let t = cur.split_whitespace().collect::<Vec<_>>().join(" ");
        if !t.is_empty() {
            paras.push(t);
        }
        cur.clear();
    };

    for page in &pages {
        let lines: Vec<&str> = page.lines().map(|l| l.trim_end()).collect();
        let mut lens: Vec<usize> = lines.iter().map(|l| l.chars().count()).filter(|&n| n > 20).collect();
        lens.sort_unstable();
        let typical = lens.get(lens.len() * 3 / 4).copied().unwrap_or(60);

        for l in lines {
            let t = l.trim();
            if t.is_empty() {
                flush(&mut cur, &mut paras);
                continue;
            }
            if is_page_number(t) || repeated(t) {
                continue;
            }
            if !cur.is_empty() {
                if cur.ends_with('-') && t.chars().next().is_some_and(|c| c.is_lowercase()) {
                    cur.pop();
                } else {
                    cur.push(' ');
                }
            }
            cur.push_str(t);
            // A short line that closes a sentence ends the paragraph.
            if t.chars().count() * 100 < typical * 70 && sentence_end.is_match(t) {
                flush(&mut cur, &mut paras);
            }
        }
        // Paragraphs may continue onto the next page unless the sentence ended.
        if !cur.is_empty() && sentence_end.is_match(cur.trim_end()) {
            flush(&mut cur, &mut paras);
        }
    }
    flush(&mut cur, &mut paras);

    if paras.is_empty() {
        return Err(NativeError::Unsupported("the PDF has no readable text".into()));
    }
    let title = path.file_stem().and_then(|s| s.to_str()).unwrap_or("book").replace('_', " ");
    Ok(book_from_paragraphs(title, paras))
}
