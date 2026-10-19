use crate::model::{Collection, Store};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Storage {
    path: PathBuf,
    /// Set when the existing file exists but could not be read; saving is then refused
    /// so unreadable user data is never overwritten with an empty store.
    read_failed: AtomicBool,
    /// Message for the user about a corrupt/unreadable file found by `load`.
    notice: Mutex<Option<String>>,
}

impl Storage {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            read_failed: AtomicBool::new(false),
            notice: Mutex::new(None),
        }
    }

    /// Default location: `<config dir>/postrust/store.json`.
    pub fn default_location() -> Option<Self> {
        let dirs = directories::ProjectDirs::from("", "", "postrust")?;
        Some(Self::new(dirs.config_dir().join("store.json")))
    }

    /// Takes the message about a problem found while loading, if any.
    pub fn take_notice(&self) -> Option<String> {
        self.notice.lock().ok().and_then(|mut n| n.take())
    }

    fn set_notice(&self, msg: String) {
        if let Ok(mut n) = self.notice.lock() {
            *n = Some(msg);
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the store. A missing file yields an empty store; invalid JSON is moved aside
    /// to `store.json.bak` (or a timestamped name if that exists) so it is never silently
    /// overwritten; an unreadable file blocks later saves.
    pub fn load(&self) -> Store {
        let bytes = match std::fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Store::default(),
            Err(e) => {
                self.read_failed.store(true, Ordering::SeqCst);
                self.set_notice(format!(
                    "Could not read {} ({e}). Starting empty; changes will NOT be saved.",
                    self.path.display()
                ));
                return Store::default();
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(store) => store,
            Err(_) => {
                let mut backup = self.path.with_extension("json.bak");
                if backup.exists() {
                    let secs = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    backup = self.path.with_extension(format!("json.{secs}.bak"));
                }
                if std::fs::rename(&self.path, &backup).is_err() {
                    self.read_failed.store(true, Ordering::SeqCst);
                    self.set_notice(format!(
                        "{} is invalid and could not be backed up. Starting empty; changes will NOT be saved.",
                        self.path.display()
                    ));
                } else {
                    self.set_notice(format!(
                        "Saved data was invalid JSON; moved to {}. Starting empty.",
                        backup.display()
                    ));
                }
                Store::default()
            }
        }
    }

    /// Atomic save: write to a private temp file, then rename over the target.
    pub fn save(&self, store: &Store) -> Result<()> {
        if self.read_failed.load(Ordering::SeqCst) {
            bail!("the existing data file could not be read, so it was not overwritten");
        }
        if let Some(dir) = self.path.parent() {
            create_private_dir(dir).context("creating config dir")?;
        }
        let tmp = tmp_path(&self.path);
        write_private(&tmp, &serde_json::to_vec_pretty(store)?).context("writing store")?;
        std::fs::rename(&tmp, &self.path).context("replacing store")?;
        Ok(())
    }
}

/// `<file name>.postrust-tmp` next to the target. Deliberately not a common extension swap
/// (`report.txt` -> `report.json.tmp`), because `write_private` replaces whatever is at this path.
fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".postrust-tmp");
    path.with_file_name(name)
}

/// Files may hold tokens and passwords: owner-only (0600) on Unix.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    // Never write through a pre-planted symlink or into a file with looser permissions:
    // remove whatever is there (a symlink is removed itself) and create fresh.
    let _ = std::fs::remove_file(path);
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(bytes)?;
    f.sync_all() // data must be on disk before the rename that publishes it
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(dir)
}

const EXPORT_FORMAT: &str = "postrust-collections";

/// Marker wrapper so import/export can tell a real export from arbitrary JSON.
#[derive(serde::Serialize, serde::Deserialize)]
struct Export {
    format: String,
    collections: Vec<Collection>,
}

pub fn export_collections(collections: &[Collection], path: &Path) -> Result<()> {
    export_json(
        &Export {
            format: EXPORT_FORMAT.into(),
            collections: collections.to_vec(),
        },
        path,
    )
}

pub fn import_collections(path: &Path) -> Result<Vec<Collection>> {
    let export: Export = import_json(path)?;
    if export.format != EXPORT_FORMAT {
        bail!("not a postrust collections export");
    }
    Ok(export.collections)
}

pub fn export_json<T: serde::Serialize>(value: &T, path: &Path) -> Result<()> {
    let tmp = tmp_path(path);
    write_private(&tmp, &serde_json::to_vec_pretty(value)?).context("writing export")?;
    std::fs::rename(&tmp, path).context("replacing export")
}

pub fn import_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path).context("reading import file")?;
    serde_json::from_str(&text).context("parsing import file")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let s = Storage::new(dir.path().join("nested/store.json"));
        let mut store = Store::default();
        store.collections.push(Collection {
            name: "c".into(),
            requests: vec![Request::new("r")],
        });
        s.save(&store).unwrap();
        assert_eq!(s.load(), store);
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Storage::new(dir.path().join("x.json")).load(), Store::default());
    }

    #[test]
    fn corrupt_file_is_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("store.json");
        std::fs::write(&p, "{ not json").unwrap();
        let s = Storage::new(p.clone());
        assert_eq!(s.load(), Store::default());
        assert!(!p.exists());
        assert!(s.take_notice().unwrap().contains("moved to"));
        assert!(s.take_notice().is_none());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("store.json.bak")).unwrap(),
            "{ not json"
        );
    }

    #[test]
    fn second_corruption_keeps_first_backup() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("store.json");
        let s = Storage::new(p.clone());
        std::fs::write(&p, "first").unwrap();
        s.load();
        std::fs::write(&p, "second").unwrap();
        s.load();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("store.json.bak")).unwrap(),
            "first"
        );
        let baks = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(baks, 2);
    }

    #[test]
    fn unreadable_file_blocks_save() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("store.json");
        std::fs::create_dir(&p).unwrap(); // reading a directory fails with a non-NotFound error
        let s = Storage::new(p);
        assert_eq!(s.load(), Store::default());
        assert!(s.take_notice().unwrap().contains("NOT be saved"));
        assert!(s.save(&Store::default()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn export_does_not_write_through_a_planted_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.txt");
        std::fs::write(&victim, "keep me").unwrap();
        let out = dir.path().join("out.json");
        std::os::unix::fs::symlink(&victim, tmp_path(&out)).unwrap();
        export_json(&vec![1, 2, 3], &out).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        assert_eq!(
            std::fs::read_to_string(&out).unwrap().replace([' ', '\n'], ""),
            "[1,2,3]"
        );
    }

    #[test]
    fn export_never_touches_an_unrelated_neighbour_file() {
        let dir = tempfile::tempdir().unwrap();
        let neighbour = dir.path().join("report.json.tmp");
        std::fs::write(&neighbour, "mine").unwrap();
        export_json(&vec![1], &dir.path().join("report.txt")).unwrap();
        assert_eq!(std::fs::read_to_string(&neighbour).unwrap(), "mine");
    }

    #[cfg(unix)]
    #[test]
    fn saved_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let s = Storage::new(dir.path().join("sub/store.json"));
        s.save(&Store::default()).unwrap();
        let mode = std::fs::metadata(s.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn partial_file_gets_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("store.json");
        std::fs::write(&p, r#"{"collections":[{"name":"x"}]}"#).unwrap();
        let store = Storage::new(p).load();
        assert_eq!(store.collections[0].name, "x");
        assert!(store.history.is_empty());
    }
}
