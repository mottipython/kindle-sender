"""Send books to a Kindle by email (Amazon's Send-to-Kindle)."""

from __future__ import annotations

import mimetypes
import os
import smtplib
import ssl
from email.message import EmailMessage

from . import config


class SendError(Exception):
    pass


def _connect(cfg, timeout=60):
    host = cfg.get("smtp_host") or ""
    port = int(cfg.get("smtp_port") or 587)
    if not host:
        raise SendError("no SMTP server configured (Settings tab)")
    context = ssl.create_default_context()
    if port == 465:
        server = smtplib.SMTP_SSL(host, port, timeout=timeout, context=context)
    else:
        server = smtplib.SMTP(host, port, timeout=timeout)
        server.ehlo()
        server.starttls(context=context)
        server.ehlo()
    user = cfg.get("smtp_user")
    password = config.clean_password(cfg.get("smtp_password"))
    if user:
        if not password:
            server.close()
            raise SendError("No password set — see Settings.")
        try:
            server.login(user, password)
        except smtplib.SMTPAuthenticationError as exc:
            server.close()
            raise SendError(
                "The mail server rejected that username/password.\n\n"
                "Gmail will not accept your normal account password here — you need "
                "a 16-character App Password from myaccount.google.com/apppasswords "
                "(the 'Get an App Password' button in Settings opens it)."
            ) from exc
    return server


def test_connection(cfg):
    """Log in and hang up. Raises SendError with a readable message on failure."""
    try:
        server = _connect(cfg)
    except SendError:
        raise
    except Exception as exc:
        raise SendError(f"{type(exc).__name__}: {exc}") from exc
    try:
        server.noop()
    finally:
        server.quit()
    return True


def send_book(cfg, path, subject=None):
    """Email one book as an attachment to the configured Kindle address."""
    to_addr = (cfg.get("kindle_addr") or "").strip()
    from_addr = (cfg.get("from_addr") or cfg.get("smtp_user") or "").strip()
    if not to_addr:
        raise SendError("no Kindle address configured (Settings tab)")
    if not from_addr:
        raise SendError("no From address configured (Settings tab)")

    limit = float(cfg.get("max_attach_mb") or 45) * 1024 * 1024
    size = os.path.getsize(path)
    if size > limit:
        raise SendError(
            f"{size / 1e6:.1f} MB exceeds the {limit / 1e6:.0f} MB attachment limit"
        )

    msg = EmailMessage()
    msg["From"] = from_addr
    msg["To"] = to_addr
    msg["Subject"] = subject or os.path.splitext(os.path.basename(path))[0]
    msg.set_content(
        "Sent by kindle-sender.\n\n"
        "Note: the sending address must be on your Amazon "
        "'Approved Personal Document E-mail List'."
    )
    ext = os.path.splitext(path)[1].lower()
    if ext == ".epub":
        maintype, subtype = "application", "epub+zip"
    elif ext == ".pdf":
        maintype, subtype = "application", "pdf"
    else:
        ctype, _ = mimetypes.guess_type(path)
        if ctype and "/" in ctype:
            maintype, subtype = ctype.split("/", 1)
        else:
            maintype, subtype = "application", "octet-stream"

    with open(path, "rb") as fh:
        msg.add_attachment(
            fh.read(),
            maintype=maintype,
            subtype=subtype,
            filename=os.path.basename(path),
        )

    server = _connect(cfg)
    try:
        server.send_message(msg)
    except smtplib.SMTPException as exc:
        raise SendError(f"{type(exc).__name__}: {exc}") from exc
    finally:
        server.quit()
    return size
