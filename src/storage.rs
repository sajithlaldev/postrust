use crate::model::{Collection, Store};
#[cfg(not(target_arch = "wasm32"))]
use anyhow::Context;
use anyhow::{Result, bail};
use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Where the serialized store physically lives: a file natively, `localStorage` in a browser.
/// The load/save policy (what to do with corrupt or unreadable data) is shared in [`Storage`].
pub trait Backend {
    /// `Ok(None)` means nothing has been stored yet; `Err` describes why existing data
    /// could not be read.
    fn read(&self) -> Result<Option<Vec<u8>>, String>;
    fn write(&self, bytes: &[u8]) -> Result<()>;
    /// Moves unparseable data out of the way so it is never silently overwritten. Returns
    /// where it went.
    fn quarantine(&self) -> Result<String, String>;
    /// Human-readable location for messages (a path, or "browser storage").
    fn describe(&self) -> String;
    fn path(&self) -> Option<&Path> {
        None
    }
    /// Write compact JSON (no indentation). Browsers cap `localStorage` at roughly 5 MB, so
    /// pretty-printing would waste a fifth of it; files stay human-readable.
    fn compact(&self) -> bool {
        false
    }
}

pub struct Storage {
    backend: Box<dyn Backend>,
    /// Set when existing data exists but could not be read; saving is then refused
    /// so unreadable user data is never overwritten with an empty store.
    read_failed: AtomicBool,
    /// Message for the user about a corrupt/unreadable store found by `load`.
    notice: Mutex<Option<String>>,
}

impl Storage {
    pub fn with_backend(backend: Box<dyn Backend>) -> Self {
        Self {
            backend,
            read_failed: AtomicBool::new(false),
            notice: Mutex::new(None),
        }
    }

    /// A file-backed store at `path`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(path: PathBuf) -> Self {
        Self::with_backend(Box::new(FileBackend { path }))
    }

    /// Default location: `<config dir>/postrust/store.json` natively, browser `localStorage`
    /// on the web.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn default_location() -> Option<Self> {
        let dirs = directories::ProjectDirs::from("", "", "postrust")?;
        Some(Self::new(dirs.config_dir().join("store.json")))
    }

    #[cfg(target_arch = "wasm32")]
    pub fn default_location() -> Option<Self> {
        crate::platform::BrowserBackend::new().map(|b| Self::with_backend(Box::new(b)))
    }

    /// The file behind a file-backed store (`None` for other backends).
    pub fn path(&self) -> Option<&Path> {
        self.backend.path()
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

    /// Loads the store. Nothing stored yields an empty store; invalid JSON is moved aside
    /// (never silently overwritten); unreadable data blocks later saves.
    pub fn load(&self) -> Store {
        let bytes = match self.backend.read() {
            Ok(Some(b)) => b,
            Ok(None) => return Store::default(),
            Err(e) => {
                self.read_failed.store(true, Ordering::SeqCst);
                self.set_notice(format!(
                    "Could not read {} ({e}). Starting empty; changes will NOT be saved.",
                    self.backend.describe()
                ));
                return Store::default();
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(store) => store,
            Err(_) => {
                match self.backend.quarantine() {
                    Ok(moved_to) => self.set_notice(format!(
                        "Saved data was invalid JSON; moved to {moved_to}. Starting empty."
                    )),
                    Err(_) => {
                        self.read_failed.store(true, Ordering::SeqCst);
                        self.set_notice(format!(
                            "{} is invalid and could not be backed up. Starting empty; changes will NOT be saved.",
                            self.backend.describe()
                        ));
                    }
                }
                Store::default()
            }
        }
    }

    pub fn save(&self, store: &Store) -> Result<()> {
        if self.read_failed.load(Ordering::SeqCst) {
            bail!("the existing data could not be read, so it was not overwritten");
        }
        let bytes = if self.backend.compact() {
            serde_json::to_vec(store)?
        } else {
            serde_json::to_vec_pretty(store)?
        };
        self.backend.write(&bytes)
    }
}

// ---------------------------------------------------------------------------------------------
// File backend (native)
// ---------------------------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
struct FileBackend {
    path: PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl Backend for FileBackend {
    fn read(&self) -> Result<Option<Vec<u8>>, String> {
        match std::fs::read(&self.path) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Atomic save: write to a private temp file, then rename over the target.
    fn write(&self, bytes: &[u8]) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            create_private_dir(dir).context("creating config dir")?;
        }
        let tmp = tmp_path(&self.path);
        write_private(&tmp, bytes).context("writing store")?;
        std::fs::rename(&tmp, &self.path).context("replacing store")?;
        Ok(())
    }

    /// Renames the file to `store.json.bak` (or a timestamped name if that exists).
    fn quarantine(&self) -> Result<String, String> {
        let mut backup = self.path.with_extension("json.bak");
        if backup.exists() {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            backup = self.path.with_extension(format!("json.{secs}.bak"));
        }
        std::fs::rename(&self.path, &backup).map_err(|e| e.to_string())?;
        Ok(backup.display().to_string())
    }

    fn describe(&self) -> String {
        self.path.display().to_string()
    }

    fn path(&self) -> Option<&Path> {
        Some(&self.path)
    }
}

/// `<file name>.postrust-tmp` next to the target. Deliberately not a common extension swap
/// (`report.txt` -> `report.json.tmp`), because `write_private` replaces whatever is at this path.
#[cfg(not(target_arch = "wasm32"))]
fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".postrust-tmp");
    path.with_file_name(name)
}

