//! Run engine: a single-slot executor with a queue.
//!
//! One run is a fixed sequence: confirmed gain and filter writes, a forced control-state
//! baseline, the stimulus recorded on the trace stream, verification, analysis and storage.
//! A ladder is a queue of runs in interleaved order with a damping stop rule. Kill posts
//! Klipper's emergency stop; abort drains the queue after the current run.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use foci_tune_analysis::ladder::{
    LadderAxis, LadderError, RungOutcome, StopDecision, interleave, probe_stop, to_raw,
};
use foci_tune_analysis::protocol::{ControlState, Frame, PROTOCOL_VERSION, decode_frame};
use foci_tune_analysis::signals::{i_gain_param, p_gain_param};
use foci_tune_analysis::units::Regime;
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Notify, broadcast, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout, timeout_at};

use crate::capture::{Capture, CaptureSummary};
use crate::live::{LiveState, TraceMode};
use crate::model::{
    CaptureInfo, ChannelBaseline, ControlSnapshot, FORMAT_VERSION, Filters, FitOutcome, Gains,
    LadderDef, Run, RunId, RunRegime, RunRequest, RunState, Session, SessionId, StepTime,
};
use crate::moonraker::{KlippyState, MoonrakerApi, PrinterState};
use crate::run_analysis::{AnalysisInput, analyze, decode_capture};
use crate::steppers::{BedLimits, Stepper, derive_steppers};
use crate::stimulus::{RunKind, compose, feedforward_reset_line};
use crate::store::{Store, build_series};
use crate::verify_run::{Window, first_failure, verify};

const GAINS_REQUEST: u8 = 1;
const FILTER_REQUESTS: [(u8, &str); 4] =
    [(3, "velocity"), (4, "torque"), (5, "position"), (6, "flux")];
const CHANNEL_SOURCE_KIND: u8 = 1;
const EVENT_CAPACITY: usize = 1024;

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
}

impl Default for EngineConfig {
    /// The spec's values: 2 s write and kill timeouts, 500 ms drain, 200 MB free.
    fn default() -> Self {
        Self {
            confirm_timeout: Duration::from_secs(2),
            baseline_timeout: Duration::from_secs(2),
            drain: Duration::from_millis(500),
            kill_timeout: Duration::from_secs(2),
            min_free_bytes: 200_000_000,
            loss_poll: Duration::from_millis(50),
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
    #[error("no trace frame from the stepper's board yet; its protocol version is unknown")]
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
}

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
    reverting: bool,
}

impl Inner {
    fn busy(&self) -> bool {
        self.current.is_some() || !self.queue.is_empty() || self.reverting
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
        .and_then(|p| match p.velocity {
            FitOutcome::Fit { zeta, .. } => Some(zeta),
            FitOutcome::Failed { .. } => None,
        });
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
    config: EngineConfig,
    inner: Mutex<Inner>,
    wake: Notify,
    events: broadcast::Sender<EngineEvent>,
    kill_requested: watch::Sender<u64>,
    kill_resolved: watch::Sender<u64>,
    next_ladder: AtomicU64,
    settings: Mutex<SettingsCache>,
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
    }
}

impl<M: MoonrakerApi + 'static> Engine<M> {
    /// Reconciles runs a previous process left in flight, then starts the worker.
    ///
    /// `frames` is the trace reader's broadcast of `(serial, frame)`; `live` its decoded
    /// state. Must be called inside a tokio runtime.
    ///
    /// # Errors
    /// I/O errors from reconciling the store.
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
        let restarts_at_kill = *moonraker.klippy_restarts().borrow();
        let shared = Arc::new(Shared {
            moonraker,
            live,
            frames,
            store,
            config,
            inner: Mutex::new(Inner {
                unconfirmed_kill,
                restarts_at_kill,
                ..Inner::default()
            }),
            wake: Notify::new(),
            events: broadcast::channel(EVENT_CAPACITY).0,
            kill_requested: watch::channel(0).0,
            kill_resolved: watch::channel(0).0,
            next_ladder: AtomicU64::new(0),
            settings: Mutex::default(),
        });
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
        s.enqueue(vec![run], None, generation)?;
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
        s.enqueue(runs, Some((slots, ladder, track)), generation)?;
        Ok(ids)
    }

    /// Stops the queue after the current run: every queued run is marked `Aborted`.
    pub async fn abort(&self) {
        let mut inner = lock(&self.shared.inner);
        self.shared.abort_queue(&mut inner);
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
        {
            let inner = lock(&self.shared.inner);
            if inner.unconfirmed_kill.is_some() {
                return Err(Precondition::KillUnconfirmed);
            }
            if inner.busy() {
                return Err(Precondition::Busy);
            }
        }
        self.shared.check_run(stepper).await.map(|_| ())
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
            if inner.unconfirmed_kill.is_some() {
                return Err(Precondition::KillUnconfirmed.to_string());
            }
            if inner.busy() {
                return Err(Precondition::Busy.to_string());
            }
            inner.reverting = true;
        }
        let _guard = RevertGuard(&s.inner);
        let resolved = s
            .resolve(&record.stepper)
            .await
            .map_err(|p| p.to_string())?;
        s.gcode("M400").await?;
        s.write_gains(&resolved.stepper, &record.gains).await?;
        s.write_filters(&resolved.stepper, &record.filters).await
    }

    /// Subscribes to run state changes, results and queue updates.
    pub fn events(&self) -> broadcast::Receiver<EngineEvent> {
        self.shared.events.subscribe()
    }

    /// True when no run is in flight or queued and no revert is being written.
    pub fn is_idle(&self) -> bool {
        !lock(&self.shared.inner).busy()
    }
}

