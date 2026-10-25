//! Runs real files of every built-in format through the whole pipeline with an
//! empty PATH, so calibre cannot be what makes it work.

use std::path::{Path, PathBuf};

use kindle_sender::config::Config;
use kindle_sender::native::epub_meta;
use kindle_sender::pipeline::{process_file, Status};

fn fixture(ext: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("Jules Verne - Test Book.{}", ext))
}

fn run(ext: &str) {
    std::env::set_var("PATH", "");
    let out = tempfile::tempdir().unwrap();
    let cfg = Config {
        do_send: false,
        do_cover: false,
        keep_copy: true,
        output_dir: out.path().to_string_lossy().to_string(),
        ..Default::default()
    };

    let result = process_file(fixture(ext), &cfg, |_| {}, || false);
    assert_eq!(result.status, Status::Ok, "{}: {:?}", ext, result.error);
    assert!(
        result.changes.iter().any(|c| c.contains("built-in")),
        "{}: expected the built-in converter, got {:?}",
        ext,
        result.changes
    );

    let epub = result.output.expect("kept copy");
    kindle_sender::epub_fix::validate_epub(&epub).unwrap();
    let meta = epub_meta::read(&epub).unwrap();
    assert_eq!(meta.title.as_deref(), Some("Test Book"), "{}", ext);
    assert_eq!(meta.authors, vec!["Jules Verne".to_string()], "{}", ext);

    // The text must actually be in there.
    let (_, files) = kindle_sender::epub_fix::read_all(&epub).unwrap();
    let text: String = files
        .iter()
        .filter(|(n, _)| n.ends_with(".xhtml"))
        .map(|(_, d)| String::from_utf8_lossy(d).into_owned())
        .collect();
    assert!(
        text.contains("captain") && text.contains("storm"),
        "{}: lost the book text",
        ext
    );
}

#[test]
fn mobi() {
    run("mobi");
}

#[test]
fn azw() {
    run("azw");
}

#[test]
fn azw3() {
    run("azw3");
}

#[test]
fn fb2() {
    run("fb2");
}

#[test]
fn pdf() {
    run("pdf");
}

#[test]
fn txt() {
    run("txt");
}

#[test]
fn html() {
    run("html");
}

#[test]
fn cover_can_be_replaced_without_calibre() {
    std::env::set_var("PATH", "");
    let dir = tempfile::tempdir().unwrap();
    let epub = dir.path().join("b.epub");
    kindle_sender::native::convert(&fixture("txt"), &epub, ".txt").unwrap();
    let png = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pic.png")).unwrap();
    epub_meta::set_cover(&epub, &png).unwrap();
    kindle_sender::epub_fix::validate_epub(&epub).unwrap();
    assert!(kindle_sender::ebook_tools::has_real_cover(&epub, 8));
}

#[test]
fn drm_and_garbage_are_rejected_cleanly() {
    std::env::set_var("PATH", "");
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("junk.mobi");
    std::fs::write(
        &bad,
        b"this is not a mobi file at all, just some text that is long enough to parse as nothing",
    )
    .unwrap();
    let cfg = Config {
        do_send: false,
        ..Default::default()
    };
    let r = process_file(&bad, &cfg, |_| {}, || false);
    assert_eq!(r.status, Status::Error);
}
