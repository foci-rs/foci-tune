//! On-disk store: sessions, runs, series and captures under the data directory.
//!
//! Layout: `sessions/<session>/session.json`, `sessions/<session>/<run>/run.json`,
//! `series.json` and `capture.bin.zst`. JSON files are written to a temporary name and renamed.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use foci_tune_analysis::protocol::{Critical, Frame, FrameHeader, decode_frame};
use foci_tune_analysis::signals::channel_series;
use foci_tune_analysis::units::Regime;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::capture_read::read_capture;
pub use crate::model::new_id;
use crate::model::{ChannelPlot, FORMAT_VERSION, Run, RunId, RunState, Series, Session, SessionId};

const SESSIONS_DIR: &str = "sessions";
const SESSION_FILE: &str = "session.json";
const RUN_FILE: &str = "run.json";
const SERIES_FILE: &str = "series.json";
const CAPTURE_FILE: &str = "capture.bin.zst";
const SERIES_DECIMATION: u32 = 10;
const ID_RETRIES: usize = 8;

/// Why a stored document could not be produced.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The file system failed.
    #[error("store I/O error: {0}")]
    Io(#[from] io::Error),
    /// A stored document is not valid JSON of the expected shape.
    #[error("stored document is unreadable: {0}")]
    Json(#[from] serde_json::Error),
    /// The capture cannot be turned into a series; the reason is meant for display.
    #[error("capture cannot be decoded: {0}")]
    Capture(String),
}

/// A run directory as found on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSummary {
    /// Directory name.
    pub id: RunId,
    /// The run's state; `Failed` with a reason when `run.json` is missing or unreadable.
    pub state: RunState,
    /// The parsed `run.json`, absent for an incomplete directory.
    pub run: Option<Run>,
}

/// A session directory as found on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    /// Directory name.
    pub id: SessionId,
    /// The parsed `session.json`, absent when missing or unreadable.
    pub session: Option<Session>,
    /// Why `session.json` is absent.
    pub error: Option<String>,
    /// Runs in id (time) order.
    pub runs: Vec<RunSummary>,
}

/// Handle on the data directory.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

/// Serialises `value` and writes it to `path` through a temporary file and a rename.
///
/// The value is serialised before anything touches the disk, so a serializer error leaves
/// neither a temporary nor a partial target.
///
/// # Errors
/// Serialisation failures (as `InvalidData`) and any I/O error; the temporary file is removed
/// on failure.
pub fn write_json_atomic<T: Serialize + ?Sized>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let mut tmp_name = name.to_owned();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, StoreError> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

impl Store {
    /// Opens the store at `root`, creating `sessions/`.
    ///
    /// # Errors
    /// I/O errors from creating the directory.
    pub fn open(root: PathBuf) -> io::Result<Self> {
        std::fs::create_dir_all(root.join(SESSIONS_DIR))?;
        Ok(Self { root })
    }

    fn session_dir(&self, session: &SessionId) -> PathBuf {
        self.root.join(SESSIONS_DIR).join(session.as_str())
    }

    fn run_dir(&self, session: &SessionId, run: &RunId) -> PathBuf {
        self.session_dir(session).join(run.as_str())
    }

