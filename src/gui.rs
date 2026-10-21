use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use slint::{ComponentHandle, Model, ModelRc, VecModel};
use walkdir::WalkDir;

use crate::config::{self, Config};
use crate::pipeline::{self, Status};
use crate::sender;
use crate::watcher::{self, FolderWatcher, WATCHED_EXT};

slint::include_modules!();

enum GuiEvent {
    Working {
        index: usize,
        filename: String,
        progress: f32,
    },
    ItemDone {
        index: usize,
        status: String,
        details: String,
    },
    Log(String),
    Finished {
        done: usize,
    },
}

pub fn run_gui() -> anyhow::Result<()> {
    slint::BackendSelector::new().select()?;
    let _ = slint::set_xdg_app_id("kindle-sender");
    let app = AppWindow::new()?;
    let _ = slint::set_xdg_app_id("kindle-sender");
    let cfg = Rc::new(RefCell::new(config::load()));

    // Populate initial settings
    {
        let c = cfg.borrow();
        app.set_kindle_addr(c.kindle_addr.clone().into());
        app.set_from_addr(c.from_addr.clone().into());
        app.set_smtp_host(c.smtp_host.clone().into());
        app.set_smtp_port(c.smtp_port.to_string().into());
        app.set_smtp_user(c.smtp_user.clone().into());
        app.set_smtp_password(c.smtp_password.clone().into());
        app.set_storage_note(config::storage_note().into());

        app.set_do_convert(c.do_convert);
        app.set_do_metadata(c.do_metadata);
        app.set_do_cover(c.do_cover);
        app.set_do_fix(c.do_fix);
        app.set_do_send(c.do_send);
        app.set_keep_copy(c.keep_copy);
        app.set_output_dir(c.output_dir.clone().into());

        app.set_watch_folder(c.watch_folder.clone().into());
        app.set_watch_enabled(c.watch_enabled);

        if !config::is_configured(&c) {
            app.set_status_text("Email isn't set up yet — fill in the Settings tab, then Test connection.".into());
        }
    }

    let books_model: Rc<VecModel<BookItem>> = Rc::new(VecModel::default());
    app.set_books(ModelRc::from(books_model.clone()));

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let watcher_handle: Arc<Mutex<Option<FolderWatcher>>> = Arc::new(Mutex::new(None));

    // Start watch folder if enabled
    if cfg.borrow().watch_enabled {
        start_watcher(&cfg.borrow(), app.as_weak(), watcher_handle.clone());
    }

    // Callback: Add files
    {
        let model = books_model.clone();
        app.on_add_files(move || {
            let files = rfd::FileDialog::new()
                .add_filter(
                    "Ebooks",
                    &[
                        "epub", "mobi", "azw", "azw3", "prc", "lit", "pdf", "fb2", "rtf", "txt", "docx", "cbz",
                    ],
                )
                .add_filter("EPUB", &["epub"])
                .add_filter("All files", &["*"])
                .pick_files();

            if let Some(paths) = files {
                for path in paths {
                    let filename = path
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_else(|| "book".to_string());
                    let path_str = path.to_string_lossy().to_string();
                    let next_id = model.row_count() as i32;

                    model.push(BookItem {
                        id: next_id,
                        path: path_str.into(),
                        filename: filename.into(),
                        status: "pending".into(),
                        details: "queued".into(),
                    });
                }
            }
        });
    }

    // Callback: Add folder (recursive)
    {
        let model = books_model.clone();
        app.on_add_folder(move || {
            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                for entry in WalkDir::new(folder).into_iter().filter_map(|e| e.ok()) {
                    let p = entry.path();
                    if p.is_file() {
                        let ext = p
                            .extension()
                            .and_then(|e| e.to_str())
                            .map(|e| format!(".{}", e.to_lowercase()))
                            .unwrap_or_default();
                        if WATCHED_EXT.contains(&ext.as_str()) {
                            let filename = p
                                .file_name()
                                .map(|f| f.to_string_lossy().to_string())
                                .unwrap_or_else(|| "book".to_string());
                            let path_str = p.to_string_lossy().to_string();
                            let next_id = model.row_count() as i32;

                            model.push(BookItem {
                                id: next_id,
                                path: path_str.into(),
                                filename: filename.into(),
                                status: "pending".into(),
                                details: "queued".into(),
                            });
                        }
                    }
                }
            }
        });
    }

    // Callback: Clear finished
    {
        let model = books_model.clone();
        app.on_clear_finished(move || {
            let mut i = 0;
            while i < model.row_count() {
                if let Some(item) = model.row_data(i) {
                    if item.status == "ok" || item.status == "skipped" {
                        model.remove(i);
                        continue;
                    }
                }
                i += 1;
            }
        });
    }

    // Callback: Clear all
    {
        let model = books_model.clone();
        app.on_clear_all(move || {
            model.set_vec(Vec::new());
        });
    }

    // Callback: Open external URL
    app.on_open_url(|url| {
        let url_str = url.to_string();
        let _ = std::process::Command::new("xdg-open").arg(url_str).spawn();
    });

    // Callback: Browse watch folder
    {
        let weak = app.as_weak();
        app.on_browse_watch_dir(move || {
            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                if let Some(ui) = weak.upgrade() {
                    ui.set_watch_folder(folder.to_string_lossy().to_string().into());
                }
            }
        });
    }

    // Callback: Test Connection
    {
        let weak = app.as_weak();
        app.on_test_connection(move || {
            let ui = match weak.upgrade() {
                Some(u) => u,
                None => return,
            };

            let mut test_cfg = Config::default();
            test_cfg.smtp_host = ui.get_smtp_host().to_string();
            test_cfg.smtp_port = ui.get_smtp_port().to_string().parse().unwrap_or(587);
            test_cfg.smtp_user = ui.get_smtp_user().to_string();
            test_cfg.smtp_password = ui.get_smtp_password().to_string();

            ui.set_has_test_result(true);
            ui.set_test_result_text("Testing connection...".into());
            ui.set_test_result_is_ok(true);

            let weak_thread = weak.clone();
            thread::spawn(move || {
                let res = sender::test_connection(&test_cfg);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak_thread.upgrade() {
                        ui.set_has_test_result(true);
                        match res {
                            Ok(()) => {
                                ui.set_test_result_is_ok(true);
                                ui.set_test_result_text("✓ Connection successful! Credentials accepted.".into());
                            }
                            Err(e) => {
                                ui.set_test_result_is_ok(false);
                                ui.set_test_result_text(format!("✗ Connection failed: {}", e).into());
                            }
                        }
                    }
                });
            });
        });
    }

    // Callback: Save Settings
    {
        let weak = app.as_weak();
        let cfg_ref = cfg.clone();
        let watcher_ref = watcher_handle.clone();

        app.on_save_settings(move || {
            let ui = match weak.upgrade() {
                Some(u) => u,
                None => return,
            };

            let mut c = cfg_ref.borrow_mut();
            c.kindle_addr = ui.get_kindle_addr().to_string();
            c.from_addr = ui.get_from_addr().to_string();
            c.smtp_host = ui.get_smtp_host().to_string();
            c.smtp_port = ui.get_smtp_port().to_string().parse().unwrap_or(587);
            c.smtp_user = ui.get_smtp_user().to_string();
            c.smtp_password = ui.get_smtp_password().to_string();
            c.watch_folder = ui.get_watch_folder().to_string();

            let old_watch_enabled = c.watch_enabled;
            c.watch_enabled = ui.get_watch_enabled();

            let _ = config::save(&mut c);
            ui.set_storage_note(config::storage_note().into());
            ui.set_status_text("Settings saved.".into());

            // Toggle watcher if watch_enabled setting changed
            if c.watch_enabled != old_watch_enabled || c.watch_enabled {
                if c.watch_enabled {
                    start_watcher(&c, weak.clone(), watcher_ref.clone());
                } else if let Ok(mut lock) = watcher_ref.lock() {
                    if let Some(mut w) = lock.take() {
                        w.stop();
                    }
                }
            }
        });
    }

    // Callback: Cancel Processing
    {
        let flag = cancel_flag.clone();
        app.on_cancel_processing(move || {
            flag.store(true, Ordering::SeqCst);
        });
    }

    // Callback: Start Processing
    {
        let weak = app.as_weak();
        let model = books_model.clone();
        let flag = cancel_flag.clone();
        let cfg_ref = cfg.clone();

        app.on_start_processing(move || {
            let ui = match weak.upgrade() {
                Some(u) => u,
                None => return,
            };

            // Collect active options from UI
            let mut run_cfg = cfg_ref.borrow().clone();
            run_cfg.do_convert = ui.get_do_convert();
            run_cfg.do_metadata = ui.get_do_metadata();
            run_cfg.do_cover = ui.get_do_cover();
            run_cfg.do_fix = ui.get_do_fix();
            run_cfg.do_send = ui.get_do_send();
            run_cfg.keep_copy = ui.get_keep_copy();
            run_cfg.output_dir = ui.get_output_dir().to_string();

            if run_cfg.do_send && !config::is_configured(&run_cfg) {
                ui.set_status_text("Cannot send: please configure email in Settings tab.".into());
                return;
            }

            ui.set_is_processing(true);
            flag.store(false, Ordering::SeqCst);

            // Collect pending items to process
            let count = model.row_count();
            let mut pending_items = Vec::new();
            for i in 0..count {
                if let Some(it) = model.row_data(i) {
                    if it.status != "ok" {
                        pending_items.push((i, PathBuf::from(it.path.to_string()), it.filename.to_string()));
                    }
                }
            }

            let (tx, rx): (Sender<GuiEvent>, Receiver<GuiEvent>) = channel();
            let rx = Arc::new(Mutex::new(rx));

            let weak_thread = weak.clone();
            let cancel_thread = flag.clone();

            // Spawn worker thread
            thread::spawn(move || {
                let total = pending_items.len();
                let mut done = 0;

                for (idx, path, filename) in pending_items {
                    if cancel_thread.load(Ordering::SeqCst) {
                        break;
                    }

                    let progress = done as f32 / total.max(1) as f32;
                    let _ = tx.send(GuiEvent::Working {
                        index: idx,
                        filename: filename.clone(),
                        progress,
                    });
                    ping_ui(&weak_thread, &rx);

                    let tx_log = tx.clone();
                    let wt_log = weak_thread.clone();
                    let rx_log = rx.clone();
                    let log_cb = move |msg: &str| {
                        let _ = tx_log.send(GuiEvent::Log(format!("{}\n", msg)));
                        ping_ui(&wt_log, &rx_log);
                    };

                    let cancel_check = {
                        let c = cancel_thread.clone();
                        move || c.load(Ordering::SeqCst)
                    };

                    let result = pipeline::process_file(&path, &run_cfg, log_cb, cancel_check);

                    let status_str = match result.status {
                        Status::Ok => "ok",
                        Status::Skipped => "skipped",
                        Status::Error => "error",
                        Status::Pending => "pending",
                    };

                    let _ = tx.send(GuiEvent::ItemDone {
                        index: idx,
                        status: status_str.to_string(),
                        details: result.summary(),
                    });
                    ping_ui(&weak_thread, &rx);

                    done += 1;
                }

                let _ = tx.send(GuiEvent::Finished { done });
                ping_ui(&weak_thread, &rx);
            });
        });
    }

    app.run()?;
    Ok(())
}

