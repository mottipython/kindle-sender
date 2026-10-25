#![no_main]
use std::collections::HashMap;
use std::path::Path;

use libfuzzer_sys::fuzz_target;

use kindle_sender::{config, ebook_tools, epub_fix, pipeline};

// Splits the input into a name and a body and pushes both through every pure
// string/markup routine: path helpers, filename parsing, and the four repairs.
fuzz_target!(|data: &[u8]| {
    let split = data.first().copied().unwrap_or(0) as usize % (data.len().max(1));
    let (a, b) = data.split_at(split.min(data.len()));
    let name = String::from_utf8_lossy(a).to_string();

    let _ = epub_fix::normalize_posix_path(&name);
    let _ = epub_fix::join_posix_path(&name, &String::from_utf8_lossy(b));
    let _ = epub_fix::posix_dirname(&name);
    let _ = epub_fix::decode_text(b);
    let _ = pipeline::safe_filename(&name);
    let _ = ebook_tools::guess_from_filename(Path::new(&name));
    let _ = ebook_tools::sort_name(&name);
    let _ = config::kindle_address_warning(&name);
    let _ = config::clean_password(&name);

    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    files.insert("META-INF/container.xml".into(), a.to_vec());
    files.insert("OEBPS/content.opf".into(), b.to_vec());
    files.insert(format!("OEBPS/{}.xhtml", name), b.to_vec());
    files.insert("OEBPS/other.html".into(), a.to_vec());

    let mut changes = Vec::new();
    let _ = epub_fix::find_opf_path(&files);
    epub_fix::fix_encoding(&mut files, &mut changes);
    epub_fix::fix_language(&mut files, &mut changes, "en");
    epub_fix::fix_body_id_links(&mut files, &mut changes);
    epub_fix::fix_stray_images(&mut files, &mut changes);
});
