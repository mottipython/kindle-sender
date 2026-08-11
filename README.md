# Kindle Sender

A small Linux desktop app that takes any ebook, repairs the things that break
on a Kindle, tidies up its metadata and cover, and emails it to your
Send-to-Kindle address.

<img src="packaging/kindle-sender.svg" alt="Kindle Sender icon" width="72">

## What it does to each book

1. **Convert** — anything calibre can read (mobi, azw3, lit, pdf, txt, docx,
   cbz…) becomes an EPUB. Files already in EPUB pass straight through.
2. **Metadata** — if the book sits in a calibre library folder, the
   `metadata.opf` next to it wins. Otherwise title and author are parsed out of
   the filename (`Terry Pratchett - Mort.epub`), but only where the existing
   fields are missing or are calibre's own guess.
3. **Cover** — if the book has no cover, or only a flat placeholder, one is
   fetched from Open Library on a strict author-surname + title-word match.
   Real cover art is never replaced.
4. **Kindle EPUB fix** — a Python port of
   [kindle-epub-fix](https://github.com/innocenat/kindle-epub-fix):
   - adds the `<?xml … encoding="utf-8"?>` declaration Kindle needs — without
     it, every accented character turns to mojibake;
   - repairs a missing or invalid `<dc:language>`;
   - strips link fragments pointing at an id on `<body>`, which Kindle
     dead-ends on (this is why some tables of contents silently do nothing);
   - removes `<img>` tags pointing at files that aren't in the archive.

   The book is repacked with `mimetype` first and uncompressed, and only
   replaces the original once the new archive validates.
5. **Send** — emailed as an attachment to your Send-to-Kindle address.

Every step is a checkbox, so you can fix without sending, or send an
already-good book untouched.

## Installing

### RPM (Fedora, Nobara, RHEL…)

```bash
sudo dnf install rpm-build rpmdevtools python3-devel desktop-file-utils
./packaging/build-rpm.sh
sudo dnf install ~/rpmbuild/RPMS/noarch/kindle-sender-*.noarch.rpm
```

### pipx / pip (any distro)

```bash
pipx install 'kindle-sender[all] @ git+https://github.com/mottipython/kindle-sender'
```

### From the source tree, no install

```bash
./packaging/setup-dev-venv.sh
./run.sh
```

### Dependencies

Only `python3` and `python3-tkinter` are required. Everything else is optional
and the app degrades cleanly without it:

| Optional | Enables | Without it |
| --- | --- | --- |
| **calibre** | conversion, metadata, cover embedding | EPUB repair and email still work; other formats are skipped |
| `keyring` + `SecretStorage` | password in the system wallet | password stored in the config file, mode 0600 |
| `tkinterdnd2` | drag-and-drop onto the book list | use the Add files button |
| `Pillow` | detecting flat placeholder covers | covers are left alone |

## Setup

The app opens on the **Settings** tab until email is configured, and a banner
says exactly what is still missing. Three steps:

**Step 1 — Where books go.** Your Kindle address (`something@kindle.com`) and
the address you send from. If calibre is set up on the machine, both are
pre-filled from its email settings — but treat that as a guess and check it,
because calibre's copy is often stale. The app flags an unchecked
calibre-derived address until you either edit it or mark it verified.

Two ways to get this wrong, and **both fail identically and silently** —
Amazon accepts the mail, drops it, and sends no bounce and no confirmation:

- the Kindle address doesn't match a registered device. *Find your Kindle
  address* opens Amazon's device list, where each device's real address is
  shown.
- the From address isn't on the **Approved Personal Document E-mail List**.
  *Approved-senders list* opens Preferences → Personal Document Settings.

If a book never arrives, check your mail client's Sent folder first: if the
message is there with the book attached, delivery got as far as Amazon and one
of those two settings is the cause.

**Step 2 — The sending account.** Defaults to `smtp.gmail.com` / `587`. Click
**Get a Gmail App Password**: it opens the Google page where you sign in and
create one. Google shows a 16-character code like `abcd efgh ijkl mnop` —
paste it into the App password field. The spaces are stripped for you, and
*show* lets you check the paste. A normal Google account password will not
work here; Gmail rejects it outright.

**Step 3 — Check it works.** **Test connection** logs in and hangs up without
sending anything, reporting the result inline. **Send a test book…** runs one
book all the way through.

### Where the password is kept

If a system wallet is running (KWallet or GNOME Keyring, via the Secret Service
API), the password goes there and never touches disk in readable form. Without
one it falls back to `~/.config/kindle-sender/config.json`, mode `0600`, in
plain text. The Settings tab always states which of the two applies. Nothing
else in that file is sensitive.

## Adding books

- Drag them onto the list, or use **Add files…** / **Add folder…** (recursive).
- Or turn on **watch folder** in Settings: anything dropped into
  `~/Downloads/to-kindle` is processed automatically, then moved into `sent/`
  or `failed/` inside that folder so nothing is handled twice. Watch mode uses
  the last **saved** settings.

Amazon caps attachments at 50 MB; the app refuses anything over 45 MB rather
than letting the mail bounce silently. Change `max_attach_mb` in the config
file to push it.

## Layout

```
kindle_sender/
  epub_fix.py     the four Kindle fixes, stdlib only, atomic repack + validate
  ebook_tools.py  calibre CLI wrappers: convert, metadata, Open Library covers
  sender.py       SMTP delivery
  config.py       settings; password via system wallet, file as fallback
  pipeline.py     convert -> metadata -> cover -> fix -> send, per book
  watcher.py      polling folder watch (safe on half-copied downloads)
  gui.py          tkinter UI; all work runs on a worker thread
packaging/        .spec, .desktop, icon, build scripts
tests/            builds deliberately broken epubs and asserts each fix
```

```bash
python3 tests/test_epub_fix.py
```

No network, no calibre and no display needed — it constructs broken EPUBs in a
temp dir, asserts every fix fires, and checks the result is a valid archive
that a second pass leaves alone.

## Notes

- Calls the native calibre binaries. A Flatpak calibre works but launching the
  sandbox once per file is drastically slower.
- Converting formats that carry no metadata of their own (txt, pdf, rtf, docx,
  html, cbz) makes calibre invent a title and author from the filename — often
  putting the title in the author field. For those formats this app parses the
  filename itself instead of trusting the result.

## License

MIT — see [LICENSE](LICENSE).
