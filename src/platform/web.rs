//! Browser implementation: futures on the JS event loop, `localStorage`, and file downloads.

use crate::storage::Backend;
use anyhow::anyhow;
use std::cell::RefCell;
use std::future::Future;
use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};

/// Runs background work (HTTP requests) on the browser's event loop. There are no threads.
pub struct Executor;

impl Executor {
    pub fn new() -> Self {
        Self
    }

    pub fn spawn<F>(&self, future: F) -> Task
    where
        F: Future<Output = ()> + 'static,
    {
        let (abortable, handle) = futures_util::future::abortable(future);
        wasm_bindgen_futures::spawn_local(async move {
            let _ = abortable.await;
        });
        Task(handle)
    }
}

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}

/// A spawned task that can be cancelled.
pub struct Task(futures_util::future::AbortHandle);

impl Task {
    pub fn abort(&self) {
        self.0.abort();
    }
}

/// There is no thread to move work to in a browser, so it simply runs here. The work is bounded:
/// responses over 2 MB are not parsed or re-formatted, so this stays within tens of milliseconds.
pub async fn run_blocking<T, F>(work: F) -> Result<T, String>
where
    F: FnOnce() -> T,
{
    Ok(work())
}

/// Browsers can't read a typed file path; the Import window uses a text box instead.
pub fn default_export_path() -> String {
    String::new()
}

fn describe_js_error(value: &JsValue) -> String {
    if let Some(error) = value.dyn_ref::<js_sys::Error>() {
        return String::from(error.message());
    }
    value.as_string().unwrap_or_else(|| format!("{value:?}"))
}

// ---------------------------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------------------------

const STORE_KEY: &str = "postrust.store";
const BACKUP_KEY: &str = "postrust.store.bak";

/// Keeps the store in the page's `localStorage` (about 5 MB, plain text, per origin).
pub struct BrowserBackend {
    storage: web_sys::Storage,
    /// What this tab last read or wrote. If the stored text is different when saving, another
    /// tab has written in the meantime, and overwriting it would silently delete its work.
    /// Best effort: `localStorage` has no compare-and-set, so two tabs saving in the very same
    /// instant can still race; the check closes the common case (a stale tab saving later).
    last_seen: RefCell<Option<String>>,
}

impl BrowserBackend {
    /// `None` when the browser has no `localStorage` (or blocks it, as private modes sometimes do).
    pub fn new() -> Option<Self> {
        let storage = web_sys::window()?.local_storage().ok()??;
        Some(Self {
            storage,
            last_seen: RefCell::new(None),
        })
    }
}

impl Backend for BrowserBackend {
    fn read(&self) -> Result<Option<Vec<u8>>, String> {
        match self.storage.get_item(STORE_KEY) {
            Ok(value) => {
                *self.last_seen.borrow_mut() = value.clone();
                Ok(value.map(String::into_bytes))
            }
            Err(e) => Err(describe_js_error(&e)),
        }
    }

    fn write(&self, bytes: &[u8]) -> anyhow::Result<()> {
        let text = std::str::from_utf8(bytes)?;
        let current = self
            .storage
            .get_item(STORE_KEY)
            .map_err(|e| anyhow!("{}", describe_js_error(&e)))?;
        if current != *self.last_seen.borrow() {
            return Err(anyhow!(
                "another tab changed the data: reload this page (saving is paused)"
            ));
        }
        self.storage.set_item(STORE_KEY, text).map_err(|e| {
            anyhow!(
                "the browser refused to store the data (storage may be full or disabled): {}",
                describe_js_error(&e)
            )
        })?;
        *self.last_seen.borrow_mut() = Some(text.to_string());
        Ok(())
    }

    /// Copies the unparseable text to a backup key and removes the original.
    fn quarantine(&self) -> Result<String, String> {
        let data = self
            .storage
            .get_item(STORE_KEY)
            .map_err(|e| describe_js_error(&e))?
            .ok_or("nothing to move")?;
        let taken = self
            .storage
            .get_item(BACKUP_KEY)
            .map_err(|e| describe_js_error(&e))?
            .is_some();
        let key = if taken {
            format!("{BACKUP_KEY}.{}", js_sys::Date::now() as u64)
        } else {
            BACKUP_KEY.to_string()
        };
        self.storage.set_item(&key, &data).map_err(|e| describe_js_error(&e))?;
        self.storage.remove_item(STORE_KEY).map_err(|e| describe_js_error(&e))?;
        *self.last_seen.borrow_mut() = None;
        Ok(format!("browser storage key \"{key}\""))
    }

    fn describe(&self) -> String {
        "browser storage".into()
    }

    fn compact(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------------------------
// Downloads
// ---------------------------------------------------------------------------------------------

/// Offers `bytes` to the user as a file download.
pub fn download(file_name: &str, bytes: &[u8], mime: &str) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;

    let parts = js_sys::Array::new();
    parts.push(&js_sys::Uint8Array::from(bytes));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(mime);
    let blob =
        web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options).map_err(|e| describe_js_error(&e))?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(|e| describe_js_error(&e))?;

    let link: web_sys::HtmlAnchorElement = document
        .create_element("a")
        .map_err(|e| describe_js_error(&e))?
        .dyn_into()
        .map_err(|_| "could not create a download link")?;
    link.set_href(&url);
    link.set_download(file_name);
    link.click();

    // The browser needs the URL for a moment after the click; release it a little later.
    let release = Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(release.unchecked_ref(), 60_000);
    Ok(())
}
