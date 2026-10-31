//! Zstd-compressed capture files in the `foci-trace` format.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::JoinHandle;

use foci_trace::capture_file::CaptureWriter;
use foci_trace::clock::CaptureClock;
use foci_tune_analysis::protocol::{ControlState, Frame, decode_frame};
use structured_zstd::encoding::{CompressionLevel, StreamingEncoder};

type Encoder = StreamingEncoder<BufWriter<File>>;

const QUEUE_DEPTH: usize = 8192;

#[derive(Clone)]
struct SharedEncoder(Rc<RefCell<Option<Encoder>>>);

impl Write for SharedEncoder {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .borrow_mut()
            .as_mut()
            .ok_or_else(|| io::Error::other("capture already finished"))?
            .write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.borrow_mut().as_mut().map_or(Ok(()), Write::flush)
    }
}

/// What a finished capture holds.
#[derive(Debug, Clone)]
pub struct CaptureSummary {
    /// Where the capture was written.
    pub path: PathBuf,
    /// Firmware frames written.
    pub frames: u64,
    /// Uncompressed frame bytes written.
    pub frame_bytes: u64,
    /// Size of the compressed file.
    pub file_bytes: u64,
    /// First control state seen per channel (source index).
    pub first_control_state: HashMap<u8, ControlState>,
}

/// A capture file being written: a zstd frame holding a `foci-trace` capture.
///
/// The zstd encoder is not `Send`, so a worker thread owns it and `push` hands frames
/// over a bounded queue; the `Capture` itself can move between tasks.
pub struct Capture {
    path: PathBuf,
    frames_tx: mpsc::SyncSender<Vec<u8>>,
    worker: JoinHandle<io::Result<()>>,
    frames: u64,
    frame_bytes: u64,
    first_control_state: HashMap<u8, ControlState>,
}

impl Capture {
    /// Creates `path` (which must not exist) and writes the file header and the device
    /// table record for `serial`.
    ///
    /// # Errors
    ///
    /// I/O errors from creating or writing the file, or from the system clock.
    pub fn start(path: &Path, serial: &str) -> io::Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let (frames_tx, frames_rx) = mpsc::sync_channel(QUEUE_DEPTH);
        let (ready_tx, ready_rx) = mpsc::channel();
        let serial = serial.to_owned();
        let worker = std::thread::Builder::new()
            .name("capture-writer".into())
            .spawn(move || write_capture(file, &serial, &frames_rx, &ready_tx))?;
        ready_rx
            .recv()
            .map_err(|_| io::Error::other("capture writer exited during start"))??;
        Ok(Self {
            path: path.to_owned(),
            frames_tx,
            worker,
            frames: 0,
            frame_bytes: 0,
            first_control_state: HashMap::new(),
        })
    }

    /// Appends one assembled frame. A write failure ends the writer and is reported by
    /// [`Capture::finish`].
    pub fn push(&mut self, frame: &[u8]) {
        if self.frames_tx.send(frame.to_vec()).is_err() {
            return;
        }
        self.frames += 1;
        self.frame_bytes += frame.len() as u64;
        if let Ok(Frame::ControlState(h, state)) = decode_frame(frame) {
            self.first_control_state
                .entry(h.source_index)
                .or_insert(state);
        }
    }

    /// Ends the zstd frame and closes the file. Consumes the capture, so nothing can be
    /// pushed afterwards.
    ///
    /// # Errors
    ///
    /// The first write error hit by the writer, or an error finalizing the file.
    pub fn finish(self) -> io::Result<CaptureSummary> {
        drop(self.frames_tx);
        self.worker
            .join()
            .map_err(|_| io::Error::other("capture writer panicked"))??;
        Ok(CaptureSummary {
            file_bytes: std::fs::metadata(&self.path)?.len(),
            path: self.path,
            frames: self.frames,
            frame_bytes: self.frame_bytes,
            first_control_state: self.first_control_state,
        })
    }
}

fn write_capture(
    file: File,
    serial: &str,
    frames_rx: &mpsc::Receiver<Vec<u8>>,
    ready_tx: &mpsc::Sender<io::Result<()>>,
) -> io::Result<()> {
    let encoder = SharedEncoder(Rc::new(RefCell::new(Some(StreamingEncoder::new(
        BufWriter::new(file),
        CompressionLevel::Fastest,
    )))));
    let clock = CaptureClock::new().map_err(io::Error::other)?;
    let mut writer = match begin(&encoder, serial, &clock) {
        Ok(writer) => {
            let _ = ready_tx.send(Ok(()));
            writer
        }
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return Ok(());
        }
    };
    while let Ok(frame) = frames_rx.recv() {
        writer.write_firmware_frame(0, host_recv_ns(&clock)?, &frame)?;
    }
    writer.flush()?;
    let encoder = encoder
        .0
        .borrow_mut()
        .take()
        .ok_or_else(|| io::Error::other("capture already finished"))?;
    let mut out = encoder.finish()?;
    out.flush()
}

fn begin(
    encoder: &SharedEncoder,
    serial: &str,
    clock: &CaptureClock,
) -> io::Result<CaptureWriter<SharedEncoder>> {
    let mut writer = CaptureWriter::new(encoder.clone(), clock)?;
    writer.write_device_table(0, host_recv_ns(clock)?, "", "", serial, b"")?;
    Ok(writer)
}

fn host_recv_ns(clock: &CaptureClock) -> io::Result<u64> {
    clock.host_recv_ns().map_err(io::Error::other)
}
