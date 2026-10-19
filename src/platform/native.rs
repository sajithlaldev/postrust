//! Desktop implementation: a tokio runtime, files and the OS downloads folder.

use std::future::Future;
use std::path::PathBuf;

/// Runs background work (HTTP requests) off the UI thread.
pub struct Executor {
    rt: tokio::runtime::Runtime,
}

impl Executor {
    pub fn new() -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime");
        Self { rt }
    }

    pub fn spawn<F>(&self, future: F) -> Task
    where
        F: Future<Output = ()> + Send + 'static,
    {
        Task(self.rt.spawn(future))
    }
}

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}

/// A spawned task that can be cancelled.
pub struct Task(tokio::task::JoinHandle<()>);

impl Task {
    pub fn abort(&self) {
        self.0.abort();
    }
}

/// Runs CPU-heavy work (parsing a big response) without blocking the async workers.
pub async fn run_blocking<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(work).await.map_err(|e| e.to_string())
}

/// The user's Downloads folder (or the current directory if there is none).
pub fn downloads_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|u| u.download_dir().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Suggested path shown in the Import / Export window.
pub fn default_export_path() -> String {
    directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(|p| p.join("postrust-collections.json")))
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "postrust-collections.json".into())
}
