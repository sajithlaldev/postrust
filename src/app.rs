use crate::http::{self, ResponseData};
use crate::model::{Auth, Collection, Environment, Request, Store};
use crate::storage::{self, Storage};
use crate::{ui, vars};
use std::sync::{Arc, mpsc};
use std::time::Duration;

/// Bodies are rendered as text up to this many bytes; the rest is cut off in the viewer.
const DISPLAY_LIMIT: usize = 200_000;
/// Bodies above this size are shown as-is instead of being re-formatted as JSON.
const PRETTY_LIMIT: usize = 2_000_000;

pub struct Resp {
    pub data: ResponseData,
    /// Computed once on arrival; validating a 20 MB body every frame is too slow.
    pub is_text: bool,
    /// Pretty-printed text computed once on arrival (empty for binary bodies).
    pub display: String,
    pub display_cut: bool,
}

pub type Outcome = Result<Resp, String>;

pub struct Pending {
    rx: mpsc::Receiver<Result<Resp, String>>,
    handle: tokio::task::JoinHandle<()>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReqTab {
    Params,
    Headers,
    Body,
    Auth,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RespTab {
    Body,
    Headers,
}

pub struct Tab {
    pub id: u64,
    pub request: Request,
    /// (collection index, request index) the tab was opened from / saved to.
    pub source: Option<(usize, usize)>,
    pub response: Option<Outcome>,
    pub pending: Option<Pending>,
    pub req_tab: ReqTab,
    pub resp_tab: RespTab,
}

impl Drop for Tab {
    fn drop(&mut self) {
        if let Some(p) = &self.pending {
            p.handle.abort();
        }
    }
}

pub enum RenameTarget {
    Collection(usize),
    Request(usize, usize),
}

pub struct App {
    rt: Arc<tokio::runtime::Runtime>,
    http: Arc<http::Http>,
    storage: Option<Storage>,
    pub store: Store,
    dirty: bool,
    pub tabs: Vec<Tab>,
    pub active: usize,
    next_id: u64,
    pub show_env: bool,
    pub env_sel: usize,
    pub show_io: bool,
    pub io_path: String,
    pub rename: Option<(RenameTarget, String)>,
    pub new_collection: String,
    pub status: Option<String>,
    /// Top-bar logo texture, loaded on first draw.
    pub logo: Option<egui::TextureHandle>,
}

impl App {
    pub fn new(storage: Option<Storage>) -> Self {
        let mut store = storage.as_ref().map(|s| s.load()).unwrap_or_default();
        let load_notice = storage.as_ref().and_then(|s| s.take_notice());
        store.fix_active_env(); // a hand-edited file may point past the environments list
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime");
        let mut app = Self {
            rt: Arc::new(rt),
            http: Arc::new(http::Http::new().expect("http client")),
            storage,
            store,
            dirty: false,
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
            show_env: false,
            env_sel: 0,
            show_io: false,
            io_path: default_export_path(),
            rename: None,
            new_collection: String::new(),
            status: None,
            logo: None,
        };
        app.new_tab(Request::default(), None);
        if load_notice.is_some() {
            app.status = load_notice;
        } else if app.storage.is_none() {
            app.status = Some("No data directory found: collections and environments will not be saved".into());
        }
        app
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn new_tab(&mut self, request: Request, source: Option<(usize, usize)>) {
        let tab = Tab {
            id: self.next_id,
            request,
            source,
            response: None,
            pending: None,
            req_tab: ReqTab::Params,
            resp_tab: RespTab::Body,
        };
        self.next_id += 1;
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
    }

    pub fn open_saved(&mut self, c: usize, r: usize) {
        if let Some(i) = self.tabs.iter().position(|t| t.source == Some((c, r))) {
            self.active = i;
        } else if let Some(req) = self.store.collections.get(c).and_then(|c| c.requests.get(r)) {
            let req = req.clone();
            self.new_tab(req, Some((c, r)));
        }
    }

    pub fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        self.tabs.remove(i);
        if self.tabs.is_empty() {
            self.new_tab(Request::default(), None);
        }
        if i < self.active {
            self.active -= 1;
        }
        self.active = self.active.min(self.tabs.len() - 1);
    }

    /// Variables referenced by the request that no enabled variable defines.
    pub fn missing_vars(&self, req: &Request) -> Vec<String> {
        http::missing_vars(req, &vars::to_map(self.store.active_vars()))
    }

    pub fn send(&mut self, idx: usize, ctx: &egui::Context) {
        let map = vars::to_map(self.store.active_vars());
        let timeout = Duration::from_secs(self.store.timeout_secs.unwrap_or(http::DEFAULT_TIMEOUT_SECS).max(1));
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        if tab.pending.is_some() {
            return;
        }
        let critical = http::critical_missing_vars(&tab.request, &map);
        if !critical.is_empty() {
            tab.response = Some(Err(format!(
                "Undefined variable(s): {}. Select an environment or define them under Manage.",
                critical.join(", ")
            )));
            return;
        }
        match http::prepare(&tab.request, &map) {
            Err(e) => tab.response = Some(Err(format!("{e:#}"))),
            Ok(prepared) => {
                let (tx, rx) = mpsc::channel();
                let ctx = ctx.clone();
                let client = self.http.clone();
                let handle = self.rt.spawn(async move {
                    let res = match client.send(prepared, timeout).await {
                        // Parsing and pretty-printing big bodies must not run on the UI thread.
                        Ok(data) => tokio::task::spawn_blocking(move || into_resp(data))
                            .await
                            .map_err(|e| e.to_string()),
                        Err(e) => Err(format!("{e:#}")),
                    };
                    let _ = tx.send(res);
                    ctx.request_repaint();
                });
                tab.pending = Some(Pending { rx, handle });
                // History is persisted; keep Auth-tab credentials out of it.
                let mut req = tab.request.clone();
                req.auth = Auth::None;
                self.store.push_history(&req);
                self.dirty = true;
            }
        }
    }

    pub fn cancel(&mut self, idx: usize) {
        if let Some(tab) = self.tabs.get_mut(idx)
            && let Some(p) = tab.pending.take()
        {
            p.handle.abort();
            tab.response = Some(Err("Request cancelled".into()));
        }
    }

    fn poll(&mut self) {
        for tab in &mut self.tabs {
            let Some(p) = &tab.pending else { continue };
            match p.rx.try_recv() {
                Ok(res) => {
                    tab.pending = None;
                    tab.response = Some(res);
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    tab.pending = None;
                    tab.response = Some(Err("Request task ended unexpectedly".into()));
                }
            }
        }
    }

    /// Saves the tab's request into its source slot, or into the first collection.
    pub fn save_tab(&mut self, idx: usize) {
        let Some(tab) = self.tabs.get(idx) else { return };
        let req = tab.request.clone();
        let valid = tab
            .source
            .filter(|&(c, r)| self.store.collections.get(c).is_some_and(|c| r < c.requests.len()));
        let source = match valid {
            Some((c, r)) => {
                self.store.collections[c].requests[r] = req;
                (c, r)
            }
            None => {
                if self.store.collections.is_empty() {
                    self.store.collections.push(Collection {
                        name: "My Collection".into(),
                        requests: vec![],
                    });
                }
                let coll = &mut self.store.collections[0];
                coll.requests.push(req);
                (0, coll.requests.len() - 1)
            }
        };
        self.tabs[idx].source = Some(source);
        self.status = Some("Saved".into());
        self.dirty = true;
    }

    pub fn rename(&mut self, target: &RenameTarget, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        match *target {
            RenameTarget::Collection(c) => {
                if let Some(coll) = self.store.collections.get_mut(c) {
                    coll.name = name.to_string();
                }
            }
            RenameTarget::Request(c, r) => {
                if let Some(req) = self.store.collections.get_mut(c).and_then(|c| c.requests.get_mut(r)) {
                    req.name = name.to_string();
                }
                for tab in &mut self.tabs {
                    if tab.source == Some((c, r)) {
                        tab.request.name = name.to_string();
                    }
                }
            }
        }
        self.dirty = true;
    }

    pub fn add_collection(&mut self, name: &str) {
        let name = name.trim();
        if !name.is_empty() {
            self.store.collections.push(Collection {
                name: name.to_string(),
                requests: vec![],
            });
            self.dirty = true;
        }
    }

    pub fn clear_history(&mut self) {
        self.store.history.clear();
        self.dirty = true;
    }

    pub fn delete_request(&mut self, c: usize, r: usize) {
        // Out-of-range indices are a no-op: nothing moved, so nothing may be re-indexed.
        let Some(coll) = self.store.collections.get_mut(c) else {
            return;
        };
        if r >= coll.requests.len() {
            return;
        }
        coll.requests.remove(r);
        // Later requests in the same collection shift down by one; the deleted one detaches.
        for tab in &mut self.tabs {
            tab.source = match tab.source {
                Some((sc, sr)) if sc == c && sr == r => None,
                Some((sc, sr)) if sc == c && sr > r => Some((sc, sr - 1)),
                other => other,
            };
        }
        self.rename = None; // its indices may now be stale
        self.dirty = true;
    }

    pub fn delete_collection(&mut self, c: usize) {
        if c >= self.store.collections.len() {
            return;
        }
        self.store.collections.remove(c);
        for tab in &mut self.tabs {
            tab.source = match tab.source {
                Some((sc, _)) if sc == c => None,
                Some((sc, sr)) if sc > c => Some((sc - 1, sr)),
                other => other,
            };
        }
        self.rename = None; // its indices may now be stale
        self.dirty = true;
    }

    pub fn set_active_env(&mut self, env: Option<usize>) {
        self.store.active_env = env;
        self.store.fix_active_env();
        self.dirty = true;
    }

    pub fn set_timeout(&mut self, secs: u64) {
        self.store.timeout_secs = Some(secs.clamp(1, 600));
        self.dirty = true;
    }

    pub fn add_environment(&mut self) {
        self.store.environments.push(Environment {
            name: "New environment".into(),
            vars: vec![],
        });
        self.env_sel = self.store.environments.len() - 1;
        self.dirty = true;
    }

    pub fn delete_environment(&mut self, i: usize) {
        if i < self.store.environments.len() {
            self.store.environments.remove(i);
            self.store.active_env = match self.store.active_env {
                Some(a) if a == i => None,
                Some(a) if a > i => Some(a - 1),
                other => other,
            };
            self.store.fix_active_env();
            if self.env_sel > i {
                self.env_sel -= 1;
            }
            self.env_sel = self.env_sel.min(self.store.environments.len().saturating_sub(1));
            self.dirty = true;
        }
    }

    /// Writes the response body of tab `idx` to the Downloads folder (never overwriting).
    pub fn save_body(&mut self, idx: usize) {
        let Some(Ok(resp)) = self.tabs.get(idx).and_then(|t| t.response.as_ref()) else {
            return;
        };
        let d = &resp.data;
        let ext = http::extension_for(d.content_type());
        let dir = directories::UserDirs::new()
            .and_then(|u| u.download_dir().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        self.status = Some(match write_new_file(&dir, "postrust-response", ext, &d.body) {
            Ok(path) => format!("Saved response body to {}", path.display()),
            Err(e) => format!("Could not save body: {e}"),
        });
    }

    pub fn export_collections(&mut self) {
        let path = std::path::PathBuf::from(self.io_path.trim());
        // Only replace an existing file if it is a previous export, never arbitrary user data.
        if path.exists() && storage::import_collections(&path).is_err() {
            self.status = Some(format!(
                "Export refused: {} exists and is not a postrust export",
                path.display()
            ));
            return;
        }
        self.status = Some(match storage::export_collections(&self.store.collections, &path) {
            Ok(()) => format!(
                "Exported {} collection(s) to {}",
                self.store.collections.len(),
                path.display()
            ),
            Err(e) => format!("Export failed: {e:#}"),
        });
    }

    pub fn import_collections(&mut self) {
        let path = std::path::PathBuf::from(self.io_path.trim());
        self.status = Some(match storage::import_collections(&path) {
            Ok(cols) => {
                let n = cols.len();
                self.store.collections.extend(cols);
                self.dirty = true;
                format!("Imported {n} collection(s)")
            }
            Err(e) => format!("Import failed: {e:#}"),
        });
    }

    fn persist(&mut self) {
        if !self.dirty {
            return;
        }
        let Some(s) = &self.storage else {
            self.dirty = false;
            return;
        };
        // One attempt per change: a failing disk must not hammer the filesystem or overwrite
        // the status bar every frame. The next edit saves the whole store again.
        self.dirty = false;
        if let Err(e) = s.save(&self.store) {
            self.status = Some(format!("Could not save data: {e:#}"));
        }
    }
}

/// Creates `<stem>.<ext>` (or `<stem>-N.<ext>`) with `create_new`, so an existing file or
/// symlink is never written through and concurrent creators can't be overwritten.
fn write_new_file(dir: &std::path::Path, stem: &str, ext: &str, bytes: &[u8]) -> std::io::Result<std::path::PathBuf> {
    use std::io::Write;
    for n in 0..1000 {
        let name = if n == 0 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}-{n}.{ext}")
        };
        let path = dir.join(name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                f.write_all(bytes)?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "too many existing response files",
    ))
}

fn default_export_path() -> String {
    directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(|p| p.join("postrust-collections.json")))
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "postrust-collections.json".into())
}

fn into_resp(data: ResponseData) -> Resp {
    let is_text = data.is_text();
    if !is_text {
        return Resp {
            data,
            is_text,
            display: String::new(),
            display_cut: false,
        };
    }
    // Pretty-printing parses and re-serializes the whole body; skip it for very large ones.
    let mut display = if data.body.len() <= PRETTY_LIMIT {
        data.pretty_text()
    } else {
        data.text()
    };
    let mut cut = data.truncated;
    if display.len() > DISPLAY_LIMIT {
        let mut end = DISPLAY_LIMIT;
        while !display.is_char_boundary(end) {
            end -= 1;
        }
        display.truncate(end);
        cut = true;
    }
    Resp {
        data,
        is_text,
        display,
        display_cut: cut,
    }
}

impl eframe::App for App {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui::draw(self, ui);
        self.persist();
    }
}
