"""Wrappers around the native calibre CLI (ebook-convert / ebook-meta).

Uses the system binaries directly — no flatpak, which stalls badly per-file.
"""

from __future__ import annotations

import io
import os
import re
import shutil
import subprocess
import urllib.parse
import urllib.request
import zipfile

CONVERTIBLE = {".mobi", ".azw", ".azw3", ".prc", ".lit", ".pdf", ".fb2", ".rtf",
               ".txt", ".doc", ".docx", ".html", ".htm", ".cbz", ".cbr", ".pdb"}

# Formats with no metadata container — calibre invents title/author from the
# filename during conversion, so our own filename parse is at least as good.
METADATA_POOR = {".txt", ".rtf", ".html", ".htm", ".pdf", ".cbz", ".cbr",
                 ".doc", ".docx", ".pdb"}

_OPEN_LIBRARY = "https://openlibrary.org/search.json"
_COVER_URL = "https://covers.openlibrary.org/b/id/{cover_id}-L.jpg"
_USER_AGENT = "kindle-sender/1.0 (personal ebook tool)"


class ToolError(Exception):
    pass


def _which(name):
    path = shutil.which(name)
    if not path:
        raise ToolError(f"{name} not found — install calibre (dnf install calibre)")
    return path


def tools_available():
    return {n: shutil.which(n) is not None for n in ("ebook-convert", "ebook-meta")}


def _run(args, timeout=600):
    proc = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
    if proc.returncode != 0:
        tail = (proc.stderr or proc.stdout or "").strip().splitlines()
        raise ToolError((tail[-1] if tail else f"exit {proc.returncode}")[:300])
    return proc.stdout


def convert_to_epub(src, dest, timeout=600):
    """Convert any calibre-supported format to epub. Returns dest."""
    _run([_which("ebook-convert"), src, dest], timeout=timeout)
    if not os.path.exists(dest):
        raise ToolError("ebook-convert produced no output")
    return dest


# --------------------------------------------------------------------------
# metadata
# --------------------------------------------------------------------------

_META_LINE = re.compile(r"^([^:]+?)\s*:\s*(.*)$")
_FILENAME_SPLIT = re.compile(r"\s+-\s+")


def read_metadata(path):
    out = _run([_which("ebook-meta"), path], timeout=120)
    meta = {}
    for line in out.splitlines():
        m = _META_LINE.match(line)
        if m:
            meta[m.group(1).strip().lower()] = m.group(2).strip()
    return meta


def _placeholder_title(title, path):
    if not title:
        return True
    stem = os.path.splitext(os.path.basename(path))[0]
    return title.strip().lower() in {"unknown", stem.lower()}


def _placeholder_author(author, title=None, stem=None):
    """True when the author field carries no real information.

    Beyond the obvious "Unknown", calibre fills this in by pattern-matching the
    filename when the source format has no metadata, which routinely yields the
    title (or half of it) as the author.
    """
    value = (author or "").strip().lower()
    if value in {"", "unknown", "unknown author", "anonymous", "calibre"}:
        return True
    if title and value == title.strip().lower():
        return True
    if stem and value in (stem.strip().lower(), stem.strip().lower().replace("_", " ")):
        return True
    return False


def guess_from_filename(path):
    """'Terry Pratchett - Mort.epub' -> ('Mort', 'Terry Pratchett')."""
    stem = os.path.splitext(os.path.basename(path))[0].replace("_", " ").strip()
    parts = [p.strip() for p in _FILENAME_SPLIT.split(stem) if p.strip()]
    if len(parts) >= 2:
        first, last = parts[0], parts[-1]
        # "Author - Title" if the first chunk looks like a person's name.
        if len(first.split()) <= 4 and not first[:1].isdigit():
            return last, first
        return first, None
    return stem or None, None


def sidecar_opf(src_path):
    """A calibre library folder keeps metadata.opf next to the book."""
    opf = os.path.join(os.path.dirname(os.path.abspath(src_path)), "metadata.opf")
    return opf if os.path.exists(opf) else None


