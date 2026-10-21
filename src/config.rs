use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

pub const KEYRING_SERVICE: &str = "kindle-sender";
pub const APP_PASSWORD_URL: &str = "https://myaccount.google.com/apppasswords";
pub const AMAZON_DEVICES_URL: &str = "https://www.amazon.com/hz/mycd/digital-console/alldevices";
pub const AMAZON_SENDERS_URL: &str = "https://www.amazon.com/hz/mycd/myx#/home/settings/pdoc";
pub const VALID_KINDLE_DOMAINS: &[&str] = &["@kindle.com", "@free.kindle.com"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_user: String,
    #[serde(default)]
    pub smtp_password: String,
    pub from_addr: String,
    pub kindle_addr: String,
    pub watch_folder: String,
    pub watch_enabled: bool,
    pub output_dir: String,
    pub keep_copy: bool,
    pub do_convert: bool,
    pub do_metadata: bool,
    pub do_cover: bool,
    pub do_fix: bool,
    pub do_send: bool,
    pub default_lang: String,
    pub max_attach_mb: f64,
    pub kindle_addr_verified: bool,

    #[serde(skip_serializing, default)]
    pub calibre_kindle_addr: String,
    #[serde(default)]
    pub password_in_wallet: bool,
}

impl Default for Config {
    fn default() -> Self {
        let watch_folder = dirs::home_dir()
            .map(|h| h.join("Downloads").join("to-kindle").to_string_lossy().to_string())
            .unwrap_or_else(|| "~/Downloads/to-kindle".to_string());

        Self {
            smtp_host: "smtp.gmail.com".to_string(),
            smtp_port: 587,
            smtp_user: String::new(),
            smtp_password: String::new(),
            from_addr: String::new(),
            kindle_addr: String::new(),
            watch_folder,
            watch_enabled: false,
            output_dir: String::new(),
            keep_copy: false,
            do_convert: true,
            do_metadata: true,
            do_cover: true,
            do_fix: true,
            do_send: true,
            default_lang: "en".to_string(),
            max_attach_mb: 45.0,
            kindle_addr_verified: false,
            calibre_kindle_addr: String::new(),
            password_in_wallet: false,
        }
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .map(|c| c.join("kindle-sender"))
        .unwrap_or_else(|| PathBuf::from("~/.config/kindle-sender"))
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn calibre_smtp_path() -> Option<PathBuf> {
    dirs::config_dir().map(|c| c.join("calibre").join("smtp.py.json"))
}

pub fn clean_password(val: &str) -> String {
    val.chars().filter(|c| !c.is_whitespace()).collect()
}

pub fn keyring_account(cfg: &Config) -> String {
    if !cfg.smtp_user.is_empty() {
        cfg.smtp_user.clone()
    } else if !cfg.from_addr.is_empty() {
        cfg.from_addr.clone()
    } else {
        "default".to_string()
    }
}

pub fn keyring_available() -> bool {
    let entry = match keyring::Entry::new(KEYRING_SERVICE, "__probe__") {
        Ok(e) => e,
        Err(_) => return false,
    };
    if entry.set_password("1").is_ok() {
        let _ = entry.delete_credential();
        true
    } else {
        false
    }
}

pub fn storage_note() -> String {
    if keyring_available() {
        "Password is stored in your system wallet (KWallet/Keyring), not on disk.".to_string()
    } else {
        format!(
            "No system wallet available — password is stored in {} (mode 0600, plain text).",
            config_path().display()
        )
    }
}

pub fn kindle_address_warning(addr: &str) -> Option<String> {
    let addr = addr.trim().to_lowercase();
    if addr.is_empty() {
        return None;
    }
    if !VALID_KINDLE_DOMAINS.iter().any(|dom| addr.ends_with(dom)) {
        return Some("That is not a Send-to-Kindle address — it should end in @kindle.com.".to_string());
    }
    let parts: Vec<&str> = addr.split('@').collect();
    if parts.len() != 2 || parts[0].is_empty() {
        return Some("That address looks malformed.".to_string());
    }
    None
}

pub fn is_calibre_guess(cfg: &Config, addr: &str) -> bool {
    let guess = cfg.calibre_kindle_addr.trim().to_lowercase();
    !guess.is_empty() && addr.trim().to_lowercase() == guess && !cfg.kindle_addr_verified
}

#[derive(Deserialize)]
struct CalibreSmtp {
    #[serde(rename = "from_")]
    from_addr: Option<String>,
    accounts: Option<std::collections::HashMap<String, serde_json::Value>>,
}

fn from_calibre() -> (Option<String>, Option<String>) {
    let path = match calibre_smtp_path() {
        Some(p) => p,
        None => return (None, None),
    };
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return (None, None),
    };
    let parsed: CalibreSmtp = match serde_json::from_str(&content) {
        Ok(p) => p,
        Err(_) => return (None, None),
    };

    let mut kindle_addr = None;
    if let Some(accounts) = parsed.accounts {
        for key in accounts.keys() {
            if let Some((_, domain)) = key.split_once('@') {
                let dom = domain.to_lowercase();
                if dom == "kindle.com" || dom == "free.kindle.com" {
                    kindle_addr = Some(key.clone());
                    break;
                }
            }
        }
    }

    (parsed.from_addr, kindle_addr)
}

pub fn load() -> Config {
    let mut cfg = Config::default();
    let (calibre_from, calibre_kindle) = from_calibre();
    if let Some(ref from) = calibre_from {
        cfg.from_addr = from.clone();
        cfg.smtp_user = from.clone();
    }
    if let Some(ref kindle) = calibre_kindle {
        cfg.kindle_addr = kindle.clone();
        cfg.calibre_kindle_addr = kindle.clone();
    }

    let path = config_path();
    if let Ok(mut file) = File::open(&path) {
        let mut buf = String::new();
        if file.read_to_string(&mut buf).is_ok() {
            if let Ok(saved) = serde_json::from_str::<serde_json::Value>(&buf) {
                if let Some(s) = saved.get("smtp_host").and_then(|v| v.as_str()) { cfg.smtp_host = s.to_string(); }
                if let Some(s) = saved.get("smtp_port").and_then(|v| v.as_u64()) { cfg.smtp_port = s as u16; }
                if let Some(s) = saved.get("smtp_user").and_then(|v| v.as_str()) { cfg.smtp_user = s.to_string(); }
                if let Some(s) = saved.get("smtp_password").and_then(|v| v.as_str()) { cfg.smtp_password = s.to_string(); }
                if let Some(s) = saved.get("from_addr").and_then(|v| v.as_str()) { cfg.from_addr = s.to_string(); }
                if let Some(s) = saved.get("kindle_addr").and_then(|v| v.as_str()) { cfg.kindle_addr = s.to_string(); }
                if let Some(s) = saved.get("watch_folder").and_then(|v| v.as_str()) { cfg.watch_folder = s.to_string(); }
                if let Some(s) = saved.get("watch_enabled").and_then(|v| v.as_bool()) { cfg.watch_enabled = s; }
                if let Some(s) = saved.get("output_dir").and_then(|v| v.as_str()) { cfg.output_dir = s.to_string(); }
                if let Some(s) = saved.get("keep_copy").and_then(|v| v.as_bool()) { cfg.keep_copy = s; }
                if let Some(s) = saved.get("do_convert").and_then(|v| v.as_bool()) { cfg.do_convert = s; }
                if let Some(s) = saved.get("do_metadata").and_then(|v| v.as_bool()) { cfg.do_metadata = s; }
                if let Some(s) = saved.get("do_cover").and_then(|v| v.as_bool()) { cfg.do_cover = s; }
                if let Some(s) = saved.get("do_fix").and_then(|v| v.as_bool()) { cfg.do_fix = s; }
                if let Some(s) = saved.get("do_send").and_then(|v| v.as_bool()) { cfg.do_send = s; }
                if let Some(s) = saved.get("default_lang").and_then(|v| v.as_str()) { cfg.default_lang = s.to_string(); }
                if let Some(s) = saved.get("max_attach_mb").and_then(|v| v.as_f64()) { cfg.max_attach_mb = s; }
                if let Some(s) = saved.get("kindle_addr_verified").and_then(|v| v.as_bool()) { cfg.kindle_addr_verified = s; }
            }
        }
    }

    if cfg.smtp_password.is_empty() {
        let account = keyring_account(&cfg);
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, &account) {
            if let Ok(pwd) = entry.get_password() {
                cfg.smtp_password = pwd;
            }
        }
    }

