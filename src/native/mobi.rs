//! MOBI / AZW / AZW3 (KF8) to EPUB, DRM-free books only.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use regex::Regex;

use super::epub_writer::{sniff_image, Book, Chapter, Resource, TocEntry};
use super::xhtml::{self, Hook, Hooks, PAGEBREAK_CLASS};
use super::{book_id, fix_links, NativeError};

const NONE: u32 = 0xFFFF_FFFF;

fn be16(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as u32)
}

fn be32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn corrupt<T>(msg: &str) -> Result<T, NativeError> {
    Err(NativeError::Corrupt(msg.to_string()))
}

// ------------------------------------------------------------------ PalmDB

struct Pdb<'a> {
    data: &'a [u8],
    offs: Vec<usize>,
}

impl<'a> Pdb<'a> {
    fn parse(data: &'a [u8]) -> Result<Self, NativeError> {
        if data.len() < 80 {
            return corrupt("file is too small to be a MOBI book");
        }
        let n = be16(data, 76).unwrap_or(0) as usize;
        if n == 0 || 78 + n * 8 > data.len() {
            return corrupt("bad PalmDB record table");
        }
        let mut offs = Vec::with_capacity(n);
        for i in 0..n {
            let o = be32(data, 78 + i * 8).unwrap() as usize;
            if o > data.len() || offs.last().is_some_and(|&p| o < p) {
                return corrupt("bad PalmDB record offsets");
            }
            offs.push(o);
        }
        Ok(Pdb { data, offs })
    }

    fn len(&self) -> usize {
        self.offs.len()
    }

    fn rec(&self, i: usize) -> &'a [u8] {
        if i >= self.offs.len() {
            return &[];
        }
        let end = self.offs.get(i + 1).copied().unwrap_or(self.data.len());
        &self.data[self.offs[i]..end]
    }
}

// ------------------------------------------------------------------ header

#[derive(Default)]
struct Header {
    compression: u32,
    text_len: u32,
    rec_count: u32,
    crypto: u32,
    encoding: u32,
    version: u32,
    first_image: u32,
    extra_flags: u32,
    ncx_idx: u32,
    chunk_idx: u32,
    skel_idx: u32,
    fdst_idx: u32,
    full_name: String,
    exth: HashMap<u32, Vec<Vec<u8>>>,
}

fn parse_header(r0: &[u8]) -> Result<Header, NativeError> {
    if r0.len() < 32 {
        return corrupt("missing book header");
    }
    if r0.get(16..20) != Some(b"MOBI") {
        return Err(NativeError::Unsupported(
            "not a MOBI book (plain PalmDOC is not supported)".into(),
        ));
    }
    let hlen = be32(r0, 20).unwrap_or(0) as usize;
    let mut h = Header {
        compression: be16(r0, 0).unwrap_or(0),
        text_len: be32(r0, 4).unwrap_or(0),
        rec_count: be16(r0, 8).unwrap_or(0),
        crypto: be16(r0, 12).unwrap_or(0),
        encoding: be32(r0, 28).unwrap_or(1252),
        version: be32(r0, 36).unwrap_or(0),
        first_image: be32(r0, 0x6C).unwrap_or(NONE),
        ncx_idx: NONE,
        chunk_idx: NONE,
        skel_idx: NONE,
        fdst_idx: NONE,
        ..Default::default()
    };
    if hlen >= 0xF4 {
        h.extra_flags = be32(r0, 0xF0).unwrap_or(0) & 0xFFFF;
    }
    if hlen >= 0xF8 {
        h.ncx_idx = be32(r0, 0xF4).unwrap_or(NONE);
    }
    if hlen >= 0x108 {
        h.chunk_idx = be32(r0, 0xF8).unwrap_or(NONE);
        h.skel_idx = be32(r0, 0xFC).unwrap_or(NONE);
    }
    if h.version >= 8 && hlen >= 0xC4 {
        h.fdst_idx = be32(r0, 0xC0).unwrap_or(NONE);
    }

    let (no, nl) = (
        be32(r0, 0x54).unwrap_or(0) as usize,
        be32(r0, 0x58).unwrap_or(0) as usize,
    );
    if let Some(name) = r0.get(no..no.saturating_add(nl)) {
        h.full_name = String::from_utf8_lossy(name).trim().to_string();
    }

    // EXTH block follows the MOBI header.
    let flags = be32(r0, 0x80).unwrap_or(0);
    let ex = 16 + hlen;
    if flags & 0x40 != 0 && r0.get(ex..ex + 4) == Some(b"EXTH") {
        let count = be32(r0, ex + 8).unwrap_or(0);
        let mut p = ex + 12;
        for _ in 0..count {
            let (Some(t), Some(l)) = (be32(r0, p), be32(r0, p + 4)) else {
                break;
            };
            let l = l as usize;
            if l < 8 {
                break;
            }
            if let Some(val) = r0.get(p + 8..p + l) {
                h.exth.entry(t).or_default().push(val.to_vec());
            }
            p += l;
        }
    }
    Ok(h)
}

