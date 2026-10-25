# Kindle Sender

A small Linux desktop app that takes any ebook, repairs the things that break
on a Kindle, tidies up its metadata and cover, and emails it to your
Send-to-Kindle address.

<img src="packaging/kindle-sender.svg" alt="Kindle Sender icon" width="72">

## What it does to each book

1. **Convert** to EPUB. These formats are converted by the app itself, with
   nothing else installed: **MOBI, AZW, AZW3 (KF8), PDF, FB2, TXT and HTML**.
   Files already in EPUB pass straight through. LIT, RTF, DOC/DOCX, CBZ/CBR
   and PDB need [calibre](https://calibre-ebook.com/) if you have it; calibre
   is also tried as a fallback for any built-in conversion that fails.
2. **Metadata** — if the book sits in a calibre library folder, the
   `metadata.opf` next to it wins. Otherwise title and author are parsed out of
   the filename (`Terry Pratchett - Mort.epub`), but only where the existing
   fields are missing or are a placeholder.
3. **Cover** — if the book has no cover, or only a flat placeholder, one is
   fetched from Open Library on a strict author-surname + title-word match.
   Real cover art is never replaced.
4. **Kindle EPUB fix** — a Rust port of
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

### What the built-in converters can and cannot do

| Format | Notes |
| --- | --- |
| MOBI / AZW | Text, images, table of contents and internal links are kept. |
| AZW3 (KF8) | Same, including the stylesheet. |
| FB2 | Text, images, cover, footnote links. |
| TXT | Chapters are found from headings such as "Chapter 3" or "Part II". |
| HTML | Split into one file per h1/h2; local images are embedded. |
| PDF | Reflowable text only. Page numbers and running headers are dropped, hyphenated words rejoined. Images and layout are not kept, and scanned PDFs (no text layer) cannot be converted without OCR. |

**DRM-protected books** (most Kindle store purchases) cannot be converted by
anything here or in calibre; the app says so instead of producing garbage.

## Installing

Download a package from the
[releases page](https://github.com/mottipython/kindle-sender/releases):

```bash
sudo dnf install ./kindle-sender-*.x86_64.rpm      # Fedora, Nobara, RHEL…
sudo apt install ./kindle-sender_*_amd64.deb       # Debian, Ubuntu…
```

or unpack the `.tar.gz` and run `kindle-sender`. To build from source:

```bash
cargo build --release          # needs a Rust toolchain
./target/release/kindle-sender # GUI; `kindle-sender --help` lists the CLI
```

No Python is involved. Nothing else is required; calibre is optional (above),
and a system wallet (KWallet or GNOME Keyring) is used for the password when
one is running.

## Setup

Open the **Settings** tab until email is configured; the status bar says what
is still missing. Three steps:

**Step 1 — Where books go.** Your Kindle address (`something@kindle.com`) and
the address you send from. If calibre is set up on the machine, both are
pre-filled from its email settings — but treat that as a guess and check it,
because calibre's copy is often stale.

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
sending anything, reporting the result inline.

### Where the password is kept

If a system wallet is running (KWallet or GNOME Keyring, via the Secret Service
API), the password goes there and never touches disk in readable form. Without
one it falls back to `~/.config/kindle-sender/config.json`, mode `0600`, in
plain text. The Settings tab always states which of the two applies. Nothing
else in that file is sensitive.

## Adding books

- Use **Add files…** / **Add folder…** (recursive).
- Or turn on **watch folder** in Settings: anything dropped into
  `~/Downloads/to-kindle` is processed automatically, then moved into `sent/`
  or `failed/` inside that folder so nothing is handled twice. Watch mode uses
  the last **saved** settings.

Amazon caps attachments at 50 MB; the app refuses anything over 45 MB rather
than letting the mail bounce silently. Change `max_attach_mb` in the config
file to push it.

## Layout

```
src/
  native/         built-in converters: mobi.rs (MOBI6 + KF8), text.rs (txt, html, fb2),
                  pdf.rs, epub_writer.rs, epub_meta.rs (metadata + cover editing)
  epub_fix.rs     the four Kindle fixes, atomic repack + validate
  ebook_tools.rs  conversion routing (built-in, then calibre), metadata, Open Library covers
  sender.rs       SMTP delivery
  config.rs       settings; password via system wallet, file as fallback
  pipeline.rs     convert -> metadata -> cover -> fix -> send, per book
  watcher.rs      polling folder watch (safe on half-copied downloads)
  gui.rs, ui/     Slint desktop UI; all work runs on a worker thread
examples/         native_check: converts files and verifies the EPUBs
fuzz/             cargo-fuzz targets for every parser
packaging/        .desktop, icons, package build script
tests/            fixtures in every built-in format, run through the whole
                  pipeline with an empty PATH (so calibre cannot be the reason it works)
```

```bash
cargo test --release
cargo run --release --example native_check -- some.mobi other.azw3 book.pdf
```

## Notes

- Converting formats that carry little metadata (txt, pdf, html…) leaves the
  title and author to be guessed; for those the app parses the filename
  instead of trusting the result.
- Fuzzing: `cd fuzz && RUSTC_BOOTSTRAP=1 cargo fuzz run -s none native_convert -- -fork=8`
  (stable Rust; drop `-s none` on nightly for AddressSanitizer).

## License

MIT — see [LICENSE](LICENSE).
