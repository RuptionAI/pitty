//! Local folder connections. Connecting and browsing read metadata only;
//! selected file contents remain the attachment layer's responsibility.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const CONFIG_LIMIT: u64 = 1024 * 1024;
const LIST_LIMIT: usize = 2000;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Folder {
    pub path: PathBuf,
}

#[derive(Clone)]
pub struct Store {
    pub roots: Vec<Folder>,
    config: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Connections {
    roots: Vec<Folder>,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

#[derive(Clone, Debug)]
pub struct Listing {
    pub directory: PathBuf,
    pub entries: Vec<Entry>,
    pub truncated: bool,
    pub skipped_entries: usize,
}

fn config_path() -> Result<PathBuf, String> {
    crate::paths::config_dir().map(|dir| dir.join("folders.json"))
}

fn nonblocking_read(path: &Path) -> io::Result<fs::File> {
    // Avoid blocking on a FIFO in place of the configuration file, including
    // a replacement between the metadata check and opening the descriptor.
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(target_os = "linux")]
        const NONBLOCK: i32 = 0x800;
        #[cfg(target_os = "macos")]
        const NONBLOCK: i32 = 0x4;
        options.custom_flags(NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("not a regular configuration file"));
    }
    Ok(file)
}

impl Store {
    pub fn load() -> Result<Self, String> {
        Self::load_from(config_path()?)
    }

