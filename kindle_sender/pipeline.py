"""The fix-and-send pipeline. Pure logic — the GUI just drives it."""

from __future__ import annotations

import os
import re
import shutil
import tempfile
import unicodedata

from . import ebook_tools, sender
from .epub_fix import EpubFixError, fix_epub


class Result:
    def __init__(self, source):
        self.source = source
        self.name = os.path.basename(source)
        self.title = None
        self.changes = []
        self.status = "pending"   # pending | ok | skipped | error
        self.error = None
        self.output = None
        self.sent = False

    @property
    def summary(self):
        if self.status == "error":
            return self.error or "failed"
        if not self.changes:
            return "sent (no changes needed)" if self.sent else "no changes needed"
        head = f"{len(self.changes)} fix(es)"
        return f"sent, {head}" if self.sent else head


def _safe_filename(name):
    name = unicodedata.normalize("NFKD", name)
    name = re.sub(r'[\\/:*?"<>|]+', "_", name).strip()
    return re.sub(r"\s+", " ", name)[:150] or "book"


def process_file(path, cfg, log=print, cancelled=lambda: False):
    """Run one book through convert -> metadata -> cover -> fix -> send."""
    result = Result(path)
    work = tempfile.mkdtemp(prefix="kindle-sender-")
    try:
        ext = os.path.splitext(path)[1].lower()
        staged = os.path.join(work, _safe_filename(os.path.splitext(os.path.basename(path))[0]) + ".epub")

        if ext == ".epub":
            shutil.copy2(path, staged)
        elif cfg.get("do_convert") and ext in ebook_tools.CONVERTIBLE:
            log(f"  converting {ext} -> epub (this can take a minute)")
            ebook_tools.convert_to_epub(path, staged)
            result.changes.append(f"converted {ext} -> epub")
        else:
            result.status = "skipped"
            result.error = (
                f"{ext or 'no extension'} is not an epub"
                + ("" if cfg.get("do_convert") else " and conversion is off")
            )
            return result

        if cancelled():
            result.status = "skipped"
            result.error = "cancelled"
            return result

        if cfg.get("do_metadata"):
            try:
                result.changes += ebook_tools.fix_metadata(staged, source_path=path)
            except ebook_tools.ToolError as exc:
                log(f"  metadata skipped: {exc}")

        try:
            meta = ebook_tools.read_metadata(staged)
            result.title = meta.get("title")
        except ebook_tools.ToolError:
            pass

        if cfg.get("do_cover"):
            try:
                if ebook_tools.has_real_cover(staged):
                    log("  cover looks fine, leaving it")
                else:
                    fetched = ebook_tools.fetch_cover(staged, work)
                    result.changes += fetched
                    if not fetched:
                        log("  no cover match on Open Library")
            except Exception as exc:
                log(f"  cover skipped: {type(exc).__name__}: {exc}")

        if cfg.get("do_fix"):
            try:
                result.changes += fix_epub(
                    staged, default_lang=cfg.get("default_lang") or "en"
                )
            except EpubFixError as exc:
                result.status = "error"
                result.error = f"epub fix failed: {exc}"
                return result

        if cancelled():
            result.status = "skipped"
            result.error = "cancelled"
            return result

        # Keep a copy before sending, so a send failure doesn't lose the work.
        out_dir = cfg.get("output_dir") or ""
        if cfg.get("keep_copy") and out_dir:
            os.makedirs(out_dir, exist_ok=True)
            final = os.path.join(out_dir, os.path.basename(staged))
            shutil.copy2(staged, final)
            result.output = final

        if cfg.get("do_send"):
            size = sender.send_book(cfg, staged, subject=result.title)
            result.sent = True
            log(f"  emailed {size / 1e6:.1f} MB to {cfg.get('kindle_addr')}")

        result.status = "ok"
        return result
    except Exception as exc:
        result.status = "error"
        result.error = f"{type(exc).__name__}: {exc}"
        return result
    finally:
        shutil.rmtree(work, ignore_errors=True)