/// Files may hold tokens and passwords: owner-only (0600) on Unix.
#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
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

/// The export file's contents (pretty JSON with a format marker).
pub fn collections_to_json(collections: &[Collection]) -> Result<String> {
    Ok(serde_json::to_string_pretty(&Export {
        format: EXPORT_FORMAT.into(),
        collections: collections.to_vec(),
    })?)
}

/// Parses an export; anything that is not a postrust export (including other JSON) is refused.
pub fn collections_from_json(text: &str) -> Result<Vec<Collection>> {
    let export: Export = serde_json::from_str(text).map_err(|e| anyhow::anyhow!("parsing import: {e}"))?;
    if export.format != EXPORT_FORMAT {
        bail!("not a postrust collections export");
    }
    Ok(export.collections)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn export_collections(collections: &[Collection], path: &Path) -> Result<()> {
    write_export_file(collections_to_json(collections)?.as_bytes(), path)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn import_collections(path: &Path) -> Result<Vec<Collection>> {
    let text = std::fs::read_to_string(path).context("reading import file")?;
    collections_from_json(&text)
}

#[cfg(not(target_arch = "wasm32"))]
fn write_export_file(bytes: &[u8], path: &Path) -> Result<()> {
    let tmp = tmp_path(path);
    write_private(&tmp, bytes).context("writing export")?;
    std::fs::rename(&tmp, path).context("replacing export")
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub fn export_json<T: serde::Serialize>(value: &T, path: &Path) -> Result<()> {
    write_export_file(&serde_json::to_vec_pretty(value)?, path)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
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
        let mode = std::fs::metadata(s.path().unwrap()).unwrap().permissions().mode() & 0o777;
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

    /// A backend that lives in memory and can be told to fail, to test the shared policy.
    #[derive(Default)]
    struct Memory {
        compact: bool,
        data: std::cell::RefCell<Option<Vec<u8>>>,
        backup: std::cell::RefCell<Option<Vec<u8>>>,
        fail_read: bool,
        fail_quarantine: bool,
    }

    impl Backend for std::rc::Rc<Memory> {
        fn read(&self) -> Result<Option<Vec<u8>>, String> {
            if self.fail_read {
                return Err("disk on fire".into());
            }
            Ok(self.data.borrow().clone())
        }
        fn write(&self, bytes: &[u8]) -> Result<()> {
            *self.data.borrow_mut() = Some(bytes.to_vec());
            Ok(())
        }
        fn quarantine(&self) -> Result<String, String> {
            if self.fail_quarantine {
                return Err("cannot move".into());
            }
            *self.backup.borrow_mut() = self.data.borrow_mut().take();
            Ok("memory backup".into())
        }
        fn describe(&self) -> String {
            "memory".into()
        }
        fn compact(&self) -> bool {
            self.compact
        }
    }

    fn memory_storage(mem: &std::rc::Rc<Memory>) -> Storage {
        Storage::with_backend(Box::new(mem.clone()))
    }

    #[test]
    fn shared_policy_round_trips_through_any_backend() {
        let mem = std::rc::Rc::new(Memory::default());
        let s = memory_storage(&mem);
        assert_eq!(s.load(), Store::default());
        assert!(s.take_notice().is_none());
        let store = Store {
            timeout_secs: Some(7),
            ..Default::default()
        };
        s.save(&store).unwrap();
        assert_eq!(memory_storage(&mem).load(), store);
        assert!(s.path().is_none(), "only file backends have a path");
    }

    #[test]
    fn compact_backends_store_smaller_json_that_loads_identically() {
        let store = Store {
            collections: vec![Collection {
                name: "c".into(),
                requests: vec![Request::new("r")],
            }],
            ..Default::default()
        };
        let pretty = std::rc::Rc::new(Memory::default());
        let compact = std::rc::Rc::new(Memory {
            compact: true,
            ..Default::default()
        });
        memory_storage(&pretty).save(&store).unwrap();
        memory_storage(&compact).save(&store).unwrap();
        let (p, c) = (
            pretty.data.borrow().clone().unwrap(),
            compact.data.borrow().clone().unwrap(),
        );
        assert!(p.contains(&b'\n'), "files stay readable");
        assert!(
            !c.contains(&b'\n') && c.len() < p.len(),
            "compact: {} vs {} bytes",
            c.len(),
            p.len()
        );
        assert_eq!(memory_storage(&compact).load(), store);
        assert_eq!(memory_storage(&pretty).load(), store);
    }

    #[test]
    fn shared_policy_quarantines_invalid_data() {
        let mem = std::rc::Rc::new(Memory::default());
        *mem.data.borrow_mut() = Some(b"{ nope".to_vec());
        let s = memory_storage(&mem);
        assert_eq!(s.load(), Store::default());
        assert!(s.take_notice().unwrap().contains("moved to memory backup"));
        assert_eq!(mem.backup.borrow().as_deref(), Some(&b"{ nope"[..]));
        assert!(mem.data.borrow().is_none());
        s.save(&Store::default())
            .expect("saving is allowed once the bad data is out of the way");
    }

    #[test]
    fn shared_policy_never_overwrites_data_it_could_not_read_or_move() {
        let unreadable = std::rc::Rc::new(Memory {
            fail_read: true,
            ..Default::default()
        });
        let s = memory_storage(&unreadable);
        assert_eq!(s.load(), Store::default());
        assert!(s.take_notice().unwrap().contains("disk on fire"));
        assert!(s.save(&Store::default()).is_err());

        let stuck = std::rc::Rc::new(Memory {
            fail_quarantine: true,
            ..Default::default()
        });
        *stuck.data.borrow_mut() = Some(b"garbage".to_vec());
        let s = memory_storage(&stuck);
        assert_eq!(s.load(), Store::default());
        assert!(s.take_notice().unwrap().contains("could not be backed up"));
        assert!(s.save(&Store::default()).is_err());
        assert_eq!(
            stuck.data.borrow().as_deref(),
            Some(&b"garbage"[..]),
            "the data is untouched"
        );
    }

    #[test]
    fn export_text_round_trips_and_refuses_foreign_json() {
        let cols = vec![Collection {
            name: "c".into(),
            requests: vec![Request::new("r")],
        }];
        let text = collections_to_json(&cols).unwrap();
        assert_eq!(collections_from_json(&text).unwrap(), cols);
        for bad in ["[]", "{}", r#"{"format":"other","collections":[]}"#, "not json", ""] {
            assert!(collections_from_json(bad).is_err(), "{bad:?}");
        }
    }
}
