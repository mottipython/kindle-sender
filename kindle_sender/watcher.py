"""Poll a folder for new books and hand them to a callback.

Polling (rather than inotify) keeps this dependency-free and works fine on
network/USB mounts. Files are only picked up once their size has stopped
changing, so half-copied downloads are left alone.
"""

from __future__ import annotations

import os
import threading
import time

WATCHED_EXT = {".epub", ".mobi", ".azw", ".azw3", ".prc", ".lit", ".pdf",
               ".fb2", ".rtf", ".txt", ".docx", ".cbz"}
SUBDIRS = ("sent", "failed")


class FolderWatcher:
    def __init__(self, folder, on_file, log=print, interval=5.0):
        self.folder = folder
        self.on_file = on_file
        self.log = log
        self.interval = interval
        self._stop = threading.Event()
        self._thread = None
        self._sizes = {}

    def start(self):
        if self._thread and self._thread.is_alive():
            return
        os.makedirs(self.folder, exist_ok=True)
        for sub in SUBDIRS:
            os.makedirs(os.path.join(self.folder, sub), exist_ok=True)
        self._stop.clear()
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()
        self.log(f"Watching {self.folder}")

    def stop(self):
        self._stop.set()
        self.log(f"Stopped watching {self.folder}")

    @property
    def running(self):
        return self._thread is not None and self._thread.is_alive() and not self._stop.is_set()

    def _candidates(self):
        try:
            entries = os.listdir(self.folder)
        except OSError as exc:
            self.log(f"Watch folder unreadable: {exc}")
            return []
        out = []
        for name in entries:
            path = os.path.join(self.folder, name)
            if not os.path.isfile(path):
                continue
            if os.path.splitext(name)[1].lower() not in WATCHED_EXT:
                continue
            out.append(path)
        return out

    def _run(self):
        while not self._stop.is_set():
            for path in self._candidates():
                try:
                    size = os.path.getsize(path)
                except OSError:
                    continue
                previous = self._sizes.get(path)
                self._sizes[path] = size
                if previous is None or previous != size:
                    continue  # still being written
                self._sizes.pop(path, None)
                self.log(f"Watch: picked up {os.path.basename(path)}")
                try:
                    self.on_file(path)
                except Exception as exc:  # never let the watch thread die
                    self.log(f"Watch: {type(exc).__name__}: {exc}")
            self._stop.wait(self.interval)


def archive(path, watch_folder, ok):
    """Move a processed file into sent/ or failed/ so it isn't picked up again."""
    dest_dir = os.path.join(watch_folder, "sent" if ok else "failed")
    os.makedirs(dest_dir, exist_ok=True)
    dest = os.path.join(dest_dir, os.path.basename(path))
    stem, ext = os.path.splitext(dest)
    n = 1
    while os.path.exists(dest):
        dest = f"{stem} ({n}){ext}"
        n += 1
    try:
        os.replace(path, dest)
    except OSError:
        import shutil

        shutil.move(path, dest)
    return dest
