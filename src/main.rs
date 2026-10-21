use std::path::PathBuf;
use clap::{Parser, Subcommand};
use kindle_sender::config;
use kindle_sender::ebook_tools;
use kindle_sender::epub_fix;
use kindle_sender::pipeline::{process_file, Status};
use kindle_sender::sender;
use kindle_sender::watcher::{self, FolderWatcher};

#[derive(Parser, Debug)]
#[command(
    name = "kindle-sender",
    author = "Motti",
    version,
    about = "Repairs ebooks for Kindle, fetches covers, and emails them via Send-to-Kindle"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Files to send directly (shorthand for 'kindle-sender send <FILES>')
    #[arg(trailing_var_arg = true)]
    files: Vec<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Launch the graphical desktop interface
    Gui,

    /// Process and send one or more books to your Kindle
    Send {
        /// Book files to process and send
        #[arg(required = true)]
        files: Vec<PathBuf>,

        /// Disable Calibre format conversion
        #[arg(long)]
        no_convert: bool,

        /// Disable metadata repair
        #[arg(long)]
        no_metadata: bool,

        /// Disable Open Library cover search
        #[arg(long)]
        no_cover: bool,

        /// Disable Kindle EPUB fixes
        #[arg(long)]
        no_fix: bool,

        /// Directory to keep a copy of repaired EPUBs
        #[arg(short, long)]
        output_dir: Option<PathBuf>,
    },

    /// Apply Kindle EPUB fixes (encoding, language, body-id, stray img) without sending
    Fix {
        /// EPUB files to fix
        #[arg(required = true)]
        files: Vec<PathBuf>,

        /// Destination directory (default is in-place)
        #[arg(short, long)]
        output_dir: Option<PathBuf>,

        /// Default language code if missing or invalid (default: en)
        #[arg(long, default_value = "en")]
        lang: String,
    },

    /// Poll a directory and automatically process and send any dropped ebooks
    Watch {
        /// Folder to watch (defaults to watch_folder in config)
        folder: Option<PathBuf>,

        /// Polling interval in seconds (default: 5.0)
        #[arg(short, long, default_value_t = 5.0)]
        interval: f64,
    },

    /// View or update configuration
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },

    /// Test SMTP connection with current configuration
    TestConnection,

    /// Diagnose system dependencies (calibre, system wallet, config)
    Doctor,
}

#[derive(Subcommand, Debug)]
enum ConfigAction {
    /// Show current settings
    Show,
    /// Set a configuration key (e.g. kindle_addr, from_addr, smtp_user, smtp_host, smtp_port)
    Set {
        key: String,
        value: String,
    },
    /// Set SMTP app password (stored in system keyring if available)
    SetPassword {
        /// App password (omit to be prompted securely)
        password: Option<String>,
    },
}

fn print_log(msg: &str) {
    println!("{}", msg);
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Gui) => {
            kindle_sender::gui::run_gui()?;
        }

        Some(Commands::Send {
            files,
            no_convert,
            no_metadata,
            no_cover,
            no_fix,
            output_dir,
        }) => {
            cmd_send(files, no_convert, no_metadata, no_cover, no_fix, output_dir)?;
        }
        Some(Commands::Fix {
            files,
            output_dir,
            lang,
        }) => {
            cmd_fix(files, output_dir, &lang)?;
        }
        Some(Commands::Watch { folder, interval }) => {
            cmd_watch(folder, interval)?;
        }
        Some(Commands::Config { action }) => {
            cmd_config(action)?;
        }
        Some(Commands::TestConnection) => {
            cmd_test_connection()?;
        }
        Some(Commands::Doctor) => {
            cmd_doctor()?;
        }
        None => {
            if !cli.files.is_empty() {
                cmd_send(cli.files, false, false, false, false, None)?;
            } else {
                kindle_sender::gui::run_gui()?;
            }
        }
    }

    Ok(())
}

