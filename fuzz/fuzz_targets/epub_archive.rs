#![no_main]
use libfuzzer_sys::fuzz_target;

use kindle_sender::{ebook_tools, epub_fix};

// Feeds arbitrary bytes in as an .epub: every reader and the full repair pass
// must reject or repair it without panicking.
fuzz_target!(|data: &[u8]| {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("book.epub");
    std::fs::write(&book, data).unwrap();

    let _ = epub_fix::read_all(&book);
    let _ = epub_fix::validate_epub(&book);
    let _ = ebook_tools::has_real_cover(&book, 64);
    let _ = epub_fix::fix_epub(&book, true, None, "en");
});
