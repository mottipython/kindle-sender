"""Build deliberately broken epubs and check each fix fires (and validates)."""

import os
import shutil
import sys
import tempfile
import zipfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from kindle_sender.epub_fix import fix_epub  # noqa: E402

CONTAINER = """<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf"
    media-type="application/oebps-package+xml"/></rootfiles>
</container>"""

OPF = """<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Broken Book</dc:title>
    <dc:creator>Test Author</dc:creator>
    <dc:identifier id="id">urn:uuid:1234</dc:identifier>
    {lang}
  </metadata>
  <manifest>
    <item id="toc" href="toc.xhtml" media-type="application/xhtml+xml"/>
    <item id="ch1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="toc"/><itemref idref="ch1"/></spine>
</package>"""

TOC = """<html xmlns="http://www.w3.org/1999/xhtml"><head><title>TOC</title></head>
<body id="toc-top">
<a href="chapter1.xhtml#chap1-body">Chapter One</a>
<a href="chapter1.xhtml#real-anchor">Deep link</a>
<a href="https://example.com/x#frag">External</a>
</body></html>"""

CHAPTER = """<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Ch1</title></head>
<body id="chap1-body">
<h1>Café naïve résumé</h1>
<p id="real-anchor">Text</p>
<img src="missing.jpg" alt="gone"/>
<img src="there.png" alt="ok"/>
<img alt="no src"/>
</body></html>"""


def build(path, lang="<dc:language>und</dc:language>"):
    with zipfile.ZipFile(path, "w") as zf:
        zf.writestr("mimetype", "application/epub+zip", zipfile.ZIP_STORED)
        zf.writestr("META-INF/container.xml", CONTAINER)
        zf.writestr("OEBPS/content.opf", OPF.format(lang=lang))
        zf.writestr("OEBPS/toc.xhtml", TOC)
        zf.writestr("OEBPS/chapter1.xhtml", CHAPTER)
        zf.writestr("OEBPS/there.png", b"\x89PNG\r\n\x1a\n fake")


def read(path, name):
    with zipfile.ZipFile(path) as zf:
        return zf.read(name).decode("utf-8")


def check(cond, label):
    print(("  PASS  " if cond else "  FAIL  ") + label)
    return cond


def main():
    tmp = tempfile.mkdtemp()
    ok = True
    try:
        for label, lang in (
            ("invalid dc:language", "<dc:language>und</dc:language>"),
            ("missing dc:language", ""),
        ):
            book = os.path.join(tmp, "broken.epub")
            build(book, lang)
            changes = fix_epub(book)
            print(f"\n{label}: {len(changes)} change(s)")
            for c in changes:
                print(f"   - {c}")

            opf = read(book, "OEBPS/content.opf")
            toc = read(book, "OEBPS/toc.xhtml")
            chapter = read(book, "OEBPS/chapter1.xhtml")

            ok &= check("<dc:language>en</dc:language>" in opf, "language set to en")
            ok &= check(chapter.startswith("<?xml"), "XML declaration prepended")
            ok &= check("utf-8" in chapter.split("\n")[0], "declaration names utf-8")
            ok &= check("Café naïve" in chapter, "accents survived the round trip")
            ok &= check('href="chapter1.xhtml"' in toc, "body-id fragment stripped")
            ok &= check('href="chapter1.xhtml#real-anchor"' in toc, "real anchor kept")
            ok &= check('href="https://example.com/x#frag"' in toc, "external link kept")
            ok &= check("missing.jpg" not in chapter, "stray img removed")
            ok &= check("there.png" in chapter, "valid img kept")
            ok &= check('alt="no src"' not in chapter, "src-less img removed")

            with zipfile.ZipFile(book) as zf:
                names = zf.namelist()
                info = zf.getinfo("mimetype")
            ok &= check(names[0] == "mimetype", "mimetype first in archive")
            ok &= check(info.compress_type == zipfile.ZIP_STORED, "mimetype stored uncompressed")
            ok &= check(len(names) == 6, "no entries lost")

        # Idempotence: a second pass should find nothing left to fix.
        book = os.path.join(tmp, "broken.epub")
        second = fix_epub(book)
        ok &= check(second == [], "second run is a no-op")

        # A clean book must be left completely alone.
        clean = os.path.join(tmp, "clean.epub")
        build(clean, "<dc:language>en</dc:language>")
        fix_epub(clean)          # normalises it
        before = open(clean, "rb").read()
        fix_epub(clean)
        ok &= check(open(clean, "rb").read() == before, "already-clean book untouched")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    print("\nALL PASS" if ok else "\nFAILURES")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
