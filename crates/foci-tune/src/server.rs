//! The service: Moonraker client, trace reader, run engine, store and HTTP server in one
//! process.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::broadcast;

use crate::api::{Api, router};
use crate::engine::{Engine, EngineConfig};
use crate::live::LiveState;
use crate::moonraker::{ClientConfig, MoonrakerApi, MoonrakerClient};
use crate::store::Store;
use crate::trace::run_trace_reader;

const DATA_DIR: &str = "foci-results";
const CONFIG_ROOT_ATTEMPTS: u32 = 30;
const CONFIG_ROOT_RETRY: Duration = Duration::from_secs(2);
/// About eight seconds of two channels at 1 kHz plus control and metadata frames, so a
/// capture task that stalls briefly does not lose frames.
const FRAME_CAPACITY: usize = 1 << 14;

/// Command-line settings of the service.
#[derive(Debug, Clone)]
pub struct ServeConfig {
    /// Moonraker base URL.
    pub moonraker_url: String,
    /// Optional Moonraker API key.
    pub api_key: Option<String>,
    /// Listen address of the API and UI.
    pub listen: String,
}

/// Why the service stopped or did not start.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// Moonraker never reported a `config` file root.
    #[error("moonraker reported no config root: {0}")]
    ConfigRoot(String),
    /// The data directory under the config root cannot be written.
    #[error("data directory {path} is not writable: {source}")]
    DataDir {
        /// The directory.
        path: PathBuf,
        /// The failure.
        source: io::Error,
    },
    /// The listen address cannot be bound.
    #[error("cannot listen on {addr}: {source}")]
    Listen {
        /// The address.
        addr: String,
        /// The failure.
        source: io::Error,
    },
    /// The HTTP server failed.
    #[error("http server failed: {0}")]
    Server(io::Error),
}

async fn config_root(moonraker: &MoonrakerClient) -> Result<PathBuf, ServeError> {
    let mut last = String::new();
    for _ in 0..CONFIG_ROOT_ATTEMPTS {
        match moonraker.config_root().await {
            Ok(root) => return Ok(root),
            Err(e) => {
                last = e.to_string();
                tracing::warn!(error = %e, "waiting for moonraker's config root");
            }
        }
        tokio::time::sleep(CONFIG_ROOT_RETRY).await;
    }
    Err(ServeError::ConfigRoot(last))
}

fn open_store(path: PathBuf) -> Result<Store, ServeError> {
    let probe = path.join(".foci-tune-write-check");
    let opened = Store::open(path.clone())
        .and_then(|store| std::fs::write(&probe, b"").map(|()| store))
        .and_then(|store| std::fs::remove_file(&probe).map(|()| store));
    opened.map_err(|source| ServeError::DataDir { path, source })
}

async fn shutdown_signal() {
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        () = terminate => {}
    }
}

/// Runs the service until SIGINT or SIGTERM.
///
/// # Errors
/// [`ServeError`] when Moonraker reports no config root, the data directory is not writable,
/// the address cannot be bound, or the server fails.
pub async fn serve(config: ServeConfig) -> Result<(), ServeError> {
    serve_with(config, |live, frames, stop| {
        tokio::spawn(run_trace_reader(live, frames, stop))
    })
    .await
}

/// [`serve`] with the trace reader started by `start_reader` instead of the USB reader.
pub async fn serve_with(
    config: ServeConfig,
    start_reader: impl FnOnce(
        Arc<Mutex<LiveState>>,
        broadcast::Sender<(String, Bytes)>,
        Arc<AtomicBool>,
    ) -> tokio::task::JoinHandle<()>,
) -> Result<(), ServeError> {
    let moonraker = Arc::new(MoonrakerClient::connect(ClientConfig::new(
        config.moonraker_url,
        config.api_key,
    )));
    let data = config_root(&moonraker).await?.join(DATA_DIR);
    let store = open_store(data.clone())?;
    let live = Arc::new(Mutex::new(LiveState::default()));
    let (frames, _) = broadcast::channel(FRAME_CAPACITY);
    let stop = Arc::new(AtomicBool::new(false));
    let reader = start_reader(live.clone(), frames.clone(), stop.clone());
    let engine = Engine::start(
        moonraker.clone(),
        live.clone(),
        frames.clone(),
        store.clone(),
        EngineConfig::default(),
    )
    .map_err(|source| ServeError::DataDir {
        path: data.clone(),
        source,
    })?;
    let app = router(Api {
        engine: Arc::new(engine),
        moonraker,
        live,
        frames,
        store,
    });
    let listener = tokio::net::TcpListener::bind(&config.listen)
        .await
        .map_err(|source| ServeError::Listen {
            addr: config.listen.clone(),
            source,
        })?;
    tracing::info!(listen = %config.listen, data = %data.display(), "foci-tune serving");
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(ServeError::Server);
    stop.store(true, Ordering::Relaxed);
    reader.abort();
    result
}
