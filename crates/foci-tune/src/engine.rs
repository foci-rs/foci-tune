//! Run engine: a single-slot executor with a queue.
//!
//! One run is a fixed sequence: confirmed gain and filter writes, a forced control-state
//! baseline, the stimulus recorded on the trace stream, verification, analysis and storage.
//! A ladder is a queue of runs in interleaved order with a damping stop rule. Kill posts
//! Klipper's emergency stop; abort drains the queue after the current run.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::path::Path;
#[cfg(test)]
use std::sync::Condvar;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use foci_tune_analysis::ladder::{
    LadderAxis, LadderError, PID_GAIN_MAX_RAW, RungOutcome, StopDecision, interleave, probe_stop,
    to_raw,
};
use foci_tune_analysis::protocol::{ControlState, Frame, PROTOCOL_VERSION, decode_frame};
use foci_tune_analysis::signals::{i_gain_param, p_gain_param};
use foci_tune_analysis::units::Regime;
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Notify, broadcast, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout, timeout_at};

use crate::capture::{Capture, CaptureSummary};
use crate::live::{LiveState, TraceMode};
pub use crate::model::Lock;
use crate::model::{
    CaptureInfo, ChannelBaseline, ControlSnapshot, FORMAT_VERSION, Filters, Gains, LadderDef,
    ProbeRecord, Run, RunId, RunRegime, RunRequest, RunState, Session, SessionId, StepTime,
};
use crate::moonraker::{KlippyState, MoonrakerApi, PrinterState};
use crate::run_analysis::{AnalysisInput, analyze, decode_capture, stimulus_window};
use crate::steppers::{BedLimits, Stepper, derive_steppers};
use crate::stimulus::{RunKind, compose, feedforward_reset_line};
use crate::store::{Store, build_series};
use crate::verify_run::{Window, first_failure, verify};

const GAINS_REQUEST: u8 = 1;
const FILTER_REQUESTS: [(u8, &str); 4] =
    [(3, "velocity"), (4, "torque"), (5, "position"), (6, "flux")];
const CHANNEL_SOURCE_KIND: u8 = 1;
const EVENT_CAPACITY: usize = 1024;
const OUTSIDE_SHUTDOWN: &str =
    "Klipper shut down during the run (emergency stop from outside foci-tune)";
const OUTSIDE_ERROR: &str = "Klipper entered its error state during the run";
const RESTART_NOTE: &str = "Klipper was shut down; gains return to the config values and the \
                            trace mode to dormant after a firmware restart";

/// Timeouts and limits of the run sequence.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// How long a gain or filter write may take to show up in a control-state frame.
    pub confirm_timeout: Duration,
    /// How long the baseline frames may take after the trace mode command.
    pub baseline_timeout: Duration,
    /// How long the capture keeps recording after the stimulus returns.
    pub drain: Duration,
    /// Bound on the emergency stop request, and separately on the shutdown report after it.
    pub kill_timeout: Duration,
    /// Free space the data directory needs to start a run.
    pub min_free_bytes: u64,
    /// How often a run checks the trace interface between Moonraker state changes.
    pub loss_poll: Duration,
    /// How long a failed run waits for a Klipper shutdown report, which can trail the reply
    /// of the command the shutdown failed.
    pub shutdown_notice: Duration,
}

impl Default for EngineConfig {
    /// The spec's values: 2 s write and kill timeouts, 500 ms drain, 200 MB free; 1 s for a
    /// shutdown report.
    fn default() -> Self {
        Self {
            confirm_timeout: Duration::from_secs(2),
            baseline_timeout: Duration::from_secs(2),
            drain: Duration::from_millis(500),
            kill_timeout: Duration::from_secs(2),
            min_free_bytes: 200_000_000,
            loss_poll: Duration::from_millis(50),
            shutdown_notice: Duration::from_secs(1),
        }
    }
}

/// Why a run cannot start; the text is meant for display.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Precondition {
    /// Klipper is not in the ready state, or its config cannot be read.
    #[error("klipper is not ready")]
    KlippyNotReady,
    /// A print is running or paused.
    #[error("the printer is printing")]
    Printing,
    /// The stepper's axis is not homed.
    #[error("axis {0} is not homed")]
    NotHomed(char),
    /// The stepper's `foci` section has no identified electrical model.
    #[error("the stepper has no identified electrical model")]
    NoElectricalModel,
    /// The trace interface of the stepper's board is not open.
    #[error("the trace interface of the stepper's board is not open")]
    TraceNotOpen,
    /// The board speaks another trace protocol.
    #[error("the board speaks trace protocol {0}, this tool needs {PROTOCOL_VERSION}")]
    ProtocolVersion(u8),
    /// Too little free space on the data directory, in bytes.
    #[error("only {0} bytes free on the data directory")]
    LowDisk(u64),
    /// A run is in flight or queued, or a revert is being written.
    #[error("a run is in flight")]
    Busy,
    /// No such stepper in the stepper table.
    #[error("unknown stepper {0}")]
    UnknownStepper(String),
    /// The stepper is listed but unusable, with the table's reason.
    #[error("{0}")]
    StepperUnusable(String),
    /// The request names a session that is not in the store.
    #[error("unknown session {0}")]
    UnknownSession(String),
    /// The trace interface is open but no frame has arrived, so its protocol is unknown.
    #[error(
        "the stepper's board is open but has sent no trace frame yet; foci-tune is setting its \
         trace mode to wake it"
    )]
    NoTraceFrames,
    /// An emergency stop was posted and not confirmed.
    #[error(
        "the last emergency stop is unconfirmed and motion may continue; confirm the stop or \
         restart Klipper"
    )]
    KillUnconfirmed,
    /// The kill button was pressed while the request was being checked.
    #[error("the emergency stop was pressed while the run was being queued")]
    KillPressed,
    /// The stepper's gains must be restored before it runs again; carries the lock's reason.
    #[error("the stepper is locked until its starting gains are restored: {0}")]
    TuneLocked(String),
}

/// Most values a ladder may list.
pub const MAX_LADDER_VALUES: usize = 20;
/// Most repeats per ladder value.
pub const MAX_LADDER_REPEATS: usize = 10;
/// Most runs one ladder may queue, values times repeats.
pub const MAX_LADDER_RUNS: usize = 60;

/// Why a ladder was not queued; nothing is queued in any of these cases.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LadderRejected {
    /// A run precondition failed.
    #[error(transparent)]
    Precondition(#[from] Precondition),
    /// The axis is not `velocity_p`, `velocity_i_corner_hz` or `position_p`.
    #[error("unknown ladder axis {0}")]
    UnknownAxis(String),
    /// No values or no repeats.
    #[error("a ladder needs at least one value and one repeat")]
    Empty,
    /// The value list or the repeat count exceeds its ceiling.
    #[error("{parameter} must be at most {max}")]
    TooLarge {
        /// `values` or `repeats`.
        parameter: &'static str,
        /// The ceiling.
        max: usize,
    },
    /// Values times repeats exceeds [`MAX_LADDER_RUNS`].
    #[error("a ladder queues at most {max} runs; this one would queue {runs}")]
    TooManyRuns {
        /// Runs the ladder would queue.
        runs: usize,
        /// The ceiling.
        max: usize,
    },
    /// No control-state frame for the channel yet, so its PWM frequency is unknown.
    #[error("no control-state frame for the channel yet")]
    NoControlState,
    /// A value converts to a raw gain `FOCI_SET_GAINS` refuses; it carries the value.
    #[error(transparent)]
    OutOfRange(#[from] LadderError),
}

/// A run request with one gain axis varied over a value list.
#[derive(Debug, Clone, PartialEq)]
pub struct LadderRequest {
    /// The fixed part of every run.
    pub run: RunRequest,
    /// The varied axis, its values, repeats and stop thresholds.
    pub ladder: LadderDef,
}

/// What the engine reports to clients.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    /// A run changed state.
    State {
        /// Owning session.
        session: SessionId,
        /// The run.
        run: RunId,
        /// Its new state.
        state: RunState,
    },
    /// A run reached its final state; the record as stored.
    Finished(Box<Run>),
    /// The queued runs, in execution order.
    Queue(Vec<RunId>),
    /// A stepper's restore lock was removed from the store and memory.
    LockCleared {
        /// The stepper.
        stepper: String,
    },
}

/// A trace mode `FOCI_TRACE_MODE` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceModeCommand {
    /// No critical frames.
    Dormant,
    /// Critical frames.
    Critical,
    /// Diagnostic frames.
    Diagnostic,
}

impl TraceModeCommand {
    /// Parses the command's `MODE` value.
    pub fn parse(mode: &str) -> Option<Self> {
        match mode {
            "dormant" => Some(Self::Dormant),
            "critical" => Some(Self::Critical),
            "diagnostic" => Some(Self::Diagnostic),
            _ => None,
        }
    }

    /// The `MODE` value.
    pub fn name(self) -> &'static str {
        match self {
            Self::Dormant => "dormant",
            Self::Critical => "critical",
            Self::Diagnostic => "diagnostic",
        }
    }
}

/// Why an operator command was not carried out.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    /// The engine refused it before anything was sent.
    #[error(transparent)]
    Precondition(#[from] Precondition),
    /// Klipper or Moonraker refused the G-code; the text is their message.
    #[error("{0}")]
    Gcode(String),
    /// The board did not report its control state in time; the text says what was missing.
    #[error("{0}")]
    NoControlState(String),
}

/// Why a restore lock is not durable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LockError {
    /// The lock holds in memory, so runs stay refused, but would not survive a restart.
    #[error("the restore lock holds now but was not persisted: {0}")]
    NotDurable(String),
}

/// What an emergency stop achieved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillOutcome {
    /// The run that was in flight when the stop was posted.
    pub run: Option<RunId>,
    /// [`RunState::Killed`] when Klipper confirmed the shutdown, [`RunState::KillUnconfirmed`]
    /// otherwise.
    pub state: RunState,
}

/// The run in flight, the queue and an unconfirmed kill, at one instant.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EngineStatus {
    /// The run in flight: its session, id and state.
    pub current: Option<(SessionId, RunId, RunState)>,
    /// Queued runs in execution order.
    pub queue: Vec<RunId>,
    /// The run whose emergency stop is unconfirmed; it gates every new run.
    pub unconfirmed_kill: Option<RunId>,
}

#[derive(Debug, Clone, Copy)]
struct LadderSlot {
    ladder: u64,
    value: usize,
}

struct Queued {
    session: SessionId,
    run: Run,
    ladder: Option<LadderSlot>,
}

struct LadderTrack {
    probe: bool,
    repeats: usize,
    floor: f64,
    axis: String,
    values: Vec<f64>,
    outcomes: Vec<Vec<RungOutcome>>,
}

#[derive(Default)]
struct Inner {
    queue: VecDeque<Queued>,
    current: Option<Queued>,
    ladders: HashMap<u64, LadderTrack>,
    unconfirmed_kill: Option<(SessionId, RunId)>,
    restarts_at_kill: u64,
    writing: bool,
    locks: HashMap<String, HeldLock>,
}

