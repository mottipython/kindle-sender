#![no_main]
use libfuzzer_sys::fuzz_target;

use kindle_sender::native::epub_meta::{self, Edit};

// Arbitrary bytes as an .epub through the metadata reader, editor and cover setter.
fuzz_target!(|data: &[u8]| {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("book.epub");
    std::fs::write(&book, data).unwrap();
    let _ = epub_meta::read(&book);
    let edit = Edit {
        title: Some("T & <t>".into()),
        title_sort: Some("T".into()),
        author: Some("A \"B\"".into()),
        author_sort: Some("B, A".into()),
    };
    let _ = epub_meta::edit(&book, &edit);
    let _ = epub_meta::set_cover(&book, b"\x89PNG\r\n\x1a\nxxxx");
});
