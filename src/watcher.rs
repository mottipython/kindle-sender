use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub const WATCHED_EXT: &[&str] = &[
    ".epub", ".mobi", ".azw", ".azw3", ".prc", ".lit", ".pdf",
    ".fb2", ".rtf", ".txt", ".docx", ".cbz",
];

pub const SUBDIRS: &[&str] = &["sent", "failed"];

pub struct FolderWatcher {
    folder: PathBuf,
    interval: Duration,
    stop_flag: Arc<AtomicBool>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl FolderWatcher {
    pub fn new<P: AsRef<Path>>(folder: P, interval_secs: f64) -> Self {
        Self {
            folder: folder.as_ref().to_path_buf(),
            interval: Duration::from_secs_f64(interval_secs),
            stop_flag: Arc::new(AtomicBool::new(false)),
            thread_handle: None,
        }
    }

    pub fn start<F, L>(&mut self, on_file: F, log: L)
    where
        F: Fn(&Path) + Send + 'static,
        L: Fn(&str) + Send + 'static,
    {
        if self.thread_handle.is_some() {
            return;
        }

        let folder = self.folder.clone();
        let _ = fs::create_dir_all(&folder);
        for sub in SUBDIRS {
            let _ = fs::create_dir_all(folder.join(sub));
        }

        self.stop_flag.store(false, Ordering::SeqCst);
        let stop_clone = Arc::clone(&self.stop_flag);
        let interval = self.interval;

        log(&format!("Watching {}", folder.display()));

        let handle = thread::spawn(move || {
            let mut sizes: HashMap<PathBuf, u64> = HashMap::new();

            while !stop_clone.load(Ordering::SeqCst) {
                let candidates = match fs::read_dir(&folder) {
                    Ok(entries) => {
                        let mut list = Vec::new();
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if !path.is_file() {
                                continue;
                            }
                            let ext = path
                                .extension()
                                .and_then(|e| e.to_str())
                                .map(|e| format!(".{}", e.to_lowercase()))
                                .unwrap_or_default();
                            if WATCHED_EXT.contains(&ext.as_str()) {
                                list.push(path);
                            }
                        }
                        list
                    }
                    Err(e) => {
                        log(&format!("Watch folder unreadable: {}", e));
                        Vec::new()
                    }
                };

                for path in candidates {
                    let size = match fs::metadata(&path) {
                        Ok(m) => m.len(),
                        Err(_) => continue,
                    };

                    let prev = sizes.get(&path).copied();
                    sizes.insert(path.clone(), size);

                    if prev.is_none() || prev != Some(size) {
                        // File size still changing or first seen
                        continue;
                    }

                    // Size is stable across two intervals!
                    sizes.remove(&path);
                    let filename = path.file_name().unwrap_or_default().to_string_lossy();
                    log(&format!("Watch: picked up {}", filename));

                    // Catch panics to keep watch thread alive
                    let on_file_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        on_file(&path);
                    }));
                    if let Err(err) = on_file_res {
                        log(&format!("Watch error processing {}: {:?}", filename, err));
                    }
                }

                // Sleep with short checks for stop flag
                let steps = (interval.as_millis() / 250).max(1);
                for _ in 0..steps {
                    if stop_clone.load(Ordering::SeqCst) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(250));
                }
            }
        });

        self.thread_handle = Some(handle);
    }

    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }

    pub fn is_running(&self) -> bool {
        self.thread_handle.is_some() && !self.stop_flag.load(Ordering::SeqCst)
    }
}

impl Drop for FolderWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn archive<P: AsRef<Path>, Q: AsRef<Path>>(
    path: P,
    watch_folder: Q,
    ok: bool,
) -> Result<PathBuf, std::io::Error> {
    let sub = if ok { "sent" } else { "failed" };
    let dest_dir = watch_folder.as_ref().join(sub);
    fs::create_dir_all(&dest_dir)?;

    let src = path.as_ref();
    let filename = src.file_name().unwrap_or_default();
    let stem = src.file_stem().unwrap_or_default().to_string_lossy();
    let ext = src
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    let mut dest = dest_dir.join(filename);
    let mut n = 1;
    while dest.exists() {
        dest = dest_dir.join(format!("{} ({}){}", stem, n, ext));
        n += 1;
    }

    if let Err(_) = fs::rename(src, &dest) {
        fs::copy(src, &dest)?;
        fs::remove_file(src)?;
    }

    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_archive_moves_and_deduplicates() {
        let tmp = tempdir().unwrap();
        let watch_dir = tmp.path().join("watch");
        fs::create_dir_all(&watch_dir).unwrap();

        let file1 = watch_dir.join("book.epub");
        fs::write(&file1, b"test content").unwrap();

        let archived1 = archive(&file1, &watch_dir, true).unwrap();
        assert!(archived1.exists());
        assert!(!file1.exists());
        assert_eq!(archived1.parent().unwrap(), watch_dir.join("sent"));
        assert_eq!(archived1.file_name().unwrap(), "book.epub");

        // Now create a second file with the same name
        let file2 = watch_dir.join("book.epub");
        fs::write(&file2, b"test content 2").unwrap();
        let archived2 = archive(&file2, &watch_dir, true).unwrap();
        assert!(archived2.exists());
        assert_eq!(archived2.file_name().unwrap(), "book (1).epub");

        // Now test failed archive
        let file3 = watch_dir.join("bad.epub");
        fs::write(&file3, b"bad").unwrap();
        let archived3 = archive(&file3, &watch_dir, false).unwrap();
        assert_eq!(archived3.parent().unwrap(), watch_dir.join("failed"));
    }
}