/// A restore lock in force; `generation` tells its watcher whether the lock it was started
/// for is still the one held.
struct HeldLock {
    lock: Lock,
    generation: u64,
}

impl Inner {
    fn busy(&self) -> bool {
        self.current.is_some() || !self.queue.is_empty() || self.writing
    }

    /// The engine-wide preconditions, the same for every stepper: an unconfirmed kill and a
    /// run or write in flight.
    fn admission_failures(&self) -> Vec<Precondition> {
        let mut failing = Vec::new();
        if self.unconfirmed_kill.is_some() {
            failing.push(Precondition::KillUnconfirmed);
        }
        if self.busy() {
            failing.push(Precondition::Busy);
        }
        failing
    }

    /// [`Inner::admission_failures`] plus the stepper's restore lock: every route that queues a
    /// run on `stepper` goes through this.
    fn run_admission_failures(&self, stepper: &str) -> Vec<Precondition> {
        let mut failing = self.admission_failures();
        if let Some(held) = self.locks.get(stepper) {
            failing.push(Precondition::TuneLocked(held.lock.reason.clone()));
        }
        failing
    }
}

struct Resolved {
    printer: PrinterState,
    stepper: Stepper,
    steppers: Vec<Stepper>,
    limits: Option<BedLimits>,
}

enum Outcome {
    Recorded,
    Failed(String),
    Killed,
}

#[derive(Default)]
struct RunCtx {
    recorder: Option<Recorder>,
    prior_mode: Option<TraceMode>,
    critical_sent: bool,
    script_sent: bool,
    script_ok: bool,
}

struct Recorded {
    summary: CaptureSummary,
    lost: u64,
}

/// Feeds the run's board's frames into the capture on a blocking thread, since
/// [`Capture::push`] blocks when the writer falls behind.
struct Recorder {
    stop: watch::Sender<bool>,
    task: JoinHandle<io::Result<Recorded>>,
}

impl Recorder {
    fn start(
        path: &Path,
        serial: &str,
        mut rx: broadcast::Receiver<(String, Bytes)>,
    ) -> io::Result<Self> {
        let mut capture = Capture::start(path, serial)?;
        let (stop, mut stop_rx) = watch::channel(false);
        let serial = serial.to_owned();
        let handle = tokio::runtime::Handle::current();
        let task = tokio::task::spawn_blocking(move || {
            let mut lost = 0;
            loop {
                let next = handle.block_on(async {
                    tokio::select! {
                        biased;
                        r = rx.recv() => Some(r),
                        _ = stop_rx.wait_for(|stop| *stop) => None,
                    }
                });
                match next {
                    None | Some(Err(RecvError::Closed)) => break,
                    Some(Err(RecvError::Lagged(n))) => lost += n,
                    Some(Ok((from, frame))) if from == serial => capture.push(&frame),
                    Some(Ok(_)) => {}
                }
            }
            Ok(Recorded {
                summary: capture.finish()?,
                lost,
            })
        });
        Ok(Self { stop, task })
    }

