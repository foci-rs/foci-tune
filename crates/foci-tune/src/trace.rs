//! Trace interface readers: one blocking task per board, frames broadcast by serial.

use std::collections::HashSet;
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use bytes::Bytes;
use foci_trace::protocol::FrameAssembler;
use foci_trace::usb::{OpenTraceDevice, find_trace_devices};
use tokio::sync::broadcast;
use tracing::{debug, warn};

use crate::live::LiveState;

const TRANSFER_SIZE: usize = 4096;
const READ_TIMEOUT: Duration = Duration::from_millis(100);
const BACKOFF_START: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(5);

/// A source of raw trace bytes for one board.
pub trait FrameSource: Send {
    /// USB serial of the board.
    fn serial(&self) -> &str;

    /// Reads the next chunk into `buf`: `Ok(Some(n))` for data, `Ok(None)` when the read
    /// timed out with nothing to deliver.
    ///
    /// # Errors
    ///
    /// Any error means the interface is lost.
    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>>;
}

/// Why [`pump_source`] returned.
#[derive(Debug)]
pub enum PumpEnd {
    /// The stop flag was set.
    Stopped,
    /// The source failed; the interface is gone.
    Lost(io::Error),
}

/// Reads `source` until it fails or `stop` is set, updating `live` and broadcasting each
/// assembled frame as `(serial, frame)`.
///
/// Garbage bytes never end the loop: the assembler resyncs, and a header it cannot
/// parse drops its buffer. The board is marked open on entry and closed on exit.
pub fn pump_source(
    source: &mut dyn FrameSource,
    live: &Mutex<LiveState>,
    tx: &broadcast::Sender<(String, Bytes)>,
    stop: &AtomicBool,
) -> PumpEnd {
    let serial = source.serial().to_owned();
    lock(live).mark_open(&serial, Instant::now());
    let mut assembler = FrameAssembler::new();
    let mut buf = vec![0u8; TRANSFER_SIZE];
    let end = loop {
        if stop.load(Ordering::Relaxed) {
            break PumpEnd::Stopped;
        }
        let n = match source.read_chunk(&mut buf) {
            Ok(Some(n)) => n,
            Ok(None) => continue,
            Err(e) => break PumpEnd::Lost(e),
        };
        let frames = match assembler.push(&buf[..n]) {
            Ok(frames) => frames,
            Err(e) => {
                debug!(%serial, error = %e, "dropping unparseable trace bytes");
                assembler.clear();
                continue;
            }
        };
        if frames.is_empty() {
            continue;
        }
        let now = Instant::now();
        let mut state = lock(live);
        for frame in frames {
            state.ingest(&serial, &frame, now);
            let _ = tx.send((serial.clone(), frame));
        }
    };
    lock(live).mark_closed(&serial);
    end
}

fn lock(live: &Mutex<LiveState>) -> std::sync::MutexGuard<'_, LiveState> {
    live.lock().unwrap_or_else(PoisonError::into_inner)
}

struct UsbSource {
    serial: String,
    reader: Box<dyn Read + Send>,
    _device: OpenTraceDevice,
}

impl FrameSource for UsbSource {
    fn serial(&self) -> &str {
        &self.serial
    }

    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        match self.reader.read(buf) {
            Ok(n) => Ok(Some(n)),
            Err(e) if e.kind() == io::ErrorKind::TimedOut => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Discovers FOCI trace interfaces and keeps one reader task per board running until
/// `stop` is set, retrying lost interfaces with backoff.
pub async fn run_trace_reader(
    live: Arc<Mutex<LiveState>>,
    tx: broadcast::Sender<(String, Bytes)>,
    stop: Arc<AtomicBool>,
) {
    let active: Arc<Mutex<HashSet<String>>> = Arc::default();
    let mut backoff = BACKOFF_START;
    while !stop.load(Ordering::Relaxed) {
        let filter = scan_filter(&live, &active);
        let found = match filter {
            Some(filter) => tokio::task::spawn_blocking(move || find_trace_devices(&filter))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string())),
            None => Ok(Vec::new()),
        };
        match found {
            Ok(devices) if !devices.is_empty() => {
                backoff = BACKOFF_START;
                for device in devices {
                    spawn_reader(device, &live, &tx, &stop, &active);
                }
            }
            Ok(_) => backoff = (backoff * 2).min(BACKOFF_MAX),
            Err(e) => {
                warn!(error = %e, "trace interface scan failed");
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
        tokio::time::sleep(backoff).await;
    }
}

/// Serial filter for the next scan, `None` when there is nothing to look for.
///
/// Opening claims the interface, so a scan must not touch boards already being read:
/// with none active everything is scanned, otherwise only boards known and closed.
fn scan_filter(live: &Mutex<LiveState>, active: &Mutex<HashSet<String>>) -> Option<Vec<String>> {
    let active = active.lock().unwrap_or_else(PoisonError::into_inner);
    if active.is_empty() {
        return Some(Vec::new());
    }
    let closed: Vec<String> = lock(live)
        .boards
        .iter()
        .filter(|(serial, board)| !board.open && !active.contains(*serial))
        .map(|(serial, _)| serial.clone())
        .collect();
    (!closed.is_empty()).then_some(closed)
}

fn spawn_reader(
    device: OpenTraceDevice,
    live: &Arc<Mutex<LiveState>>,
    tx: &broadcast::Sender<(String, Bytes)>,
    stop: &Arc<AtomicBool>,
    active: &Arc<Mutex<HashSet<String>>>,
) {
    let serial = device.info.serial.clone();
    if !active
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(serial.clone())
    {
        return;
    }
    let (live, tx, stop, active) = (live.clone(), tx.clone(), stop.clone(), active.clone());
    tokio::task::spawn_blocking(move || {
        match device.bulk_reader(TRANSFER_SIZE, READ_TIMEOUT) {
            Ok(reader) => {
                let mut source = UsbSource {
                    serial: serial.clone(),
                    reader: Box::new(reader),
                    _device: device,
                };
                if let PumpEnd::Lost(e) = pump_source(&mut source, &live, &tx, &stop) {
                    warn!(%serial, error = %e, "trace interface lost");
                }
            }
            Err(e) => warn!(%serial, error = %e, "cannot open trace endpoint"),
        }
        active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&serial);
    });
}
