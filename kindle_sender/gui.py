"""Tkinter GUI for kindle-sender."""

from __future__ import annotations

import os
import queue
import threading
import tkinter as tk
from tkinter import filedialog, messagebox, ttk
from tkinter.scrolledtext import ScrolledText

from . import config, ebook_tools, pipeline, sender, watcher

try:
    from tkinterdnd2 import DND_FILES, TkinterDnD

    HAVE_DND = True
except ImportError:  # optional — file picker still works
    HAVE_DND = False

APP_TITLE = "Kindle Sender"
BOOK_TYPES = [
    ("Ebooks", "*.epub *.mobi *.azw *.azw3 *.prc *.lit *.pdf *.fb2 *.rtf *.txt *.docx *.cbz"),
    ("EPUB", "*.epub"),
    ("All files", "*"),
]
STATUS_COLORS = {
    "pending": "#666666",
    "working": "#0a58ca",
    "ok": "#1a7f37",
    "skipped": "#9a6700",
    "error": "#b42318",
}


class App:
    def __init__(self, root):
        self.root = root
        self.cfg = config.load()
        self.events = queue.Queue()
        self.rows = {}            # tree iid -> path
        self.worker = None
        self.cancel_flag = threading.Event()
        self.watcher = None

        root.title(APP_TITLE)
        root.geometry("880x620")
        root.minsize(720, 500)

        self._build_vars()
        self._build_ui()
        self._apply_dnd()

        if self.cfg.get("watch_enabled"):
            self._toggle_watch(initial=True)

        root.protocol("WM_DELETE_WINDOW", self._on_close)
        root.after(100, self._drain_events)
        self._check_tools()

        # First run (or an incomplete setup) opens on Settings rather than on an
        # empty book list with a Send button that can't work yet.
        if not config.is_configured(self.cfg):
            self.nb.select(self.settings_tab)
            self.log("Email isn't set up yet — fill in the Settings tab, then Test connection.")

    # ------------------------------------------------------------------ vars
    def _build_vars(self):
        c = self.cfg
        self.v = {
            "do_convert": tk.BooleanVar(value=c["do_convert"]),
            "do_metadata": tk.BooleanVar(value=c["do_metadata"]),
            "do_cover": tk.BooleanVar(value=c["do_cover"]),
            "do_fix": tk.BooleanVar(value=c["do_fix"]),
            "do_send": tk.BooleanVar(value=c["do_send"]),
            "keep_copy": tk.BooleanVar(value=c["keep_copy"]),
            "smtp_host": tk.StringVar(value=c["smtp_host"]),
            "smtp_port": tk.StringVar(value=str(c["smtp_port"])),
            "smtp_user": tk.StringVar(value=c["smtp_user"]),
            "smtp_password": tk.StringVar(value=c["smtp_password"]),
            "from_addr": tk.StringVar(value=c["from_addr"]),
            "kindle_addr": tk.StringVar(value=c["kindle_addr"]),
            "output_dir": tk.StringVar(value=c["output_dir"]),
            "watch_folder": tk.StringVar(value=c["watch_folder"]),
            "watch_enabled": tk.BooleanVar(value=c["watch_enabled"]),
            "default_lang": tk.StringVar(value=c["default_lang"]),
        }
        self.status_text = tk.StringVar(value="Ready.")

    def _collect(self):
        """Pull the widget values back into the config dict."""
        for key, var in self.v.items():
            self.cfg[key] = var.get()
        try:
            self.cfg["smtp_port"] = int(self.v["smtp_port"].get() or 587)
        except ValueError:
            self.cfg["smtp_port"] = 587
        return self.cfg

    # -------------------------------------------------------------------- ui
    def _build_ui(self):
        nb = ttk.Notebook(self.root)
        nb.pack(fill="both", expand=True, padx=8, pady=8)
        self.nb = nb
        self.queue_tab = ttk.Frame(nb)
        self.settings_tab = ttk.Frame(nb)
        self.log_tab = ttk.Frame(nb)
        nb.add(self.queue_tab, text="  Books  ")
        nb.add(self.settings_tab, text="  Settings  ")
        nb.add(self.log_tab, text="  Log  ")

        self._build_queue_tab()
        self._build_settings_tab()
        self._build_log_tab()

        bar = ttk.Frame(self.root)
        bar.pack(fill="x", padx=8, pady=(0, 8))
        self.progress = ttk.Progressbar(bar, mode="determinate")
        self.progress.pack(side="left", fill="x", expand=True)
        ttk.Label(bar, textvariable=self.status_text, width=42, anchor="e").pack(
            side="left", padx=(10, 0)
        )

    def _build_queue_tab(self):
        f = self.queue_tab
        hint = "Drag books here, or use Add files" if HAVE_DND else "Use Add files to queue books"
        ttk.Label(f, text=hint, foreground="#666").pack(anchor="w", padx=6, pady=(6, 2))

        tree_frame = ttk.Frame(f)
        tree_frame.pack(fill="both", expand=True, padx=6)
        cols = ("status", "detail")
        self.tree = ttk.Treeview(tree_frame, columns=cols, show="tree headings", height=12)
        self.tree.heading("#0", text="Book")
        self.tree.heading("status", text="Status")
        self.tree.heading("detail", text="Detail")
        self.tree.column("#0", width=330, stretch=True)
        self.tree.column("status", width=90, anchor="w", stretch=False)
        self.tree.column("detail", width=380, stretch=True)
        for name, color in STATUS_COLORS.items():
            self.tree.tag_configure(name, foreground=color)
        sb = ttk.Scrollbar(tree_frame, orient="vertical", command=self.tree.yview)
        self.tree.configure(yscrollcommand=sb.set)
        self.tree.pack(side="left", fill="both", expand=True)
        sb.pack(side="right", fill="y")

        btns = ttk.Frame(f)
        btns.pack(fill="x", padx=6, pady=6)
        ttk.Button(btns, text="Add files…", command=self._add_files).pack(side="left")
        ttk.Button(btns, text="Add folder…", command=self._add_folder).pack(side="left", padx=4)
        ttk.Button(btns, text="Remove", command=self._remove_selected).pack(side="left")
        ttk.Button(btns, text="Clear", command=self._clear).pack(side="left", padx=4)

        opts = ttk.LabelFrame(f, text="Pipeline")
        opts.pack(fill="x", padx=6, pady=(0, 6))
        row = ttk.Frame(opts)
        row.pack(fill="x", padx=6, pady=6)
        for key, label in (
            ("do_convert", "Convert to EPUB"),
            ("do_metadata", "Fix metadata"),
            ("do_cover", "Fetch cover"),
            ("do_fix", "Kindle EPUB fix"),
            ("do_send", "Email to Kindle"),
        ):
            ttk.Checkbutton(row, text=label, variable=self.v[key]).pack(side="left", padx=(0, 14))

        row2 = ttk.Frame(opts)
        row2.pack(fill="x", padx=6, pady=(0, 6))
        ttk.Checkbutton(
            row2, text="Also keep a fixed copy in:", variable=self.v["keep_copy"]
        ).pack(side="left")
        ttk.Entry(row2, textvariable=self.v["output_dir"]).pack(
            side="left", fill="x", expand=True, padx=6
        )
        ttk.Button(row2, text="Browse…", command=self._pick_output).pack(side="left")

        go = ttk.Frame(f)
        go.pack(fill="x", padx=6, pady=(0, 8))
        self.run_btn = ttk.Button(go, text="Fix & Send to Kindle", command=self._start)
        self.run_btn.pack(side="left")
        self.cancel_btn = ttk.Button(go, text="Cancel", command=self._cancel, state="disabled")
        self.cancel_btn.pack(side="left", padx=6)

    def _build_settings_tab(self):
        outer = self.settings_tab
        f = ttk.Frame(outer)
        f.pack(fill="both", expand=True)

        self.banner = tk.Label(
            f, text="", anchor="w", padx=10, pady=6, font=("sans", 10, "bold")
        )
        self.banner.pack(fill="x", padx=8, pady=(8, 0))

        # -- step 1 ---------------------------------------------------------
        step1 = ttk.LabelFrame(f, text="Step 1 — Where books go")
        step1.pack(fill="x", padx=8, pady=8)
        g1 = ttk.Frame(step1)
        g1.pack(fill="x", padx=8, pady=8)
        g1.columnconfigure(1, weight=1)
        self._field(g1, 0, "Send to (your Kindle address)", "kindle_addr")
        self._field(g1, 1, "From (your email address)", "from_addr")
        ttk.Label(
            step1,
            text="Amazon only accepts documents from addresses you have approved.\n"
                 "Add the From address under Preferences → Personal Document Settings.",
            foreground="#666", justify="left",
        ).pack(anchor="w", padx=8)
        ttk.Button(
            step1, text="Open Amazon approved-senders page",
            command=lambda: self._open_url(config.AMAZON_SENDERS_URL),
        ).pack(anchor="w", padx=8, pady=(6, 8))

        # -- step 2 ---------------------------------------------------------
        step2 = ttk.LabelFrame(f, text="Step 2 — The account that sends the mail")
        step2.pack(fill="x", padx=8, pady=(0, 8))
        g2 = ttk.Frame(step2)
        g2.pack(fill="x", padx=8, pady=8)
        g2.columnconfigure(1, weight=1)
        self._field(g2, 0, "SMTP server", "smtp_host")
        self._field(g2, 1, "Port", "smtp_port", width=8)
        self._field(g2, 2, "Username", "smtp_user")

        ttk.Label(g2, text="App password").grid(row=3, column=0, sticky="w", pady=3, padx=(0, 8))
        pw_row = ttk.Frame(g2)
        pw_row.grid(row=3, column=1, sticky="ew", pady=3)
        self.pw_entry = ttk.Entry(pw_row, textvariable=self.v["smtp_password"], show="•")
        self.pw_entry.pack(side="left", fill="x", expand=True)
        self.show_pw = tk.BooleanVar(value=False)
        ttk.Checkbutton(pw_row, text="show", variable=self.show_pw,
                        command=self._toggle_pw).pack(side="left", padx=6)

        ttk.Label(
            step2,
            text="Gmail will reject your normal password here. Click below, sign in, and\n"
                 "create an App Password — Google shows a 16-character code like\n"
                 "\"abcd efgh ijkl mnop\". Paste it in; the spaces are stripped for you.",
            foreground="#666", justify="left",
        ).pack(anchor="w", padx=8)
        ttk.Button(
            step2, text="Get a Gmail App Password",
            command=lambda: self._open_url(config.APP_PASSWORD_URL),
        ).pack(anchor="w", padx=8, pady=(6, 4))
        self.storage_label = ttk.Label(step2, text=config.storage_note(), foreground="#666")
        self.storage_label.pack(anchor="w", padx=8, pady=(0, 8))

        # -- step 3 ---------------------------------------------------------
        step3 = ttk.LabelFrame(f, text="Step 3 — Check it works")
        step3.pack(fill="x", padx=8, pady=(0, 8))
        btns = ttk.Frame(step3)
        btns.pack(anchor="w", padx=8, pady=8)
        ttk.Button(btns, text="Save", command=self._save).pack(side="left")
        ttk.Button(btns, text="Test connection", command=self._test_smtp).pack(side="left", padx=6)
        ttk.Button(btns, text="Send a test book…", command=self._send_test_book).pack(side="left")
        self.test_result = ttk.Label(step3, text="", foreground="#666")
        self.test_result.pack(anchor="w", padx=8, pady=(0, 8))

        watch = ttk.LabelFrame(f, text="Watch folder")
        watch.pack(fill="x", padx=8, pady=(0, 8))
        row = ttk.Frame(watch)
        row.pack(fill="x", padx=8, pady=8)
        ttk.Checkbutton(
            row, text="Auto-process new files in:", variable=self.v["watch_enabled"],
            command=self._toggle_watch,
        ).pack(side="left")
        ttk.Entry(row, textvariable=self.v["watch_folder"]).pack(
            side="left", fill="x", expand=True, padx=6
        )
        ttk.Button(row, text="Browse…", command=self._pick_watch).pack(side="left")
        ttk.Label(
            watch,
            text="Processed files move into sent/ or failed/ inside that folder.",
            foreground="#666",
        ).pack(anchor="w", padx=8, pady=(0, 8))

        misc = ttk.LabelFrame(f, text="Other")
        misc.pack(fill="x", padx=8, pady=(0, 8))
        row = ttk.Frame(misc)
        row.pack(fill="x", padx=8, pady=8)
        ttk.Label(row, text="Default language when missing:").pack(side="left")
        ttk.Entry(row, textvariable=self.v["default_lang"], width=8).pack(side="left", padx=6)

        ttk.Label(
            f, text=f"Settings file: {config.CONFIG_PATH}", foreground="#888",
        ).pack(anchor="w", padx=8, pady=(0, 8))

        self._refresh_banner()

    def _field(self, parent, row, label, key, width=None):
        ttk.Label(parent, text=label).grid(row=row, column=0, sticky="w", pady=3, padx=(0, 8))
        entry = ttk.Entry(parent, textvariable=self.v[key], **({"width": width} if width else {}))
        entry.grid(row=row, column=1, sticky="w" if width else "ew", pady=3)
        return entry

    def _open_url(self, url):
        import webbrowser

        webbrowser.open(url)
        self.log(f"Opened {url} in your browser.")

    def _refresh_banner(self):
        """Green when email is ready to go, amber when something is missing."""
        cfg = self._collect()
        if config.is_configured(cfg):
            self.banner.configure(
                text=f"✓ Ready — books will be emailed to {cfg['kindle_addr']}",
                bg="#e6f4ea", fg="#1a7f37",
            )
        else:
            missing = []
            if not cfg.get("kindle_addr"):
                missing.append("Kindle address")
            if not (cfg.get("from_addr") or cfg.get("smtp_user")):
                missing.append("From address")
            if not cfg.get("smtp_host"):
                missing.append("SMTP server")
            if not cfg.get("smtp_password"):
                missing.append("app password")
            self.banner.configure(
                text="⚠ Not ready to send yet — still need: " + ", ".join(missing),
                bg="#fff4e5", fg="#9a6700",
            )
        self.storage_label.configure(text=config.storage_note())

    def _build_log_tab(self):
        f = self.log_tab
        self.log_text = ScrolledText(f, wrap="word", height=20, font=("monospace", 9))
        self.log_text.pack(fill="both", expand=True, padx=8, pady=8)
        self.log_text.configure(state="disabled")
        ttk.Button(f, text="Clear log", command=self._clear_log).pack(anchor="w", padx=8, pady=(0, 8))

    def _apply_dnd(self):
        if not HAVE_DND:
            return
        self.tree.drop_target_register(DND_FILES)
        self.tree.dnd_bind("<<Drop>>", self._on_drop)

    # --------------------------------------------------------------- helpers
    def _toggle_pw(self):
        self.pw_entry.configure(show="" if self.show_pw.get() else "•")

    def log(self, message):
        self.events.put(("log", message))

    def _write_log(self, message):
        self.log_text.configure(state="normal")
        self.log_text.insert("end", message.rstrip() + "\n")
        self.log_text.see("end")
        self.log_text.configure(state="disabled")

    def _clear_log(self):
        self.log_text.configure(state="normal")
        self.log_text.delete("1.0", "end")
        self.log_text.configure(state="disabled")

    def _check_tools(self):
        missing = [n for n, ok in ebook_tools.tools_available().items() if not ok]
        if missing:
            self.log(f"WARNING: {', '.join(missing)} not found — install calibre for "
                     "conversion, metadata and covers. The EPUB fix and email still work.")

    # ----------------------------------------------------------------- queue
    def _add_paths(self, paths):
        added = 0
        for path in paths:
            path = os.path.abspath(os.path.expanduser(path))
            if not os.path.isfile(path):
                continue
            if path in self.rows.values():
                continue
            iid = self.tree.insert(
                "", "end", text=os.path.basename(path),
                values=("pending", ""), tags=("pending",),
            )
            self.rows[iid] = path
            added += 1
        if added:
            self.status_text.set(f"{len(self.rows)} book(s) queued.")
        return added

    def _on_drop(self, event):
        paths = self.root.tk.splitlist(event.data)
        n = self._add_paths(paths)
        self.log(f"Added {n} file(s) by drag and drop.")

    def _add_files(self):
        paths = filedialog.askopenfilenames(title="Add books", filetypes=BOOK_TYPES)
        self._add_paths(paths)

    def _add_folder(self):
        folder = filedialog.askdirectory(title="Add every book in a folder")
        if not folder:
            return
        found = []
        for root_dir, _dirs, names in os.walk(folder):
            for name in names:
                if os.path.splitext(name)[1].lower() in watcher.WATCHED_EXT:
                    found.append(os.path.join(root_dir, name))
        n = self._add_paths(sorted(found))
        self.log(f"Added {n} file(s) from {folder}.")

    def _remove_selected(self):
        for iid in self.tree.selection():
            self.rows.pop(iid, None)
            self.tree.delete(iid)

    def _clear(self):
        self.tree.delete(*self.tree.get_children())
        self.rows.clear()
        self.status_text.set("Ready.")

    def _pick_output(self):
        folder = filedialog.askdirectory(title="Where to keep fixed copies")
        if folder:
            self.v["output_dir"].set(folder)

    def _pick_watch(self):
        folder = filedialog.askdirectory(title="Folder to watch")
        if folder:
            self.v["watch_folder"].set(folder)

    # --------------------------------------------------------------- actions
    def _save(self):
        cfg = self._collect()
        # Normalise the pasted app password in the field too, so what you see
        # is what got saved.
        self.v["smtp_password"].set(config.clean_password(cfg["smtp_password"]))
        in_wallet = config.save(self._collect())
        self.log(
            "Settings saved — password in "
            + ("the system wallet." if in_wallet else f"{config.CONFIG_PATH}.")
        )
        self.status_text.set("Settings saved.")
        self._refresh_banner()

    def _test_smtp(self):
        self._save()
        cfg = dict(self.cfg)
        self.test_result.configure(text="Connecting…", foreground="#666")

        def run():
            try:
                sender.test_connection(cfg)
                self.events.put(("test", (True, f"Logged in to {cfg['smtp_host']} as {cfg['smtp_user']}.")))
                self.log("SMTP test: OK")
            except Exception as exc:
                self.events.put(("test", (False, str(exc))))
                self.log(f"SMTP test failed: {exc}")

        threading.Thread(target=run, daemon=True).start()
        self.status_text.set("Testing SMTP…")

    def _send_test_book(self):
        path = filedialog.askopenfilename(title="Pick one book to send", filetypes=BOOK_TYPES)
        if not path:
            return
        self._clear()
        self._add_paths([path])
        self.nb.select(self.queue_tab)
        self._start()

    def _start(self):
        if self.worker and self.worker.is_alive():
            return
        pending = [(iid, path) for iid, path in self.rows.items()
                   if self.tree.set(iid, "status") in ("pending", "error", "skipped")]
        if not pending:
            messagebox.showinfo(APP_TITLE, "Nothing queued. Add some books first.")
            return
        cfg = self._collect()
        if cfg["do_send"] and not config.is_configured(cfg):
            messagebox.showwarning(
                APP_TITLE,
                "Email isn't set up yet — the Settings tab walks through it.\n\n"
                "Or untick \"Email to Kindle\" to just fix the books.",
            )
            self.nb.select(self.settings_tab)
            self._refresh_banner()
            return
        config.save(cfg)

        self.cancel_flag.clear()
        self.run_btn.configure(state="disabled")
        self.cancel_btn.configure(state="normal")
        self.progress.configure(maximum=len(pending), value=0)
        self.worker = threading.Thread(target=self._run_batch, args=(pending, cfg), daemon=True)
        self.worker.start()

    def _cancel(self):
        self.cancel_flag.set()
        self.status_text.set("Cancelling after the current book…")
        self.log("Cancel requested.")

    def _run_batch(self, pending, cfg):
        done = 0
        for iid, path in pending:
            if self.cancel_flag.is_set():
                self.events.put(("row", (iid, "skipped", "cancelled")))
                continue
            self.events.put(("row", (iid, "working", "…")))
            self.events.put(("status", f"Processing {os.path.basename(path)}"))
            self.log(f"→ {path}")
            result = pipeline.process_file(
                path, cfg, log=self.log, cancelled=self.cancel_flag.is_set
            )
            for change in result.changes:
                self.log(f"  {change}")
            if result.status == "error":
                self.log(f"  FAILED: {result.error}")
            self.events.put(("row", (iid, result.status, result.summary)))
            done += 1
            self.events.put(("progress", done))
        self.events.put(("finished", done))

    # ------------------------------------------------------------ watch mode
    def _toggle_watch(self, initial=False):
        cfg = self._collect()
        if cfg["watch_enabled"]:
            folder = cfg["watch_folder"]
            if not folder:
                messagebox.showwarning(APP_TITLE, "Pick a folder to watch first.")
                self.v["watch_enabled"].set(False)
                return
            self.watcher = watcher.FolderWatcher(folder, self._watch_hit, log=self.log)
            self.watcher.start()
        elif self.watcher:
            self.watcher.stop()
            self.watcher = None
        if not initial:
            config.save(self._collect())

    def _watch_hit(self, path):
        """Called on the watch thread — process, then file the book away."""
        cfg = dict(self.cfg)
        result = pipeline.process_file(path, cfg, log=self.log)
        for change in result.changes:
            self.log(f"  {change}")
        ok = result.status == "ok"
        if not ok:
            self.log(f"  FAILED: {result.error}")
        dest = watcher.archive(path, cfg["watch_folder"], ok)
        self.log(f"  moved to {dest}")
        self.events.put(("watched", (dest, result.status, result.summary)))

    # ----------------------------------------------------------------- pumps
    def _drain_events(self):
        try:
            while True:
                kind, payload = self.events.get_nowait()
                if kind == "log":
                    self._write_log(payload)
                elif kind == "row":
                    iid, status, detail = payload
                    if self.tree.exists(iid):
                        self.tree.item(iid, values=(status, detail), tags=(status,))
                elif kind == "progress":
                    self.progress.configure(value=payload)
                elif kind == "status":
                    self.status_text.set(payload)
                elif kind == "info":
                    messagebox.showinfo(*payload)
                    self.status_text.set(payload[0])
                elif kind == "error":
                    messagebox.showerror(*payload)
                    self.status_text.set(payload[0])
                elif kind == "test":
                    ok, message = payload
                    self.test_result.configure(
                        text=("✓ " if ok else "✗ ") + message.replace("\n\n", " "),
                        foreground="#1a7f37" if ok else "#b42318",
                        wraplength=780, justify="left",
                    )
                    self.status_text.set("Connection OK." if ok else "Connection failed.")
                    if ok:
                        messagebox.showinfo(APP_TITLE, "Connection OK — " + message)
                    else:
                        messagebox.showerror("Connection failed", message)
                elif kind == "watched":
                    path, status, detail = payload
                    iid = self.tree.insert(
                        "", "end", text=os.path.basename(path),
                        values=(status, detail), tags=(status,),
                    )
                    self.rows[iid] = path
                elif kind == "finished":
                    self.run_btn.configure(state="normal")
                    self.cancel_btn.configure(state="disabled")
                    self.status_text.set(f"Finished {payload} book(s).")
        except queue.Empty:
            pass
        self.root.after(100, self._drain_events)

    def _on_close(self):
        if self.worker and self.worker.is_alive():
            if not messagebox.askokcancel(APP_TITLE, "Still processing. Quit anyway?"):
                return
            self.cancel_flag.set()
        if self.watcher:
            self.watcher.stop()
        try:
            config.save(self._collect())
        except OSError:
            pass
        self.root.destroy()


def main():
    root = TkinterDnD.Tk() if HAVE_DND else tk.Tk()
    try:
        ttk.Style().theme_use("clam")
    except tk.TclError:
        pass
    App(root)
    root.mainloop()


if __name__ == "__main__":
    main()
