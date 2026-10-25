#![no_main]
use libfuzzer_sys::fuzz_target;

use kindle_sender::native;

const EXTS: &[&str] = &[".mobi", ".azw3", ".azw", ".prc", ".fb2", ".pdf", ".txt", ".html"];

// First byte picks the format, the rest is the file: every built-in converter
// must reject or convert arbitrary bytes without panicking or hanging.
fuzz_target!(|data: &[u8]| {
    let Some((&sel, body)) = data.split_first() else { return };
    let ext = EXTS[sel as usize % EXTS.len()];
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join(format!("book{}", ext));
    std::fs::write(&src, body).unwrap();
    let _ = native::convert(&src, &dir.path().join("out.epub"), ext);
});