    cfg
}

pub fn save(cfg: &mut Config) -> Result<bool, anyhow::Error> {
    cfg.smtp_password = clean_password(&cfg.smtp_password);
    let account = keyring_account(cfg);

    let mut stored_in_wallet = false;
    if !cfg.smtp_password.is_empty() {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, &account) {
            if entry.set_password(&cfg.smtp_password).is_ok() {
                stored_in_wallet = true;
            }
        }
    }

    cfg.password_in_wallet = stored_in_wallet;

    let dir = config_dir();
    fs::create_dir_all(&dir)?;

    #[cfg(unix)]
    {
        let perms = fs::Permissions::from_mode(0o700);
        let _ = fs::set_permissions(&dir, perms);
    }

    let mut on_disk = serde_json::to_value(&cfg)?;
    if let Some(map) = on_disk.as_object_mut() {
        if stored_in_wallet {
            map.remove("smtp_password");
        }
        map.remove("calibre_kindle_addr");
        map.insert("password_in_wallet".to_string(), serde_json::Value::Bool(stored_in_wallet));
    }

    let tmp_path = dir.join("config.json.tmp");
    {
        #[cfg(unix)]
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp_path)?;

        #[cfg(not(unix))]
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;

        let formatted = serde_json::to_string_pretty(&on_disk)?;
        file.write_all(formatted.as_bytes())?;
        file.flush()?;
    }

    #[cfg(unix)]
    {
        let perms = fs::Permissions::from_mode(0o600);
        let _ = fs::set_permissions(&tmp_path, perms);
    }

    let dest = config_path();
    fs::rename(&tmp_path, &dest)?;
    Ok(stored_in_wallet)
}

