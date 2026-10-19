//! Everything that differs between the desktop app and the browser build, behind one
//! interface: running background work, persistent storage, and saving files.

//!
//! Both implementations provide the same surface, and the shared code relies on exactly this:
//! `Executor::{new, spawn}`, `Task::abort`, `run_blocking`, `default_export_path`, plus
//! `downloads_dir` (desktop only), and `download` / `BrowserBackend` (web only). The one
//! deliberate difference: a desktop task must be `Send`, a browser one need not be.

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::*;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::*;