    pub fn load_from(config: PathBuf) -> Result<Self, String> {
        let metadata = match fs::metadata(&config) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self {
                    roots: Vec::new(),
                    config,
                });
            }
            Err(error) => {
                return Err(format!(
                    "Cannot read folder connections {}: {error}",
                    config.display()
                ));
            }
        };
        if !metadata.is_file() || metadata.len() > CONFIG_LIMIT {
            return Err(format!(
                "Cannot read folder connections {}: expected a regular JSON file no larger than 1 MiB.",
                config.display()
            ));
        }
        let mut bytes = Vec::new();
        nonblocking_read(&config)
            .and_then(|file| file.take(CONFIG_LIMIT + 1).read_to_end(&mut bytes))
            .map_err(|e| format!("Cannot read folder connections {}: {e}", config.display()))?;
        if bytes.len() as u64 > CONFIG_LIMIT {
            return Err(format!(
                "Folder connections {} exceed the 1 MiB limit.",
                config.display()
            ));
        }
        let connections: Connections = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Invalid folder connections {}: {e}", config.display()))?;
        let mut roots = Vec::<Folder>::new();
        for folder in connections.roots {
            if !folder.path.is_absolute()
                || folder
                    .path
                    .components()
                    .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
            {
                return Err(format!(
                    "Invalid connected folder {} in {}: expected an absolute canonical path; reconnect this folder.",
                    folder.path.display(),
                    config.display()
                ));
            }
            if !roots.iter().any(|root| root.path == folder.path) {
                roots.push(folder);
            }
        }
        // Do not probe the roots here: a disconnected network mount must not
        // delay startup or discard the saved connection.
        Ok(Self { roots, config })
    }

    pub fn connect(&mut self, paths: Vec<PathBuf>) -> Result<usize, String> {
        // Resolve network paths before taking the local configuration lock.
        let mut additions = Vec::new();
        for path in paths {
            let canonical = fs::canonicalize(&path)
                .map_err(|e| format!("Cannot connect folder {}: {e}", path.display()))?;
            if !fs::metadata(&canonical)
                .map_err(|e| format!("Cannot connect folder {}: {e}", path.display()))?
                .is_dir()
            {
                return Err(format!(
                    "Cannot connect folder {}: not a directory.",
                    path.display()
                ));
            }
            additions.push(canonical);
        }
        let _lock = self.write_lock()?;
        let mut proposed = Self::load_from(self.config.clone())?.roots;
        let previous = proposed.len();
        for path in additions {
            if !proposed.iter().any(|folder| folder.path == path) {
                proposed.push(Folder { path });
            }
        }
        let added = proposed.len() - previous;
        if added > 0 {
            self.persist(&proposed)?;
        }
        self.roots = proposed;
        Ok(added)
    }

    pub fn disconnect(&mut self, index: usize) -> Result<Folder, String> {
        let removed = self
            .roots
            .get(index)
            .cloned()
            .ok_or("Connected folder number is out of range.")?;
        let _lock = self.write_lock()?;
        let mut proposed = Self::load_from(self.config.clone())?.roots;
        // A displayed index is session-local. Remove its intended path, never
        // another session's differently numbered connection.
        if let Some(current) = proposed.iter().position(|f| f.path == removed.path) {
            proposed.remove(current);
            self.persist(&proposed)?;
        }
        self.roots = proposed;
        Ok(removed)
    }

    fn write_lock(&self) -> Result<fs::File, String> {
        let result = (|| -> io::Result<fs::File> {
            let parent = self
                .config
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .ok_or_else(|| io::Error::other("configuration path needs a parent directory"))?;
            fs::create_dir_all(parent)?;
            let lock_path = self.config.with_extension("json.lock");
            let mut options = fs::OpenOptions::new();
            options.read(true).write(true).create(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            #[cfg(target_os = "macos")]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(0x4 | 0x100);
            }
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(0x800 | 0x20000);
            }
            let file = options.open(lock_path)?;
            if !file.metadata()?.is_file() {
                return Err(io::Error::other("configuration lock is not a regular file"));
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                match file.try_lock() {
                    Ok(()) => return Ok(file),
                    Err(fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(std::time::Duration::from_millis(20))
                    }
                    Err(fs::TryLockError::WouldBlock) => {
                        return Err(io::Error::other(
                            "another Pitty session is updating folder connections; try again",
                        ));
                    }
                    Err(fs::TryLockError::Error(e)) => return Err(e),
                }
            }
        })();
        result.map_err(|e| {
            format!(
                "Cannot lock folder connections {}: {e}",
                self.config.display()
            )
        })
    }

    fn persist(&self, roots: &[Folder]) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(&Connections {
            roots: roots.to_vec(),
        })
        .map_err(|e| format!("Cannot encode folder connections: {e}"))?;
        if bytes.len() as u64 > CONFIG_LIMIT {
            return Err("Folder connections exceed the 1 MiB configuration limit.".into());
        }
        let result = (|| -> io::Result<()> {
            let parent = self
                .config
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .ok_or_else(|| io::Error::other("configuration path needs a parent directory"))?;
            fs::create_dir_all(parent)?;
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let temporary = parent.join(format!(
                ".pitty-folders-{}-{now}-{sequence}.tmp",
                std::process::id()
            ));
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            let write = (|| -> io::Result<()> {
                file.write_all(&bytes)?;
                file.sync_all()?;
                fs::rename(&temporary, &self.config)
            })();
            if write.is_err() {
                let _ = fs::remove_file(&temporary);
            }
            write
        })();
        result.map_err(|e| {
            format!(
                "Cannot save folder connections {}: {e}",
                self.config.display()
            )
        })
    }
}

