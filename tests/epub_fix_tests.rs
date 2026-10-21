use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use tempfile::tempdir;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;
use kindle_sender::epub_fix::fix_epub;

const CONTAINER: &str = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf"
    media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;

const OPF_TEMPLATE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Broken Book</dc:title>
    <dc:creator>Test Author</dc:creator>
    <dc:identifier id="id">urn:uuid:1234</dc:identifier>
    {LANG}
  </metadata>
  <manifest>
    <item id="toc" href="toc.xhtml" media-type="application/xhtml+xml"/>
    <item id="ch1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="toc"/><itemref idref="ch1"/></spine>
</package>"#;

const TOC: &str = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>TOC</title></head>
<body id="toc-top">
<a href="chapter1.xhtml#chap1-body">Chapter One</a>
<a href="chapter1.xhtml#real-anchor">Deep link</a>
<a href="https://example.com/x#frag">External</a>
</body></html>"#;

const CHAPTER: &str = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Ch1</title></head>
<body id="chap1-body">
<h1>Café naïve résumé</h1>
<p id="real-anchor">Text</p>
<img src="missing.jpg" alt="gone"/>
<img src="there.png" alt="ok"/>
<img alt="no src"/>
</body></html>"#;

fn build_epub(path: &Path, lang: &str) {
    let file = File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);

    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/epub+zip").unwrap();

    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("META-INF/container.xml", deflated).unwrap();
    zip.write_all(CONTAINER.as_bytes()).unwrap();

    let opf = OPF_TEMPLATE.replace("{LANG}", lang);
    zip.start_file("OEBPS/content.opf", deflated).unwrap();
    zip.write_all(opf.as_bytes()).unwrap();

    zip.start_file("OEBPS/toc.xhtml", deflated).unwrap();
    zip.write_all(TOC.as_bytes()).unwrap();

    zip.start_file("OEBPS/chapter1.xhtml", deflated).unwrap();
    zip.write_all(CHAPTER.as_bytes()).unwrap();

    zip.start_file("OEBPS/there.png", deflated).unwrap();
    zip.write_all(b"\x89PNG\r\n\x1a\n fake").unwrap();

    zip.finish().unwrap();
}

fn read_entry(path: &Path, name: &str) -> String {
    let file = File::open(path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut entry = archive.by_name(name).unwrap();
    let mut s = String::new();
    entry.read_to_string(&mut s).unwrap();
    s
}

#[test]
fn test_epub_fixes_invalid_and_missing_language() {
    let tmp = tempdir().unwrap();

    for (label, lang) in [
        ("invalid dc:language", "<dc:language>und</dc:language>"),
        ("missing dc:language", ""),
    ] {
        let book = tmp.path().join("broken.epub");
        build_epub(&book, lang);

        let changes = fix_epub(&book, true, None, "en").unwrap();
        println!("\n{}: {} change(s)", label, changes.len());
        for c in &changes {
            println!("   - {}", c);
        }

        let opf = read_entry(&book, "OEBPS/content.opf");
        let toc = read_entry(&book, "OEBPS/toc.xhtml");
        let chapter = read_entry(&book, "OEBPS/chapter1.xhtml");

        assert!(opf.contains("<dc:language>en</dc:language>"), "language set to en");
        assert!(chapter.starts_with("<?xml"), "XML declaration prepended");
        let first_line = chapter.lines().next().unwrap_or("");
        assert!(first_line.contains("utf-8"), "declaration names utf-8");
        assert!(chapter.contains("Café naïve"), "accents survived the round trip");
        assert!(toc.contains(r#"href="chapter1.xhtml""#), "body-id fragment stripped");
        assert!(toc.contains(r#"href="chapter1.xhtml#real-anchor""#), "real anchor kept");
        assert!(toc.contains(r#"href="https://example.com/x#frag""#), "external link kept");
        assert!(!chapter.contains("missing.jpg"), "stray img removed");
        assert!(chapter.contains("there.png"), "valid img kept");
        assert!(!chapter.contains(r#"alt="no src""#), "src-less img removed");

        let file = File::open(&book).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        assert_eq!(archive.by_index(0).unwrap().name(), "mimetype", "mimetype first in archive");
        assert_eq!(
            archive.by_index(0).unwrap().compression(),
            CompressionMethod::Stored,
            "mimetype stored uncompressed"
        );
        assert_eq!(archive.len(), 6, "no entries lost");
    }

    // Idempotence: a second pass should find nothing left to fix.
    let book = tmp.path().join("broken.epub");
    let second = fix_epub(&book, true, None, "en").unwrap();
    assert!(second.is_empty(), "second run is a no-op");

    // A clean book must be left completely alone.
    let clean = tmp.path().join("clean.epub");
    build_epub(&clean, "<dc:language>en</dc:language>");
    fix_epub(&clean, true, None, "en").unwrap(); // normalizes it
    let before = std::fs::read(&clean).unwrap();
    let changes_clean = fix_epub(&clean, true, None, "en").unwrap();
    assert!(changes_clean.is_empty());
    assert_eq!(std::fs::read(&clean).unwrap(), before, "already-clean book untouched");
}
