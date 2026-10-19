use crate::commands::Command;
use crate::http::{self, ResponseData, pretty_json};
use crate::model::{Auth, BodyKind, Collection, Environment, Request, SplitLayout, Store, ThemeMode};
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
    /// The body parsed as JSON (so the viewer highlights it and offers Pretty/Raw).
    pub is_json: bool,
    /// Unformatted text for the Raw view; only filled when `is_json` (otherwise equals `display`).
    pub raw: String,
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
    /// Show pretty-printed JSON (true) or the body exactly as received (false).
    pub pretty: bool,
    /// What the request looked like when the tab was opened. A tab that is not linked to a saved
    /// request (e.g. opened from History) is "unsaved" only if it differs from this.
    pub baseline: Request,
    /// `(body before, body after)` of the last Beautify, so it can be undone while the body is
    /// still exactly what Beautify produced.
    pub beautify_undo: Option<(String, String)>,
    /// `(request before, request after)` of the last cURL import, so it can be undone while the
    /// request is still exactly what the import produced.
    pub import_undo: Option<(Request, Request)>,
}

/// What to do with a tab that has unsaved changes when the user closes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseChoice {
    Save,
    Discard,
    Cancel,
}

impl Tab {
    /// Cancels a request in flight and forgets the last response (it no longer matches the
    /// request in the tab).
    pub fn discard_response(&mut self) {
        if let Some(p) = self.pending.take() {
            p.handle.abort();
        }
        self.response = None;
    }
}

impl Drop for Tab {
    fn drop(&mut self) {
        if let Some(p) = &self.pending {
            p.handle.abort();
        }
    }
}