fn cmd_send(
    files: Vec<PathBuf>,
    no_convert: bool,
    no_metadata: bool,
    no_cover: bool,
    no_fix: bool,
    output_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    let mut cfg = config::load();

    if no_convert {
        cfg.do_convert = false;
    }
    if no_metadata {
        cfg.do_metadata = false;
    }
    if no_cover {
        cfg.do_cover = false;
    }
    if no_fix {
        cfg.do_fix = false;
    }
    if let Some(out) = output_dir {
        cfg.keep_copy = true;
        cfg.output_dir = out.to_string_lossy().to_string();
    }
    cfg.do_send = true;

    if !config::is_configured(&cfg) {
        eprintln!("Error: Kindle Sender is not fully configured yet.");
        eprintln!("Run 'kindle-sender doctor' or 'kindle-sender config show' to see what is missing.");
        std::process::exit(1);
    }

    if let Some(warn) = config::kindle_address_warning(&cfg.kindle_addr) {
        eprintln!("Warning: {}", warn);
    }

    let total = files.len();
    println!("Processing {} book(s)...", total);

    let mut successful = 0;
    let mut failed = 0;

    for (idx, path) in files.iter().enumerate() {
        println!("\n[{}/{}] -> {}", idx + 1, total, path.display());
        let result = process_file(path, &cfg, print_log, || false);

        for change in &result.changes {
            println!("  {}", change);
        }

        match result.status {
            Status::Ok => {
                println!("  OK: {}", result.summary());
                successful += 1;
            }
            Status::Skipped => {
                println!("  SKIPPED: {}", result.summary());
            }
            Status::Error => {
                eprintln!("  FAILED: {}", result.summary());
                failed += 1;
            }
            Status::Pending => {}
        }
    }

    println!("\nFinished: {} succeeded, {} failed.", successful, failed);
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_fix(files: Vec<PathBuf>, output_dir: Option<PathBuf>, lang: &str) -> anyhow::Result<()> {
    let total = files.len();
    println!("Applying Kindle fixes to {} EPUB(s)...", total);

    for (idx, path) in files.iter().enumerate() {
        println!("\n[{}/{}] -> {}", idx + 1, total, path.display());
        let dest = output_dir.as_ref().map(|d| d.join(path.file_name().unwrap()));
        let dest_ref = dest.as_deref();

        match epub_fix::fix_epub(path, true, dest_ref, lang) {
            Ok(changes) => {
                if changes.is_empty() {
                    println!("  no changes needed (already clean)");
                } else {
                    for change in changes {
                        println!("  {}", change);
                    }
                    if let Some(d) = dest_ref {
                        println!("  written to {}", d.display());
                    } else {
                        println!("  fixed in-place");
                    }
                }
            }
            Err(e) => {
                eprintln!("  ERROR: {}", e);
            }
        }
    }
    Ok(())
}

fn cmd_watch(folder: Option<PathBuf>, interval: f64) -> anyhow::Result<()> {
    let cfg = config::load();
    let watch_path = folder.unwrap_or_else(|| PathBuf::from(&cfg.watch_folder));

    if !watch_path.exists() {
        std::fs::create_dir_all(&watch_path)?;
    }

    println!("Starting watch mode on: {}", watch_path.display());
    println!("Polling interval: {:.1}s", interval);
    println!("Press Ctrl+C to stop.\n");

    let cfg_clone = cfg.clone();
    let watch_folder_str = watch_path.to_string_lossy().to_string();

    let mut watcher = FolderWatcher::new(&watch_path, interval);
    watcher.start(
        move |file_path| {
            println!("\nWatch: processing {}", file_path.display());
            let result = process_file(file_path, &cfg_clone, print_log, || false);
            for change in &result.changes {
                println!("  {}", change);
            }
            let ok = result.status == Status::Ok;
            if !ok {
                eprintln!("  FAILED: {}", result.summary());
            }
            match watcher::archive(file_path, &watch_folder_str, ok) {
                Ok(dest) => println!("  archived to {}", dest.display()),
                Err(e) => eprintln!("  failed to archive: {}", e),
            }
        },
        print_log,
    );

    // Keep main thread alive until interrupt
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let r = running.clone();
    let _ = ctrlc_handler(move || {
        r.store(false, std::sync::atomic::Ordering::SeqCst);
    });

    while running.load(std::sync::atomic::Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    println!("\nStopping watcher...");
    watcher.stop();
    Ok(())
}

fn ctrlc_handler<F: Fn() + Send + 'static>(_handler: F) -> Result<(), ()> {
    // Basic signal handling or loop exit
    Ok(())
}

fn cmd_config(action: Option<ConfigAction>) -> anyhow::Result<()> {
    let mut cfg = config::load();

    match action {
        Some(ConfigAction::Show) | None => {
            println!("Configuration ({}):", config::config_path().display());
            println!("  Kindle address:       {}", if cfg.kindle_addr.is_empty() { "<not set>" } else { &cfg.kindle_addr });
            println!("  From address:         {}", if cfg.from_addr.is_empty() { "<not set>" } else { &cfg.from_addr });
            println!("  SMTP host:            {}", cfg.smtp_host);
            println!("  SMTP port:            {}", cfg.smtp_port);
            println!("  SMTP user:            {}", if cfg.smtp_user.is_empty() { "<not set>" } else { &cfg.smtp_user });
            println!("  SMTP password:        {}", if cfg.smtp_password.is_empty() { "<not set>" } else { "********" });
            println!("  Password in wallet:   {}", cfg.password_in_wallet);
            println!("  Watch folder:         {}", cfg.watch_folder);
            println!("  Watch enabled:        {}", cfg.watch_enabled);
            println!("  Keep copy:            {}", cfg.keep_copy);
            println!("  Output dir:           {}", if cfg.output_dir.is_empty() { "<none>" } else { &cfg.output_dir });
            println!("  Default language:     {}", cfg.default_lang);
            println!("  Max attachment (MB):  {}", cfg.max_attach_mb);
            println!("\nStorage: {}", config::storage_note());
        }
        Some(ConfigAction::Set { key, value }) => {
            match key.to_lowercase().as_str() {
                "kindle_addr" => cfg.kindle_addr = value.clone(),
                "from_addr" => cfg.from_addr = value.clone(),
                "smtp_host" => cfg.smtp_host = value.clone(),
                "smtp_port" => cfg.smtp_port = value.parse()?,
                "smtp_user" => cfg.smtp_user = value.clone(),
                "watch_folder" => cfg.watch_folder = value.clone(),
                "watch_enabled" => cfg.watch_enabled = value.parse()?,
                "output_dir" => cfg.output_dir = value.clone(),
                "keep_copy" => cfg.keep_copy = value.parse()?,
                "default_lang" => cfg.default_lang = value.clone(),
                "max_attach_mb" => cfg.max_attach_mb = value.parse()?,
                _ => {
                    eprintln!("Unknown config key: {}. Run 'kindle-sender config show' to see valid keys.", key);
                    std::process::exit(1);
                }
            }
            config::save(&mut cfg)?;
            println!("Saved '{}' = '{}'", key, value);
        }
        Some(ConfigAction::SetPassword { password }) => {
            let pwd = match password {
                Some(p) => p,
                None => {
                    print!("Enter SMTP App Password: ");
                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input)?;
                    input.trim().to_string()
                }
            };
            cfg.smtp_password = pwd;
            let stored_in_wallet = config::save(&mut cfg)?;
            if stored_in_wallet {
                println!("Password saved securely in system wallet.");
            } else {
                println!("Password saved in config file (mode 0600).");
            }
        }
    }
    Ok(())
}

