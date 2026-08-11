"""Kindle EPUB fixes — Python port of innocenat/kindle-epub-fix.

Four fixes, all applied in-memory then repacked atomically:

  encoding   Kindle assumes latin-1 for XHTML with no XML declaration, which
             turns every non-ASCII character into mojibake. Prepend one.
  language   A missing/invalid <dc:language> makes the Kindle refuse the book
             or file it under the wrong dictionary.
  body-id    A hyperlink targeting an id that lives on <body> (common in TOCs)
             silently dead-ends on Kindle. The fragment is dropped so the link
             lands at the top of the file, which is the same place.
  stray img  <img> pointing at a file that isn't in the archive aborts the
             Kindle's renderer for the whole chapter.
"""

from __future__ import annotations

import os
import posixpath
import re
import shutil
import tempfile
import zipfile

XHTML_EXT = (".xhtml", ".html", ".htm", ".xht")

# Matches an XML declaration at the very start of the document.
_XML_DECL = re.compile(r"^\s*<\?xml[^>]*\?>", re.IGNORECASE)
_XML_DECL_ENCODING = re.compile(r'encoding\s*=\s*["\'][^"\']*["\']', re.IGNORECASE)
_BODY_ID = re.compile(r"<body\b[^>]*\bid\s*=\s*[\"']([^\"']+)[\"']", re.IGNORECASE)
_HREF = re.compile(r"""(href\s*=\s*)(["'])([^"']+)(\2)""", re.IGNORECASE)
_IMG_TAG = re.compile(r"<img\b[^>]*?/?>", re.IGNORECASE | re.DOTALL)
_IMG_SRC = re.compile(r"""\bsrc\s*=\s*["']([^"']*)["']""", re.IGNORECASE)
_DC_LANGUAGE = re.compile(
    r"<(dc:language|language)\b([^>]*)>(.*?)</\1>", re.IGNORECASE | re.DOTALL
)
_METADATA_OPEN = re.compile(r"<(\w*:?metadata)\b[^>]*>", re.IGNORECASE)
_LANG_CODE = re.compile(r"^[a-z]{2,3}(-[a-z0-9]{2,8})*$", re.IGNORECASE)
_BAD_LANGS = {"", "und", "unknown", "none", "null", "xx"}


class EpubFixError(Exception):
    pass


def _read_all(path):
    """Return (ordered names, {name: bytes}) for every entry in the epub."""
    with zipfile.ZipFile(path, "r") as zf:
        bad = zf.testzip()
        if bad is not None:
            raise EpubFixError(f"corrupt entry in archive: {bad}")
        names = zf.namelist()
        return names, {n: zf.read(n) for n in names}


def _decode(data):
    for enc in ("utf-8", "utf-8-sig", "cp1252", "latin-1"):
        try:
            return data.decode(enc), enc
        except UnicodeDecodeError:
            continue
    return data.decode("latin-1", "replace"), "latin-1"


def _opf_path(files):
    """Locate the OPF via META-INF/container.xml, falling back to a scan."""
    container = files.get("META-INF/container.xml")
    if container:
        m = re.search(
            r"""full-path\s*=\s*["']([^"']+)["']""", container.decode("utf-8", "replace")
        )
        if m:
            return m.group(1)
    for name in files:
        if name.lower().endswith(".opf"):
            return name
    return None


def _fix_encoding(files, changes):
    for name in list(files):
        if not name.lower().endswith(XHTML_EXT):
            continue
        text, _ = _decode(files[name])
        decl = _XML_DECL.match(text)
        if decl is None:
            files[name] = ('<?xml version="1.0" encoding="utf-8"?>\n' + text).encode("utf-8")
            changes.append(f"encoding: added XML declaration to {name}")
        elif not _XML_DECL_ENCODING.search(decl.group(0)):
            fixed = decl.group(0).rstrip()[:-2].rstrip() + ' encoding="utf-8"?>'
            files[name] = (fixed + text[decl.end():]).encode("utf-8")
            changes.append(f"encoding: added utf-8 to declaration in {name}")


def _fix_language(files, changes, default_lang="en"):
    opf = _opf_path(files)
    if not opf or opf not in files:
        return
    text, _ = _decode(files[opf])
    m = _DC_LANGUAGE.search(text)
    if m:
        current = m.group(3).strip()
        if current.lower() in _BAD_LANGS or not _LANG_CODE.match(current):
            new = f"<{m.group(1)}{m.group(2)}>{default_lang}</{m.group(1)}>"
            text = text[: m.start()] + new + text[m.end():]
            files[opf] = text.encode("utf-8")
            changes.append(f"language: {current!r} -> {default_lang!r}")
        return
    # No language element at all — insert one right after <metadata>.
    meta = _METADATA_OPEN.search(text)
    if meta:
        text = (
            text[: meta.end()]
            + f"\n    <dc:language>{default_lang}</dc:language>"
            + text[meta.end():]
        )
        files[opf] = text.encode("utf-8")
        changes.append(f"language: missing -> {default_lang!r}")


