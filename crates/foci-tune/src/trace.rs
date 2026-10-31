//! Trace interface readers: one blocking task per board, frames broadcast by serial.

use std::collections::{HashMap, HashSet};
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use bytes::Bytes;
use foci_trace::protocol::FrameAssembler;
use foci_trace::usb::{
    OpenTraceDevice, TRACE_INTERFACE_CLASS, TRACE_PRODUCT_ID, TRACE_VENDOR_ID, find_trace_devices,
};
use nusb::MaybeFuture;
use tokio::sync::broadcast;
use tracing::{debug, warn};

use crate::live::LiveState;

const TRANSFER_SIZE: usize = 4096;
const READ_TIMEOUT: Duration = Duration::from_millis(100);
const BACKOFF_START: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(5);
const WARN_INTERVAL: Duration = Duration::from_secs(60);

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
    let mut last_warned: HashMap<String, Instant> = HashMap::new();
    while !stop.load(Ordering::Relaxed) {
        let active_now = active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let scan = tokio::task::spawn_blocking(move || {
            let wanted = serials_to_open(
                &candidate_serials().map_err(|e| e.to_string())?,
                &active_now,
            );
            let mut opened = Vec::new();
            let mut failed = Vec::new();
            for serial in wanted {
                match find_trace_devices(std::slice::from_ref(&serial)) {
                    Ok(devices) => opened.extend(devices),
                    Err(e) => failed.push((serial, e.to_string())),
                }
            }
            Ok::<_, String>((opened, failed))
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
        match scan {
            Ok((devices, failed)) => {
                let now = Instant::now();
                for (serial, error) in failed {
                    let due = last_warned
                        .get(&serial)
                        .is_none_or(|at| now.duration_since(*at) >= WARN_INTERVAL);
                    if due {
                        warn!(%serial, %error, "cannot open trace interface");
                        last_warned.insert(serial, now);
                    }
                }
                if devices.is_empty() {
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                } else {
                    backoff = BACKOFF_START;
                    for device in devices {
                        last_warned.remove(&device.info.serial);
                        spawn_reader(device, &live, &tx, &stop, &active);
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "trace interface scan failed");
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
        tokio::time::sleep(backoff).await;
    }
}

/// Serials to open on a rescan: every candidate not currently being read, so a board that
/// was lost and a board never seen before are treated alike.
pub fn serials_to_open(candidates: &[String], active: &HashSet<String>) -> Vec<String> {
    let mut wanted: Vec<String> = candidates
        .iter()
        .filter(|serial| !active.contains(*serial))
        .cloned()
        .collect();
    wanted.sort();
    wanted.dedup();
    wanted
}

/// Serials of attached FOCI trace devices, listed without claiming any interface.
fn candidate_serials() -> Result<Vec<String>, nusb::Error> {
    Ok(nusb::list_devices()
        .wait()?
        .filter(|d| {
            d.vendor_id() == TRACE_VENDOR_ID
                && d.product_id() == TRACE_PRODUCT_ID
                && d.product_string().unwrap_or_default().starts_with("FOCI ")
                && d.interfaces().any(|i| i.class() == TRACE_INTERFACE_CLASS)
        })
        .map(|d| d.serial_number().unwrap_or_default().to_owned())
        .collect())
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