pub fn list(folder: &Folder, relative: &Path) -> Result<Listing, String> {
    if relative.is_absolute() {
        return Err(format!(
            "Cannot browse {}: choose a path relative to connected folder {}.",
            relative.display(),
            folder.path.display()
        ));
    }
    let root = fs::canonicalize(&folder.path).map_err(|e| {
        format!(
            "Connected folder {} is unavailable: {e}",
            folder.path.display()
        )
    })?;
    if root != folder.path {
        return Err(format!(
            "Connected folder {} now resolves to a different location. Reconnect it before browsing.",
            folder.path.display()
        ));
    }
    let requested = root.join(relative);
    let directory = fs::canonicalize(&requested)
        .map_err(|e| format!("Cannot browse folder {}: {e}", requested.display()))?;
    if !directory.starts_with(&root) {
        return Err(format!(
            "Cannot browse {}: the path leaves connected folder {}.",
            requested.display(),
            root.display()
        ));
    }
    let entries = fs::read_dir(&directory)
        .map_err(|e| format!("Cannot browse folder {}: {e}", directory.display()))?;
    let mut listing = Listing {
        directory,
        entries: Vec::new(),
        truncated: false,
        skipped_entries: 0,
    };
    // Bound directory work even when thousands of entries are inaccessible.
    for (index, entry) in entries.enumerate() {
        if index == LIST_LIMIT {
            listing.truncated = true;
            break;
        }
        let Ok(entry) = entry else {
            listing.skipped_entries += 1;
            continue;
        };
        let path = match fs::canonicalize(entry.path()) {
            Ok(path) if path.starts_with(&root) => path,
            _ => {
                listing.skipped_entries += 1;
                continue;
            }
        };
        let Ok(metadata) = fs::metadata(&path) else {
            listing.skipped_entries += 1;
            continue;
        };
        if !metadata.is_dir() && !metadata.is_file() {
            listing.skipped_entries += 1;
            continue;
        }
        listing.entries.push(Entry {
            name: entry.file_name().to_string_lossy().into_owned(),
            path,
            is_dir: metadata.is_dir(),
        });
    }
    listing
        .entries
        .sort_by_cached_key(|entry| (!entry.is_dir, entry.name.to_lowercase(), entry.name.clone()));
    Ok(listing)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "pitty-folders-test-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(fs::canonicalize(path).unwrap())
        }
        fn directory(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir_all(&path).unwrap();
            path
        }
        fn config(&self) -> PathBuf {
            self.0.join("config/folders.json")
        }
        fn store(&self) -> Store {
            Store::load_from(self.config()).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn multiple_roots_spaces_deduplication_and_disconnect_persist() {
        let fixture = Fixture::new();
        let a = fixture.directory("Desktop files");
        let b = fixture.directory("Network drive");
        let mut store = fixture.store();
        assert_eq!(
            store
                .connect(vec![a.clone(), b.clone(), a.clone()])
                .unwrap(),
            2
        );
        assert_eq!(store.connect(vec![a.clone()]).unwrap(), 0);
        let mut loaded = fixture.store();
        assert_eq!(loaded.roots.len(), 2);
        assert_eq!(loaded.disconnect(0).unwrap().path, a);
        assert_eq!(fixture.store().roots[0].path, b);
        assert!(loaded.disconnect(5).is_err());
        assert_eq!(loaded.roots.len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(fixture.config()).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn invalid_batch_and_failed_persistence_leave_existing_roots_unchanged() {
        let fixture = Fixture::new();
        let a = fixture.directory("first");
        let missing = fixture.0.join("unavailable drive");
        let mut store = fixture.store();
        let error = store.connect(vec![a.clone(), missing.clone()]).unwrap_err();
        assert!(error.contains(&missing.display().to_string()));
        assert!(store.roots.is_empty());
        assert!(!fixture.config().exists());
        store.connect(vec![a]).unwrap();
        let b = fixture.directory("second");
        fs::remove_file(fixture.config()).unwrap();
        fs::create_dir(fixture.config()).unwrap();
        assert!(store.connect(vec![b]).is_err());
        assert_eq!(store.roots.len(), 1);
        assert!(store.disconnect(0).is_err());
        assert_eq!(store.roots.len(), 1);
    }

    #[test]
    fn listing_is_sorted_nonrecursive_and_missing_roots_remain_connected() {
        let fixture = Fixture::new();
        let root = fixture.directory("root");
        let mut store = fixture.store();
        store.connect(vec![root.clone()]).unwrap();
        fs::write(root.join("z.txt"), b"secret contents are never loaded").unwrap();
        fs::write(root.join(".hidden"), b"metadata only").unwrap();
        fs::create_dir(root.join("A folder")).unwrap();
        fs::write(root.join("A folder/nested.txt"), b"nested").unwrap();
        let listed = list(&store.roots[0], Path::new("")).unwrap();
        assert_eq!(listed.directory, root);
        assert_eq!(
            listed
                .entries
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["A folder", ".hidden", "z.txt"]
        );
        assert!(listed.entries[0].is_dir);
        assert!(!listed.truncated);
        assert_eq!(listed.skipped_entries, 0);
        fs::remove_dir_all(&root).unwrap();
        let loaded = fixture.store();
        assert_eq!(loaded.roots.len(), 1);
        assert!(
            list(&loaded.roots[0], Path::new(""))
                .unwrap_err()
                .contains(&root.display().to_string())
        );
    }

    #[test]
    fn config_is_bounded_and_malformed_paths_are_rejected() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.config().parent().unwrap()).unwrap();
        fs::write(fixture.config(), br#"{"roots":[{"path":"relative"}]}"#).unwrap();
        assert!(Store::load_from(fixture.config()).is_err());
        fs::write(fixture.config(), b"invalid JSON").unwrap();
        assert!(Store::load_from(fixture.config()).is_err());
        let file = fs::File::create(fixture.config()).unwrap();
        file.set_len(CONFIG_LIMIT + 1).unwrap();
        assert!(Store::load_from(fixture.config()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn containment_rejects_traversal_outside_links_and_replaced_root() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let root = fixture.directory("root");
        let outside = fixture.directory("outside");
        fs::write(outside.join("private.txt"), b"private").unwrap();
        fs::create_dir(root.join("inside")).unwrap();
        fs::write(root.join("inside/file.txt"), b"ok").unwrap();
        symlink(&outside, root.join("escape")).unwrap();
        symlink(root.join("inside"), root.join("inside alias")).unwrap();
        let folder = Folder { path: root.clone() };
        let listed = list(&folder, Path::new("")).unwrap();
        assert_eq!(listed.skipped_entries, 1);
        assert!(!listed.entries.iter().any(|e| e.name == "escape"));
        assert!(list(&folder, Path::new("../outside")).is_err());
        assert!(list(&folder, Path::new("escape")).is_err());
        assert!(list(&folder, &outside).is_err());
        assert_eq!(
            list(&folder, Path::new("inside alias")).unwrap().entries[0].path,
            root.join("inside/file.txt")
        );
        fs::remove_dir_all(&root).unwrap();
        symlink(&outside, &root).unwrap();
        assert!(
            list(&folder, Path::new(""))
                .unwrap_err()
                .contains("different location")
        );
    }

    #[test]
    fn listing_caps_work_and_reports_truncation() {
        let fixture = Fixture::new();
        let root = fixture.directory("large");
        for n in 0..=LIST_LIMIT {
            fs::write(root.join(format!("{n}.txt")), []).unwrap();
        }
        let listed = list(&Folder { path: root }, Path::new("")).unwrap();
        assert_eq!(listed.entries.len(), LIST_LIMIT);
        assert!(listed.truncated);
    }
    #[test]
    fn separate_sessions_merge_connections_and_disconnect_by_path() {
        let fixture = Fixture::new();
        let a = fixture.directory("a");
        let b = fixture.directory("b");
        let c = fixture.directory("c");
        let mut first = fixture.store();
        let mut second = fixture.store();
        first.connect(vec![a.clone()]).unwrap();
        second.connect(vec![b.clone()]).unwrap();
        assert_eq!(fixture.store().roots.len(), 2);
        let mut stale = fixture.store();
        first.disconnect(0).unwrap();
        second.connect(vec![c.clone()]).unwrap();
        // Stale index1 meant b, although b is now index0 on disk.
        assert_eq!(stale.disconnect(1).unwrap().path, b);
        assert_eq!(
            fixture
                .store()
                .roots
                .iter()
                .map(|f| f.path.clone())
                .collect::<Vec<_>>(),
            vec![c]
        );
    }
    #[test]
    fn simultaneous_writers_preserve_both_connections() {
        let fixture = Fixture::new();
        let a = fixture.directory("writer a");
        let b = fixture.directory("writer b");
        let mut first = fixture.store();
        let mut second = fixture.store();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let ready = barrier.clone();
        let one = std::thread::spawn(move || {
            ready.wait();
            first.connect(vec![a]).unwrap();
        });
        let two = std::thread::spawn(move || {
            barrier.wait();
            second.connect(vec![b]).unwrap();
        });
        one.join().unwrap();
        two.join().unwrap();
        assert_eq!(fixture.store().roots.len(), 2);
    }
}
