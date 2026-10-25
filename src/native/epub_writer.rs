//! Minimal EPUB 2 writer used by every native converter.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::NativeError;

pub struct Resource {
    /// Path relative to the OEBPS folder, e.g. `images/img1.jpg`.
    pub path: String,
    pub media_type: String,
    pub data: Vec<u8>,
}

pub struct Chapter {
    pub title: String,
    /// File name inside OEBPS, e.g. `chapter3.xhtml`.
    pub file: String,
    /// Well-formed XHTML for the inside of `<body>`, or a complete document when `raw`.
    pub body: String,
    pub raw: bool,
}

pub struct TocEntry {
    pub title: String,
    pub href: String,
}

#[derive(Default)]
pub struct Book {
    pub title: String,
    pub authors: Vec<String>,
    pub language: String,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub identifier: String,
    /// Path (as in `resources`) of the cover image, if any.
    pub cover: Option<String>,
    pub chapters: Vec<Chapter>,
    pub resources: Vec<Resource>,
    pub toc: Vec<TocEntry>,
}

pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if is_xml_char(c) => out.push(c),
            _ => {}
        }
    }
    out
}

pub fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

/// Sniffs an image from its magic bytes: (extension, media type).
pub fn sniff_image(data: &[u8]) -> Option<(&'static str, &'static str)> {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(("jpg", "image/jpeg"))
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("png", "image/png"))
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some(("gif", "image/gif"))
    } else {
        None
    }
}

const XHTML_HEAD: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.1//EN\" \"http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd\">\n\
<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"en\">\n";

pub const DEFAULT_CSS: &str = "body { margin: 5%; line-height: 1.4; }\n\
p { margin: 0 0 0.8em 0; text-align: justify; }\n\
h1, h2, h3 { text-align: center; page-break-after: avoid; }\n\
img { max-width: 100%; }\n";

pub fn write(book: &Book, dest: &Path) -> Result<(), NativeError> {
    let file = File::create(dest)?;
    let mut zip = ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let zerr = |e: zip::result::ZipError| NativeError::Corrupt(e.to_string());

    zip.start_file("mimetype", stored).map_err(zerr)?;
    zip.write_all(b"application/epub+zip")?;

    zip.start_file("META-INF/container.xml", deflated).map_err(zerr)?;
    zip.write_all(
        b"<?xml version=\"1.0\"?>\n<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n\
<rootfiles><rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles>\n</container>\n",
    )?;

    zip.start_file("OEBPS/style.css", deflated).map_err(zerr)?;
    zip.write_all(DEFAULT_CSS.as_bytes())?;

    for ch in &book.chapters {
        zip.start_file(format!("OEBPS/{}", ch.file), deflated).map_err(zerr)?;
        if ch.raw {
            let doc = if ch.body.trim_start().starts_with("<?xml") {
                ch.body.clone()
            } else {
                format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n{}", ch.body)
            };
            zip.write_all(doc.as_bytes())?;
            continue;
        }
        let doc = format!(
            "{}<head><title>{}</title><link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/></head>\n<body>\n{}\n</body>\n</html>\n",
            XHTML_HEAD,
            esc(&ch.title),
            ch.body
        );
        zip.write_all(doc.as_bytes())?;
    }
    for res in &book.resources {
        zip.start_file(format!("OEBPS/{}", res.path), deflated).map_err(zerr)?;
        zip.write_all(&res.data)?;
    }

    zip.start_file("OEBPS/toc.ncx", deflated).map_err(zerr)?;
    zip.write_all(ncx(book).as_bytes())?;

    zip.start_file("OEBPS/content.opf", deflated).map_err(zerr)?;
    zip.write_all(opf(book).as_bytes())?;

    zip.finish().map_err(zerr)?;
    Ok(())
}

fn ncx(book: &Book) -> String {
    let mut s = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\">\n\
<head><meta name=\"dtb:uid\" content=\"{}\"/><meta name=\"dtb:depth\" content=\"1\"/>\
<meta name=\"dtb:totalPageCount\" content=\"0\"/><meta name=\"dtb:maxPageNumber\" content=\"0\"/></head>\n\
<docTitle><text>{}</text></docTitle>\n<navMap>\n",
        esc(&book.identifier),
        esc(&book.title)
    );
    for (i, e) in book.toc.iter().enumerate() {
        s.push_str(&format!(
            "<navPoint id=\"nav{0}\" playOrder=\"{0}\"><navLabel><text>{1}</text></navLabel><content src=\"{2}\"/></navPoint>\n",
            i + 1,
            esc(&e.title),
            esc(&e.href)
        ));
    }
    s.push_str("</navMap>\n</ncx>\n");
    s
}

fn opf(book: &Book) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" unique-identifier=\"bookid\" version=\"2.0\">\n\
<metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:opf=\"http://www.idpf.org/2007/opf\">\n",
    );
    s.push_str(&format!("<dc:title>{}</dc:title>\n", esc(&book.title)));
    if book.authors.is_empty() {
        s.push_str("<dc:creator opf:role=\"aut\">Unknown</dc:creator>\n");
    }
    for a in &book.authors {
        s.push_str(&format!("<dc:creator opf:role=\"aut\">{}</dc:creator>\n", esc(a)));
    }
    let lang = if book.language.is_empty() { "en" } else { &book.language };
    s.push_str(&format!("<dc:language>{}</dc:language>\n", esc(lang)));
    s.push_str(&format!(
        "<dc:identifier id=\"bookid\">{}</dc:identifier>\n",
        esc(&book.identifier)
    ));
    if let Some(p) = &book.publisher {
        s.push_str(&format!("<dc:publisher>{}</dc:publisher>\n", esc(p)));
    }
    if let Some(d) = &book.description {
        s.push_str(&format!("<dc:description>{}</dc:description>\n", esc(d)));
    }
    if let Some(c) = &book.cover {
        if let Some(i) = book.resources.iter().position(|r| &r.path == c) {
            s.push_str(&format!("<meta name=\"cover\" content=\"res{}\"/>\n", i));
        }
    }
    s.push_str("</metadata>\n<manifest>\n");
    s.push_str("<item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n");
    s.push_str("<item id=\"css\" href=\"style.css\" media-type=\"text/css\"/>\n");
    for (i, ch) in book.chapters.iter().enumerate() {
        s.push_str(&format!(
            "<item id=\"ch{}\" href=\"{}\" media-type=\"application/xhtml+xml\"/>\n",
            i,
            esc(&ch.file)
        ));
    }
    for (i, r) in book.resources.iter().enumerate() {
        s.push_str(&format!(
            "<item id=\"res{}\" href=\"{}\" media-type=\"{}\"/>\n",
            i,
            esc(&r.path),
            esc(&r.media_type)
        ));
    }
    s.push_str("</manifest>\n<spine toc=\"ncx\">\n");
    for i in 0..book.chapters.len() {
        s.push_str(&format!("<itemref idref=\"ch{}\"/>\n", i));
    }
    s.push_str("</spine>\n</package>\n");
    s
}
