//! Converts files with the built-in converters and checks the EPUBs they produce:
//! every XHTML parses as XML, every internal link and image resolves, and the
//! word count is reported so it can be compared with calibre's output.
//!
//! usage: cargo run --release --example native_check -- FILE...

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

use kindle_sender::native;

fn words(xhtml: &str) -> usize {
    let mut n = 0;
    let mut in_tag = false;
    let mut in_word = false;
    for c in xhtml.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag && c.is_alphanumeric() => {
                if !in_word {
                    n += 1;
                    in_word = true;
                }
            }
            _ => in_word = false,
        }
    }
    n
}

fn check(src: &Path) -> Result<String, String> {
    let ext = format!(
        ".{}",
        src.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase()
    );
    let dest = std::env::temp_dir().join(format!("native_check_{}.epub", std::process::id()));
    native::convert(src, &dest, &ext).map_err(|e| format!("convert: {}", e))?;
    kindle_sender::epub_fix::validate_epub(&dest).map_err(|e| format!("validate: {}", e))?;

    let mut zip = zip::ZipArchive::new(std::fs::File::open(&dest).unwrap()).map_err(|e| e.to_string())?;
    let mut names = HashSet::new();
    let mut docs: HashMap<String, String> = HashMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = f.name().map_err(|e| e.to_string())?.into_owned();
        names.insert(name.clone());
        if name.ends_with(".xhtml") || name.ends_with(".opf") || name.ends_with(".ncx") {
            let mut s = String::new();
            f.read_to_string(&mut s).map_err(|e| format!("{}: {}", name, e))?;
            docs.insert(name, s);
        }
    }

    let mut problems: Vec<String> = Vec::new();
    let mut ids: HashMap<String, HashSet<String>> = HashMap::new();
    let mut chapters = 0;
    let mut total_words = 0;
    for (name, s) in &docs {
        let opts = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        };
        let doc = match roxmltree::Document::parse_with_options(s, opts) {
            Ok(d) => d,
            Err(e) => {
                problems.push(format!("{} is not well-formed: {}", name, e));
                continue;
            }
        };
        if name.ends_with(".xhtml") {
            chapters += 1;
            total_words += words(s);
            ids.insert(
                name.clone(),
                doc.descendants()
                    .filter_map(|n| n.attribute("id").map(String::from))
                    .collect(),
            );
        }
    }
    let mut bad_links = 0;
    let mut bad_imgs = 0;
    let mut links = 0;
    for (name, s) in &docs {
        if !name.ends_with(".xhtml") {
            continue;
        }
        let Ok(doc) = roxmltree::Document::parse_with_options(
            s,
            roxmltree::ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            },
        ) else {
            continue;
        };
        let base = name.rsplit_once('/').map(|x| x.0).unwrap_or("");
        for n in doc.descendants().filter(|n| n.is_element()) {
            if let Some(h) = n.attribute("href") {
                if n.tag_name().name() == "a" && !h.starts_with("http") && !h.starts_with("mailto:") {
                    links += 1;
                    let (file, frag) = h.split_once('#').unwrap_or((h, ""));
                    let target = if file.is_empty() {
                        name.clone()
                    } else {
                        format!("{}/{}", base, file)
                    };
                    let ok_file = names.contains(&target);
                    let ok_frag = frag.is_empty() || ids.get(&target).is_some_and(|s| s.contains(frag));
                    if !(ok_file && ok_frag) {
                        bad_links += 1;
                        if bad_links <= 3 {
                            problems.push(format!("{}: dead link {:?}", name, h));
                        }
                    }
                }
            }
            if let Some(src) = n.attribute("src") {
                if n.tag_name().name() == "img" && !names.contains(&format!("{}/{}", base, src)) {
                    bad_imgs += 1;
                    if bad_imgs <= 3 {
                        problems.push(format!("{}: missing image {:?}", name, src));
                    }
                }
            }
        }
    }
    let images = names.iter().filter(|n| n.contains("/images/")).count();
    let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    let _ = std::fs::remove_file(&dest);

    let summary = format!(
        "{} chapters, {} words, {} images, {} links ({} dead), {} missing img, {} KB",
        chapters,
        total_words,
        images,
        links,
        bad_links,
        bad_imgs,
        size / 1024
    );
    if problems.is_empty() {
        Ok(summary)
    } else {
        Err(format!("{}\n      {}", summary, problems.join("\n      ")))
    }
}

fn main() {
    let mut failed = 0;
    for p in std::env::args().skip(1) {
        let path = Path::new(&p);
        let name = path.file_name().unwrap().to_string_lossy();
        match check(path) {
            Ok(s) => println!("OK    {}: {}", name, s),
            Err(e) => {
                failed += 1;
                println!("FAIL  {}: {}", name, e);
            }
        }
    }
    std::process::exit(if failed == 0 { 0 } else { 1 });
}