def _fix_body_id_links(files, changes):
    # Collect every id that sits on a <body> tag, keyed by archive path.
    body_ids = {}
    for name in files:
        if not name.lower().endswith(XHTML_EXT):
            continue
        text, _ = _decode(files[name])
        m = _BODY_ID.search(text)
        if m:
            body_ids[name] = m.group(1)
    if not body_ids:
        return

    for name in list(files):
        if not name.lower().endswith(XHTML_EXT):
            continue
        text, _ = _decode(files[name])
        base = posixpath.dirname(name)
        hits = []

        def repl(match):
            href = match.group(3)
            if "#" not in href or href.startswith(("http:", "https:", "mailto:")):
                return match.group(0)
            target, _, frag = href.partition("#")
            resolved = posixpath.normpath(posixpath.join(base, target)) if target else name
            if body_ids.get(resolved) != frag:
                return match.group(0)
            hits.append(href)
            return f"{match.group(1)}{match.group(2)}{target or ''}{match.group(4)}"

        new_text = _HREF.sub(repl, text)
        if hits:
            files[name] = new_text.encode("utf-8")
            changes.append(f"body-id: stripped {len(hits)} link fragment(s) in {name}")


def _fix_stray_images(files, changes):
    present = set(files)
    for name in list(files):
        if not name.lower().endswith(XHTML_EXT):
            continue
        text, _ = _decode(files[name])
        base = posixpath.dirname(name)
        removed = []

        def repl(match):
            tag = match.group(0)
            src_match = _IMG_SRC.search(tag)
            src = src_match.group(1).strip() if src_match else ""
            if src.startswith(("http:", "https:", "data:")):
                return tag
            if src:
                resolved = posixpath.normpath(posixpath.join(base, src.split("#")[0]))
                if resolved in present:
                    return tag
            removed.append(src)
            return ""

        new_text = _IMG_TAG.sub(repl, text)
        if removed:
            files[name] = new_text.encode("utf-8")
            changes.append(f"stray-img: removed {len(removed)} broken <img> in {name}")


def _repack(src_path, names, files, dest_path):
    """Write a spec-compliant epub: mimetype first and stored, rest deflated."""
    order = [n for n in names if n in files]
    order += [n for n in files if n not in names]
    if "mimetype" in order:
        order.remove("mimetype")
    order.insert(0, "mimetype")
    if "mimetype" not in files:
        files["mimetype"] = b"application/epub+zip"

    with zipfile.ZipFile(dest_path, "w") as zf:
        zf.writestr(
            zipfile.ZipInfo("mimetype"), files["mimetype"], compress_type=zipfile.ZIP_STORED
        )
        for name in order[1:]:
            zf.writestr(name, files[name], compress_type=zipfile.ZIP_DEFLATED)


def _validate(path):
    with zipfile.ZipFile(path, "r") as zf:
        if zf.testzip() is not None:
            raise EpubFixError("repacked archive failed CRC check")
        names = zf.namelist()
        if not names or names[0] != "mimetype":
            raise EpubFixError("mimetype is not the first entry")
        if zf.read("mimetype").strip() != b"application/epub+zip":
            raise EpubFixError("mimetype content is wrong")
        if "META-INF/container.xml" not in names:
            raise EpubFixError("META-INF/container.xml missing")
        opf = _opf_path({n: zf.read(n) for n in ("META-INF/container.xml",)})
        if opf and opf not in names:
            raise EpubFixError(f"OPF {opf} referenced but missing")


def fix_epub(path, in_place=True, dest=None, default_lang="en"):
    """Apply all Kindle fixes to `path`.

    Returns a list of human-readable change descriptions (empty = nothing to do).
    The original is only replaced once the rewritten file validates.
    """
    if not zipfile.is_zipfile(path):
        raise EpubFixError("not a zip archive — is this really an epub?")

    names, files = _read_all(path)
    changes = []
    _fix_encoding(files, changes)
    _fix_language(files, changes, default_lang)
    _fix_body_id_links(files, changes)
    _fix_stray_images(files, changes)

    target = dest or path
    if not changes and in_place and target == path:
        return changes

    tmp_dir = os.path.dirname(os.path.abspath(target)) or "."
    fd, tmp = tempfile.mkstemp(suffix=".epub", dir=tmp_dir)
    os.close(fd)
    try:
        _repack(path, names, files, tmp)
        _validate(tmp)
        shutil.move(tmp, target)
    except Exception:
        if os.path.exists(tmp):
            os.unlink(tmp)
        raise
    return changes