impl Header {
    fn exth_str(&self, t: u32) -> Vec<String> {
        self.exth
            .get(&t)
            .map(|v| {
                v.iter()
                    .map(|b| String::from_utf8_lossy(b).trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn exth_u32(&self, t: u32) -> Option<u32> {
        self.exth.get(&t).and_then(|v| v.first()).and_then(|b| be32(b, 0))
    }
}

// ------------------------------------------------------------ text records

fn trailing_entry_size(data: &[u8], end: usize) -> usize {
    let (mut bitpos, mut result, mut pos) = (0u32, 0usize, end);
    while pos > 0 {
        let v = data[pos - 1];
        result |= ((v & 0x7F) as usize) << bitpos;
        bitpos += 7;
        pos -= 1;
        if v & 0x80 != 0 || bitpos >= 28 {
            break;
        }
    }
    result
}

fn strip_trailing(rec: &[u8], flags: u32) -> &[u8] {
    let mut end = rec.len();
    let mut f = flags >> 1;
    while f != 0 {
        if f & 1 != 0 {
            end = end.saturating_sub(trailing_entry_size(rec, end));
        }
        f >>= 1;
    }
    if flags & 1 != 0 && end > 0 {
        end = end.saturating_sub((rec[end - 1] & 3) as usize + 1);
    }
    &rec[..end.min(rec.len())]
}

fn palmdoc_decompress(src: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(src.len() * 3);
    let mut i = 0;
    while i < src.len() {
        let c = src[i];
        i += 1;
        match c {
            0x00 | 0x09..=0x7F => out.push(c),
            0x01..=0x08 => {
                let n = c as usize;
                out.extend_from_slice(&src[i..(i + n).min(src.len())]);
                i += n;
            }
            0x80..=0xBF => {
                let Some(&c2) = src.get(i) else { break };
                i += 1;
                let dist = ((((c as usize) << 8) | c2 as usize) >> 3) & 0x7FF;
                let len = (c2 & 7) as usize + 3;
                if dist == 0 || dist > out.len() {
                    continue;
                }
                for _ in 0..len {
                    let b = out[out.len() - dist];
                    out.push(b);
                }
            }
            0xC0..=0xFF => {
                out.push(b' ');
                out.push(c ^ 0x80);
            }
        }
    }
    out
}

fn raw_text(pdb: &Pdb, start: usize, h: &Header) -> Result<Vec<u8>, NativeError> {
    if h.compression != 1 && h.compression != 2 {
        return Err(NativeError::Unsupported("this MOBI uses HUFF/CDIC compression".into()));
    }
    let mut out = Vec::with_capacity(h.text_len as usize);
    for i in 1..=h.rec_count as usize {
        if start + i >= pdb.len() {
            break;
        }
        let rec = strip_trailing(pdb.rec(start + i), h.extra_flags);
        if h.compression == 2 {
            out.extend(palmdoc_decompress(rec));
        } else {
            out.extend_from_slice(rec);
        }
    }
    if out.is_empty() {
        return corrupt("the book has no text");
    }
    Ok(out)
}

// ------------------------------------------------------------------- INDX

struct IndexEntry {
    key: Vec<u8>,
    tags: HashMap<u32, Vec<u32>>,
}

fn vwi(d: &[u8], pos: usize) -> (u32, usize) {
    let (mut v, mut n) = (0u32, 0usize);
    while pos + n < d.len() {
        let b = d[pos + n];
        v = (v << 7) | (b & 0x7F) as u32;
        n += 1;
        if b & 0x80 != 0 || n >= 5 {
            break;
        }
    }
    (v, n)
}

fn read_index(pdb: &Pdb, idx: usize) -> Result<(Vec<IndexEntry>, HashMap<u32, String>), NativeError> {
    let head = pdb.rec(idx);
    if head.get(0..4) != Some(b"INDX") {
        return corrupt("bad index record");
    }
    let hlen = be32(head, 4).unwrap_or(0) as usize;
    let count = be32(head, 24).unwrap_or(0) as usize;
    let nctoc = be32(head, 52).unwrap_or(0) as usize;
    if count > 4096 || nctoc > 64 {
        return corrupt("implausible index size");
    }

    let mut cncx = HashMap::new();
    for k in 0..nctoc {
        let rec = pdb.rec(idx + count + 1 + k);
        let mut pos = 0;
        while pos < rec.len() {
            let off = pos;
            let (len, used) = vwi(rec, pos);
            pos += used;
            let end = pos + len as usize;
            if used == 0 || end > rec.len() {
                break;
            }
            cncx.insert(
                (k * 0x10000 + off) as u32,
                String::from_utf8_lossy(&rec[pos..end]).into_owned(),
            );
            pos = end;
        }
    }

    // TAGX: (tag, values per entry, bitmask, end flag)
    if head.get(hlen..hlen + 4) != Some(b"TAGX") {
        return corrupt("missing TAGX table");
    }
    let first = be32(head, hlen + 4).unwrap_or(12) as usize;
    let ctrl = be32(head, hlen + 8).unwrap_or(1) as usize;
    let mut table = Vec::new();
    let mut p = hlen + 12;
    while p + 4 <= hlen + first && p + 4 <= head.len() {
        table.push((head[p] as u32, head[p + 1] as usize, head[p + 2] as u32, head[p + 3]));
        p += 4;
    }

    let mut entries = Vec::new();
    for rno in idx + 1..idx + 1 + count {
        let data = pdb.rec(rno);
        if data.get(0..4) != Some(b"INDX") {
            return corrupt("bad index entry record");
        }
        let idxt = be32(data, 20).unwrap_or(0) as usize;
        let n = be32(data, 24).unwrap_or(0) as usize;
        if data.get(idxt..idxt + 4) != Some(b"IDXT") || n > 65536 {
            return corrupt("bad IDXT table");
        }
        let mut pos: Vec<usize> = (0..n)
            .map(|j| be16(data, idxt + 4 + 2 * j).unwrap_or(0) as usize)
            .collect();
        pos.push(idxt);
        for j in 0..n {
            let (start, end) = (pos[j], pos[j + 1].min(data.len()));
            if start >= end {
                continue;
            }
            let klen = data[start] as usize;
            if start + 1 + klen > end {
                continue;
            }
            let key = data[start + 1..start + 1 + klen].to_vec();
            let tags = tag_map(&table, ctrl, data, start + 1 + klen, end);
            entries.push(IndexEntry { key, tags });
        }
    }
    Ok((entries, cncx))
}

fn tag_map(table: &[(u32, usize, u32, u8)], ctrl: usize, d: &[u8], start: usize, end: usize) -> HashMap<u32, Vec<u32>> {
    let mut found: Vec<(u32, Option<u32>, Option<usize>, usize)> = Vec::new();
    let mut cb_index = 0;
    let mut p = start + ctrl;
    for &(tag, per_entry, mask, end_flag) in table {
        if end_flag == 1 {
            cb_index += 1;
            continue;
        }
        let Some(&cb) = d.get(start + cb_index) else { break };
        let mut value = cb as u32 & mask;
        if value == 0 {
            continue;
        }
        if value == mask {
            if mask.count_ones() > 1 {
                let (total, used) = vwi(d, p);
                p += used;
                found.push((tag, None, Some(total as usize), per_entry));
            } else {
                found.push((tag, Some(1), None, per_entry));
            }
        } else {
            let mut m = mask;
            while m & 1 == 0 {
                m >>= 1;
                value >>= 1;
            }
            found.push((tag, Some(value), None, per_entry));
        }
    }
    let mut map = HashMap::new();
    for (tag, count, bytes, per_entry) in found {
        let mut vals = Vec::new();
        match (count, bytes) {
            (Some(c), _) => {
                for _ in 0..c * per_entry as u32 {
                    if p >= end {
                        break;
                    }
                    let (v, used) = vwi(d, p);
                    p += used;
                    vals.push(v);
                }
            }
            (None, Some(total)) => {
                let mut consumed = 0;
                while consumed < total && p < end {
                    let (v, used) = vwi(d, p);
                    p += used;
                    consumed += used;
                    vals.push(v);
                }
            }
            _ => {}
        }
        map.insert(tag, vals);
    }
    map
}

// ------------------------------------------------------------------ images

struct Image {
    ext: &'static str,
    media: &'static str,
    data: Vec<u8>,
}

/// Image records in order; entry `n` is image number `n + 1`. Non-images stay as `None`.
fn collect_images(pdb: &Pdb, first: usize) -> Vec<Option<Image>> {
    (first..pdb.len())
        .map(|i| {
            let rec = pdb.rec(i);
            sniff_image(rec).map(|(ext, media)| Image {
                ext,
                media,
                data: rec.to_vec(),
            })
        })
        .collect()
}

struct ImageSet {
    images: Vec<Option<Image>>,
    used: HashSet<usize>,
}

impl ImageSet {
    fn path(&mut self, n: usize) -> Option<String> {
        let img = self.images.get(n)?.as_ref()?;
        self.used.insert(n);
        Some(format!("images/img{}.{}", n + 1, img.ext))
    }

    fn into_resources(self) -> Vec<Resource> {
        let used = self.used;
        self.images
            .into_iter()
            .enumerate()
            .filter(|(n, _)| used.contains(n))
            .filter_map(|(n, i)| {
                i.map(|i| Resource {
                    path: format!("images/img{}.{}", n + 1, i.ext),
                    media_type: i.media.into(),
                    data: i.data,
                })
            })
            .collect()
    }
}

// --------------------------------------------------------------- top level

pub fn convert(path: &Path) -> Result<Book, NativeError> {
    let data = std::fs::read(path)?;
    let pdb = Pdb::parse(&data)?;
    let mut start = 0usize;
    if pdb.data.get(60..68) == Some(b"TEXtREAd") && pdb.rec(0).get(16..20) != Some(b"MOBI") {
        return palmdoc(path, &pdb);
    }
    let mut h = parse_header(pdb.rec(0))?;

    // Joint MOBI6+KF8 files carry a second header after a BOUNDARY record.
    if let Some(k8) = h.exth_u32(121) {
        let k8 = k8 as usize;
        if k8 > 0 && k8 < pdb.len() && pdb.rec(k8).get(16..20) == Some(b"MOBI") {
            if let Ok(h8) = parse_header(pdb.rec(k8)) {
                if h8.version >= 8 {
                    start = k8;
                    h = h8;
                }
            }
        }
    }
    if h.crypto != 0 {
        return Err(NativeError::Drm);
    }

    let mut book = Book {
        identifier: book_id(&data),
        ..Default::default()
    };
    let first_image = if h.first_image == NONE {
        pdb.len()
    } else {
        start + h.first_image as usize
    };
    let mut imgs = ImageSet {
        images: collect_images(&pdb, first_image),
        used: HashSet::new(),
    };

    // Metadata
    book.title = h
        .exth_str(503)
        .into_iter()
        .next()
        .or_else(|| Some(h.full_name.clone()).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or("book").to_string());
    book.authors = h.exth_str(100);
    book.publisher = h.exth_str(101).into_iter().next();
    book.description = h.exth_str(103).into_iter().next();
    book.language = h.exth_str(524).into_iter().next().unwrap_or_else(|| "en".into());
    let cover_n = h.exth_u32(201).map(|n| n as usize);

    let text = raw_text(&pdb, start, &h)?;
    if h.version >= 8 && h.skel_idx != NONE {
        kf8(&pdb, start, &h, &text, &mut book, &mut imgs)?;
    } else {
        mobi6(&pdb, start, &h, text, &mut book, &mut imgs)?;
    }

    if let Some(n) = cover_n {
        if let Some(p) = imgs.path(n) {
            book.cover = Some(p);
        }
    }
    book.resources.extend(imgs.into_resources());
    if book.chapters.is_empty() {
        return corrupt("no readable chapters found");
    }
    if book.toc.is_empty() {
        for ch in &book.chapters {
            book.toc.push(TocEntry {
                title: ch.title.clone(),
                href: ch.file.clone(),
            });
        }
    }
    Ok(book)
}

fn decode(h: &Header, bytes: &[u8]) -> String {
    if h.encoding == 65001 {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        encoding_rs::WINDOWS_1252.decode(bytes).0.into_owned()
    }
}

// ------------------------------------------------------------------- MOBI 6

struct Mobi6Hooks<'a> {
    imgs: &'a mut ImageSet,
}

impl Hooks for Mobi6Hooks<'_> {
    fn attr(&mut self, tag: &str, name: &str, value: &str) -> Hook {
        match (tag, name) {
            ("img", "recindex") => match value
                .trim()
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .and_then(|n| self.imgs.path(n))
            {
                Some(p) => Hook::Set("src".into(), p),
                None => Hook::Drop,
            },
            ("a", "filepos") => match value.trim().parse::<u64>() {
                Ok(n) => Hook::Set("href".into(), format!("#filepos{}", n)),
                Err(_) => Hook::Drop,
            },
            ("a", "href") if !(value.starts_with("http") || value.starts_with('#') || value.starts_with("mailto:")) => {
                Hook::Drop
            }
            _ => Hook::Pass,
        }
    }
}

fn mobi6(
    pdb: &Pdb,
    start: usize,
    h: &Header,
    mut text: Vec<u8>,
    book: &mut Book,
    imgs: &mut ImageSet,
) -> Result<(), NativeError> {
    // NCX gives the real table of contents (label + byte offset into the text).
    let mut ncx: Vec<(String, u64)> = Vec::new();
    if h.ncx_idx != NONE {
        if let Ok((entries, cncx)) = read_index(pdb, start + h.ncx_idx as usize) {
            for e in entries {
                let label = e
                    .tags
                    .get(&3)
                    .and_then(|v| v.first())
                    .and_then(|o| cncx.get(o))
                    .cloned();
                let pos = e.tags.get(&1).and_then(|v| v.first()).copied();
                if let (Some(l), Some(p)) = (label, pos) {
                    ncx.push((l, p as u64));
                }
            }
        }
    }

    // Anchor every byte offset that a link or the NCX points at.
    let fp = regex::bytes::Regex::new(r#"(?i)filepos\s*=\s*["']?0*(\d+)"#).unwrap();
    let mut targets: Vec<usize> = fp
        .captures_iter(&text)
        .filter_map(|c| std::str::from_utf8(&c[1]).ok().and_then(|s| s.parse().ok()))
        .chain(ncx.iter().map(|(_, p)| *p as usize))
        .filter(|&p| p < text.len())
        .collect();
    targets.sort_unstable();
    targets.dedup();

    let mut out = Vec::with_capacity(text.len() + targets.len() * 32);
    let mut last = 0;
    for t in targets {
        // An offset inside a tag has to move to the end of that tag.
        let mut at = t.max(last);
        let lt = text[..at].iter().rposition(|&b| b == b'<');
        let gt = text[..at].iter().rposition(|&b| b == b'>');
        if let Some(l) = lt {
            if gt.is_none_or(|g| g < l) {
                if let Some(e) = text[at..].iter().position(|&b| b == b'>') {
                    at += e + 1;
                }
            }
        }
        out.extend_from_slice(&text[last..at]);
        out.extend_from_slice(format!("<a id=\"filepos{}\"></a>", t).as_bytes());
        last = at;
    }
    out.extend_from_slice(&text[last..]);
    text = out;

    let mut html = decode(h, &text);
    let pb = Regex::new(r"(?i)</?mbp:pagebreak[^>]*>").unwrap();
    if pb.is_match(&html) {
        html = pb
            .replace_all(&html, format!("<hr class=\"{}\"/>", PAGEBREAK_CLASS).as_str())
            .into_owned();
    } else {
        // No explicit breaks: start a new file at each major heading.
        let hd = Regex::new(r"(?i)<h[12][\s>]").unwrap();
        html = hd
            .replace_all(&html, |c: &regex::Captures| {
                format!("<hr class=\"{}\"/>{}", PAGEBREAK_CLASS, &c[0])
            })
            .into_owned();
    }

    let mut hooks = Mobi6Hooks { imgs };
    let mut parsed = xhtml::tidy(&html, &mut hooks);
    if parsed.parts.is_empty() {
        return corrupt("no readable text in the book");
    }
    let files: Vec<String> = (0..parsed.parts.len())
        .map(|i| format!("chapter{}.xhtml", i + 1))
        .collect();
    fix_links(&mut parsed.parts, &files, &parsed.anchors);

    for (i, body) in parsed.parts.into_iter().enumerate() {
        let title = parsed.titles[i].clone().unwrap_or_else(|| format!("Part {}", i + 1));
        book.chapters.push(Chapter {
            title,
            file: files[i].clone(),
            body,
            raw: false,
        });
    }
    for (label, pos) in ncx {
        let id = format!("filepos{}", pos);
        if let Some(&part) = parsed.anchors.get(&id) {
            if part < files.len() {
                book.toc.push(TocEntry {
                    title: label,
                    href: format!("{}#{}", files[part], id),
                });
            }
        }
    }
    Ok(())
}

// ----------------------------------------------------------------------- KF8

fn base32(s: &str) -> Option<usize> {
    let mut v = 0usize;
    for c in s.chars() {
        let d = match c {
            '0'..='9' => c as usize - '0' as usize,
            'A'..='V' => c as usize - 'A' as usize + 10,
            'a'..='v' => c as usize - 'a' as usize + 10,
            _ => return None,
        };
        v = v.checked_mul(32)?.checked_add(d)?;
    }
    Some(v)
}

struct Fragment {
    insert_pos: usize,
    length: usize,
}

fn kf8(
    pdb: &Pdb,
    start: usize,
    h: &Header,
    text: &[u8],
    book: &mut Book,
    imgs: &mut ImageSet,
) -> Result<(), NativeError> {
    // Flow table: flow 0 is the markup, the rest are CSS / SVG.
    let mut flows: Vec<(usize, usize)> = vec![(0, text.len())];
    if h.fdst_idx != NONE {
        let r = pdb.rec(start + h.fdst_idx as usize);
        if r.get(0..4) == Some(b"FDST") {
            let (off, n) = (be32(r, 4).unwrap_or(12) as usize, be32(r, 8).unwrap_or(0) as usize);
            let f: Vec<(usize, usize)> = (0..n.min(4096))
                .filter_map(|i| Some((be32(r, off + i * 8)? as usize, be32(r, off + i * 8 + 4)? as usize)))
                .filter(|&(a, b)| a <= b && b <= text.len())
                .collect();
            if !f.is_empty() {
                flows = f;
            }
        }
    }
    let main = &text[flows[0].0..flows[0].1];

    let (skel_entries, _) = read_index(pdb, start + h.skel_idx as usize)?;
    let (frag_entries, _) = read_index(pdb, start + h.chunk_idx as usize)?;

    let frags: Vec<Fragment> = frag_entries
        .iter()
        .map(|e| Fragment {
            insert_pos: std::str::from_utf8(&e.key)
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0),
            length: e.tags.get(&6).and_then(|v| v.get(1)).copied().unwrap_or(0) as usize,
        })
        .collect();

    // Assemble each skeleton with its chunks.
    let mut parts: Vec<Vec<u8>> = Vec::new();
    // For each fragment: (part index, start position inside the assembled part)
    let mut frag_pos: Vec<(usize, usize)> = Vec::new();
    let mut fptr = 0;
    for e in &skel_entries {
        let count = e.tags.get(&1).and_then(|v| v.first()).copied().unwrap_or(0) as usize;
        let (spos, slen) = match e.tags.get(&6) {
            Some(v) if v.len() >= 2 => (v[0] as usize, v[1] as usize),
            _ => return corrupt("bad skeleton table"),
        };
        if spos + slen > main.len() {
            return corrupt("skeleton outside the text");
        }
        let mut base = spos + slen;
        let mut skeleton = main[spos..base].to_vec();
        let part_no = parts.len();
        for _ in 0..count {
            let Some(f) = frags.get(fptr) else {
                return corrupt("fragment table too short");
            };
            if base + f.length > main.len() {
                return corrupt("fragment outside the text");
            }
            let at = f.insert_pos.saturating_sub(spos).min(skeleton.len());
            let chunk = main[base..base + f.length].to_vec();
            skeleton.splice(at..at, chunk);
            frag_pos.push((part_no, at));
            base += f.length;
            fptr += 1;
        }
        parts.push(skeleton);
    }
    if parts.is_empty() {
        return corrupt("the book has no content files");
    }

    // Convert every part to a string and fix its resource links.
    let mut css: HashMap<usize, String> = HashMap::new();
    let mut strings: Vec<String> = parts.iter().map(|p| String::from_utf8_lossy(p).into_owned()).collect();

    let embed = Regex::new(r#"kindle:embed:([0-9A-Va-v]+)(?:\?mime=[^"')\s]*)?"#).unwrap();
    let flow = Regex::new(r#"kindle:flow:([0-9A-Va-v]+)\?mime=([^"')\s]*)"#).unwrap();

    let mut flow_files: HashMap<usize, (String, &'static str)> = HashMap::new();
    for (i, &(a, b)) in flows.iter().enumerate().skip(1) {
        let body = String::from_utf8_lossy(&text[a..b]).into_owned();
        if body.trim_start().starts_with("<?xml") || body.contains("<svg") {
            flow_files.insert(i, (format!("images/flow{}.svg", i), "image/svg+xml"));
            css.insert(i, body);
        } else {
            flow_files.insert(i, (format!("css/flow{}.css", i), "text/css"));
            css.insert(i, body);
        }
    }

    let rewrite_res = |s: &str, imgs: &mut ImageSet| -> String {
        let s = embed
            .replace_all(s, |c: &regex::Captures| {
                base32(&c[1])
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|n| imgs.path(n))
                    .unwrap_or_default()
            })
            .into_owned();
        flow.replace_all(&s, |c: &regex::Captures| {
            base32(&c[1])
                .and_then(|n| flow_files.get(&n))
                .map(|(p, _)| p.clone())
                .unwrap_or_default()
        })
        .into_owned()
    };

    for s in strings.iter_mut() {
        *s = rewrite_res(s, imgs);
    }
    let mut resources = Vec::new();
    let mut flow_ids: Vec<usize> = css.keys().copied().collect();
    flow_ids.sort_unstable();
    for i in flow_ids {
        let (path, media) = flow_files[&i].clone();
        let body = rewrite_res(&css[&i], imgs);
        resources.push(Resource {
            path,
            media_type: media.into(),
            data: body.into_bytes(),
        });
    }

    // Internal links: kindle:pos:fid:XXXX:off:YYYYYYYYYY
    let pos_re = Regex::new(r"kindle:pos:fid:([0-9A-Va-v]+):off:([0-9A-Va-v]+)").unwrap();
    let id_re = Regex::new(r#"<[^<>]*?\s(?:id|aid)\s*=\s*["']([^"']+)["']"#).unwrap();
    let files: Vec<String> = (0..strings.len()).map(|i| format!("part{:04}.xhtml", i + 1)).collect();

    let resolve = |fid: usize, off: usize, strings: &[String]| -> Option<String> {
        let &(part, at) = frag_pos.get(fid)?;
        let s = strings.get(part)?;
        // Offsets were measured before link/resource rewriting; clamp to stay inside the part.
        let mut npos = (at + off).min(s.len());
        while !s.is_char_boundary(npos) {
            npos -= 1;
        }
        let lt = s[npos..].find('<').map(|x| x + npos);
        let gt = s[npos..].find('>').map(|x| x + npos);
        if let (Some(l), Some(g)) = (lt, gt) {
            if g < l {
                npos = g + 1;
            }
        }
        let head = &s[..npos];
        let id = id_re.captures_iter(head).last().map(|c| c[1].to_string());
        Some(match id {
            Some(id) => format!("{}#{}", files[part], id),
            None => files[part].clone(),
        })
    };

    let snapshot = strings.clone();
    for s in strings.iter_mut() {
        *s = pos_re
            .replace_all(s, |c: &regex::Captures| {
                let (Some(fid), Some(off)) = (base32(&c[1]), base32(&c[2])) else {
                    return "#".to_string();
                };
                resolve(fid, off, &snapshot).unwrap_or_else(|| "#".to_string())
            })
            .into_owned();
    }

    // aid="..." becomes a real id wherever the element has none.
    let tag_re = Regex::new(r"<[^<>]+>").unwrap();
    let aid_re = Regex::new(r#"\said\s*=\s*["']([^"']+)["']"#).unwrap();
    let has_id_re = Regex::new(r#"\sid\s*="#).unwrap();
    for s in strings.iter_mut() {
        *s = tag_re
            .replace_all(s, |c: &regex::Captures| {
                let t = &c[0];
                if !t.contains("aid") {
                    return t.to_string();
                }
                if has_id_re.is_match(t) {
                    aid_re.replace_all(t, "").into_owned()
                } else {
                    aid_re.replace_all(t, " id=\"$1\"").into_owned()
                }
            })
            .into_owned();
    }

    // Table of contents from the NCX.
    if h.ncx_idx != NONE {
        if let Ok((entries, cncx)) = read_index(pdb, start + h.ncx_idx as usize) {
            for e in entries {
                let label = e
                    .tags
                    .get(&3)
                    .and_then(|v| v.first())
                    .and_then(|o| cncx.get(o))
                    .cloned();
                let target = e.tags.get(&6).and_then(|v| {
                    if v.len() >= 2 {
                        Some((v[0] as usize, v[1] as usize))
                    } else {
                        None
                    }
                });
                if let (Some(l), Some((fid, off))) = (label, target) {
                    if let Some(href) = resolve(fid, off, &snapshot) {
                        book.toc.push(TocEntry { title: l, href });
                    }
                }
            }
        }
    }

    let title_re = Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap();
    for (i, s) in strings.into_iter().enumerate() {
        let title = title_re
            .captures(&s)
            .map(|c| c[1].trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| format!("Part {}", i + 1));
        book.chapters.push(Chapter {
            title,
            file: files[i].clone(),
            body: s,
            raw: true,
        });
    }
    book.resources.extend(resources);
    Ok(())
}

/// Plain PalmDOC (.pdb/.prc "TEXtREAd"): compressed text, no MOBI header.
fn palmdoc(path: &Path, pdb: &Pdb) -> Result<Book, NativeError> {
    let r0 = pdb.rec(0);
    let h = Header {
        compression: be16(r0, 0).unwrap_or(0),
        text_len: be32(r0, 4).unwrap_or(0),
        rec_count: be16(r0, 8).unwrap_or(0),
        crypto: be16(r0, 12).unwrap_or(0),
        encoding: 1252,
        ..Default::default()
    };
    if h.crypto != 0 {
        return Err(NativeError::Drm);
    }
    let text = raw_text(pdb, 0, &h)?;
    let text = encoding_rs::WINDOWS_1252
        .decode(&text)
        .0
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let title = String::from_utf8_lossy(&pdb.data[..32.min(pdb.data.len())])
        .split('\0')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let title = if title.is_empty() {
        path.file_stem().and_then(|s| s.to_str()).unwrap_or("book").to_string()
    } else {
        title
    };
    let paras: Vec<String> = text
        .split("\n\n")
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .collect();
    if paras.is_empty() {
        return corrupt("the book has no text");
    }
    Ok(super::text::book_from_paragraphs(title, paras))
}