struct RevertGuard<'a>(&'a Mutex<Inner>);

impl Drop for RevertGuard<'_> {
    fn drop(&mut self) {
        lock(self.0).reverting = false;
    }
}

impl<M: MoonrakerApi + 'static> Shared<M> {
    fn persist(&self, session: &SessionId, run: &Run) {
        if let Err(e) = self.store.write_run(session, run) {
            tracing::warn!(run = %run.id, error = %e, "cannot write run.json");
        }
    }

    fn publish(&self, session: &SessionId, run: &Run) {
        self.persist(session, run);
        let _ = self.events.send(EngineEvent::State {
            session: session.clone(),
            run: run.id.clone(),
            state: run.state.clone(),
        });
    }

    fn transition(&self, q: &mut Queued, state: RunState) {
        q.run.steps.push(StepTime {
            step: state.name().to_owned(),
            unix_ms: now_ms(),
        });
        q.run.state = state;
        self.publish(&q.session, &q.run);
    }

    fn publish_queue(&self, inner: &Inner) {
        let ids = inner.queue.iter().map(|q| q.run.id.clone()).collect();
        let _ = self.events.send(EngineEvent::Queue(ids));
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
        {
            let inner = lock(&self.inner);
            if inner.unconfirmed_kill.is_some() {
                return Err(Precondition::KillUnconfirmed);
            }
            if inner.busy() {
                return Err(Precondition::Busy);
            }
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

    fn enqueue(
        &self,
        runs: Vec<Run>,
        ladder: Option<(Vec<LadderSlot>, u64, LadderTrack)>,
        generation: u64,
    ) -> Result<(), Precondition> {
        let mut inner = lock(&self.inner);
        if inner.unconfirmed_kill.is_some() {
            return Err(Precondition::KillUnconfirmed);
        }
        if *self.kill_requested.borrow() != generation {
            return Err(Precondition::KillPressed);
        }
        if inner.busy() {
            return Err(Precondition::Busy);
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
        drop(inner);
        self.wake.notify_one();
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

    async fn resolve(&self, stepper: &str) -> Result<Resolved, Precondition> {
        let printer = self.moonraker.state().borrow().clone();
        if printer.klippy != KlippyState::Ready {
            return Err(Precondition::KlippyNotReady);
        }
        let settings = self.settings().await?;
        let open = self.open_serials();
        let protocol: HashMap<String, Option<u8>> = lock(&self.live)
            .boards
            .iter()
            .map(|(serial, b)| (serial.clone(), b.protocol_version))
            .collect();
        let steppers = derive_steppers(&settings, &open);
        let found = steppers
            .iter()
            .find(|s| s.name == stepper)
            .cloned()
            .ok_or_else(|| Precondition::UnknownStepper(stepper.to_owned()))?;
        if !open.contains(&found.serial) {
            return Err(Precondition::TraceNotOpen);
        }
        match protocol.get(&found.serial) {
            Some(Some(PROTOCOL_VERSION)) => {}
            Some(Some(version)) => return Err(Precondition::ProtocolVersion(*version)),
            Some(None) | None => return Err(Precondition::NoTraceFrames),
        }
        if let Some(reason) = &found.unusable {
            return Err(Precondition::StepperUnusable(reason.clone()));
        }
        Ok(Resolved {
            printer,
            limits: BedLimits::from_steppers(&steppers),
            stepper: found,
            steppers,
        })
    }

    async fn check_run(&self, stepper: &str) -> Result<Resolved, Precondition> {
        let printer = self.moonraker.state().borrow().clone();
        if printer.klippy != KlippyState::Ready {
            return Err(Precondition::KlippyNotReady);
        }
        if matches!(printer.print_state.as_str(), "printing" | "paused") {
            return Err(Precondition::Printing);
        }
        let resolved = self.resolve(stepper).await?;
        if !resolved.printer.homed_axes.contains(resolved.stepper.axis) {
            return Err(Precondition::NotHomed(resolved.stepper.axis));
        }
        if !resolved.stepper.electrical_model_present {
            return Err(Precondition::NoElectricalModel);
        }
        let free = self.store.free_bytes().unwrap_or(0);
        if free < self.config.min_free_bytes {
            return Err(Precondition::LowDisk(free));
        }
        Ok(resolved)
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
    /// every channel it declares.
    async fn wait_baseline(
        &self,
        mut rx: broadcast::Receiver<(String, Bytes)>,
        stepper: &Stepper,
    ) -> Result<(), String> {
        let deadline = Instant::now() + self.config.baseline_timeout;
        let mut channels: Option<Vec<u8>> = None;
        let mut seen: HashSet<u8> = HashSet::new();
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
                Ok(Frame::ControlState(h, _)) if channels.is_some() => {
                    seen.insert(h.source_index);
                }
                _ => {}
            }
            if channels
                .as_ref()
                .is_some_and(|c| c.iter().all(|ch| seen.contains(ch)))
            {
                return Ok(());
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
        self.wait_baseline(baseline, &r.stepper).await?;

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

    /// G-code after a run that was not killed: the feedforward gain when a script that
    /// changes it did not finish, then the trace mode when the board was known dormant.
    async fn cleanup(&self, req: &RunRequest, stepper: &Stepper, ctx: &RunCtx) {
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
            let metrics = analyze(
                &AnalysisInput {
                    kind: &kind,
                    declared: stepper.channel,
                    frames: &decoded.frames,
                    regimes: &regimes,
                    applied_speeds: &applied,
                },
                &mut notes,
            );
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
        self.set_state(RunState::Preparing);
        let resolved = match self.check_run(&request.stepper).await {
            Ok(r) => r,
            Err(p) => {
                self.end_run(Some(RunState::Failed(p.to_string())), &mut resolved_rx)
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
                self.cleanup(&request, &resolved.stepper, &ctx).await;
                Some(RunState::Failed(reason))
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
                        let _ = self
                            .events
                            .send(EngineEvent::Finished(Box::new(finished.run)));
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
        match self.store.read_run(&session, id) {
            Ok(run) if run.state == RunState::KillUnconfirmed => {
                let mut q = Queued {
                    session,
                    run,
                    ladder: None,
                };
                self.transition(&mut q, RunState::Killed);
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(run = %id, error = %e, "cannot confirm the kill"),
        }
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
            state: watch::channel(printer).0,
            gcode: broadcast::channel(4).0,
        });
        let shared = Shared {
            moonraker: stub.clone(),
            live: Arc::default(),
            frames: broadcast::channel(4).0,
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
            events: broadcast::channel(4).0,
            kill_requested: watch::channel(0).0,
            kill_resolved: watch::channel(0).0,
            next_ladder: AtomicU64::new(0),
            settings: Mutex::default(),
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
}