    /// Creates a session directory under a fresh id and writes `session.json`.
    ///
    /// # Errors
    /// I/O errors, or an id collision that survives several fresh suffixes.
    pub fn new_session(&self, s: &Session) -> io::Result<SessionId> {
        for _ in 0..ID_RETRIES {
            let id = SessionId::generate(SystemTime::now());
            match std::fs::create_dir(self.session_dir(&id)) {
                Ok(()) => {
                    write_json_atomic(&self.session_dir(&id).join(SESSION_FILE), s)?;
                    return Ok(id);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not find an unused session id",
        ))
    }

    /// Writes `run.json`, creating the run directory.
    ///
    /// # Errors
    /// See [`write_json_atomic`].
    pub fn write_run(&self, session: &SessionId, run: &Run) -> io::Result<()> {
        let dir = self.run_dir(session, &run.id);
        std::fs::create_dir_all(&dir)?;
        write_json_atomic(&dir.join(RUN_FILE), run)
    }

    /// Writes `series.json`, creating the run directory.
    ///
    /// # Errors
    /// See [`write_json_atomic`].
    pub fn write_series(&self, session: &SessionId, run: &RunId, s: &Series) -> io::Result<()> {
        let dir = self.run_dir(session, run);
        std::fs::create_dir_all(&dir)?;
        write_json_atomic(&dir.join(SERIES_FILE), s)
    }

    /// Where the run's capture lives.
    pub fn capture_path(&self, session: &SessionId, run: &RunId) -> PathBuf {
        self.run_dir(session, run).join(CAPTURE_FILE)
    }

    /// Lists every session, newest first, with its runs.
    ///
    /// A run directory without a readable `run.json` is listed as failed with the reason.
    ///
    /// # Errors
    /// I/O errors from listing the directories.
    pub fn scan(&self) -> io::Result<Vec<SessionSummary>> {
        let mut sessions = Vec::new();
        for id in child_dirs(&self.root.join(SESSIONS_DIR))? {
            let Ok(id) = SessionId::parse(&id) else {
                continue;
            };
            let (session, error) =
                match read_json::<Session>(&self.session_dir(&id).join(SESSION_FILE)) {
                    Ok(s) => (Some(s), None),
                    Err(e) => (None, Some(format!("session.json: {e}"))),
                };
            let mut runs = Vec::new();
            for run_id in child_dirs(&self.session_dir(&id))? {
                let Ok(run_id) = RunId::parse(&run_id) else {
                    continue;
                };
                runs.push(self.summarise_run(&id, run_id));
            }
            sessions.push(SessionSummary {
                id,
                session,
                error,
                runs,
            });
        }
        sessions.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(sessions)
    }

    fn summarise_run(&self, session: &SessionId, id: RunId) -> RunSummary {
        match read_json::<Run>(&self.run_dir(session, &id).join(RUN_FILE)) {
            Ok(run) => RunSummary {
                id,
                state: run.state.clone(),
                run: Some(run),
            },
            Err(e) => RunSummary {
                state: RunState::Failed(format!("incomplete run directory: run.json: {e}")),
                id,
                run: None,
            },
        }
    }

    /// The run's plot series, regenerated from the capture and persisted when `series.json` is
    /// missing, unreadable or from another format version.
    ///
    /// # Errors
    /// [`StoreError::Capture`] with a displayable reason when the capture cannot be decoded or
    /// the run records no regime; [`StoreError::Io`] and [`StoreError::Json`] otherwise.
    pub fn series(&self, session: &SessionId, run: &RunId) -> Result<Series, StoreError> {
        let dir = self.run_dir(session, run);
        match read_json::<Series>(&dir.join(SERIES_FILE)) {
            Ok(s) if s.format_version == FORMAT_VERSION => return Ok(s),
            Ok(_) | Err(StoreError::Json(_)) => {}
            Err(StoreError::Io(e)) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let record: Run = read_json(&dir.join(RUN_FILE))?;
        let regime = record.regime.as_ref().ok_or_else(|| {
            StoreError::Capture("run.json records no regime to rebuild the series from".into())
        })?;
        let series = build_series(&self.capture_path(session, run), &Regime::from(regime))?;
        self.write_series(session, run, &series)?;
        Ok(series)
    }

    /// Bytes available to the data directory's file system.
    ///
    /// # Errors
    /// The OS error from `statvfs`.
    pub fn free_bytes(&self) -> io::Result<u64> {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(self.root.as_os_str().as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        // SAFETY: `path` is a valid NUL-terminated string and `st` is a writable statvfs;
        // it is read only after statvfs reports success.
        let st = unsafe {
            let mut st = std::mem::MaybeUninit::<libc::statvfs>::uninit();
            if libc::statvfs(path.as_ptr(), st.as_mut_ptr()) != 0 {
                return Err(io::Error::last_os_error());
            }
            st.assume_init()
        };
        // The field widths differ across targets; widen them all to u64.
        #[allow(clippy::unnecessary_cast)]
        Ok(st.f_bavail as u64 * st.f_frsize as u64)
    }
}

fn child_dirs(dir: &Path) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && let Ok(name) = entry.file_name().into_string()
        {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

fn plot(v: &[f64], step: usize) -> Vec<Option<f64>> {
    v.iter()
        .step_by(step)
        .map(|&x| x.is_finite().then_some(x))
        .collect()
}

/// Decodes the capture at `path` and builds the decimated series for every channel that has
/// critical frames.
///
/// # Errors
/// [`StoreError::Capture`] when the file cannot be read or decompressed or holds no decodable
/// critical frame.
pub fn build_series(path: &Path, regime: &Regime) -> Result<Series, StoreError> {
    let capture = read_capture(path).map_err(|e| StoreError::Capture(e.to_string()))?;
    let mut by_channel: BTreeMap<u8, Vec<(FrameHeader, Critical)>> = BTreeMap::new();
    for record in capture.firmware_frames() {
        if let Ok(Frame::Critical(h, c)) = decode_frame(&record.payload) {
            by_channel.entry(h.source_index).or_default().push((h, c));
        }
    }
    if by_channel.is_empty() {
        return Err(StoreError::Capture(
            "capture holds no decodable critical frames".into(),
        ));
    }
    let step = SERIES_DECIMATION as usize;
    let channels = by_channel
        .into_iter()
        .map(|(channel, mut frames)| {
            frames.sort_by_key(|(h, _)| h.timestamp_ticks);
            let s = channel_series(&frames, regime);
            let torque: Vec<f64> = s.torque.iter().map(|&v| f64::from(v)).collect();
            let ff: Vec<f64> = s.ff_offset.iter().map(|&v| f64::from(v)).collect();
            ChannelPlot {
                channel,
                t_s: plot(&s.t_s, step),
                position_error_um: plot(&s.position_error_um, step),
                velocity_mm_s: plot(&s.velocity_mm_s, step),
                torque: plot(&torque, step),
                ff_offset: plot(&ff, step),
            }
        })
        .collect();
    Ok(Series {
        format_version: FORMAT_VERSION,
        decimation: SERIES_DECIMATION,
        channels,
    })
}