pub fn is_configured(cfg: &Config) -> bool {
    !cfg.kindle_addr.is_empty()
        && (!cfg.from_addr.is_empty() || !cfg.smtp_user.is_empty())
        && !cfg.smtp_host.is_empty()
        && !cfg.smtp_password.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_password() {
        assert_eq!(clean_password("abcd efgh ijkl mnop"), "abcdefghijklmnop");
        assert_eq!(clean_password("  foo \t bar \n "), "foobar");
    }

    #[test]
    fn test_kindle_address_warning() {
        assert!(kindle_address_warning("").is_none());
        assert!(kindle_address_warning("myname@kindle.com").is_none());
        assert!(kindle_address_warning("myname@free.kindle.com").is_none());
        assert!(kindle_address_warning("notanemail").is_some());
        assert!(kindle_address_warning("user@gmail.com").is_some());
        assert!(kindle_address_warning("@kindle.com").is_some());
    }

    #[test]
    fn test_is_calibre_guess() {
        let mut cfg = Config::default();
        cfg.calibre_kindle_addr = "guess@kindle.com".to_string();
        cfg.kindle_addr_verified = false;

        assert!(is_calibre_guess(&cfg, "guess@kindle.com"));
        assert!(!is_calibre_guess(&cfg, "other@kindle.com"));

        cfg.kindle_addr_verified = true;
        assert!(!is_calibre_guess(&cfg, "guess@kindle.com"));
    }
}
