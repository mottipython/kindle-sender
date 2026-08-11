"""Config for kindle-sender.

Everything except the password lives in ~/.config/kindle-sender/config.json.
The password goes to the system wallet (KWallet/GNOME Keyring via the Secret
Service API) when one is available, and only falls back to the 0600 config
file when it isn't.
"""

from __future__ import annotations

import json
import os

CONFIG_DIR = os.path.expanduser("~/.config/kindle-sender")
CONFIG_PATH = os.path.join(CONFIG_DIR, "config.json")
CALIBRE_SMTP = os.path.expanduser("~/.config/calibre/smtp.py.json")

KEYRING_SERVICE = "kindle-sender"

APP_PASSWORD_URL = "https://myaccount.google.com/apppasswords"
AMAZON_SENDERS_URL = "https://www.amazon.com/hz/mycd/digital-console/alldevices"

DEFAULTS = {
    "smtp_host": "smtp.gmail.com",
    "smtp_port": 587,
    "smtp_user": "",
    "smtp_password": "",
    "from_addr": "",
    "kindle_addr": "",
    "watch_folder": os.path.expanduser("~/Downloads/to-kindle"),
    "watch_enabled": False,
    "output_dir": "",
    "keep_copy": False,
    "do_convert": True,
    "do_metadata": True,
    "do_cover": True,
    "do_fix": True,
    "do_send": True,
    "default_lang": "en",
    "max_attach_mb": 45,
}

# Keys never written to the JSON file.
_SECRET_KEYS = ("smtp_password",)


# ---------------------------------------------------------------- the wallet


def _keyring():
    try:
        import keyring
        from keyring.backends.fail import Keyring as FailKeyring
    except ImportError:
        return None
    try:
        backend = keyring.get_keyring()
    except Exception:
        return None
    if isinstance(backend, FailKeyring):
        return None
    return keyring


def keyring_available():
    """True only if the wallet actually accepts a write — not just imports."""
    kr = _keyring()
    if kr is None:
        return False
    try:
        kr.set_password(KEYRING_SERVICE, "__probe__", "1")
        kr.delete_password(KEYRING_SERVICE, "__probe__")
        return True
    except Exception:
        return False


def storage_note():
    if keyring_available():
        return "Password is stored in your system wallet (KWallet/Keyring), not on disk."
    return f"No system wallet available — password is stored in {CONFIG_PATH} (mode 0600, plain text)."


def _keyring_account(cfg):
    """One wallet entry per sending account."""
    return cfg.get("smtp_user") or cfg.get("from_addr") or "default"


def clean_password(value):
    """Google displays app passwords as 'abcd efgh ijkl mnop'; SMTP wants them
    without the spaces, and pasting them with spaces is the usual first
    failure."""
    return "".join((value or "").split())


# ------------------------------------------------------------- load and save


def _from_calibre():
    """Seed sender/recipient from calibre's own email settings if present."""
    seed = {}
    try:
        with open(CALIBRE_SMTP) as fh:
            data = json.load(fh)
    except (OSError, ValueError):
        return seed
    if data.get("from_"):
        seed["from_addr"] = data["from_"]
        seed["smtp_user"] = data["from_"]
    for addr in (data.get("accounts") or {}):
        if addr.endswith("kindle.com"):
            seed["kindle_addr"] = addr
            break
    return seed


def exists():
    return os.path.exists(CONFIG_PATH)


def load():
    cfg = dict(DEFAULTS)
    cfg.update(_from_calibre())
    try:
        with open(CONFIG_PATH) as fh:
            cfg.update(json.load(fh))
    except (OSError, ValueError):
        pass

    if not cfg.get("smtp_password"):
        kr = _keyring()
        if kr is not None:
            try:
                cfg["smtp_password"] = kr.get_password(KEYRING_SERVICE, _keyring_account(cfg)) or ""
            except Exception:
                pass
    return cfg


def save(cfg):
    cfg = dict(cfg)
    cfg["smtp_password"] = clean_password(cfg.get("smtp_password"))
    on_disk = {k: v for k, v in cfg.items() if k not in _SECRET_KEYS}

    stored_in_wallet = False
    kr = _keyring()
    if kr is not None and cfg["smtp_password"]:
        try:
            kr.set_password(KEYRING_SERVICE, _keyring_account(cfg), cfg["smtp_password"])
            stored_in_wallet = True
        except Exception:
            stored_in_wallet = False
    if not stored_in_wallet:
        on_disk["smtp_password"] = cfg["smtp_password"]
    on_disk["password_in_wallet"] = stored_in_wallet

    os.makedirs(CONFIG_DIR, mode=0o700, exist_ok=True)
    tmp = CONFIG_PATH + ".tmp"
    with open(tmp, "w") as fh:
        json.dump(on_disk, fh, indent=2, sort_keys=True)
    os.chmod(tmp, 0o600)
    os.replace(tmp, CONFIG_PATH)
    return stored_in_wallet


def is_configured(cfg):
    """Enough set to actually deliver a book?"""
    return bool(
        cfg.get("kindle_addr")
        and (cfg.get("from_addr") or cfg.get("smtp_user"))
        and cfg.get("smtp_host")
        and cfg.get("smtp_password")
    )