/// State of the Cmd/Ctrl+K command palette while it is open.
#[derive(Default)]
pub struct PaletteState {
    pub query: String,
    pub selected: usize,
    /// Set after the search box took focus once, so later frames don't pull focus back from
    /// whatever the user clicked.
    pub focused: bool,
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
    pub palette: Option<PaletteState>,
    /// Sidebar filter text.
    pub search: String,
    pub adding_collection: bool,
    /// Id of a tab with unsaved changes the user asked to close and has to confirm.
    pub confirm_close: Option<u64>,
    /// One-shot: focus the new-collection box on the next frame.
    pub focus_new_collection: bool,
    /// Theme currently applied to egui; `None` until the first frame.
    pub applied_theme: Option<ThemeMode>,
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
            palette: None,
            search: String::new(),
            adding_collection: false,
            confirm_close: None,
            focus_new_collection: false,
            applied_theme: None,
        };
        app.new_tab(Request::default(), None);
        if load_notice.is_some() {
            app.status = load_notice;
        } else if app.storage.is_none() {
            app.status = Some("No data directory found: collections and environments will not be saved".into());
        }
        app
    }

    /// True when the tab holds changes that are not in its saved request (or, for a tab that is
    /// not linked to one, anything that differs from how it was opened).
    pub fn is_tab_dirty(&self, idx: usize) -> bool {
        let Some(tab) = self.tabs.get(idx) else { return false };
        match tab
            .source
            .and_then(|(c, r)| self.store.collections.get(c).and_then(|c| c.requests.get(r)))
        {
            Some(saved) => *saved != tab.request,
            None => tab.request != tab.baseline,
        }
    }

    pub fn set_theme(&mut self, mode: ThemeMode) {
        if self.store.ui.theme != mode {
            self.store.ui.theme = mode;
            self.dirty = true;
        }
    }

    pub fn toggle_split(&mut self) {
        self.store.ui.split = self.store.ui.split.toggled();
        self.dirty = true;
    }

    /// Remembers a panel size once the user stops dragging (`size` changes while dragging).
    pub fn remember_sidebar_width(&mut self, width: f32) {
        remember(&mut self.store.ui.sidebar_width, width, &mut self.dirty);
    }

    pub fn remember_response_size(&mut self, split: SplitLayout, size: f32) {
        let slot = match split {
            SplitLayout::Below => &mut self.store.ui.response_height,
            SplitLayout::Side => &mut self.store.ui.response_width,
        };
        remember(slot, size, &mut self.dirty);
    }

    /// Pretty-prints the active tab's raw JSON body (keeping `{{variables}}`).
    pub fn beautify_body(&mut self, idx: usize) {
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        let msg = if tab.request.body_kind != BodyKind::Raw {
            "Switch the body to Raw to beautify it".to_string()
        } else if tab.request.body.trim().is_empty() {
            "Nothing to beautify: the body is empty".to_string()
        } else {
            match crate::json::beautify(&tab.request.body) {
                Ok(pretty) => {
                    let before = std::mem::replace(&mut tab.request.body, pretty.clone());
                    tab.beautify_undo = Some((before, pretty));
                    "Body beautified (Undo is available next to the button)".to_string()
                }
                Err(e) => format!("Cannot beautify: not valid JSON ({e})"),
            }
        };
        self.status = Some(msg);
    }

    /// Replaces the tab's request with the parsed `curl` command (keeping the request name) and
    /// remembers the old one for `undo_import`. Returns false, changing nothing, if the text
    /// can't be parsed.
    pub fn import_curl(&mut self, idx: usize, text: &str) -> bool {
        match crate::curl::parse(text) {
            Ok(imported) => self.apply_import(idx, imported),
            Err(e) => {
                self.status = Some(format!("Could not import cURL: {e}"));
                false
            }
        }
    }

    /// Applies an already-parsed import (see `import_curl`).
    pub fn apply_import(&mut self, idx: usize, imported: crate::curl::Imported) -> bool {
        let Some(tab) = self.tabs.get_mut(idx) else {
            return false;
        };
        let mut request = imported.request;
        request.name = tab.request.name.clone();
        let before = std::mem::replace(&mut tab.request, request);
        tab.import_undo = Some((before, tab.request.clone()));
        tab.beautify_undo = None;
        // A response (or one still on its way) belongs to the request that was just replaced.
        tab.discard_response();
        let mut msg = String::from("Imported cURL command");
        if !imported.warnings.is_empty() {
            msg.push_str(". Note: ");
            msg.push_str(&imported.warnings.join("; "));
        }
        self.status = Some(msg);
        true
    }

    /// Restores the request from before the last cURL import, unless it was edited since.
    pub fn undo_import(&mut self, idx: usize) {
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        self.status = Some(match tab.import_undo.take() {
            Some((before, after)) if tab.request == after => {
                tab.request = before;
                tab.discard_response();
                "cURL import undone".to_string()
            }
            _ => "Nothing to undo".to_string(),
        });
    }

    /// Restores the body from before the last Beautify, unless the user has edited it since.
    pub fn undo_beautify(&mut self, idx: usize) {
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        self.status = Some(match tab.beautify_undo.take() {
            Some((before, after)) if tab.request.body == after => {
                tab.request.body = before;
                "Beautify undone".to_string()
            }
            _ => "Nothing to undo".to_string(),
        });
    }

    /// Closes the tab, or asks first when it has unsaved changes (see `resolve_close`).
    pub fn request_close_tab(&mut self, idx: usize) {
        if self.is_tab_dirty(idx) {
            self.confirm_close = self.tabs.get(idx).map(|t| t.id);
        } else {
            self.close_tab(idx);
        }
    }

    /// Applies the user's answer to the pending close confirmation.
    pub fn resolve_close(&mut self, choice: CloseChoice) {
        let Some(id) = self.confirm_close.take() else { return };
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        match choice {
            CloseChoice::Save => {
                self.save_tab(idx);
                self.close_tab(idx);
            }
            CloseChoice::Discard => self.close_tab(idx),
            CloseChoice::Cancel => {}
        }
    }

    /// Copies the tab's request as a `curl` command (variables resolved).
    pub fn copy_curl(&mut self, idx: usize, ctx: &egui::Context) {
        let map = vars::to_map(self.store.active_vars());
        let Some(tab) = self.tabs.get(idx) else { return };
        self.status = Some(match http::prepare(&tab.request, &map) {
            Ok(prepared) => {
                ctx.copy_text(http::to_curl(&prepared));
                "Copied cURL command to the clipboard".to_string()
            }
            Err(e) => format!("Cannot build cURL: {e:#}"),
        });
    }

    pub fn run_command(&mut self, command: Command, ctx: &egui::Context) {
        let idx = self.active;
        match command {
            Command::Send => self.send(idx, ctx),
            Command::Save => self.save_tab(idx),
            Command::NewTab => self.new_tab(Request::default(), None),
            Command::CloseTab => self.request_close_tab(idx),
            Command::CopyCurl => self.copy_curl(idx, ctx),
            Command::BeautifyBody => self.beautify_body(idx),
            Command::CycleTheme => {
                let next = self.store.ui.theme.next();
                self.set_theme(next);
            }
            Command::ToggleSplit => self.toggle_split(),
            Command::ManageEnvironments => self.show_env = true,
            Command::ImportExport => self.show_io = true,
        }
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn new_tab(&mut self, request: Request, source: Option<(usize, usize)>) {
        let tab = Tab {
            id: self.next_id,
            baseline: request.clone(),
            request,
            source,
            response: None,
            pending: None,
            req_tab: ReqTab::Params,
            resp_tab: RespTab::Body,
            pretty: true,
            beautify_undo: None,
            import_undo: None,
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

/// Stores `value` in `slot` if it moved by more than a pixel (avoids rewriting the file for
/// sub-pixel layout noise).
fn remember(slot: &mut Option<f32>, value: f32, dirty: &mut bool) {
    if value.is_finite() && value > 0.0 && slot.is_none_or(|old| (old - value).abs() > 1.0) {
        *slot = Some(value);
        *dirty = true;
    }
}

fn default_export_path() -> String {
    directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(|p| p.join("postrust-collections.json")))
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "postrust-collections.json".into())
}

/// Cuts `s` to at most `limit` bytes on a char boundary; true if anything was removed.
fn truncate_at_boundary(s: &mut String, limit: usize) -> bool {
    if s.len() <= limit {
        return false;
    }
    let mut end = limit;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    true
}

/// True if the first non-space byte opens a JSON object/array (cheap, no parsing).
fn starts_like_json(text: &str) -> bool {
    matches!(text.trim_start().as_bytes().first(), Some(b'{' | b'['))
}

fn into_resp(data: ResponseData) -> Resp {
    let is_text = data.is_text();
    if !is_text {
        return Resp {
            data,
            is_text,
            display: String::new(),
            display_cut: false,
            is_json: false,
            raw: String::new(),
        };
    }
    let mut text = data.text();
    let mut cut = data.truncated;
    // Pretty-printing parses and re-serializes the whole body; skip it for very large ones,
    // which are still highlighted (cheap check) but shown exactly as received.
    let pretty = if data.body.len() <= PRETTY_LIMIT {
        pretty_json(&text)
    } else {
        None
    };
    let (display, raw, is_json) = match pretty {
        Some(mut pretty) => {
            cut |= truncate_at_boundary(&mut pretty, DISPLAY_LIMIT);
            cut |= truncate_at_boundary(&mut text, DISPLAY_LIMIT);
            (pretty, text, true)
        }
        None => {
            // Without a parse, trust a JSON Content-Type plus a leading `{`/`[` (so `[INFO] ...`
            // in a text/plain log is not coloured as JSON).
            let json_type = data
                .content_type()
                .is_some_and(|ct| ct.to_ascii_lowercase().contains("json"));
            let is_json = json_type && starts_like_json(&text);
            cut |= truncate_at_boundary(&mut text, DISPLAY_LIMIT);
            (text, String::new(), is_json)
        }
    };
    Resp {
        data,
        is_text,
        display,
        display_cut: cut,
        is_json,
        raw,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn response(content_type: &str, body: &[u8], truncated: bool) -> ResponseData {
        ResponseData {
            status: 200,
            reason: "OK".into(),
            elapsed_ms: 1,
            size: body.len(),
            truncated,
            headers: vec![("content-type".into(), content_type.into())],
            body: body.to_vec(),
        }
    }

    #[test]
    fn small_json_is_pretty_with_a_separate_raw_view() {
        let r = into_resp(response("application/json", br#"{"b":1,"a":[1,2]}"#, false));
        assert!(r.is_json && r.is_text);
        assert!(r.display.contains("\n  \"b\": 1"));
        assert_eq!(r.raw, r#"{"b":1,"a":[1,2]}"#);
        assert!(!r.display_cut);
    }

    #[test]
    fn non_json_text_has_no_raw_copy_or_highlighting() {
        let r = into_resp(response("text/html", b"<html>{\"a\":1}</html>", false));
        assert!(!r.is_json);
        assert!(r.raw.is_empty());
        assert_eq!(r.display, "<html>{\"a\":1}</html>");
    }

    #[test]
    fn invalid_json_that_looks_like_json_is_highlighted_but_not_reformatted() {
        let r = into_resp(response("application/json", b"{\"a\": ", false));
        assert!(r.is_json);
        assert!(r.raw.is_empty(), "no Pretty/Raw toggle without a pretty version");
        assert_eq!(r.display, "{\"a\": ");
    }

    #[test]
    fn huge_json_skips_pretty_printing_but_keeps_highlighting() {
        let body = format!("[{}0]", "0,".repeat(PRETTY_LIMIT / 2 + 10));
        let r = into_resp(response("application/json", body.as_bytes(), false));
        assert!(r.is_json);
        assert!(r.raw.is_empty());
        assert!(r.display_cut);
        assert!(r.display.len() <= DISPLAY_LIMIT);
        assert!(r.display.starts_with("[0,0,"), "shown as received, not re-formatted");
    }

    #[test]
    fn truncation_never_splits_a_multibyte_char() {
        let body = "é".repeat(DISPLAY_LIMIT); // 2 bytes each, so the cut falls mid-way for odd limits
        let mut s = body.clone();
        assert!(truncate_at_boundary(&mut s, DISPLAY_LIMIT - 1));
        assert!(s.len() < DISPLAY_LIMIT);
        assert!(s.chars().all(|c| c == 'é'));
        let mut short = "abc".to_string();
        assert!(!truncate_at_boundary(&mut short, 10));
    }

    #[test]
    fn binary_bodies_have_no_text_views() {
        let r = into_resp(response("application/octet-stream", &[0xff, 0xfe, 0x00], false));
        assert!(!r.is_text && !r.is_json);
        assert!(r.display.is_empty() && r.raw.is_empty());
    }

    #[test]
    fn json_looking_plain_text_and_scalars_are_not_json() {
        for (ct, body) in [
            ("text/plain", &b"[INFO] started"[..]),
            ("text/plain", b"42"),
            ("text/plain", b"true"),
        ] {
            let r = into_resp(response(ct, body, false));
            assert!(!r.is_json, "{body:?}");
            assert!(r.raw.is_empty());
        }
        // A genuinely valid object is JSON whatever the Content-Type says.
        assert!(into_resp(response("text/plain", br#"{"a":1}"#, false)).is_json);
    }

    #[test]
    fn raw_view_cut_is_reported_too() {
        let big = format!("[{}0]", "1,".repeat(DISPLAY_LIMIT)); // pretty is much bigger than raw
        let r = into_resp(response("application/json", big.as_bytes(), false));
        assert!(r.is_json && !r.raw.is_empty());
        assert!(r.display_cut);
        assert!(r.raw.len() <= DISPLAY_LIMIT);
    }

    #[test]
    fn truncated_downloads_are_flagged() {
        let r = into_resp(response("text/plain", b"hello", true));
        assert!(r.display_cut);
    }
}