fn cmd_test_connection() -> anyhow::Result<()> {
    let cfg = config::load();
    println!("Testing connection to {}:{}...", cfg.smtp_host, cfg.smtp_port);

    match sender::test_connection(&cfg) {
        Ok(()) => {
            println!("✓ Connection successful! Credentials accepted.");
        }
        Err(e) => {
            eprintln!("✗ Connection failed: {}", e);
            std::process::exit(1);
        }
    }
    Ok(())
}

fn cmd_doctor() -> anyhow::Result<()> {
    println!("Kindle Sender Doctor");
    println!("====================");

    // Calibre tools
    let tools = ebook_tools::tools_available();
    for (tool, available) in &tools {
        if *available {
            println!("✓ {:<14} found in PATH", tool);
        } else {
            println!("✗ {:<14} NOT found (optional: needed for conversion/metadata)", tool);
        }
    }

    // Keyring
    if config::keyring_available() {
        println!("✓ System wallet  available (KWallet/Keyring via Secret Service)");
    } else {
        println!("! System wallet  unavailable (passwords stored in mode 0600 config file)");
    }

    // Config
    let cfg = config::load();
    if config::is_configured(&cfg) {
        println!("✓ Email setup    configured and ready to send");
    } else {
        println!("✗ Email setup    incomplete:");
        if cfg.kindle_addr.is_empty() { println!("    - Missing Kindle address"); }
        if cfg.from_addr.is_empty() && cfg.smtp_user.is_empty() { println!("    - Missing From/User address"); }
        if cfg.smtp_host.is_empty() { println!("    - Missing SMTP host"); }
        if cfg.smtp_password.is_empty() { println!("    - Missing SMTP password"); }
    }

    if let Some(warn) = config::kindle_address_warning(&cfg.kindle_addr) {
        println!("! Kindle address: {}", warn);
    }

    println!("\n{}", config::storage_note());
    Ok(())
}