fn ping_ui(app_weak: &slint::Weak<AppWindow>, rx_arc: &Arc<Mutex<Receiver<GuiEvent>>>) {
    let weak = app_weak.clone();
    let rx_clone = rx_arc.clone();

    let _ = slint::invoke_from_event_loop(move || {
        let ui = match weak.upgrade() {
            Some(u) => u,
            None => return,
        };

        if let Ok(rx) = rx_clone.lock() {
            while let Ok(event) = rx.try_recv() {
                match event {
                    GuiEvent::Working {
                        index,
                        filename,
                        progress,
                    } => {
                        ui.set_status_text(format!("Processing {}...", filename).into());
                        ui.set_progress(progress);

                        let books = ui.get_books();
                        if let Some(mut item) = books.row_data(index) {
                            item.status = "working".into();
                            item.details = "processing...".into();
                            books.set_row_data(index, item);
                        }
                    }
                    GuiEvent::ItemDone {
                        index,
                        status,
                        details,
                    } => {
                        let books = ui.get_books();
                        if let Some(mut item) = books.row_data(index) {
                            item.status = status.into();
                            item.details = details.into();
                            books.set_row_data(index, item);
                        }
                    }
                    GuiEvent::Log(msg) => {
                        let mut current = ui.get_log_text().to_string();
                        current.push_str(&msg);
                        ui.set_log_text(current.into());
                    }
                    GuiEvent::Finished { done } => {
                        ui.set_is_processing(false);
                        ui.set_progress(1.0);
                        ui.set_status_text(format!("Finished {} book(s).", done).into());
                    }
                }
            }
        }
    });
}

fn start_watcher(
    cfg: &Config,
    app_weak: slint::Weak<AppWindow>,
    handle_mutex: Arc<Mutex<Option<FolderWatcher>>>,
) {
    if let Ok(mut lock) = handle_mutex.lock() {
        if let Some(mut old) = lock.take() {
            old.stop();
        }

        let folder = PathBuf::from(&cfg.watch_folder);
        let mut watcher = FolderWatcher::new(&folder, 5.0);
        let cfg_clone = cfg.clone();
        let watch_folder_str = cfg.watch_folder.clone();

        watcher.start(
            move |path| {
                let weak = app_weak.clone();
                let log_cb = move |msg: &str| {
                    let line = format!("Watch: {}\n", msg);
                    let w = weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = w.upgrade() {
                            let mut current = ui.get_log_text().to_string();
                            current.push_str(&line);
                            ui.set_log_text(current.into());
                        }
                    });
                };

                let result = pipeline::process_file(path, &cfg_clone, log_cb, || false);
                let ok = result.status == Status::Ok;
                let _ = watcher::archive(path, &watch_folder_str, ok);
            },
            |_msg| {},
        );

        *lock = Some(watcher);
    }
}