    async fn finish(self) -> Result<Recorded, String> {
        let _ = self.stop.send(true);
        self.task
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn is_kill_state(state: &RunState) -> bool {
    matches!(
        state,
        RunState::Killing | RunState::KillUnconfirmed | RunState::Killed
    )
}

fn gain_param(raw: i32, format: fn(i32) -> String) -> String {
    if raw == 0 {
        "0".to_owned()
    } else {
        format(raw)
    }
}

fn gains_line(stepper: &str, g: &Gains) -> String {
    format!(
        "FOCI_SET_GAINS STEPPER={stepper} VELOCITY_P={} VELOCITY_I={} POSITION_P={} POSITION_I={}",
        gain_param(g.velocity_p, p_gain_param),
        gain_param(g.velocity_i, i_gain_param),
        gain_param(g.position_p, p_gain_param),
        gain_param(g.position_i, i_gain_param),
    )
}

fn filters_line(stepper: &str, f: &Filters) -> String {
    format!(
        "FOCI_SET_FILTERS STEPPER={stepper} VELOCITY_HZ={} TORQUE_HZ={} POSITION_HZ={} FLUX_HZ={}",
        f.velocity_hz, f.torque_hz, f.position_hz, f.flux_hz
    )
}

fn cutoff(c: &ControlState, index: usize) -> u16 {
    match index {
        0 => c.velocity_filter_hz,
        1 => c.torque_filter_hz,
        2 => c.position_filter_hz,
        _ => c.flux_filter_hz,
    }
}

fn filter_value(f: &Filters, index: usize) -> u16 {
    match index {
        0 => f.velocity_hz,
        1 => f.torque_hz,
        2 => f.position_hz,
        _ => f.flux_hz,
    }
}

/// Fails with the first of the four outer gains and four filter cutoffs that the forced
/// baseline does not carry as requested, so a write by another client is never run under.
fn baseline_matches(g: &Gains, f: &Filters, c: &ControlState) -> Result<(), String> {
    let gains = [
        ("velocity_p", g.velocity_p, c.velocity_p),
        ("velocity_i", g.velocity_i, c.velocity_i),
        ("position_p", g.position_p, c.position_p),
        ("position_i", g.position_i, c.position_i),
    ];
    let filters = [
        ("velocity_hz", f.velocity_hz, c.velocity_filter_hz),
        ("torque_hz", f.torque_hz, c.torque_filter_hz),
        ("position_hz", f.position_hz, c.position_filter_hz),
        ("flux_hz", f.flux_hz, c.flux_filter_hz),
    ];
    let differs = gains
        .into_iter()
        .map(|(name, want, got)| (name, i64::from(want), i64::from(got)))
        .chain(
            filters
                .into_iter()
                .map(|(name, want, got)| (name, i64::from(want), i64::from(got))),
        )
        .find(|(_, want, got)| want != got);
    match differs {
        None => Ok(()),
        Some((name, want, got)) => Err(format!(
            "baseline differs: {name} requested {want}, found {got}"
        )),
    }
}

fn check_gains(g: &Gains) -> Result<(), String> {
    let gains = [
        ("velocity_p", g.velocity_p),
        ("velocity_i", g.velocity_i),
        ("position_p", g.position_p),
        ("position_i", g.position_i),
    ];
    match gains
        .into_iter()
        .find(|(_, raw)| !(0..=PID_GAIN_MAX_RAW).contains(raw))
    {
        None => Ok(()),
        Some((name, raw)) => Err(format!(
            "{name} raw {raw} is outside 0..={PID_GAIN_MAX_RAW}"
        )),
    }
}

fn parse_axis(axis: &str) -> Result<LadderAxis, LadderRejected> {
    match axis {
        "velocity_p" => Ok(LadderAxis::VelocityP),
        "velocity_i_corner_hz" => Ok(LadderAxis::VelocityICornerHz),
        "position_p" => Ok(LadderAxis::PositionP),
        other => Err(LadderRejected::UnknownAxis(other.to_owned())),
    }
}

fn rung_outcome(run: &Run) -> RungOutcome {
    let zeta = run
        .metrics
        .as_ref()
        .and_then(|m| m.channels.iter().find(|c| c.declared))
        .and_then(|c| c.probe.as_ref())
        .and_then(ProbeRecord::headline_zeta);
    match (&run.state, zeta) {
        (RunState::Done, Some(z)) => RungOutcome::Assessed(z),
        _ => RungOutcome::Unassessable,
    }
}

fn baselines(first: &HashMap<u8, ControlState>) -> Vec<ChannelBaseline> {
    let mut out: Vec<ChannelBaseline> = first
        .iter()
        .map(|(&channel, c)| ChannelBaseline {
            channel,
            control: ControlSnapshot::from(c),
        })
        .collect();
    out.sort_by_key(|b| b.channel);
    out
}

/// Marks runs a crashed process left in a non-terminal state as `Failed("interrupted")`,
/// except `Killing`, which becomes `KillUnconfirmed`: the stop was posted and never confirmed.
/// [`Engine::start`] keeps new runs gated on such a run until the kill is cleared.
///
/// # Errors
/// I/O errors from scanning the store or rewriting a run.
pub fn reconcile_interrupted(store: &Store) -> io::Result<Vec<RunId>> {
    Ok(reconcile(store)?
        .into_iter()
        .map(|(_, run)| run.id)
        .collect())
}

fn reconcile(store: &Store) -> io::Result<Vec<(SessionId, Run)>> {
    let mut reconciled = Vec::new();
    for session in store.scan()? {
        for summary in session.runs {
            let Some(mut run) = summary.run else {
                continue;
            };
            if run.state.is_terminal() {
                continue;
            }
            run.state = match run.state {
                RunState::Killing => RunState::KillUnconfirmed,
                _ => RunState::Failed("interrupted".to_owned()),
            };
            store.write_run(&session.id, &run)?;
            reconciled.push((session.id.clone(), run));
        }
    }
    Ok(reconciled)
}

fn advance(run: &mut Run, state: RunState) {
    run.steps.push(StepTime {
        step: state.name().to_owned(),
        unix_ms: now_ms(),
    });
    run.state = state;
}

enum Job {
    Write(SessionId, Box<Run>),
    Event(EngineEvent),
    MarkKilled(SessionId, RunId),
    Flush(oneshot::Sender<()>),
}

struct Writer {
    store: Store,
    events: broadcast::Sender<EngineEvent>,
    #[cfg(test)]
    paused: Arc<(Mutex<bool>, Condvar)>,
}

impl Writer {
    fn handle(&self, job: Job) {
        #[cfg(test)]
        {
            let (paused, resumed) = &*self.paused;
            let mut p = lock(paused);
            while *p {
                p = resumed.wait(p).unwrap_or_else(PoisonError::into_inner);
            }
        }
        match job {
            Job::Write(session, run) => self.write(&session, &run),
            Job::Event(event) => {
                let _ = self.events.send(event);
            }
            Job::MarkKilled(session, id) => match self.store.read_run(&session, &id) {
                Ok(mut run) if run.state == RunState::KillUnconfirmed => {
                    advance(&mut run, RunState::Killed);
                    self.write(&session, &run);
                    let _ = self.events.send(EngineEvent::State {
                        session,
                        run: run.id.clone(),
                        state: run.state,
                    });
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(run = %id, error = %e, "cannot confirm the kill"),
            },
            Job::Flush(done) => {
                let _ = done.send(());
            }
        }
    }

    fn write(&self, session: &SessionId, run: &Run) {
        if let Err(e) = self.store.write_run(session, run) {
            tracing::warn!(run = %run.id, error = %e, "cannot write run.json");
        }
    }
}

/// Writes `run.json` and sends engine events in submission order, off the engine lock: a
/// stalled disk must not hold the lock the emergency stop needs.
struct Persister {
    jobs: std::sync::mpsc::Sender<Job>,
    pending: Arc<AtomicUsize>,
    #[cfg(test)]
    paused: Arc<(Mutex<bool>, Condvar)>,
}

impl Persister {
    fn start(store: Store, events: broadcast::Sender<EngineEvent>) -> io::Result<Self> {
        #[cfg(test)]
        let paused: Arc<(Mutex<bool>, Condvar)> = Arc::default();
        let writer = Writer {
            store,
            events,
            #[cfg(test)]
            paused: paused.clone(),
        };
        let pending = Arc::new(AtomicUsize::new(0));
        let done = pending.clone();
        let (jobs, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("run-writer".to_owned())
            .spawn(move || {
                for job in rx {
                    writer.handle(job);
                    done.fetch_sub(1, Ordering::SeqCst);
                }
            })?;
        Ok(Self {
            jobs,
            pending,
            #[cfg(test)]
            paused,
        })
    }

    fn send(&self, job: Job) {
        self.pending.fetch_add(1, Ordering::SeqCst);
        if self.jobs.send(job).is_err() {
            self.pending.fetch_sub(1, Ordering::SeqCst);
        }
    }

    fn idle(&self) -> bool {
        self.pending.load(Ordering::SeqCst) == 0
    }

    async fn flush(&self) {
        let (done, wait) = oneshot::channel();
        self.send(Job::Flush(done));
        let _ = wait.await;
    }

    #[cfg(test)]
    fn set_paused(&self, paused: bool) {
        let (flag, resumed) = &*self.paused;
        *lock(flag) = paused;
        resumed.notify_all();
    }
}

/// Klipper's `configfile.settings`, read once and kept until Klipper reports ready again.
#[derive(Default)]
struct SettingsCache {
    value: Option<Arc<Value>>,
    generation: u64,
}

struct Shared<M> {
    moonraker: Arc<M>,
    live: Arc<Mutex<LiveState>>,
    frames: broadcast::Sender<(String, Bytes)>,
    store: Store,
    persister: Persister,
    config: EngineConfig,
    inner: Mutex<Inner>,
    wake: Notify,
    events: broadcast::Sender<EngineEvent>,
    kill_requested: watch::Sender<u64>,
    kill_resolved: watch::Sender<u64>,
    next_ladder: AtomicU64,
    settings: Mutex<SettingsCache>,
    /// Serialises restore-lock changes with their file writes, apart from `inner`, so a slow
    /// disk never holds the lock the emergency stop needs.
    lock_io: Mutex<()>,
    lock_generation: AtomicU64,
    lock_watchers: Mutex<HashMap<String, JoinHandle<()>>>,
    runtime: tokio::runtime::Handle,
}

/// The run engine. Dropping it stops its tasks; a run in flight is abandoned.
pub struct Engine<M> {
    shared: Arc<Shared<M>>,
    worker: JoinHandle<()>,
    watcher: JoinHandle<()>,
}

impl<M> Drop for Engine<M> {
    fn drop(&mut self) {
        self.worker.abort();
        self.watcher.abort();
        for (_, task) in lock(&self.shared.lock_watchers).drain() {
            task.abort();
        }
    }
}

impl<M: MoonrakerApi + 'static> Engine<M> {
    /// Reconciles runs a previous process left in flight, reloads the persisted restore locks
    /// with a watcher each, then starts the worker.
    ///
    /// `frames` is the trace reader's broadcast of `(serial, frame)`; `live` its decoded
    /// state. Must be called inside a tokio runtime.
    ///
    /// # Errors
    /// I/O errors from reconciling the store, and an unreadable lock file: a lock whose
    /// snapshot is unknown can never be cleared, so the engine does not start without it.
    pub fn start(
        moonraker: Arc<M>,
        live: Arc<Mutex<LiveState>>,
        frames: broadcast::Sender<(String, Bytes)>,
        store: Store,
        config: EngineConfig,
    ) -> io::Result<Self> {
        let unconfirmed_kill = reconcile(&store)?
            .into_iter()
            .rfind(|(_, run)| run.state == RunState::KillUnconfirmed)
            .map(|(session, run)| (session, run.id));
        let locks = store.locks()?;
        let restarts_at_kill = *moonraker.klippy_restarts().borrow();
        let events = broadcast::channel(EVENT_CAPACITY).0;
        let persister = Persister::start(store.clone(), events.clone())?;
        let shared = Arc::new(Shared {
            moonraker,
            live,
            frames,
            store,
            persister,
            config,
            inner: Mutex::new(Inner {
                unconfirmed_kill,
                restarts_at_kill,
                ..Inner::default()
            }),
            wake: Notify::new(),
            events,
            kill_requested: watch::channel(0).0,
            kill_resolved: watch::channel(0).0,
            next_ladder: AtomicU64::new(0),
            settings: Mutex::default(),
            lock_io: Mutex::default(),
            lock_generation: AtomicU64::new(0),
            lock_watchers: Mutex::default(),
            runtime: tokio::runtime::Handle::current(),
        });
        for (stepper, held) in locks {
            shared.install_lock(&stepper, held);
        }
        let worker = tokio::spawn(shared.clone().work());
        let watcher = tokio::spawn(shared.clone().watch_klippy());
        Ok(Self {
            shared,
            worker,
            watcher,
        })
    }

    /// Queues one run after checking every precondition.
    ///
    /// # Errors
    /// The first failing [`Precondition`].
    pub async fn submit(&self, req: RunRequest) -> Result<RunId, Precondition> {
        let s = &self.shared;
        let (resolved, session, generation) = s.admit(&req).await?;
        let run = Shared::<M>::new_run(req, resolved.stepper.channel, &session);
        let id = run.id.clone();
        s.enqueue(vec![run], None, generation).await?;
        Ok(id)
    }

    /// Queues a ladder in interleaved order (A B C A B C), every value converted to raw with
    /// the channel's live PWM frequency and range-checked first.
    ///
    /// # Errors
    /// [`LadderRejected`]; an out-of-range value rejects the whole ladder.
    pub async fn submit_ladder(&self, req: LadderRequest) -> Result<Vec<RunId>, LadderRejected> {
        let s = &self.shared;
        let axis = parse_axis(&req.ladder.axis)?;
        if req.ladder.values.is_empty() || req.ladder.repeats == 0 {
            return Err(LadderRejected::Empty);
        }
        let (count, repeats) = (req.ladder.values.len(), req.ladder.repeats as usize);
        if count > MAX_LADDER_VALUES {
            return Err(LadderRejected::TooLarge {
                parameter: "values",
                max: MAX_LADDER_VALUES,
            });
        }
        if repeats > MAX_LADDER_REPEATS {
            return Err(LadderRejected::TooLarge {
                parameter: "repeats",
                max: MAX_LADDER_REPEATS,
            });
        }
        if count * repeats > MAX_LADDER_RUNS {
            return Err(LadderRejected::TooManyRuns {
                runs: count * repeats,
                max: MAX_LADDER_RUNS,
            });
        }
        let (resolved, session, generation) = s.admit(&req.run).await?;
        let pwm_hz = lock(&s.live)
            .boards
            .get(&resolved.stepper.serial)
            .and_then(|b| b.channels.get(&resolved.stepper.channel))
            .map(|c| c.pwm_hz)
            .ok_or(LadderRejected::NoControlState)?;
        let values = req
            .ladder
            .values
            .iter()
            .map(|&v| to_raw(axis, v, pwm_hz))
            .collect::<Result<Vec<_>, _>>()?;
        let ladder = s.next_ladder.fetch_add(1, Ordering::Relaxed);
        let repeats = req.ladder.repeats as usize;
        let mut runs = Vec::new();
        let mut slots = Vec::new();
        let mut ids: HashSet<RunId> = HashSet::new();
        for (value, _) in interleave(values.len(), repeats) {
            let mut run = req.run.clone();
            let raw = values[value].raw;
            match axis {
                LadderAxis::VelocityP => run.gains.velocity_p = raw,
                LadderAxis::VelocityICornerHz => run.gains.velocity_i = raw,
                LadderAxis::PositionP => run.gains.position_p = raw,
            }
            run.ladder_value = Some(values[value].shown);
            let mut run = Shared::<M>::new_run(run, resolved.stepper.channel, &session);
            while !ids.insert(run.id.clone()) {
                run.id = RunId::generate(SystemTime::now());
            }
            runs.push(run);
            slots.push(LadderSlot { ladder, value });
        }
        let track = LadderTrack {
            probe: matches!(req.run.kind, RunKind::Probe { .. }),
            repeats,
            floor: req.ladder.floor,
            axis: req.ladder.axis.clone(),
            values: values.iter().map(|v| v.shown).collect(),
            outcomes: vec![Vec::new(); values.len()],
        };
        let ids = runs.iter().map(|r| r.id.clone()).collect();
        s.enqueue(runs, Some((slots, ladder, track)), generation)
            .await?;
        Ok(ids)
    }

    /// Stops the queue after the current run: every queued run is marked `Aborted`.
    pub async fn abort(&self) {
        {
            let mut inner = lock(&self.shared.inner);
            self.shared.abort_queue(&mut inner);
        }
        self.shared.persister.flush().await;
    }

    /// Posts the emergency stop and empties the queue; the current run becomes `Killed` once
    /// the stop succeeds and Klipper reports shutdown, `KillUnconfirmed` otherwise.
    ///
    /// The work runs on its own task, so dropping this future does not leave a run in
    /// `Killing`.
    pub async fn kill(&self) -> KillOutcome {
        let shared = self.shared.clone();
        let (run, confirmed) = tokio::spawn(async move { shared.kill().await })
            .await
            .unwrap_or((None, false));
        KillOutcome {
            run,
            state: if confirmed {
                RunState::Killed
            } else {
                RunState::KillUnconfirmed
            },
        }
    }

    /// Checks every precondition for a run on `stepper` without queueing anything.
    ///
    /// # Errors
    /// The first failing [`Precondition`].
    pub async fn check(&self, stepper: &str) -> Result<(), Precondition> {
        match self.preconditions(stepper).await.into_iter().next() {
            Some(first) => Err(first),
            None => Ok(()),
        }
    }

    /// Every failing precondition for a run on `stepper`, in the order [`Engine::check`]
    /// reports them; empty when a run could start now.
    pub async fn preconditions(&self, stepper: &str) -> Vec<Precondition> {
        let mut failing = lock(&self.shared.inner).run_admission_failures(stepper);
        if let Err(run) = self.shared.run_failures(stepper).await {
            failing.extend(run);
        }
        failing
    }

    /// Forgets the stepper table, so the next use reads `configfile.settings` again. The
    /// engine does this itself whenever Klipper reports ready.
    pub fn refresh_steppers(&self) {
        self.shared.invalidate_settings();
    }

    /// The stepper table from the cached settings and the trace interfaces open now.
    ///
    /// # Errors
    /// [`Precondition::KlippyNotReady`] when the settings cannot be read.
    pub async fn steppers(&self) -> Result<Vec<Stepper>, Precondition> {
        let settings = self.shared.settings().await?;
        Ok(derive_steppers(&settings, &self.shared.open_serials()))
    }

    /// The run in flight, the queue and any unconfirmed kill.
    pub fn status(&self) -> EngineStatus {
        let inner = lock(&self.shared.inner);
        EngineStatus {
            current: inner
                .current
                .as_ref()
                .map(|c| (c.session.clone(), c.run.id.clone(), c.run.state.clone())),
            queue: inner.queue.iter().map(|q| q.run.id.clone()).collect(),
            unconfirmed_kill: inner.unconfirmed_kill.as_ref().map(|(_, id)| id.clone()),
        }
    }

    /// Writes the session's starting gains and filters through the confirmed write path.
    ///
    /// # Errors
    /// The reason meant for display: busy, a precondition, a G-code error or an unconfirmed
    /// write.
    pub async fn revert(&self, session: &SessionId) -> Result<(), String> {
        let s = &self.shared;
        let record = s
            .store
            .read_session(session)
            .map_err(|e| format!("session {session}: {e}"))?;
        {
            let mut inner = lock(&s.inner);
            if let Some(first) = inner.admission_failures().into_iter().next() {
                return Err(first.to_string());
            }
            inner.writing = true;
        }
        let _guard = WriteGuard(&s.inner);
        let resolved = s
            .resolve(&record.stepper)
            .await
            .map_err(|p| p.to_string())?;
        s.gcode("M400").await?;
        s.write_gains(&resolved.stepper, &record.gains).await?;
        s.write_filters(&resolved.stepper, &record.filters).await
    }

    /// Sends `FOCI_TRACE_MODE` for `stepper` through the engine's G-code path, refused while a
    /// run or another write is in flight or a kill is unconfirmed, and holding off new runs
    /// until it returns.
    ///
    /// # Errors
    /// [`CommandError::Precondition`] when refused, [`CommandError::Gcode`] when Klipper
    /// rejects the command.
    pub async fn set_trace_mode(
        &self,
        stepper: &str,
        mode: TraceModeCommand,
    ) -> Result<(), CommandError> {
        let s = &self.shared;
        {
            let mut inner = lock(&s.inner);
            if let Some(first) = inner.admission_failures().into_iter().next() {
                return Err(first.into());
            }
            inner.writing = true;
        }
        let _guard = WriteGuard(&s.inner);
        s.gcode(&format!(
            "FOCI_TRACE_MODE STEPPER={stepper} MODE={}",
            mode.name()
        ))
        .await
        .map_err(CommandError::Gcode)
    }

    /// Sets `stepper`'s board to the dormant trace mode so a silent board reports its
    /// metadata and control state; refused like [`Engine::set_trace_mode`], and also while
    /// Klipper is not ready or an emergency stop is being posted. The command is bounded by
    /// the confirm timeout.
    ///
    /// # Errors
    /// [`CommandError::Precondition`] when refused, [`CommandError::Gcode`] when Klipper
    /// rejects the command or it times out.
    pub async fn wake_trace(&self, stepper: &str) -> Result<(), CommandError> {
        let s = &self.shared;
        if s.moonraker.state().borrow().klippy != KlippyState::Ready {
            return Err(Precondition::KlippyNotReady.into());
        }
        {
            let mut inner = lock(&s.inner);
            if *s.kill_requested.borrow() != *s.kill_resolved.borrow() {
                return Err(Precondition::KillPressed.into());
            }
            if let Some(first) = inner.admission_failures().into_iter().next() {
                return Err(first.into());
            }
            inner.writing = true;
        }
        let _guard = WriteGuard(&s.inner);
        s.gcode_bounded(&format!(
            "FOCI_TRACE_MODE STEPPER={stepper} MODE={}",
            TraceModeCommand::Dormant.name()
        ))
        .await
        .map_err(CommandError::Gcode)
    }

    /// Requests a fresh control-state frame of `stepper`'s channel the way a run's baseline
    /// step does, by setting the critical trace mode, then sets a board known dormant back to
    /// dormant. Refused while a run or another write is in flight or queued, a kill is
    /// unconfirmed or an emergency stop is being posted.
    ///
    /// # Errors
    /// [`CommandError::Precondition`] when refused, [`CommandError::Gcode`] when Klipper
    /// rejects the mode command, [`CommandError::NoControlState`] when no frame arrives.
    pub async fn read_control_state(&self, stepper: &str) -> Result<ControlSnapshot, CommandError> {
        let s = &self.shared;
        let _guard = s.begin_write()?;
        let resolved = s.resolve(stepper).await?;
        let rx = s.frames.subscribe();
        let prior = s.trace_mode(&resolved.stepper.serial);
        s.gcode(&format!(
            "FOCI_TRACE_MODE STEPPER={} MODE=critical",
            resolved.stepper.name
        ))
        .await
        .map_err(CommandError::Gcode)?;
        let found = s.wait_baseline(rx, &resolved.stepper).await;
        if prior == TraceMode::Dormant {
            let line = format!(
                "FOCI_TRACE_MODE STEPPER={} MODE=dormant",
                resolved.stepper.name
            );
            if let Err(e) = s.gcode_bounded(&line).await {
                tracing::warn!(stepper, error = %e, "trace mode not restored to dormant");
            }
        }
        found
            .map(|c| ControlSnapshot::from(&c))
            .map_err(CommandError::NoControlState)
    }

    /// Writes `gains` to `stepper` through the confirmed write path after range-checking every
    /// raw value; refused like [`Engine::read_control_state`].
    ///
    /// # Errors
    /// The reason meant for display: a raw value out of range, a refusal, a G-code error or an
    /// unconfirmed write.
    pub async fn apply_gains(&self, stepper: &str, gains: Gains) -> Result<(), String> {
        check_gains(&gains)?;
        let s = &self.shared;
        let _guard = s.begin_write().map_err(|p| p.to_string())?;
        let resolved = s.resolve(stepper).await.map_err(|p| p.to_string())?;
        s.gcode("M400").await?;
        s.write_gains(&resolved.stepper, &gains).await
    }

    /// Writes `gains` and `filters` to `stepper` through the confirmed write path, like
    /// [`Engine::apply_gains`]. When both are confirmed and equal the snapshot of `stepper`'s
    /// restore lock, the lock is removed from the store and then from memory. A write that
    /// fails or is unconfirmed locks `stepper` with this snapshot before new runs are admitted
    /// again, unless it is already locked with another snapshot.
    ///
    /// Returns true when the write is confirmed and `stepper` is not locked, false when it is
    /// confirmed but the lock stays (another snapshot, or its file could not be removed).
    ///
    /// # Errors
    /// The reason meant for display, as for [`Engine::apply_gains`]; a lock taken after a
    /// failed write that could not be persisted is added to it.
    pub async fn restore_snapshot(
        &self,
        stepper: &str,
        gains: Gains,
        filters: Filters,
    ) -> Result<bool, String> {
        check_gains(&gains)?;
        let s = &self.shared;
        let _guard = s.begin_write().map_err(|p| p.to_string())?;
        let resolved = s.resolve(stepper).await.map_err(|p| p.to_string())?;
        let written = async {
            s.gcode("M400").await?;
            s.write_gains(&resolved.stepper, &gains).await?;
            s.write_filters(&resolved.stepper, &filters).await
        }
        .await;
        if let Err(e) = written {
            return Err(s.lock_failed_restore(stepper, (gains, filters), e));
        }
        let snapshot = (gains, filters);
        match s.clear_lock(stepper, |held| held.lock.snapshot == snapshot) {
            Ok(_) => Ok(!lock(&s.inner).locks.contains_key(stepper)),
            Err(e) => {
                tracing::warn!(stepper, error = %e, "restore confirmed, lock file not removed");
                Ok(false)
            }
        }
    }

    /// Locks `stepper` against every run until `snapshot` is restored or reported by the
    /// board. The lock takes effect in memory before it is persisted and stays in effect when
    /// persisting fails.
    ///
    /// # Errors
    /// [`LockError::NotDurable`] when the lock file was not written.
    pub fn lock_stepper(
        &self,
        stepper: &str,
        reason: String,
        snapshot: (Gains, Filters),
    ) -> Result<(), LockError> {
        self.shared.lock_stepper(stepper, Lock { reason, snapshot })
    }

    /// `stepper`'s restore lock, if it has one.
    pub fn lock_state(&self, stepper: &str) -> Option<Lock> {
        lock(&self.shared.inner)
            .locks
            .get(stepper)
            .map(|held| held.lock.clone())
    }

    /// Subscribes to run state changes, results, queue updates and cleared restore locks.
    pub fn events(&self) -> broadcast::Receiver<EngineEvent> {
        self.shared.events.subscribe()
    }

    /// True when no run is in flight or queued, no revert is being written and every run
    /// record is on disk.
    pub fn is_idle(&self) -> bool {
        !lock(&self.shared.inner).busy() && self.shared.persister.idle()
    }
}

struct WriteGuard<'a>(&'a Mutex<Inner>);

impl Drop for WriteGuard<'_> {
    fn drop(&mut self) {
        lock(self.0).writing = false;
    }
}

impl<M: MoonrakerApi + 'static> Shared<M> {
    /// Admits a write outside a run, holding off new runs until the guard drops.
    fn begin_write(&self) -> Result<WriteGuard<'_>, Precondition> {
        let mut inner = lock(&self.inner);
        if *self.kill_requested.borrow() != *self.kill_resolved.borrow() {
            return Err(Precondition::KillPressed);
        }
        if let Some(first) = inner.admission_failures().into_iter().next() {
            return Err(first);
        }
        inner.writing = true;
        Ok(WriteGuard(&self.inner))
    }

    fn lock_stepper(self: &Arc<Self>, stepper: &str, held: Lock) -> Result<(), LockError> {
        let _io = lock(&self.lock_io);
        self.install_lock(stepper, held.clone());
        self.store
            .write_lock(stepper, &held)
            .map_err(|e| LockError::NotDurable(e.to_string()))
    }

    /// Puts `held` in force in memory and starts its watcher on a receiver subscribed after
    /// the lock is in place, so no control-state frame sent before it can clear it.
    fn install_lock(self: &Arc<Self>, stepper: &str, held: Lock) {
        let generation = self.lock_generation.fetch_add(1, Ordering::Relaxed);
        let rx = {
            let mut inner = lock(&self.inner);
            inner.locks.insert(
                stepper.to_owned(),
                HeldLock {
                    lock: held,
                    generation,
                },
            );
            self.frames.subscribe()
        };
        let task = self
            .runtime
            .spawn(self.clone().watch_lock(stepper.to_owned(), generation, rx));
        if let Some(old) = lock(&self.lock_watchers).insert(stepper.to_owned(), task) {
            old.abort();
        }
    }

    /// Locks `stepper` after a restore of `snapshot` failed with `error`, keeping a lock on
    /// another snapshot, which only that snapshot may clear; returns the error for display.
    fn lock_failed_restore(
        self: &Arc<Self>,
        stepper: &str,
        snapshot: (Gains, Filters),
        error: String,
    ) -> String {
        let other = lock(&self.inner)
            .locks
            .get(stepper)
            .is_some_and(|held| held.lock.snapshot != snapshot);
        if other {
            return error;
        }
        let held = Lock {
            reason: format!("restore failed: {error}"),
            snapshot,
        };
        match self.lock_stepper(stepper, held) {
            Ok(()) => error,
            Err(e) => format!("{error}; {e}"),
        }
    }

    /// Clears `stepper`'s restore lock when `matches` accepts it: the file first, then memory,
    /// so a failed removal leaves the lock in force. Returns whether it cleared one.
    fn clear_lock(&self, stepper: &str, matches: impl Fn(&HeldLock) -> bool) -> io::Result<bool> {
        let _io = lock(&self.lock_io);
        if !lock(&self.inner).locks.get(stepper).is_some_and(matches) {
            return Ok(false);
        }
        self.store.remove_lock(stepper)?;
        lock(&self.inner).locks.remove(stepper);
        if let Some(task) = lock(&self.lock_watchers).remove(stepper) {
            task.abort();
        }
        self.emit(EngineEvent::LockCleared {
            stepper: stepper.to_owned(),
        });
        Ok(true)
    }

    /// Clears lock `generation` of `stepper` when a control-state frame of its channel
    /// carries the lock's snapshot; returns once that lock is no longer held. Channel
    /// positions are remembered so a frame that arrives while Klipper's config is unreadable,
    /// as during a restart, is still matched.
    async fn watch_lock(
        self: Arc<Self>,
        stepper: String,
        generation: u64,
        mut rx: broadcast::Receiver<(String, Bytes)>,
    ) {
        let mut position: Option<(String, u8)> = None;
        loop {
            let (serial, frame) = match rx.recv().await {
                Ok(message) => message,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            };
            let Ok(Frame::ControlState(h, c)) = decode_frame(&frame) else {
                continue;
            };
            let snapshot = match lock(&self.inner).locks.get(&stepper) {
                Some(held) if held.generation == generation => held.lock.snapshot,
                _ => return,
            };
            if let Ok(settings) = self.settings().await
                && let Some(s) = derive_steppers(&settings, &self.open_serials())
                    .into_iter()
                    .find(|s| s.name == stepper)
            {
                position = Some((s.serial, s.channel));
            }
            let (gains, filters) = &snapshot;
            if position.as_ref() != Some(&(serial, h.source_index))
                || baseline_matches(gains, filters, &c).is_err()
            {
                continue;
            }
            match self.clear_lock(&stepper, |held| held.generation == generation) {
                Ok(_) => return,
                Err(e) => tracing::warn!(%stepper, error = %e, "restore lock not removed"),
            }
        }
    }

    fn persist(&self, session: &SessionId, run: &Run) {
        self.persister
            .send(Job::Write(session.clone(), Box::new(run.clone())));
    }

    fn emit(&self, event: EngineEvent) {
        self.persister.send(Job::Event(event));
    }

    fn publish(&self, session: &SessionId, run: &Run) {
        self.persist(session, run);
        self.emit(EngineEvent::State {
            session: session.clone(),
            run: run.id.clone(),
            state: run.state.clone(),
        });
    }

    fn transition(&self, q: &mut Queued, state: RunState) {
        advance(&mut q.run, state);
        self.publish(&q.session, &q.run);
    }

    fn publish_queue(&self, inner: &Inner) {
        let ids = inner.queue.iter().map(|q| q.run.id.clone()).collect();
        self.emit(EngineEvent::Queue(ids));
    }

    /// Moves the current run to `state` unless a kill owns its state.
    fn set_state(&self, state: RunState) {
        let mut inner = lock(&self.inner);
        if let Some(current) = inner.current.as_mut()
            && !is_kill_state(&current.run.state)
        {
            self.transition(current, state);
        }
    }

    fn update_run(&self, f: impl FnOnce(&mut Run)) {
        let mut inner = lock(&self.inner);
        if let Some(current) = inner.current.as_mut() {
            f(&mut current.run);
            self.persist(&current.session, &current.run);
        }
    }

    fn note(&self, note: String) {
        self.update_run(|run| run.notes.push(note));
    }

    fn kill_engaged(&self) -> bool {
        lock(&self.inner)
            .current
            .as_ref()
            .is_some_and(|c| is_kill_state(&c.run.state))
    }

    fn abort_queue(&self, inner: &mut Inner) {
        self.skip_queue(inner, &RunState::Aborted);
    }

    fn skip_queue(&self, inner: &mut Inner, state: &RunState) {
        for mut q in inner.queue.drain(..).collect::<Vec<_>>() {
            self.transition(&mut q, state.clone());
        }
        inner.ladders.clear();
        self.publish_queue(inner);
    }

    /// Checks a request; also returns the kill generation sampled first, so `enqueue` can
    /// refuse a request that a kill overtook while it was being checked.
    async fn admit(&self, req: &RunRequest) -> Result<(Resolved, Session, u64), Precondition> {
        let generation = *self.kill_requested.borrow();
        if let Some(first) = lock(&self.inner)
            .run_admission_failures(&req.stepper)
            .into_iter()
            .next()
        {
            return Err(first);
        }
        let session = self
            .store
            .read_session(&req.session)
            .map_err(|_| Precondition::UnknownSession(req.session.to_string()))?;
        Ok((self.check_run(&req.stepper).await?, session, generation))
    }

    fn new_run(mut request: RunRequest, channel: u8, session: &Session) -> Run {
        request.gains.position_i = session.gains.position_i;
        let now = SystemTime::now();
        Run {
            format_version: FORMAT_VERSION,
            id: RunId::generate(now),
            request,
            state: RunState::Queued,
            created_unix_s: now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
            declared_channel: channel,
            regime: None,
            baselines: Vec::new(),
            stream: None,
            steps: vec![StepTime {
                step: RunState::Queued.name().to_owned(),
                unix_ms: now_ms(),
            }],
            clamped_speeds: Vec::new(),
            applied_speeds: Vec::new(),
            capture: None,
            verification: Vec::new(),
            metrics: None,
            notes: Vec::new(),
        }
    }

    async fn enqueue(
        &self,
        runs: Vec<Run>,
        ladder: Option<(Vec<LadderSlot>, u64, LadderTrack)>,
        generation: u64,
    ) -> Result<(), Precondition> {
        self.queue_runs(runs, ladder, generation)?;
        self.persister.flush().await;
        self.wake.notify_one();
        Ok(())
    }

    fn queue_runs(
        &self,
        runs: Vec<Run>,
        ladder: Option<(Vec<LadderSlot>, u64, LadderTrack)>,
        generation: u64,
    ) -> Result<(), Precondition> {
        let mut inner = lock(&self.inner);
        let admission = runs.first().map_or_else(
            || inner.admission_failures(),
            |r| inner.run_admission_failures(&r.request.stepper),
        );
        if admission.contains(&Precondition::KillUnconfirmed) {
            return Err(Precondition::KillUnconfirmed);
        }
        if *self.kill_requested.borrow() != generation {
            return Err(Precondition::KillPressed);
        }
        if let Some(first) = admission.into_iter().next() {
            return Err(first);
        }
        let slots = match ladder {
            Some((slots, id, track)) => {
                inner.ladders.insert(id, track);
                slots
            }
            None => Vec::new(),
        };
        for (k, run) in runs.into_iter().enumerate() {
            let q = Queued {
                session: run.request.session.clone(),
                run,
                ladder: slots.get(k).copied(),
            };
            self.publish(&q.session, &q.run);
            inner.queue.push_back(q);
        }
        self.publish_queue(&inner);
        Ok(())
    }

    fn invalidate_settings(&self) {
        let mut cache = lock(&self.settings);
        cache.value = None;
        cache.generation += 1;
    }

    async fn settings(&self) -> Result<Arc<Value>, Precondition> {
        let generation = {
            let cache = lock(&self.settings);
            if let Some(value) = &cache.value {
                return Ok(value.clone());
            }
            cache.generation
        };
        let value = Arc::new(
            self.moonraker
                .settings()
                .await
                .map_err(|_| Precondition::KlippyNotReady)?,
        );
        let mut cache = lock(&self.settings);
        if cache.generation == generation {
            cache.value = Some(value.clone());
        }
        Ok(value)
    }

    fn open_serials(&self) -> Vec<String> {
        lock(&self.live)
            .boards
            .iter()
            .filter(|(_, b)| b.open)
            .map(|(serial, _)| serial.clone())
            .collect()
    }

    /// The stepper and the table it comes from; fails when Klipper is not ready or the stepper
    /// is not in the table, after which nothing else about it can be checked.
    async fn lookup(&self, stepper: &str) -> Result<Resolved, Precondition> {
        let printer = self.moonraker.state().borrow().clone();
        if printer.klippy != KlippyState::Ready {
            return Err(Precondition::KlippyNotReady);
        }
        let settings = self.settings().await?;
        let steppers = derive_steppers(&settings, &self.open_serials());
        let found = steppers
            .iter()
            .find(|s| s.name == stepper)
            .cloned()
            .ok_or_else(|| Precondition::UnknownStepper(stepper.to_owned()))?;
        Ok(Resolved {
            printer,
            limits: BedLimits::from_steppers(&steppers),
            stepper: found,
            steppers,
        })
    }

    /// Why the stepper's board cannot be used, if it cannot: each reason makes the later ones
    /// meaningless, so at most one is reported.
    fn trace_failure(&self, stepper: &Stepper) -> Option<Precondition> {
        let (open, protocol) = {
            let live = lock(&self.live);
            let board = live.boards.get(&stepper.serial);
            (
                board.is_some_and(|b| b.open),
                board.and_then(|b| b.protocol_version),
            )
        };
        if !open {
            return Some(Precondition::TraceNotOpen);
        }
        match protocol {
            Some(PROTOCOL_VERSION) => {}
            Some(version) => return Some(Precondition::ProtocolVersion(version)),
            None => return Some(Precondition::NoTraceFrames),
        }
        stepper
            .unusable
            .as_ref()
            .map(|reason| Precondition::StepperUnusable(reason.clone()))
    }

    async fn resolve(&self, stepper: &str) -> Result<Resolved, Precondition> {
        let resolved = self.lookup(stepper).await?;
        match self.trace_failure(&resolved.stepper) {
            Some(p) => Err(p),
            None => Ok(resolved),
        }
    }

    /// Every failing per-stepper precondition, in check order, or the resolved stepper.
    async fn run_failures(&self, stepper: &str) -> Result<Resolved, Vec<Precondition>> {
        let resolved = self.lookup(stepper).await.map_err(|p| vec![p])?;
        let mut failing = Vec::new();
        if matches!(resolved.printer.print_state.as_str(), "printing" | "paused") {
            failing.push(Precondition::Printing);
        }
        failing.extend(self.trace_failure(&resolved.stepper));
        if !resolved.printer.homed_axes.contains(resolved.stepper.axis) {
            failing.push(Precondition::NotHomed(resolved.stepper.axis));
        }
        if !resolved.stepper.electrical_model_present {
            failing.push(Precondition::NoElectricalModel);
        }
        let free = self.store.free_bytes().unwrap_or(0);
        if free < self.config.min_free_bytes {
            failing.push(Precondition::LowDisk(free));
        }
        if failing.is_empty() {
            Ok(resolved)
        } else {
            Err(failing)
        }
    }

    async fn check_run(&self, stepper: &str) -> Result<Resolved, Precondition> {
        self.run_failures(stepper).await.map_err(|failing| {
            failing
                .into_iter()
                .next()
                .expect("run_failures fails only with at least one failure")
        })
    }

    /// Sends G-code unless a kill owns the current run; nothing may follow a kill.
    async fn gcode(&self, line: &str) -> Result<(), String> {
        let command = line.split_whitespace().next().unwrap_or_default();
        if self.kill_engaged() {
            return Err(format!("{command}: not sent after the emergency stop"));
        }
        self.moonraker
            .script(line)
            .await
            .map_err(|e| format!("{command}: {e}"))
    }

    async fn gcode_bounded(&self, line: &str) -> Result<(), String> {
        timeout(self.config.confirm_timeout, self.gcode(line))
            .await
            .map_err(|_| "timed out".to_owned())?
    }

    fn change_sequence(&self, stepper: &Stepper) -> Option<u32> {
        lock(&self.live)
            .boards
            .get(&stepper.serial)
            .and_then(|b| b.channels.get(&stepper.channel))
            .map(|c| c.change_sequence)
    }

    /// Feeds control-state frames of the stepper's channel, newer than `after`, to `accept`
    /// until it returns true or the confirm timeout passes.
    async fn await_control(
        &self,
        mut rx: broadcast::Receiver<(String, Bytes)>,
        stepper: &Stepper,
        after: Option<u32>,
        mut accept: impl FnMut(&ControlState) -> bool,
    ) -> bool {
        let deadline = Instant::now() + self.config.confirm_timeout;
        loop {
            let (serial, frame) = match timeout_at(deadline, rx.recv()).await {
                Err(_) | Ok(Err(RecvError::Closed)) => return false,
                Ok(Err(RecvError::Lagged(_))) => continue,
                Ok(Ok(message)) => message,
            };
            if serial != stepper.serial {
                continue;
            }
            if let Ok(Frame::ControlState(h, c)) = decode_frame(&frame)
                && h.source_index == stepper.channel
                && after.is_none_or(|seq| c.change_sequence > seq)
                && accept(&c)
            {
                return true;
            }
        }
    }

    async fn write_gains(&self, stepper: &Stepper, g: &Gains) -> Result<(), String> {
        let rx = self.frames.subscribe();
        let after = self.change_sequence(stepper);
        self.gcode(&gains_line(&stepper.name, g)).await?;
        let wanted = (g.velocity_p, g.velocity_i, g.position_p, g.position_i);
        let confirmed = self
            .await_control(rx, stepper, after, |c| {
                c.applied_request == GAINS_REQUEST
                    && (c.velocity_p, c.velocity_i, c.position_p, c.position_i) == wanted
            })
            .await;
        if confirmed {
            Ok(())
        } else {
            Err("gain write unconfirmed".to_owned())
        }
    }

    async fn write_filters(&self, stepper: &Stepper, f: &Filters) -> Result<(), String> {
        let rx = self.frames.subscribe();
        let after = self.change_sequence(stepper);
        self.gcode(&filters_line(&stepper.name, f)).await?;
        let mut confirmed = [false; 4];
        self.await_control(rx, stepper, after, |c| {
            for (k, (request, _)) in FILTER_REQUESTS.iter().enumerate() {
                if c.applied_request == *request && cutoff(c, k) == filter_value(f, k) {
                    confirmed[k] = true;
                }
            }
            confirmed.iter().all(|&ok| ok)
        })
        .await;
        match confirmed.iter().position(|&ok| !ok) {
            None => Ok(()),
            Some(k) => Err(format!(
                "filter write unconfirmed: {}",
                FILTER_REQUESTS[k].1
            )),
        }
    }

    /// Waits for the metadata frame of the stepper's board, then one control-state frame for
    /// every channel it declares; returns the stepper's channel's frame.
    async fn wait_baseline(
        &self,
        mut rx: broadcast::Receiver<(String, Bytes)>,
        stepper: &Stepper,
    ) -> Result<ControlState, String> {
        let deadline = Instant::now() + self.config.baseline_timeout;
        let mut channels: Option<Vec<u8>> = None;
        let mut seen: HashMap<u8, ControlState> = HashMap::new();
        loop {
            let (serial, frame) = match timeout_at(deadline, rx.recv()).await {
                Err(_) | Ok(Err(RecvError::Closed)) => break,
                Ok(Err(RecvError::Lagged(_))) => continue,
                Ok(Ok(message)) => message,
            };
            if serial != stepper.serial {
                continue;
            }
            match decode_frame(&frame) {
                Ok(Frame::Metadata(_, m)) => {
                    let mut declared: Vec<u8> = m
                        .sources
                        .iter()
                        .filter(|(kind, _, _)| *kind == CHANNEL_SOURCE_KIND)
                        .map(|(_, index, _)| *index)
                        .collect();
                    if declared.is_empty() {
                        declared.push(stepper.channel);
                    }
                    channels = Some(declared);
                    seen.clear();
                }
                Ok(Frame::ControlState(h, c)) if channels.is_some() => {
                    seen.entry(h.source_index).or_insert(c);
                }
                _ => {}
            }
            if channels
                .as_ref()
                .is_some_and(|c| c.iter().all(|ch| seen.contains_key(ch)))
                && let Some(own) = seen.remove(&stepper.channel)
            {
                return Ok(own);
            }
        }
        Err(match channels {
            Some(_) => "baseline missing".to_owned(),
            None => "baseline missing: no metadata frame".to_owned(),
        })
    }

    fn trace_mode(&self, serial: &str) -> TraceMode {
        lock(&self.live)
            .boards
            .get(serial)
            .map_or(TraceMode::Unknown, |b| b.trace_mode)
    }

    /// Resolves with the reason once Moonraker, Klipper or the board's trace interface is
    /// gone; never resolves while all are up.
    async fn watch_loss(&self, serial: &str) -> String {
        let mut state = self.moonraker.state();
        let mut tick = tokio::time::interval(self.config.loss_poll);
        loop {
            let klippy = state.borrow_and_update().klippy;
            match klippy {
                KlippyState::Ready => {}
                KlippyState::Disconnected => return "moonraker connection lost".to_owned(),
                KlippyState::Shutdown => return "klipper shut down".to_owned(),
                KlippyState::Error => return "klipper reported an error".to_owned(),
                KlippyState::Startup => return "klipper restarted".to_owned(),
            }
            let open = lock(&self.live).boards.get(serial).is_some_and(|b| b.open);
            if !open {
                return "trace interface lost".to_owned();
            }
            tokio::select! {
                changed = state.changed() => if changed.is_err() {
                    return "moonraker client stopped".to_owned();
                },
                _ = tick.tick() => {}
            }
        }
    }

    /// Steps 1 to the drain of the run sequence. Dropping the future stops it between
    /// commands; `ctx` records what was sent so the caller can clean up.
    async fn sequence(
        &self,
        id: &RunId,
        req: &RunRequest,
        r: &Resolved,
        ctx: &mut RunCtx,
    ) -> Result<(), String> {
        let limits = r
            .limits
            .as_ref()
            .ok_or("bed limits need stepper_x and stepper_y in the config")?;
        let stimulus = compose(
            &req.kind,
            &req.pattern,
            &r.stepper.name,
            &r.stepper.kinematics,
            limits,
            r.printer.max_velocity,
        )
        .map_err(|e| format!("stimulus not composed: {e}"))?;
        self.update_run(|run| {
            run.clamped_speeds = stimulus.clamped_speeds.clone();
            run.applied_speeds = stimulus.applied_speeds.clone();
        });
        self.set_state(RunState::WritingGains);
        self.gcode("M400").await?;
        self.write_gains(&r.stepper, &req.gains).await?;
        self.write_filters(&r.stepper, &req.filters).await?;

        self.set_state(RunState::Baseline);
        let baseline = self.frames.subscribe();
        let path = self.store.capture_path(&req.session, id);
        ctx.recorder = Some(
            Recorder::start(&path, &r.stepper.serial, self.frames.subscribe())
                .map_err(|e| format!("capture not started: {e}"))?,
        );
        ctx.prior_mode = Some(self.trace_mode(&r.stepper.serial));
        ctx.critical_sent = true;
        self.gcode(&format!(
            "FOCI_TRACE_MODE STEPPER={} MODE=critical",
            r.stepper.name
        ))
        .await?;
        let found = self.wait_baseline(baseline, &r.stepper).await?;
        baseline_matches(&req.gains, &req.filters, &found)?;

        self.set_state(RunState::Recording);
        self.set_state(RunState::Moving);
        self.send_stimulus(&stimulus.gcode, ctx).await?;
        ctx.script_ok = true;
        self.set_state(RunState::Draining);
        tokio::time::sleep(self.config.drain).await;
        Ok(())
    }

    async fn send_stimulus(&self, gcode: &str, ctx: &mut RunCtx) -> Result<(), String> {
        if self.kill_engaged() {
            return Err("stimulus not sent after the emergency stop".to_owned());
        }
        ctx.script_sent = true;
        self.moonraker
            .script(gcode)
            .await
            .map_err(|e| format!("stimulus failed: {e}"))
    }

    /// Why Klipper refuses every G-code until a firmware restart, when it does.
    fn klippy_unavailable_for_gcode(&self) -> Option<&'static str> {
        match self.moonraker.state().borrow().klippy {
            KlippyState::Shutdown => Some(OUTSIDE_SHUTDOWN),
            KlippyState::Error => Some(OUTSIDE_ERROR),
            KlippyState::Ready | KlippyState::Startup | KlippyState::Disconnected => None,
        }
    }

    async fn await_shutdown_notice(&self) {
        let mut state = self.moonraker.state();
        let _ = timeout(
            self.config.shutdown_notice,
            state.wait_for(|s| s.klippy != KlippyState::Ready),
        )
        .await;
    }

    /// G-code after a run that was not killed: the feedforward gain when a script that
    /// changes it did not finish, then the trace mode when the board was known dormant.
    /// Nothing is sent to a Klipper that refuses G-code; a firmware restart resets both.
    async fn cleanup(&self, req: &RunRequest, stepper: &Stepper, ctx: &RunCtx) {
        if self.klippy_unavailable_for_gcode().is_some() {
            self.note(RESTART_NOTE.to_owned());
            return;
        }
        let toggles_feedforward = !matches!(req.kind, RunKind::Standstill { .. });
        if ctx.script_sent
            && !ctx.script_ok
            && toggles_feedforward
            && !self.kill_engaged()
            && let Err(e) = self
                .gcode_bounded(&feedforward_reset_line(&stepper.name))
                .await
        {
            self.note(format!("feedforward gain not restored to 1.0: {e}"));
        }
        if !ctx.critical_sent || self.kill_engaged() {
            return;
        }
        match ctx.prior_mode {
            Some(TraceMode::Dormant) => {
                let line = format!("FOCI_TRACE_MODE STEPPER={} MODE=dormant", stepper.name);
                if let Err(e) = self.gcode_bounded(&line).await {
                    self.note(format!("trace mode not restored to dormant: {e}"));
                }
            }
            Some(TraceMode::Streaming) => self.note(
                "trace mode left critical: the board was streaming before the run".to_owned(),
            ),
            Some(TraceMode::Unknown) | None => self.note(
                "trace mode left critical: the board's mode before the run was unknown".to_owned(),
            ),
        }
    }

    /// Verification and analysis of a recorded run; returns its final state.
    async fn evaluate(&self, req: &RunRequest, r: &Resolved, summary: &CaptureSummary) -> RunState {
        self.set_state(RunState::Verifying);
        let path = summary.path.clone();
        let kind = req.kind.clone();
        let stepper = r.stepper.clone();
        let steppers = r.steppers.clone();
        let applied = lock(&self.inner)
            .current
            .as_ref()
            .map(|c| c.run.applied_speeds.clone())
            .unwrap_or_default();
        let result = tokio::task::spawn_blocking(move || {
            let decoded = decode_capture(&path)?;
            let regimes: HashMap<u8, Regime> = decoded
                .baselines
                .iter()
                .map(|(&channel, c)| {
                    let rotation = steppers
                        .iter()
                        .find(|s| s.serial == stepper.serial && s.channel == channel)
                        .map_or(stepper.motor_rotation_distance_mm, |s| {
                            s.motor_rotation_distance_mm
                        });
                    let regime = Regime {
                        motor_rotation_distance_mm: rotation,
                        encoder_counts_per_rev: c.encoder_counts_per_rev,
                        pwm_hz: c.pwm_hz,
                        clock_freq: decoded.stream.clock_freq,
                    };
                    (channel, regime)
                })
                .collect();
            let mut notes = Vec::new();
            let input = AnalysisInput {
                kind: &kind,
                declared: stepper.channel,
                frames: &decoded.frames,
                regimes: &regimes,
                applied_speeds: &applied,
            };
            let window = stimulus_window(&input);
            let metrics = analyze(&input, &window, &mut notes);
            let probe_edges = match kind {
                RunKind::Probe { .. } => Some(
                    metrics
                        .channels
                        .iter()
                        .find(|c| c.declared)
                        .and_then(|c| c.probe.as_ref())
                        .map_or(0, |p| p.edges),
                ),
                RunKind::Tracking { .. } | RunKind::Standstill { .. } => None,
            };
            let verification = verify(&Window {
                declared: stepper.channel,
                frames: &decoded.frames,
                baselines: &decoded.baselines,
                probe_edges,
                stimulus: &window,
            });
            let regime = regimes.get(&stepper.channel).map(|g| RunRegime {
                motor_rotation_distance_mm: g.motor_rotation_distance_mm,
                encoder_counts_per_rev: g.encoder_counts_per_rev,
                pwm_hz: g.pwm_hz,
                clock_freq: g.clock_freq,
            });
            Ok::<_, String>((decoded.stream, regime, metrics, verification, notes))
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
        let (stream, regime, metrics, verification, notes) = match result {
            Ok(parts) => parts,
            Err(e) => return RunState::Failed(format!("capture cannot be analysed: {e}")),
        };
        let reason = first_failure(&verification).map(str::to_owned);
        self.update_run(|run| {
            run.stream = Some(stream);
            run.regime = regime.clone();
            run.verification = verification;
            run.metrics = Some(metrics);
            run.notes.extend(notes);
        });
        self.set_state(RunState::Analyzing);
        if let Some(regime) = regime {
            let path = summary.path.clone();
            let kind = req.kind.clone();
            let series = tokio::task::spawn_blocking(move || {
                build_series(&path, &Regime::from(&regime), &kind)
            })
            .await;
            let written = match series {
                Ok(Ok(series)) => {
                    let id = lock(&self.inner).current.as_ref().map(|c| c.run.id.clone());
                    id.map_or(Ok(()), |id| {
                        self.store
                            .write_series(&req.session, &id, &series)
                            .map_err(|e| e.to_string())
                    })
                }
                Ok(Err(e)) => Err(e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            if let Err(e) = written {
                self.note(format!("series.json not written: {e}"));
            }
        }
        match reason {
            Some(reason) => RunState::NotComparable(reason),
            None => RunState::Done,
        }
    }

    async fn work(self: Arc<Self>) {
        loop {
            let started = {
                let mut inner = lock(&self.inner);
                let next = inner.queue.pop_front();
                let started = next.is_some();
                inner.current = next;
                if started {
                    self.publish_queue(&inner);
                }
                started
            };
            if started {
                self.run_one().await;
            } else {
                self.wake.notified().await;
            }
        }
    }

    async fn run_one(&self) {
        let Some((id, request)) = lock(&self.inner)
            .current
            .as_ref()
            .map(|c| (c.run.id.clone(), c.run.request.clone()))
        else {
            return;
        };
        let mut kill_rx = self.kill_requested.subscribe();
        let mut resolved_rx = self.kill_resolved.subscribe();
        let held = lock(&self.inner)
            .locks
            .get(&request.stepper)
            .map(|held| held.lock.reason.clone());
        if let Some(reason) = held {
            let refused = Precondition::TuneLocked(reason).to_string();
            self.end_run(Some(RunState::Failed(refused)), &mut resolved_rx)
                .await;
            return;
        }
        self.set_state(RunState::Preparing);
        let resolved = match self.check_run(&request.stepper).await {
            Ok(r) => r,
            Err(p) => {
                let reason = self
                    .klippy_unavailable_for_gcode()
                    .map_or_else(|| p.to_string(), str::to_owned);
                self.end_run(Some(RunState::Failed(reason)), &mut resolved_rx)
                    .await;
                return;
            }
        };
        let mut ctx = RunCtx::default();
        let outcome = if self.kill_engaged() {
            Outcome::Killed
        } else {
            tokio::select! {
                biased;
                _ = kill_rx.changed() => Outcome::Killed,
                reason = self.watch_loss(&resolved.stepper.serial) => Outcome::Failed(reason),
                r = self.sequence(&id, &request, &resolved, &mut ctx) => match r {
                    Ok(()) => Outcome::Recorded,
                    Err(reason) => Outcome::Failed(reason),
                },
            }
        };
        let killed = self.kill_engaged() || matches!(outcome, Outcome::Killed);
        let record = match ctx.recorder.take() {
            Some(recorder) => Some(recorder.finish().await),
            None => None,
        };
        match &record {
            Some(Ok(end)) => self.update_run(|run| {
                run.capture = Some(CaptureInfo {
                    frames: end.summary.frames,
                    file_bytes: end.summary.file_bytes,
                });
                run.baselines = baselines(&end.summary.first_control_state);
            }),
            Some(Err(e)) => self.note(format!("capture not finished: {e}")),
            None => {}
        }
        let state = match (killed, outcome) {
            (true, _) | (false, Outcome::Killed) => None,
            (false, Outcome::Failed(reason)) => {
                self.await_shutdown_notice().await;
                if self.kill_engaged() {
                    None
                } else {
                    let reason = self
                        .klippy_unavailable_for_gcode()
                        .map_or(reason, str::to_owned);
                    self.cleanup(&request, &resolved.stepper, &ctx).await;
                    Some(RunState::Failed(reason))
                }
            }
            (false, Outcome::Recorded) => {
                self.cleanup(&request, &resolved.stepper, &ctx).await;
                Some(match record {
                    Some(Ok(end)) if end.lost > 0 => RunState::Failed(format!(
                        "capture lost {} trace frames between the reader and the capture",
                        end.lost
                    )),
                    Some(Ok(end)) => self.evaluate(&request, &resolved, &end.summary).await,
                    Some(Err(e)) => RunState::Failed(format!("capture: {e}")),
                    None => RunState::Failed("no capture was recorded".to_owned()),
                })
            }
        };
        self.end_run(state, &mut resolved_rx).await;
    }

    /// Applies the final state, waits out a kill still being confirmed, then retires the run
    /// and applies the queue rules: a failed run stops the queue, and a probe ladder value
    /// whose repeats are complete goes through the stop rule.
    async fn end_run(&self, state: Option<RunState>, resolved_rx: &mut watch::Receiver<u64>) {
        if let Some(state) = state {
            self.set_state(state);
        }
        let mut unresolvable = false;
        loop {
            {
                let mut inner = lock(&self.inner);
                let killing = inner
                    .current
                    .as_ref()
                    .is_some_and(|c| c.run.state == RunState::Killing);
                if unresolvable || !killing {
                    if let Some(finished) = inner.current.take() {
                        self.retire(&mut inner, &finished);
                        self.emit(EngineEvent::Finished(Box::new(finished.run)));
                    }
                    return;
                }
            }
            unresolvable = resolved_rx.changed().await.is_err();
        }
    }

    /// The queue rules after a run, applied in the same critical section that took it.
    fn retire(&self, inner: &mut Inner, finished: &Queued) {
        let mut stop = match &finished.run.state {
            RunState::Failed(_) => Some(format!("queue stopped: run {} failed", finished.run.id)),
            _ => None,
        };
        if let Some(slot) = finished.ladder
            && let Some(track) = inner.ladders.get_mut(&slot.ladder)
            && track.probe
        {
            let outcomes = &mut track.outcomes[slot.value];
            outcomes.push(rung_outcome(&finished.run));
            let at = format!("{} {}", track.axis, track.values[slot.value]);
            let reason = match probe_stop(outcomes, track.repeats, track.floor) {
                StopDecision::Continue => None,
                StopDecision::StopBelowFloor { median } => Some(format!(
                    "ladder stopped: median zeta {median:.3} below the floor {} at {at}",
                    track.floor
                )),
                StopDecision::StopUnassessable => {
                    Some(format!("ladder stopped: damping unassessable at {at}"))
                }
            };
            stop = stop.or(reason);
        }
        if let Some(reason) = stop {
            self.skip_queue(inner, &RunState::Skipped(reason));
        } else if inner.queue.is_empty() {
            inner.ladders.clear();
        }
    }

    async fn post_stop(&self) -> bool {
        let mut state = self.moonraker.state();
        let deadline = Instant::now() + self.config.kill_timeout;
        match timeout_at(deadline, self.moonraker.emergency_stop()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "emergency stop failed");
                return false;
            }
            Err(_) => {
                tracing::warn!("emergency stop timed out");
                return false;
            }
        }
        let reported = timeout_at(
            deadline,
            state.wait_for(|s| {
                matches!(s.klippy, KlippyState::Shutdown | KlippyState::Disconnected)
            }),
        )
        .await;
        matches!(reported, Ok(Ok(s)) if s.klippy == KlippyState::Shutdown)
    }

    async fn kill(&self) -> (Option<RunId>, bool) {
        let target = {
            let mut inner = lock(&self.inner);
            let target = match inner.current.as_mut() {
                Some(c)
                    if !c.run.state.is_terminal() || c.run.state == RunState::KillUnconfirmed =>
                {
                    self.transition(c, RunState::Killing);
                    Some((c.session.clone(), c.run.id.clone()))
                }
                _ => None,
            };
            self.abort_queue(&mut inner);
            self.kill_requested.send_modify(|n| *n += 1);
            target
        };
        let confirmed = self.post_stop().await;
        let killed_run = target.as_ref().map(|(_, id)| id.clone());
        {
            let mut inner = lock(&self.inner);
            let target_id = target.as_ref().map(|(_, id)| id);
            if let Some(c) = inner.current.as_mut()
                && Some(&c.run.id) == target_id
            {
                let state = if confirmed {
                    RunState::Killed
                } else {
                    RunState::KillUnconfirmed
                };
                self.transition(c, state);
            }
            if confirmed {
                if let Some((session, id)) = inner.unconfirmed_kill.take()
                    && Some(&id) != target_id
                {
                    self.mark_killed(&mut inner, session, &id);
                }
            } else if let Some(target) = target {
                inner.unconfirmed_kill = Some(target);
                inner.restarts_at_kill = *self.moonraker.klippy_restarts().borrow();
            }
        }
        let klippy = self.moonraker.state().borrow().klippy;
        let restarts = *self.moonraker.klippy_restarts().borrow();
        self.observe_klippy(klippy, restarts);
        self.kill_resolved.send_modify(|n| *n += 1);
        self.persister.flush().await;
        (killed_run, confirmed)
    }

    /// Upgrades a `KillUnconfirmed` run, current or stored, to `Killed`.
    fn mark_killed(&self, inner: &mut Inner, session: SessionId, id: &RunId) {
        if let Some(c) = inner.current.as_mut()
            && &c.run.id == id
        {
            if c.run.state == RunState::KillUnconfirmed {
                self.transition(c, RunState::Killed);
            }
            return;
        }
        self.persister.send(Job::MarkKilled(session, id.clone()));
    }

    /// Clears an unconfirmed kill once Klipper reports shutdown, which confirms the stop, or
    /// Moonraker reports that Klipper restarted after the kill.
    fn observe_klippy(&self, klippy: KlippyState, restarts: u64) {
        let mut inner = lock(&self.inner);
        let Some((session, id)) = inner.unconfirmed_kill.clone() else {
            return;
        };
        if klippy == KlippyState::Shutdown {
            inner.unconfirmed_kill = None;
            self.mark_killed(&mut inner, session, &id);
        } else if restarts > inner.restarts_at_kill {
            inner.unconfirmed_kill = None;
        }
    }

    async fn watch_klippy(self: Arc<Self>) {
        let mut state = self.moonraker.state();
        let mut restarts = self.moonraker.klippy_restarts();
        let mut was_ready = state.borrow().klippy == KlippyState::Ready;
        let mut seen_restarts = *restarts.borrow();
        loop {
            let klippy = state.borrow_and_update().klippy;
            let count = *restarts.borrow_and_update();
            let ready = klippy == KlippyState::Ready;
            if (ready && !was_ready) || count != seen_restarts {
                self.invalidate_settings();
            }
            (was_ready, seen_restarts) = (ready, count);
            self.observe_klippy(klippy, count);
            tokio::select! {
                changed = state.changed() => if changed.is_err() { return },
                changed = restarts.changed() => if changed.is_err() { return },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::Value;

    use super::*;
    use crate::model::Session;
    use crate::moonraker;
    use crate::pattern::Pattern;
    use crate::steppers::ConfigGains;

    struct Stub {
        scripts: Mutex<Vec<String>>,
        stops: AtomicUsize,
        state: watch::Sender<PrinterState>,
        gcode: broadcast::Sender<String>,
    }

    impl MoonrakerApi for Stub {
        async fn config_root(&self) -> moonraker::Result<PathBuf> {
            Ok(PathBuf::new())
        }

        async fn settings(&self) -> moonraker::Result<Value> {
            Ok(Value::Null)
        }

        async fn script(&self, gcode: &str) -> moonraker::Result<()> {
            lock(&self.scripts).push(gcode.to_owned());
            Ok(())
        }

        async fn emergency_stop(&self) -> moonraker::Result<()> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            self.state.send_modify(|s| s.klippy = KlippyState::Shutdown);
            Ok(())
        }

        async fn firmware_restart(&self) -> moonraker::Result<()> {
            Ok(())
        }

        fn state(&self) -> watch::Receiver<PrinterState> {
            self.state.subscribe()
        }

        fn gcode_responses(&self) -> broadcast::Receiver<String> {
            self.gcode.subscribe()
        }

        fn klippy_restarts(&self) -> watch::Receiver<u64> {
            watch::channel(0).1
        }
    }

    fn request(session: &SessionId) -> RunRequest {
        RunRequest {
            session: session.clone(),
            stepper: "stepper_x".into(),
            kind: RunKind::Probe {
                speed_mm_s: 195.0,
                high_gain: 1.3,
                segments_between_toggles: 3,
                strokes: 2,
            },
            pattern: Pattern {
                start: (100.0, 100.0),
                end: (200.0, 200.0),
            },
            gains: Gains {
                velocity_p: 841,
                velocity_i: 412,
                position_p: 141,
                position_i: 0,
            },
            filters: Filters {
                velocity_hz: 500,
                torque_hz: 1000,
                position_hz: 500,
                flux_hz: 1000,
            },
            ladder_value: None,
        }
    }

    fn stepper() -> Stepper {
        Stepper {
            name: "stepper_x".into(),
            mcu: "boarda".into(),
            channel: 0,
            serial: "2600AAAA".into(),
            kinematics: "corexy".into(),
            motor_rotation_distance_mm: 40.0,
            axis: 'x',
            position_min: 0.0,
            position_max: 300.0,
            encoder_ppr: 1000,
            electrical_model_present: true,
            config_gains: ConfigGains::default(),
            unusable: None,
        }
    }

    /// A `Shared` whose current run is in `state`, plus its stub and data directory.
    fn shared_with_run(state: RunState) -> (Shared<Stub>, Arc<Stub>, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(dir.path().to_owned()).unwrap();
        let req = request(&SessionId::parse("s1").unwrap());
        let session = Session {
            format_version: FORMAT_VERSION,
            name: "guard".into(),
            created_unix_s: 0,
            stepper: req.stepper.clone(),
            kind: req.kind.clone(),
            pattern: req.pattern,
            gains: req.gains,
            filters: req.filters,
            ladder: None,
        };
        let mut run = Shared::<Stub>::new_run(req.clone(), 0, &session);
        run.state = state;
        let printer = PrinterState {
            klippy: KlippyState::Ready,
            homed_axes: "xyz".into(),
            idle_state: String::new(),
            print_state: String::new(),
            max_velocity: 300.0,
            max_accel: 3000.0,
        };
        let stub = Arc::new(Stub {
            scripts: Mutex::new(Vec::new()),
            stops: AtomicUsize::new(0),
            state: watch::channel(printer).0,
            gcode: broadcast::channel(4).0,
        });
        let events = broadcast::channel(4).0;
        let shared = Shared {
            moonraker: stub.clone(),
            live: Arc::default(),
            frames: broadcast::channel(4).0,
            persister: Persister::start(store.clone(), events.clone()).unwrap(),
            store,
            config: EngineConfig::default(),
            inner: Mutex::new(Inner {
                current: Some(Queued {
                    session: req.session,
                    run,
                    ladder: None,
                }),
                ..Inner::default()
            }),
            wake: Notify::new(),
            events,
            kill_requested: watch::channel(0).0,
            kill_resolved: watch::channel(0).0,
            next_ladder: AtomicU64::new(0),
            settings: Mutex::default(),
            lock_io: Mutex::default(),
            lock_generation: AtomicU64::new(0),
            lock_watchers: Mutex::default(),
            runtime: tokio::runtime::Handle::current(),
        };
        (shared, stub, dir)
    }

    async fn sent_by_every_path(state: RunState) -> Vec<String> {
        let (shared, stub, _dir) = shared_with_run(state);
        let req = request(&SessionId::parse("s1").unwrap());
        let _ = shared.gcode("M400").await;
        let _ = shared.send_stimulus("G90", &mut RunCtx::default()).await;
        let ctx = RunCtx {
            prior_mode: Some(TraceMode::Dormant),
            critical_sent: true,
            script_sent: true,
            ..RunCtx::default()
        };
        shared.cleanup(&req, &stepper(), &ctx).await;
        lock(&stub.scripts).clone()
    }

    #[tokio::test]
    async fn no_gcode_follows_a_kill_on_any_path() {
        for state in [
            RunState::Killing,
            RunState::KillUnconfirmed,
            RunState::Killed,
        ] {
            assert_eq!(
                sent_by_every_path(state.clone()).await,
                Vec::<String>::new(),
                "{state:?}"
            );
        }
        assert_eq!(
            sent_by_every_path(RunState::Moving).await,
            vec![
                "M400".to_owned(),
                "G90".to_owned(),
                feedforward_reset_line("stepper_x"),
                "FOCI_TRACE_MODE STEPPER=stepper_x MODE=dormant".to_owned(),
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stalled_store_does_not_delay_the_emergency_stop() {
        let (shared, stub, _dir) = shared_with_run(RunState::Moving);
        let (session, id) = {
            let inner = lock(&shared.inner);
            let current = inner.current.as_ref().unwrap();
            shared
                .store
                .write_run(&current.session, &current.run)
                .unwrap();
            (current.session.clone(), current.run.id.clone())
        };
        let shared = Arc::new(shared);
        shared.persister.set_paused(true);

        let killing = shared.clone();
        let kill = tokio::spawn(async move { killing.kill().await });
        let deadline = Instant::now() + Duration::from_secs(1);
        while stub.stops.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let stops_while_stalled = stub.stops.load(Ordering::SeqCst);
        let on_disk_while_stalled = shared.store.read_run(&session, &id).unwrap().state;
        shared.persister.set_paused(false);
        let (killed, confirmed) = kill.await.unwrap();

        assert_eq!(stops_while_stalled, 1);
        assert_eq!(on_disk_while_stalled, RunState::Moving);
        assert_eq!(killed, Some(id.clone()));
        assert!(confirmed);
        assert_eq!(
            shared.store.read_run(&session, &id).unwrap().state,
            RunState::Killed
        );
    }
}