def fix_metadata(epub_path, source_path=None):
    """Fill in title/author. Prefers a calibre metadata.opf, then the filename.

    Returns a list of change descriptions.
    """
    source_path = source_path or epub_path
    changes = []
    opf = sidecar_opf(source_path)
    if opf:
        _run([_which("ebook-meta"), epub_path, "--from-opf", opf], timeout=120)
        changes.append(f"metadata: embedded from {os.path.basename(opf)}")
        return changes

    meta = read_metadata(epub_path)
    title, author = meta.get("title"), meta.get("author(s)")
    if author:
        author = re.sub(r"\s*\[[^\]]*\]\s*$", "", author).strip()

    stem = os.path.splitext(os.path.basename(source_path))[0]
    guess_title, guess_author = guess_from_filename(source_path)
    # Formats that carry no metadata of their own: whatever calibre put in the
    # fields is its own guess at the filename, so ours wins.
    trust_filename = os.path.splitext(source_path)[1].lower() in METADATA_POOR

    args = []
    if guess_title and (trust_filename or _placeholder_title(title, source_path)):
        args += ["-t", guess_title, "--title-sort", guess_title]
        changes.append(f"metadata: title -> {guess_title!r}")
    if guess_author and (trust_filename or _placeholder_author(author, title, stem)):
        args += ["-a", guess_author, "--author-sort", _sort_name(guess_author)]
        changes.append(f"metadata: author -> {guess_author!r}")
    elif _placeholder_author(author, title, stem) and not guess_author:
        # Better a blank author than the title repeated as one.
        if author and author.lower() not in {"unknown", ""}:
            args += ["-a", "Unknown"]
            changes.append(f"metadata: cleared bogus author {author!r}")
    if args:
        _run([_which("ebook-meta"), epub_path] + args, timeout=120)
    return changes


def _sort_name(author):
    parts = author.split()
    if len(parts) < 2:
        return author
    return f"{parts[-1]}, {' '.join(parts[:-1])}"


# --------------------------------------------------------------------------
# covers
# --------------------------------------------------------------------------


def has_real_cover(epub_path, max_colors=64):
    """True if the epub carries a cover image that isn't a flat placeholder.

    A generated placeholder is a few flat colours plus text; real art is not.
    """
    try:
        from PIL import Image
    except ImportError:
        return True  # can't judge — assume fine and skip fetching

    try:
        with zipfile.ZipFile(epub_path) as zf:
            candidates = [
                n for n in zf.namelist()
                if re.search(r"cover.*\.(jpe?g|png)$", n, re.I)
                or re.search(r"\.(jpe?g|png)$", n, re.I) and "cover" in n.lower()
            ]
            if not candidates:
                return False
            data = zf.read(sorted(candidates, key=len)[0])
        img = Image.open(io.BytesIO(data)).convert("RGB")
        img.thumbnail((256, 256))
        colors = img.getcolors(maxcolors=max_colors + 1)
        return colors is None or len(colors) > max_colors
    except Exception:
        return True


def _openlibrary_cover(title, author, timeout=15):
    """Strict match: author surname plus a title-token overlap, or no cover."""
    query = {"title": title, "limit": "5"}
    if author:
        query["author"] = author
    url = f"{_OPEN_LIBRARY}?{urllib.parse.urlencode(query)}"
    req = urllib.request.Request(url, headers={"User-Agent": _USER_AGENT})
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        import json

        data = json.load(resp)

    want = {w for w in re.findall(r"\w+", title.lower()) if len(w) > 2}
    surname = author.split()[-1].lower() if author else None
    for doc in data.get("docs", []):
        cover_id = doc.get("cover_i")
        if not cover_id:
            continue
        got = {w for w in re.findall(r"\w+", (doc.get("title") or "").lower()) if len(w) > 2}
        if want and not (want & got):
            continue
        if surname:
            authors = " ".join(doc.get("author_name") or []).lower()
            if surname not in authors:
                continue
        return _COVER_URL.format(cover_id=cover_id)
    return None


def fetch_cover(epub_path, work_dir, timeout=15):
    """Fetch a cover from Open Library and embed it. Returns change list."""
    meta = read_metadata(epub_path)
    title = meta.get("title") or ""
    author = re.sub(r"\s*\[[^\]]*\]\s*$", "", meta.get("author(s)") or "").strip()
    if not title or title.lower() == "unknown":
        return []
    url = _openlibrary_cover(title, author or None, timeout=timeout)
    if not url:
        return []
    req = urllib.request.Request(url, headers={"User-Agent": _USER_AGENT})
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        data = resp.read()
    if len(data) < 3000:  # Open Library serves a 1x1 for "no image"
        return []
    cover_path = os.path.join(work_dir, "cover.jpg")
    with open(cover_path, "wb") as fh:
        fh.write(data)
    _run([_which("ebook-meta"), epub_path, "--cover", cover_path], timeout=120)
    return [f"cover: fetched from Open Library for {title!r}"]
