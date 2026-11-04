//! Tune records and the scheduler that runs a tune: each step's ladder of rungs, one rung at a
//! time in ascending gain order with a verdict after every rung, the operator's choice that
//! hands the step's value on, and the restore of the tune's starting gains when it is abandoned
//! or interrupted.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex as StdMutex, OnceLock, PoisonError, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use foci_tune_analysis::ladder::{LadderAxis, LadderValue, RungOutcome, to_raw};
use foci_tune_analysis::recommend::{Extension, Recommendation, Recommended, recommend};
use foci_tune_analysis::rungs::{
    DEFAULT_OCTAVE_DIVISIONS, RungNote, Rungs, SeedRefused, rung_values, rung_values_in, seed_from,
};
use foci_tune_analysis::verdict::{
    DriftVerdict, Gates, Halt, LadderAssessment, Pause, RepeatResult, Waivers, assess_ladder_at,
    drift_verdict,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Mutex, OwnedMutexGuard, broadcast, watch};
use tokio::task::JoinHandle;

use crate::config_block;
use crate::engine::{CommandError, Engine, EngineEvent, Precondition};
pub use crate::model::TuneId;
use crate::model::{
    ChannelMetrics, ControlSnapshot, FORMAT_VERSION, Filters, Gains, ProbeRecord, Run, RunId,
    RunRequest, RunState, Session, SessionId,
};
use crate::moonraker::MoonrakerApi;
use crate::pattern::Pattern;
use crate::stimulus::RunKind;
use crate::store::{Store, StoreError};

/// Probe run parameters for every step of a tune.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProbeSettings {
    /// Probe stroke speed in mm/s.
    pub speed_mm_s: f64,
    /// Raised velocity P used to excite the plant.
    pub high_gain: f64,
    /// Segments between gain toggles.
    pub segments_between_toggles: usize,
    /// Strokes per probe run.
    pub strokes: usize,
}

/// Standstill run parameters for every step of a tune.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StandstillSettings {
    /// Rests per run.
    pub rests: usize,
    /// Dwell per rest in seconds.
    pub dwell_s: f64,
}

/// What the operator fixed when creating a tune.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TuneSetup {
    /// Declared stepper section name.
    pub stepper: String,
    /// Tracking speeds in mm/s, one to three.
    pub speeds_mm_s: Vec<f64>,
    /// Stroke pattern.
    pub pattern: Pattern,
    /// Probe run parameters.
    pub probe: ProbeSettings,
    /// Strokes per tracking run.
    pub tracking_strokes: usize,
    /// Standstill run parameters.
    pub standstill: StandstillSettings,
    /// Acceptance gates.
    pub gates: Gates,
    /// Whether marginal rungs may be chosen.
    pub include_marginal: bool,
}

/// The core sessions a step's runs are filed under, one per run type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepSessions {
    /// Session holding the probe runs.
    pub probe: SessionId,
    /// Session holding the tracking runs.
    pub tracking: SessionId,
    /// Session holding the standstill runs.
    pub standstill: SessionId,
}

/// One rung of a step's ladder with its timing and per-repeat results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RungRecord {
    /// Value in the axis' display unit.
    pub shown: f64,
    /// Raw register value written.
    pub raw: i32,
    /// Runs executed for this rung, in order.
    pub run_ids: Vec<RunId>,
    /// Wall-clock start, milliseconds since the UNIX epoch.
    pub started_unix_ms: Option<u64>,
    /// Wall-clock end, milliseconds since the UNIX epoch.
    pub ended_unix_ms: Option<u64>,
    /// Why the rung was not run, if it was skipped.
    pub skipped: Option<String>,
    /// Per-repeat results, kept so spread survives alongside the verdict.
    pub repeat_values: Vec<RepeatResult>,
    /// Start and end of every run in `run_ids`, in the same order.
    #[serde(default)]
    pub run_times: Vec<RunTime>,
    /// The operator's decision to continue past this rung's unassessable result.
    #[serde(default)]
    pub waived: Option<Waiver>,
    /// Earlier attempts at this rung that a re-run replaced, oldest first.
    #[serde(default)]
    pub superseded: Vec<RungAttempt>,
}

/// An operator decision to continue past an unassessable result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Waiver {
    /// When the operator decided, milliseconds since the UNIX epoch.
    pub unix_ms: u64,
    /// What was unassessable; a damping waiver does not cover a standstill one.
    pub reason: Pause,
}

/// The runs and results of a rung attempt that a re-run replaced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RungAttempt {
    /// Runs the attempt executed, in order.
    pub run_ids: Vec<RunId>,
    /// Start and end of every run in `run_ids`, in the same order.
    pub run_times: Vec<RunTime>,
    /// Per-repeat results the attempt produced.
    pub repeat_values: Vec<RepeatResult>,
    /// When the attempt ended, milliseconds since the UNIX epoch.
    pub ended_unix_ms: Option<u64>,
}

/// Why and where a step stopped for an operator decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PauseRecord {
    /// Index of the rung whose result could not be assessed.
    pub rung: usize,
    /// What could not be assessed.
    pub reason: Pause,
    /// When the step paused, milliseconds since the UNIX epoch.
    pub unix_ms: u64,
}

/// The operator's answer to a paused step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseDecision {
    /// Continue past the unassessable result.
    Continue,
    /// Run the rung again.
    Rerun,
    /// End the step here.
    Stop,
}

/// When one run of a tune was submitted and when it finished.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTime {
    /// The run.
    pub id: RunId,
    /// Submission time, milliseconds since the UNIX epoch.
    pub started_unix_ms: u64,
    /// Time its final state was reported, milliseconds since the UNIX epoch.
    pub ended_unix_ms: u64,
}

/// One metric of the seed rung, from the ladder and from the re-run at the step's end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftRecord {
    /// Metric name, such as `zeta` or `p99 50 mm/s`.
    pub metric: String,
    /// Median over the seed rung's repeats in the ladder.
    pub first: Option<f64>,
    /// Value of the re-run.
    pub rerun: Option<f64>,
    /// Whether the change exceeds twice the speed's repeat spread; `Some` for the `p99` metrics
    /// once the step concludes, `None` for the others.
    pub verdict: Option<DriftVerdict>,
}

/// One axis of the tune: its ladder, progress and chosen value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TuneStep {
    /// Axis the ladder varies.
    pub axis: LadderAxis,
    /// Raw value the axis held when the step began.
    pub seed_raw: i32,
    /// Repeats per rung.
    pub repeats: u32,
    /// Lowest rung exponent: rungs are the seed times `2^(k/octave_divisions)`.
    pub lower_k: i32,
    /// Highest rung exponent.
    pub upper_k: i32,
    /// Divisions of an octave between neighbouring rungs; records from before it was chosen
    /// read as the third octave.
    #[serde(default = "default_octave_divisions")]
    pub octave_divisions: u32,
    /// PWM frequency the velocity I corners were converted with.
    pub pwm_hz: u32,
    /// Gains and filters every run starts from, the swept axis replaced per rung.
    pub base: (Gains, Filters),
    /// Sessions this step's runs are filed under.
    pub sessions: StepSessions,
    /// Rungs in ladder order.
    pub rungs: Vec<RungRecord>,
    /// Why rungs of the planned ladder were left out.
    #[serde(default)]
    pub notes: Vec<RungNote>,
    /// Raw value the operator chose.
    pub chosen: Option<i32>,
    /// The chosen value in the axis' display unit.
    #[serde(default)]
    pub chosen_shown: Option<f64>,
    /// How often the step was started, by its confirmation or an extension; a step task acts
    /// only while the step is on the start it was spawned for.
    #[serde(default)]
    pub starts: u32,
    /// Wall-clock start, milliseconds since the UNIX epoch.
    pub started_unix_ms: Option<u64>,
    /// Wall-clock end, milliseconds since the UNIX epoch.
    pub ended_unix_ms: Option<u64>,
    /// Why the step stopped early on a run that did not finish, if it did.
    #[serde(default)]
    pub failed: Option<String>,
    /// Runs the step announced when it was prepared, including the drift re-run; absent in
    /// records written before it was stored.
    #[serde(default)]
    pub run_count: Option<usize>,
    /// Verdicts over the measured rungs, set when the step ends and cleared when an extension
    /// adds a rung, so verdicts always line up with `rungs`.
    #[serde(default)]
    pub assessment: Option<LadderAssessment>,
    /// Recommendation from `assessment`; absent after a failed run.
    #[serde(default)]
    pub recommendation: Option<Recommended>,
    /// The seed rung run once more after the ladder.
    #[serde(default)]
    pub drift_rung: Option<RungRecord>,
    /// Per-metric drift of the seed rung; never gates.
    #[serde(default)]
    pub drift: Vec<DriftRecord>,
    /// Set while the step waits for the operator after an unassessable result.
    #[serde(default)]
    pub paused: Option<PauseRecord>,
    /// When the operator turned the standstill gate off for this step, milliseconds since the
    /// UNIX epoch.
    #[serde(default)]
    pub standstill_gate_off: Option<u64>,
}

/// Where a tune is in its life cycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TuneState {
    /// Waiting for the next step to be prepared.
    Ready,
    /// A step is prepared and awaits the operator's confirmation.
    Announced,
    /// A step is executing.
    Running,
    /// A step finished and awaits the operator's choice.
    AwaitingChoice,
    /// A step stopped on an unassessable result and awaits the operator's decision.
    Paused,
    /// Every step is complete.
    Done,
    /// Given up by the operator.
    Abandoned,
    /// The snapshot restore has not yet been attempted.
    RestorePending,
    /// The snapshot restore failed; the reason is meant for display.
    RestoreFailed(String),
}

impl TuneState {
    /// Whether the tune is over; every state except `Done` and `Abandoned` keeps its stepper
    /// reserved.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done | Self::Abandoned)
    }

    /// Why the setup of a tune in this state cannot change, in words for the operator and
    /// ending in what to do instead; `None` when it can. `has_steps` is whether any step was
    /// prepared.
    pub fn setup_refusal(&self, has_steps: bool) -> Option<&'static str> {
        const STARTED: &str = "This tune has already started its first step, so its setup is \
            fixed. To use different settings, abandon it and start a new tune.";
        match self {
            Self::Abandoned => Some(
                "This tune was abandoned, so its setup can't change any more. Start a new tune.",
            ),
            Self::Done => {
                Some("This tune is finished, so its setup can't change any more. Start a new tune.")
            }
            Self::RestorePending | Self::RestoreFailed(_) => Some(
                "This tune is restoring the starting gains, so its setup can't change right now.",
            ),
            Self::Announced | Self::Running | Self::AwaitingChoice | Self::Paused => Some(STARTED),
            Self::Ready if has_steps => Some(STARTED),
            Self::Ready => None,
        }
    }
}

/// A tune record, persisted as `tunes/<id>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tune {
    /// [`crate::model::FORMAT_VERSION`].
    pub format_version: u32,
    /// The tune's identifier.
    pub id: TuneId,
    /// What the operator fixed at creation.
    pub setup: TuneSetup,
    /// Gains and filters in effect when the tune was created.
    pub snapshot: (Gains, Filters),
    /// Steps in execution order.
    pub steps: Vec<TuneStep>,
    /// Life-cycle state.
    pub state: TuneState,
    /// Creation time, seconds since the UNIX epoch.
    pub created_unix_s: u64,
    /// Remarks on the tune's life cycle, such as a restart or an operator decision.
    #[serde(default)]
    pub notes: Vec<TuneNote>,
}

/// A remark recorded on a tune, meant for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuneNote {
    /// When it was recorded, milliseconds since the UNIX epoch.
    pub unix_ms: u64,
    /// The remark.
    pub text: String,
}

impl TuneNote {
    fn now(text: impl Into<String>) -> Self {
        Self {
            unix_ms: now_ms(),
            text: text.into(),
        }
    }
}

/// The value the operator takes from a finished step.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Choice {
    /// The step's recommendation, measured or interpolated.
    Recommended,
    /// A rung of the step, by index, measured with every repeat.
    Rung(usize),
    /// A value in the axis' display unit: raw for velocity P and position P, Hz for the
    /// velocity I corner.
    Value(f64),
}

const AXES: [LadderAxis; 3] = [
    LadderAxis::VelocityP,
    LadderAxis::VelocityICornerHz,
    LadderAxis::PositionP,
];
const MAX_SPEEDS: usize = 3;
const STORE_POLL: Duration = Duration::from_millis(50);
const IDLE_WAIT: Duration = Duration::from_secs(10);
const CHANGE_CAPACITY: usize = 64;
const IDLE_POLL: Duration = Duration::from_millis(50);

/// Where a step's seed comes from.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Seed {
    /// The axis' live gain, read when the step is prepared.
    Live,
    /// A value in the axis' display unit: raw for velocity P and position P, Hz for the
    /// velocity I corner.
    Typed(f64),
}

/// The operator's setup of one step.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StepRequest {
    /// Seed of the ladder.
    pub seed: Seed,
    /// Repeats per rung, at least one.
    pub repeats: u32,
    /// Lowest rung exponent.
    pub lower_k: i32,
    /// Highest rung exponent, at least `lower_k`.
    pub upper_k: i32,
    /// Divisions of an octave between neighbouring rungs, one of [`OCTAVE_DIVISIONS`]; the third
    /// octave when left out.
    #[serde(default = "default_octave_divisions")]
    pub octave_divisions: u32,
}

/// The divisions of an octave a step may use: rungs 41%, 26%, 19% or 12% apart.
pub const OCTAVE_DIVISIONS: [u32; 4] = [2, 3, 4, 6];

fn default_octave_divisions() -> u32 {
    DEFAULT_OCTAVE_DIVISIONS
}

/// A prepared step, announced to the operator before it is confirmed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepPlan {
    /// Axis the ladder varies.
    pub axis: LadderAxis,
    /// Planned rungs and the reasons any were left out.
    pub rungs: Rungs,
    /// Runs the step submits, including the drift re-run of the seed rung.
    pub run_count: usize,
    /// What will happen, meant for display before the operator confirms.
    pub announcement: String,
}

/// Why a tune operation was refused or failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TuneError {
    /// The stepper's control state could not be read; the text says why.
    #[error("no control state: {0}")]
    NoControlState(String),
    /// The seed cannot start a ladder.
    #[error(transparent)]
    SeedRefused(#[from] SeedRefused),
    /// The stepper already has a tune that is not done or abandoned.
    #[error(
        "This stepper already has an unfinished tune. Finish or abandon it before starting another."
    )]
    StepperBusy(TuneId),
    /// The operation does not fit the tune's progress; the text says why.
    #[error("{0}")]
    OutOfOrder(String),
    /// No step is announced, so there is nothing to confirm.
    #[error("There is no prepared step to start. Prepare a step first.")]
    NotAnnounced,
    /// No such tune in the store.
    #[error("unknown tune {0}")]
    NotFound(TuneId),
    /// The engine refused or failed; the text is its reason.
    #[error("{0}")]
    Engine(String),
    /// A run precondition failed.
    #[error(transparent)]
    Precondition(#[from] Precondition),
    /// The setup or step request is not valid; the text says why.
    #[error("{0}")]
    InvalidRequest(String),
    /// The requested extension is not on offer or cannot be written; the text says why.
    #[error("{0}")]
    NoExtension(String),
    /// The store failed; the text is the I/O error.
    #[error("tune store: {0}")]
    Store(String),
}

impl From<io::Error> for TuneError {
    fn from(e: io::Error) -> Self {
        Self::Store(e.to_string())
    }
}

/// Measurements of one repeat from its runs, each `None` when the run is missing.
///
/// `tracking` holds one run per chosen speed, in the setup's order. Only a `Done` run
/// contributes: a probe run that is not `Done` or has no velocity fit is unassessable, and a
/// tracking or standstill run that is not `Done` gives `None`.
pub fn repeat_result(
    probe: Option<&Run>,
    tracking: &[Option<&Run>],
    standstill: Option<&Run>,
) -> RepeatResult {
    let zeta = done_channel(probe)
        .and_then(|c| c.probe.as_ref())
        .and_then(ProbeRecord::headline_zeta)
        .map_or(RungOutcome::Unassessable, RungOutcome::Assessed);
    let p99_by_speed = tracking
        .iter()
        .map(|run| {
            done_channel(*run)
                .and_then(|c| c.tracking.first())
                .and_then(|t| t.p99_um)
        })
        .collect();
    let worst_torque_sd = done_channel(standstill)
        .and_then(|c| c.standstill.as_ref())
        .and_then(|s| s.worst_torque_sd);
    RepeatResult {
        zeta,
        p99_by_speed,
        worst_torque_sd,
    }
}

fn done_channel(run: Option<&Run>) -> Option<&ChannelMetrics> {
    let run = run.filter(|r| r.state == RunState::Done)?;
    run.metrics.as_ref()?.channels.iter().find(|c| c.declared)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn now_s() -> u64 {
    now_ms() / 1000
}

fn axis_name(axis: LadderAxis) -> &'static str {
    match axis {
        LadderAxis::VelocityP => "velocity P",
        LadderAxis::VelocityICornerHz => "velocity I corner (Hz)",
        LadderAxis::PositionP => "position P",
    }
}

fn shown_text(axis: LadderAxis, shown: f64) -> String {
    match axis {
        LadderAxis::VelocityICornerHz => format!("{shown:.1} Hz"),
        LadderAxis::VelocityP | LadderAxis::PositionP => format!("{shown:.0}"),
    }
}

fn halt_text(halt: Halt, gates: &Gates) -> String {
    match halt {
        Halt::BelowFloor { median } => format!(
            "median damping {median:.3} is below the floor {}",
            gates.damping_floor
        ),
        Halt::DampingUnassessable => pause_text(Pause::DampingUnassessable),
        Halt::StandstillBaselineUnassessable => pause_text(Pause::StandstillBaselineUnassessable),
    }
}

fn pause_text(pause: Pause) -> String {
    match pause {
        Pause::DampingUnassessable => "damping unassessable".to_owned(),
        Pause::StandstillBaselineUnassessable => "standstill baseline unassessable".to_owned(),
    }
}

fn stop_text(run: &Run) -> Option<String> {
    let id = &run.id;
    match &run.state {
        RunState::Done | RunState::NotComparable(_) => None,
        RunState::Failed(r) => Some(format!("run {id} failed: {r}")),
        RunState::Skipped(r) => Some(format!("run {id} skipped: {r}")),
        other => Some(format!("run {id} ended {}", other.name())),
    }
}

fn snapshot_of(c: &ControlSnapshot) -> (Gains, Filters) {
    (
        Gains {
            velocity_p: c.velocity_p,
            velocity_i: c.velocity_i,
            position_p: c.position_p,
            position_i: c.position_i,
        },
        Filters {
            velocity_hz: c.velocity_filter_hz,
            torque_hz: c.torque_filter_hz,
            position_hz: c.position_filter_hz,
            flux_hz: c.flux_filter_hz,
        },
    )
}

fn with_axis(mut gains: Gains, axis: LadderAxis, raw: i32) -> Gains {
    match axis {
        LadderAxis::VelocityP => gains.velocity_p = raw,
        LadderAxis::VelocityICornerHz => gains.velocity_i = raw,
        LadderAxis::PositionP => gains.position_p = raw,
    }
    gains
}

fn live_seed(axis: LadderAxis, gains: &Gains, pwm_hz: u32) -> Result<i32, SeedRefused> {
    match axis {
        LadderAxis::VelocityP => seed_from(axis, f64::from(gains.velocity_p), pwm_hz),
        LadderAxis::PositionP => seed_from(axis, f64::from(gains.position_p), pwm_hz),
        LadderAxis::VelocityICornerHz => Ok(gains.velocity_i),
    }
}

fn rung_record(shown: f64, raw: i32) -> RungRecord {
    RungRecord {
        shown,
        raw,
        run_ids: Vec::new(),
        started_unix_ms: None,
        ended_unix_ms: None,
        skipped: None,
        repeat_values: Vec::new(),
        run_times: Vec::new(),
        waived: None,
        superseded: Vec::new(),
    }
}

fn runs_per_repeat(setup: &TuneSetup) -> usize {
    setup.speeds_mm_s.len() + 2
}

/// Index of the rung at `k = 0`, if the ladder holds it.
fn seed_index(step: &TuneStep) -> Option<usize> {
    let seed = rung_values(step.axis, step.seed_raw, step.pwm_hz, 0, 0);
    let raw = seed.values.first()?.raw;
    step.rungs.iter().position(|r| r.raw == raw)
}

/// The operator's decisions on `step` as assessment input; only a damping waiver lets
/// assessment past a rung's missing zeta.
fn waivers(step: &TuneStep) -> Waivers {
    Waivers {
        continued: step
            .rungs
            .iter()
            .map(|r| {
                r.waived
                    .as_ref()
                    .is_some_and(|w| w.reason == Pause::DampingUnassessable)
            })
            .collect(),
        standstill_gate_off: step.standstill_gate_off.is_some(),
    }
}

fn assess(step: &TuneStep, setup: &TuneSetup) -> LadderAssessment {
    let results: Vec<Vec<RepeatResult>> =
        step.rungs.iter().map(|r| r.repeat_values.clone()).collect();
    assess_ladder_at(
        &results,
        step.repeats as usize,
        &setup.speeds_mm_s,
        &setup.gates,
        setup.include_marginal,
        &waivers(step),
    )
}

/// Indices of the rungs above `rung` that have neither run nor been skipped, ascending.
fn unrun_above(step: &TuneStep, rung: usize) -> Vec<usize> {
    step.rungs
        .iter()
        .enumerate()
        .skip(rung + 1)
        .filter(|(_, r)| r.run_ids.is_empty() && r.skipped.is_none())
        .map(|(i, _)| i)
        .collect()
}

/// Records the assessment's pause on the step, if it has one, and marks the tune paused.
fn pause_if_unassessable(tune: &mut Tune, index: usize, assessment: LadderAssessment) -> bool {
    let Some(step) = tune.steps.get_mut(index) else {
        return false;
    };
    let Some((rung, reason)) = assessment.pause else {
        return false;
    };
    step.paused = Some(PauseRecord {
        rung,
        reason,
        unix_ms: now_ms(),
    });
    step.assessment = Some(assessment);
    tune.state = TuneState::Paused;
    true
}

fn drift_records(
    first: &LadderAssessment,
    seed: usize,
    rerun: &RepeatResult,
    speeds: &[f64],
) -> Vec<DriftRecord> {
    let Some(medians) = first.medians.get(seed) else {
        return Vec::new();
    };
    let rerun_zeta = match rerun.zeta {
        RungOutcome::Assessed(z) => Some(z),
        RungOutcome::Unassessable => None,
    };
    let mut out = vec![DriftRecord {
        metric: "zeta".to_owned(),
        first: medians.zeta,
        rerun: rerun_zeta,
        verdict: None,
    }];
    out.extend(speeds.iter().enumerate().map(|(s, speed)| DriftRecord {
        metric: format!("p99 {speed} mm/s"),
        first: medians.p99_by_speed.get(s).copied().flatten(),
        rerun: rerun.p99_by_speed.get(s).copied().flatten(),
        verdict: None,
    }));
    out.push(DriftRecord {
        metric: "worst torque sd".to_owned(),
        first: medians.worst_torque_sd,
        rerun: rerun.worst_torque_sd,
        verdict: None,
    });
    out
}

/// Which record of the running step a repeat's runs are filed in.
#[derive(Debug, Clone, Copy)]
enum Target {
    Rung(usize),
    Drift,
}

/// What a step needs from its tune while it runs; fixed between confirm and the step's end.
#[derive(Clone)]
struct StepContext {
    tune: TuneId,
    setup: TuneSetup,
    index: usize,
    start: u32,
    axis: LadderAxis,
    repeats: u32,
    sessions: StepSessions,
    base: (Gains, Filters),
}

impl StepContext {
    fn new(tune: &Tune) -> Option<Self> {
        let index = tune.steps.len().checked_sub(1)?;
        let step = &tune.steps[index];
        Some(Self {
            tune: tune.id.clone(),
            setup: tune.setup.clone(),
            index,
            start: step.starts,
            axis: step.axis,
            repeats: step.repeats,
            sessions: step.sessions.clone(),
            base: step.base,
        })
    }

    fn request(&self, kind: RunKind, shown: f64, raw: i32) -> RunRequest {
        let session = match &kind {
            RunKind::Probe { .. } => &self.sessions.probe,
            RunKind::Tracking { .. } => &self.sessions.tracking,
            RunKind::Standstill { .. } => &self.sessions.standstill,
        };
        RunRequest {
            session: session.clone(),
            stepper: self.setup.stepper.clone(),
            kind,
            pattern: self.setup.pattern,
            gains: with_axis(self.base.0, self.axis, raw),
            filters: self.base.1,
            ladder_value: Some(shown),
        }
    }
}

/// How a step's run loop, or one part of it, ended.
enum Ended {
    /// Every planned run finished.
    Completed,
    /// The ladder halted on a gate; the rungs above it are skipped.
    Halted,
    /// A rung could not be assessed; the tune is paused for the operator's decision.
    Paused,
    /// A run did not finish or was refused; the reason is recorded on the step.
    Stopped(String),
    /// The tune left `Running` under the loop, so it records nothing more.
    Superseded,
}

/// What a guarded submit did.
enum Submitted {
    /// The run is queued; the receiver was subscribed before it.
    Run(RunId, broadcast::Receiver<EngineEvent>),
    /// The engine refused the run.
    Refused(Precondition),
    /// The tune left `Running`, so nothing was submitted.
    Superseded,
}

struct Shared<M> {
    engine: Arc<Engine<M>>,
    store: Store,
    steppers: StdMutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Per tune, a flag set once its step task, failure handling included, has finished.
    tasks: StdMutex<HashMap<TuneId, watch::Receiver<bool>>>,
    /// The task that marks a tune abandoned when its stepper's restore lock clears.
    listener: OnceLock<JoinHandle<()>>,
    /// Every tune record as it is persisted.
    changes: broadcast::Sender<Arc<Tune>>,
}

impl<M> Drop for Shared<M> {
    fn drop(&mut self) {
        if let Some(listener) = self.listener.get() {
            listener.abort();
        }
    }
}

/// Runs tune steps: prepares and announces a step, starts it on the operator's confirmation,
/// and submits its core runs one at a time.
///
/// Every state transition of a tune holds its stepper's async mutex, so of two concurrent
/// transitions on one stepper exactly one succeeds. The scheduler only submits core runs; it
/// never sends motion or enable commands of its own.
pub struct TuneScheduler<M> {
    shared: Arc<Shared<M>>,
}

impl<M> Clone for TuneScheduler<M> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl<M: MoonrakerApi + 'static> TuneScheduler<M> {
    /// A scheduler over `engine` and `store`; tunes are read from and written to `store`.
    ///
    /// Starts a task that marks a tune awaiting its restore abandoned once the engine clears
    /// its stepper's restore lock; the task ends with the last clone of the scheduler. Must be
    /// called inside a tokio runtime.
    pub fn new(engine: Arc<Engine<M>>, store: Store) -> Self {
        let events = engine.events();
        Self::with_events(engine, store, events)
    }

    /// Like [`TuneScheduler::new`], reading restore-lock events from `events` instead of a fresh
    /// engine subscription; a receiver that lags makes the scheduler re-check every tune that
    /// awaits its restore against the engine's locks.
    pub fn with_events(
        engine: Arc<Engine<M>>,
        store: Store,
        events: broadcast::Receiver<EngineEvent>,
    ) -> Self {
        let shared = Arc::new(Shared {
            engine,
            store,
            steppers: StdMutex::default(),
            tasks: StdMutex::default(),
            listener: OnceLock::new(),
            changes: broadcast::channel(CHANGE_CAPACITY).0,
        });
        let listener = tokio::spawn(watch_lock_cleared(Arc::downgrade(&shared), events));
        let _ = shared.listener.set(listener);
        Self { shared }
    }

    /// Subscribes to tune records, each sent as it is persisted: a rung finishing, a state
    /// change, a choice. A subscriber that falls behind gets `Lagged` and should refetch.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Tune>> {
        self.shared.changes.subscribe()
    }

    /// Creates a tune for `setup.stepper` with the stepper's live gains and filters as its
    /// snapshot, written to the store before returning.
    ///
    /// # Errors
    /// [`TuneError::InvalidRequest`] for a setup without one to three speeds,
    /// [`TuneError::Precondition`] for a locked stepper or a refused read,
    /// [`TuneError::StepperBusy`] while the stepper has an unfinished tune,
    /// [`TuneError::NoControlState`] when the control state cannot be read, and store errors.
    pub async fn create(&self, setup: TuneSetup) -> Result<TuneId, TuneError> {
        if setup.speeds_mm_s.is_empty() || setup.speeds_mm_s.len() > MAX_SPEEDS {
            return Err(TuneError::InvalidRequest(format!(
                "a tune needs one to {MAX_SPEEDS} tracking speeds"
            )));
        }
        let s = &self.shared;
        let _held = s.hold(&setup.stepper).await;
        s.refuse_locked(&setup.stepper)?;
        if let Some(busy) = s.store.nonterminal_tune_for(&setup.stepper)? {
            return Err(TuneError::StepperBusy(busy));
        }
        let control = s.read_control_state(&setup.stepper).await?;
        let tune = Tune {
            format_version: FORMAT_VERSION,
            id: TuneId::generate(SystemTime::now()),
            setup,
            snapshot: snapshot_of(&control),
            steps: Vec::new(),
            state: TuneState::Ready,
            created_unix_s: now_s(),
            notes: Vec::new(),
        };
        s.save(&tune)?;
        Ok(tune.id)
    }

    /// Replaces the setup of a tune that has no step yet. The tune keeps its id, snapshot and
    /// creation time; the stepper cannot change.
    ///
    /// # Errors
    /// [`TuneError::InvalidRequest`] for a setup without one to three speeds or for another
    /// stepper, [`TuneError::OutOfOrder`] unless the tune is ready with no step prepared,
    /// [`TuneError::Precondition`] for a locked stepper, [`TuneError::NotFound`], and store
    /// errors.
    pub async fn update_setup(&self, tune: &TuneId, setup: TuneSetup) -> Result<(), TuneError> {
        if setup.speeds_mm_s.is_empty() || setup.speeds_mm_s.len() > MAX_SPEEDS {
            return Err(TuneError::InvalidRequest(format!(
                "a tune needs one to {MAX_SPEEDS} tracking speeds"
            )));
        }
        let s = &self.shared;
        let _held = s.hold_tune(tune).await?;
        let mut record = s.read(tune)?;
        if let Some(why) = record.state.setup_refusal(!record.steps.is_empty()) {
            return Err(TuneError::OutOfOrder(why.to_owned()));
        }
        if setup.stepper != record.setup.stepper {
            return Err(TuneError::InvalidRequest(
                "the stepper of a tune cannot change".to_owned(),
            ));
        }
        s.refuse_locked(&setup.stepper)?;
        record.setup = setup;
        s.save(&record)?;
        Ok(())
    }

    /// Plans the tune's next step (velocity P, then velocity I, then position P) from a fresh
    /// read of the stepper's control state, creates its sessions and announces it. Nothing
    /// runs until [`TuneScheduler::confirm_step`].
    ///
    /// # Errors
    /// [`TuneError::OutOfOrder`] unless the tune is ready and the previous step has a chosen
    /// value, [`TuneError::InvalidRequest`] for zero repeats, an empty exponent range or no
    /// writable rung, [`TuneError::NoControlState`], [`TuneError::SeedRefused`], and store
    /// errors.
    pub async fn prepare_step(
        &self,
        tune: &TuneId,
        request: StepRequest,
    ) -> Result<StepPlan, TuneError> {
        let s = &self.shared;
        let _held = s.hold_tune(tune).await?;
        let mut record = s.read(tune)?;
        let axis = next_axis(&record)?;
        if request.repeats == 0 || request.lower_k > request.upper_k {
            return Err(TuneError::InvalidRequest(
                "a step needs at least one repeat and lower_k at most upper_k".to_owned(),
            ));
        }
        if !OCTAVE_DIVISIONS.contains(&request.octave_divisions) {
            return Err(TuneError::InvalidRequest(
                "The step size must be 12%, 19%, 26% or 41%.".to_owned(),
            ));
        }
        let stepper = record.setup.stepper.clone();
        s.refuse_locked(&stepper)?;
        let control = s.read_control_state(&stepper).await?;
        let base = snapshot_of(&control);
        let seed_raw = match request.seed {
            Seed::Live => live_seed(axis, &base.0, control.pwm_hz)?,
            Seed::Typed(shown) => seed_from(axis, shown, control.pwm_hz)?,
        };
        let rungs = rung_values_in(
            axis,
            seed_raw,
            control.pwm_hz,
            request.octave_divisions,
            request.lower_k,
            request.upper_k,
        );
        let (Some(lowest), Some(highest)) = (rungs.values.first(), rungs.values.last()) else {
            return Err(TuneError::InvalidRequest(
                "None of the gains in this range can be written to the stepper. Choose a seed or a range that stays inside what the stepper accepts."
                    .to_owned(),
            ));
        };
        let announcement_range = (lowest.shown, highest.shown);
        let sessions = s.create_sessions(&record, record.steps.len() + 1, base)?;
        let mut step = TuneStep {
            axis,
            seed_raw,
            repeats: request.repeats,
            lower_k: request.lower_k,
            upper_k: request.upper_k,
            octave_divisions: request.octave_divisions,
            pwm_hz: control.pwm_hz,
            base,
            sessions,
            rungs: rungs
                .values
                .iter()
                .map(|v| rung_record(v.shown, v.raw))
                .collect(),
            chosen: None,
            chosen_shown: None,
            starts: 0,
            started_unix_ms: None,
            ended_unix_ms: None,
            failed: None,
            assessment: None,
            recommendation: None,
            drift_rung: None,
            drift: Vec::new(),
            notes: rungs.notes.clone(),
            run_count: None,
            paused: None,
            standstill_gate_off: None,
        };
        let per_repeat = runs_per_repeat(&record.setup);
        let ladder_runs = step.rungs.len() * request.repeats as usize * per_repeat;
        let run_count = ladder_runs + seed_index(&step).map_or(0, |_| per_repeat);
        step.run_count = Some(run_count);
        let announcement = format!(
            "{stepper}: {} ladder of {} rungs from {} to {}, {} repeats each, {run_count} runs. \
             The machine will move once the step is confirmed.",
            axis_name(axis),
            step.rungs.len(),
            shown_text(axis, announcement_range.0),
            shown_text(axis, announcement_range.1),
            request.repeats,
        );
        record.steps.push(step);
        record.state = TuneState::Announced;
        s.save(&record)?;
        Ok(StepPlan {
            axis,
            rungs,
            run_count,
            announcement,
        })
    }

    /// The operator's start of the announced step: checks the run preconditions, marks the
    /// tune running and starts submitting the step's runs on a task of its own.
    ///
    /// A run that ends failed, killed or aborted, or is refused, stops the step: the rungs not
    /// yet run are recorded skipped, the step failed with the reason and without a
    /// recommendation, and the tune awaits a choice, so the operator can pick a measured rung
    /// or abandon. A `not comparable` run does not stop it.
    ///
    /// # Errors
    /// [`TuneError::NotAnnounced`] unless a step is announced, [`TuneError::Precondition`]
    /// when a run could not start now (a locked stepper among them), and store errors.
    pub async fn confirm_step(&self, tune: &TuneId) -> Result<(), TuneError> {
        let s = &self.shared;
        let _held = s.hold_tune(tune).await?;
        let mut record = s.read(tune)?;
        if record.state != TuneState::Announced {
            return Err(TuneError::NotAnnounced);
        }
        s.engine.check(&record.setup.stepper).await?;
        let Some(step) = record.steps.last_mut() else {
            return Err(TuneError::NotAnnounced);
        };
        step.started_unix_ms = Some(now_ms());
        step.starts += 1;
        let order = (0..step.rungs.len()).collect();
        record.state = TuneState::Running;
        s.save(&record)?;
        let Some(ctx) = StepContext::new(&record) else {
            return Err(TuneError::NotAnnounced);
        };
        s.spawn_step(ctx, order, true);
        Ok(())
    }

    /// Discards the announced step before anything has run, so its settings can be changed and
    /// the step prepared again. The tune goes back to ready and the step's empty sessions are
    /// removed.
    ///
    /// # Errors
    /// [`TuneError::OutOfOrder`] unless the tune has an announced step that has not started,
    /// and store errors.
    pub async fn cancel_step(&self, tune: &TuneId) -> Result<(), TuneError> {
        let s = &self.shared;
        let _held = s.hold_tune(tune).await?;
        let mut record = s.read(tune)?;
        if record.state != TuneState::Announced
            || record.steps.last().is_none_or(|st| st.starts != 0)
        {
            return Err(TuneError::OutOfOrder(
                if record.steps.is_empty() {
                    "There is no prepared step to adjust."
                } else {
                    "This step has already started, so it can't be adjusted."
                }
                .to_owned(),
            ));
        }
        let Some(step) = record.steps.pop() else {
            return Err(TuneError::OutOfOrder(
                "There is no prepared step to adjust.".to_owned(),
            ));
        };
        record.state = TuneState::Ready;
        s.save(&record)?;
        for session in [
            step.sessions.probe,
            step.sessions.tracking,
            step.sessions.standstill,
        ] {
            if let Err(e) = s.store.remove_empty_session(&session) {
                tracing::warn!(%session, error = %e, "a discarded step's session was not removed");
            }
        }
        Ok(())
    }

    /// Runs the one extra rung the step's recommendation offers, one exponent beyond the
    /// ladder's lower or upper end, then re-assesses the step. The drift re-run is not
    /// repeated.
    ///
    /// # Errors
    /// [`TuneError::OutOfOrder`] unless the step awaits a choice, [`TuneError::NoExtension`]
    /// when the recommendation does not offer `ext`, or the rung cannot be written or repeats
    /// a measured raw value,
    /// [`TuneError::Precondition`] when a run could not start now, and store errors.
    pub async fn confirm_extension(&self, tune: &TuneId, ext: Extension) -> Result<(), TuneError> {
        let s = &self.shared;
        let _held = s.hold_tune(tune).await?;
        let mut record = s.read(tune)?;
        if record.state != TuneState::AwaitingChoice {
            return Err(TuneError::OutOfOrder(
                "an extension needs a step that awaits a choice".to_owned(),
            ));
        }
        let Some(step) = record.steps.last_mut() else {
            return Err(TuneError::OutOfOrder("the tune has no step".to_owned()));
        };
        if step.recommendation.and_then(|r| r.extend) != Some(ext) {
            return Err(TuneError::NoExtension(
                "the recommendation does not offer this extension".to_owned(),
            ));
        }
        let k = match ext {
            Extension::Below => step.lower_k - 1,
            Extension::Above => step.upper_k + 1,
        };
        let Some(value) = rung_values_in(
            step.axis,
            step.seed_raw,
            step.pwm_hz,
            step.octave_divisions,
            k,
            k,
        )
        .values
        .first()
        .copied() else {
            return Err(TuneError::NoExtension(
                "The next gain is outside what the stepper accepts, so the ladder can't be extended that way."
                    .to_owned(),
            ));
        };
        if step.rungs.iter().any(|r| r.raw == value.raw) {
            return Err(TuneError::NoExtension(
                "The next gain would be the same as one already measured, so the ladder can't be extended that way."
                    .to_owned(),
            ));
        }
        s.engine.check(&record.setup.stepper).await?;
        let index = match ext {
            Extension::Below => {
                step.lower_k = k;
                step.rungs.insert(0, rung_record(value.shown, value.raw));
                0
            }
            Extension::Above => {
                step.upper_k = k;
                step.rungs.push(rung_record(value.shown, value.raw));
                step.rungs.len() - 1
            }
        };
        step.recommendation = None;
        step.assessment = None;
        step.starts += 1;
        record.state = TuneState::Running;
        s.save(&record)?;
        let Some(ctx) = StepContext::new(&record) else {
            return Err(TuneError::OutOfOrder("the tune has no step".to_owned()));
        };
        s.spawn_step(ctx, vec![index], false);
        Ok(())
    }

    /// The operator's answer to a paused step.
    ///
    /// `Continue` waives the paused rung (a standstill baseline waiver also turns the step's
    /// standstill gate off) and runs the rungs above it, then the drift check; should the
    /// waiver uncover another pause, the step stays paused on it without running anything.
    /// `Rerun` keeps the rung's runs as a superseded attempt and runs the rung again, then the
    /// rungs above it. `Stop` skips the rungs above and ends the step without a drift check,
    /// awaiting the operator's choice.
    ///
    /// # Errors
    /// [`TuneError::OutOfOrder`] unless the tune is paused, [`TuneError::Precondition`] when a
    /// run could not start now, and store errors.
    pub async fn decide(&self, tune: &TuneId, decision: PauseDecision) -> Result<(), TuneError> {
        let s = &self.shared;
        let _held = s.hold_tune(tune).await?;
        let mut record = s.read(tune)?;
        if record.state != TuneState::Paused {
            return Err(TuneError::OutOfOrder(
                "a decision needs a paused step".to_owned(),
            ));
        }
        let index = record
            .steps
            .len()
            .checked_sub(1)
            .ok_or_else(|| TuneError::OutOfOrder("the tune has no step".to_owned()))?;
        let Some(paused) = record.steps[index].paused.clone() else {
            return Err(TuneError::OutOfOrder(
                "the paused step has no pause record".to_owned(),
            ));
        };
        let order = match decision {
            PauseDecision::Continue => {
                s.engine.check(&record.setup.stepper).await?;
                let unix_ms = now_ms();
                let step = step_mut(&mut record, index)?;
                let rung = rung_mut(step, paused.rung)?;
                let damping_waiver_stands =
                    paused.reason == Pause::StandstillBaselineUnassessable && rung.waived.is_some();
                if !damping_waiver_stands {
                    rung.waived = Some(Waiver {
                        unix_ms,
                        reason: paused.reason,
                    });
                }
                if paused.reason == Pause::StandstillBaselineUnassessable {
                    step.standstill_gate_off = Some(unix_ms);
                }
                step.paused = None;
                step.assessment = None;
                record.notes.push(TuneNote::now(format!(
                    "operator continued past rung {}: {}",
                    paused.rung,
                    pause_text(paused.reason)
                )));
                let setup = record.setup.clone();
                let assessment = assess(&record.steps[index], &setup);
                step_mut(&mut record, index)?.assessment = Some(assessment.clone());
                if pause_if_unassessable(&mut record, index, assessment) {
                    s.save(&record)?;
                    return Ok(());
                }
                unrun_above(&record.steps[index], paused.rung)
            }
            PauseDecision::Rerun => {
                s.engine.check(&record.setup.stepper).await?;
                let step = step_mut(&mut record, index)?;
                let rung = rung_mut(step, paused.rung)?;
                let attempt = RungAttempt {
                    run_ids: std::mem::take(&mut rung.run_ids),
                    run_times: std::mem::take(&mut rung.run_times),
                    repeat_values: std::mem::take(&mut rung.repeat_values),
                    ended_unix_ms: rung.ended_unix_ms.take(),
                };
                rung.superseded.push(attempt);
                rung.started_unix_ms = None;
                rung.waived = None;
                step.paused = None;
                step.assessment = None;
                let mut order = vec![paused.rung];
                order.extend(unrun_above(step, paused.rung));
                order
            }
            PauseDecision::Stop => {
                let step = step_mut(&mut record, index)?;
                let reason = format!("not run: {}", pause_text(paused.reason));
                for i in unrun_above(step, paused.rung) {
                    rung_mut(step, i)?.skipped = Some(reason.clone());
                }
                step.paused = None;
                record.notes.push(TuneNote::now(format!(
                    "operator stopped the ladder at rung {}: {}",
                    paused.rung + 1,
                    pause_text(paused.reason)
                )));
                conclude(&mut record, index, None);
                s.save(&record)?;
                return Ok(());
            }
        };
        let step = step_mut(&mut record, index)?;
        step.starts += 1;
        // A step that has ended once (halted, stopped or completed) is past its first pass:
        // what runs now is an extension, which never repeats the drift check.
        let drift = step.drift_rung.is_none() && step.ended_unix_ms.is_none();
        record.state = TuneState::Running;
        s.save(&record)?;
        let Some(ctx) = StepContext::new(&record) else {
            return Err(TuneError::OutOfOrder("the tune has no step".to_owned()));
        };
        s.spawn_step(ctx, order, drift);
        Ok(())
    }

    /// Returns once no step task of `tune` is live: its runs are no longer being submitted and
    /// a step that ended on an error has been recorded as failed.
    ///
    /// The step task takes the stepper's transition lock to file every run, so a caller that
    /// holds that lock (any transition of a tune on the same stepper) must not wait here: it
    /// would wait for itself.
    pub async fn wait_idle(&self, tune: &TuneId) {
        let live = self.shared.live_task(tune);
        if let Some(mut done) = live {
            let _ = done.wait_for(|finished| *finished).await;
        }
    }

    /// Takes `choice` as the value of the step that awaits it: writes the step's gains with
    /// the swept axis set to it through the engine's confirmed write, records it on the step
    /// and readies the next step, whose live seed then reads it, or ends the tune after
    /// position P. The filters and the other gains are left as the step ran them.
    ///
    /// # Errors
    /// [`TuneError::OutOfOrder`] unless a step awaits a choice, [`TuneError::InvalidRequest`]
    /// for no recommendation, a rung that is not fully measured or a value that cannot be
    /// written, [`TuneError::Engine`] when the write is refused or fails (the tune still
    /// awaits a choice), and store errors.
    pub async fn choose(&self, tune: &TuneId, choice: Choice) -> Result<(), TuneError> {
        let s = &self.shared;
        let _held = s.hold_tune(tune).await?;
        let mut record = s.read(tune)?;
        if record.state != TuneState::AwaitingChoice {
            return Err(TuneError::OutOfOrder(
                "a choice needs a step that awaits one".to_owned(),
            ));
        }
        let stepper = record.setup.stepper.clone();
        let done = record.steps.len() >= AXES.len();
        let Some(step) = record.steps.last_mut() else {
            return Err(TuneError::OutOfOrder("the tune has no step".to_owned()));
        };
        let value = chosen_value(step, choice)?;
        s.engine
            .apply_gains(&stepper, with_axis(step.base.0, step.axis, value.raw))
            .await
            .map_err(TuneError::Engine)?;
        step.chosen = Some(value.raw);
        step.chosen_shown = Some(value.shown);
        step.ended_unix_ms.get_or_insert_with(now_ms);
        record.state = if done {
            TuneState::Done
        } else {
            TuneState::Ready
        };
        s.save(&record)?;
        Ok(())
    }

    /// Gives the tune up and restores the gains and filters it started from.
    ///
    /// The stepper is locked with the tune's snapshot and the tune marked restore-pending
    /// first, so neither its step nor any other route submits a further run. A step in
    /// progress then has the engine's queue aborted and is waited for, the run in flight
    /// included, before the snapshot is written; only a confirmed restore removes the lock,
    /// and a failed or unconfirmed one leaves the tune restore-failed. The work runs on a task
    /// of its own, so dropping the returned future does not stop it halfway.
    ///
    /// # Errors
    /// [`TuneError::OutOfOrder`] for a tune that is done or abandoned, [`TuneError::Engine`]
    /// when the restore failed, and store errors.
    pub async fn abandon(&self, tune: &TuneId) -> Result<(), TuneError> {
        let (shared, tune) = (self.shared.clone(), tune.clone());
        detached(async move { shared.abandon(&tune).await }).await
    }

    /// Writes the snapshot of a tune that awaits its restore again, with the outcomes of
    /// [`TuneScheduler::abandon`]: abandoned when confirmed, restore-failed and locked
    /// otherwise. The stepper is locked before anything is written, and the work runs on a
    /// task of its own.
    ///
    /// # Errors
    /// [`TuneError::OutOfOrder`] unless the tune is restore-pending or restore-failed,
    /// [`TuneError::Engine`] when the restore failed, and store errors.
    pub async fn restore(&self, tune: &TuneId) -> Result<(), TuneError> {
        let (shared, tune) = (self.shared.clone(), tune.clone());
        detached(async move { shared.restore_held(&tune, false).await }).await
    }

    /// Restart recovery: marks every unfinished tune restore-pending without resuming it and
    /// locks its stepper with the tune's snapshot, keeping a lock the engine already holds.
    /// Returns the tunes it marked.
    ///
    /// # Errors
    /// I/O errors from reading or writing the tune records. A lock that is not persisted
    /// still holds in memory and is only logged.
    pub async fn recover(&self) -> io::Result<Vec<TuneId>> {
        let s = &self.shared;
        let mut recovered = Vec::new();
        for mut tune in s.store.scan_tunes()? {
            if tune.state.is_terminal() {
                continue;
            }
            let stepper = tune.setup.stepper.clone();
            let _held = s.hold(&stepper).await;
            if s.engine.lock_state(&stepper).is_none()
                && let Err(e) =
                    s.engine
                        .lock_stepper(&stepper, "restart recovery".to_owned(), tune.snapshot)
            {
                tracing::error!(stepper, error = %e, "restart recovery lock not persisted");
            }
            tune.notes.push(TuneNote::now(format!(
                "the service restarted while the tune was {}; its restore is pending",
                state_text(&tune.state)
            )));
            tune.state = TuneState::RestorePending;
            s.save(&tune)?;
            recovered.push(tune.id);
        }
        Ok(recovered)
    }

    /// The `[foci <stepper>]` block with the three chosen values and the snapshot's other
    /// gains and filters; `None` until every step has a chosen value.
    pub fn config_block(&self, tune: &TuneId) -> Option<String> {
        let record = self.shared.read(tune).ok()?;
        let [velocity_p, velocity_i, position_p] = match record.steps.as_slice() {
            [p, i, pos] => [p.chosen?, i.chosen?, pos.chosen?],
            _ => return None,
        };
        let (snapshot_gains, filters) = record.snapshot;
        let gains = Gains {
            velocity_p,
            velocity_i,
            position_p,
            ..snapshot_gains
        };
        let pwm_hz = Some(record.steps[1].pwm_hz).filter(|&hz| hz > 0);
        Some(config_block::render(
            &record.setup.stepper,
            &gains,
            &filters,
            pwm_hz,
        ))
    }
}

async fn detached(
    work: impl Future<Output = Result<(), TuneError>> + Send + 'static,
) -> Result<(), TuneError> {
    tokio::spawn(work)
        .await
        .unwrap_or_else(|e| Err(TuneError::Engine(format!("the restore task failed: {e}"))))
}

fn awaits_restore(state: &TuneState) -> bool {
    matches!(
        state,
        TuneState::RestorePending | TuneState::RestoreFailed(_)
    )
}

fn state_text(state: &TuneState) -> &'static str {
    match state {
        TuneState::Ready => "ready",
        TuneState::Announced => "announced",
        TuneState::Running => "running",
        TuneState::AwaitingChoice => "awaiting a choice",
        TuneState::Paused => "paused",
        TuneState::Done => "done",
        TuneState::Abandoned => "abandoned",
        TuneState::RestorePending => "awaiting its restore",
        TuneState::RestoreFailed(_) => "restore-failed",
    }
}

/// The written value `choice` names on `step`.
fn chosen_value(step: &TuneStep, choice: Choice) -> Result<LadderValue, TuneError> {
    match choice {
        Choice::Recommended => match step.recommendation.map(|r| r.choice) {
            Some(Recommendation::Measured { rung }) => measured_rung(step, rung),
            Some(Recommendation::Interpolated { shown, .. }) => typed_value(step, shown),
            Some(Recommendation::None(_)) | None => Err(TuneError::InvalidRequest(
                "the step has no recommendation".to_owned(),
            )),
        },
        Choice::Rung(index) => measured_rung(step, index),
        Choice::Value(shown) => typed_value(step, shown),
    }
}

fn measured_rung(step: &TuneStep, index: usize) -> Result<LadderValue, TuneError> {
    step.rungs
        .get(index)
        .filter(|r| r.skipped.is_none() && r.repeat_values.len() >= step.repeats as usize)
        .map(|r| LadderValue {
            shown: r.shown,
            raw: r.raw,
        })
        .ok_or_else(|| {
            TuneError::InvalidRequest(format!("rung {index} is not a measured rung of the step"))
        })
}

fn typed_value(step: &TuneStep, shown: f64) -> Result<LadderValue, TuneError> {
    let raw = match step.axis {
        LadderAxis::VelocityP | LadderAxis::PositionP => {
            seed_from(step.axis, shown, step.pwm_hz).map_err(|e| e.to_string())
        }
        LadderAxis::VelocityICornerHz => to_raw(step.axis, shown, step.pwm_hz)
            .map(|v| v.raw)
            .map_err(|e| e.to_string()),
    };
    raw.map(|raw| LadderValue { shown, raw })
        .map_err(TuneError::InvalidRequest)
}

/// Marks every tune of a stepper that awaits its restore abandoned when the engine reports the
/// stepper's restore lock cleared: the board then holds the snapshot.
async fn watch_lock_cleared<M: MoonrakerApi + 'static>(
    shared: Weak<Shared<M>>,
    mut events: broadcast::Receiver<EngineEvent>,
) {
    loop {
        let stepper = match events.recv().await {
            Ok(EngineEvent::LockCleared { stepper }) => stepper,
            Ok(_) => continue,
            Err(RecvError::Lagged(_)) => {
                let Some(shared) = shared.upgrade() else {
                    return;
                };
                if let Err(e) = shared.reconcile_locks().await {
                    tracing::error!(error = %e, "restore locks not reconciled after missed events");
                }
                continue;
            }
            Err(RecvError::Closed) => return,
        };
        let Some(shared) = shared.upgrade() else {
            return;
        };
        if let Err(e) = shared.lock_cleared(&stepper).await {
            tracing::error!(stepper, error = %e, "cleared restore lock not recorded on its tune");
        }
    }
}

fn next_axis(tune: &Tune) -> Result<LadderAxis, TuneError> {
    let busy = match &tune.state {
        TuneState::Ready => None,
        TuneState::Announced => Some("a step is already announced"),
        TuneState::Running => Some("a step is running"),
        TuneState::AwaitingChoice => Some("the last step awaits a choice"),
        TuneState::Paused => Some("the step is paused"),
        TuneState::Done => Some("the tune is done"),
        TuneState::Abandoned => Some("the tune was abandoned"),
        TuneState::RestorePending | TuneState::RestoreFailed(_) => {
            Some("the tune awaits its restore")
        }
    };
    if let Some(reason) = busy {
        return Err(TuneError::OutOfOrder(reason.to_owned()));
    }
    if tune.steps.last().is_some_and(|s| s.chosen.is_none()) {
        return Err(TuneError::OutOfOrder(
            "Choose a value for the previous step before preparing the next one.".to_owned(),
        ));
    }
    AXES.get(tune.steps.len()).copied().ok_or_else(|| {
        TuneError::OutOfOrder("All three steps of this tune are already done.".to_owned())
    })
}

impl<M: MoonrakerApi + 'static> Shared<M> {
    async fn hold(&self, stepper: &str) -> OwnedMutexGuard<()> {
        let mutex = self
            .steppers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(stepper.to_owned())
            .or_default()
            .clone();
        mutex.lock_owned().await
    }

    async fn hold_tune(&self, tune: &TuneId) -> Result<OwnedMutexGuard<()>, TuneError> {
        let stepper = self.read(tune)?.setup.stepper;
        Ok(self.hold(&stepper).await)
    }

    fn live_task(&self, tune: &TuneId) -> Option<watch::Receiver<bool>> {
        self.tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(tune)
            .cloned()
    }

    async fn abandon(&self, tune: &TuneId) -> Result<(), TuneError> {
        {
            let _held = self.hold_tune(tune).await?;
            let mut record = self.read(tune)?;
            if record.state.is_terminal() {
                return Err(TuneError::OutOfOrder(
                    "This tune is already finished or abandoned, so there is nothing to abandon."
                        .to_owned(),
                ));
            }
            self.ensure_locked(&record, "abandon in progress");
            record.state = TuneState::RestorePending;
            self.save(&record)?;
        }
        if let Some(mut done) = self.live_task(tune) {
            self.engine.abort().await;
            let _ = done.wait_for(|finished| *finished).await;
        }
        self.wait_engine_idle().await;
        self.restore_held(tune, true).await
    }

    /// Waits a bounded time for the engine to finish persisting the last run, so the restore
    /// is not refused as busy; the lock already covers a restore that still is.
    async fn wait_engine_idle(&self) {
        let deadline = tokio::time::Instant::now() + IDLE_WAIT;
        while !self.engine.is_idle() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(IDLE_POLL).await;
        }
    }

    /// Locks the tune's stepper with its snapshot unless the stepper already has a lock.
    fn ensure_locked(&self, record: &Tune, reason: &str) {
        let stepper = &record.setup.stepper;
        if self.engine.lock_state(stepper).is_none()
            && let Err(e) = self
                .engine
                .lock_stepper(stepper, reason.to_owned(), record.snapshot)
        {
            tracing::error!(stepper, error = %e, "restore lock not persisted");
        }
    }

    /// Restores the snapshot of a tune that awaits its restore (or, `after_abandon`, one the
    /// lock-cleared update already marked abandoned), holding its stepper's transition lock
    /// throughout so concurrent restores and the lock-cleared update queue behind it. The
    /// stepper is locked with the snapshot before anything is written; a failure re-locks it
    /// with the reason unless it holds a lock on another snapshot.
    async fn restore_held(&self, tune: &TuneId, after_abandon: bool) -> Result<(), TuneError> {
        let _held = self.hold_tune(tune).await?;
        let mut record = self.read(tune)?;
        let abandoned = after_abandon && record.state == TuneState::Abandoned;
        if !awaits_restore(&record.state) && !abandoned {
            return Err(TuneError::OutOfOrder(
                "the tune does not await a restore".to_owned(),
            ));
        }
        self.ensure_locked(&record, "restore in progress");
        let stepper = record.setup.stepper.clone();
        let (gains, filters) = record.snapshot;
        let failure = match self.engine.restore_snapshot(&stepper, gains, filters).await {
            Ok(true) => None,
            Ok(false) => Some("the restore was confirmed but the stepper stays locked".to_owned()),
            Err(e) => Some(e),
        };
        let own_lock = self
            .engine
            .lock_state(&stepper)
            .is_none_or(|l| l.snapshot == record.snapshot);
        if let Some(reason) = &failure
            && own_lock
            && let Err(e) = self.engine.lock_stepper(
                &stepper,
                format!("restore failed: {reason}"),
                record.snapshot,
            )
        {
            tracing::error!(stepper, error = %e, "failed restore lock not persisted");
        }
        record.state = match &failure {
            None => TuneState::Abandoned,
            Some(reason) => TuneState::RestoreFailed(reason.clone()),
        };
        self.save(&record)?;
        failure.map_or(Ok(()), |reason| Err(TuneError::Engine(reason)))
    }

    /// A cleared event can trail a newer lock on the same stepper, so it applies only while
    /// the engine holds no lock for it.
    async fn reconcile_locks(&self) -> Result<(), TuneError> {
        let mut steppers: Vec<String> = self
            .store
            .scan_tunes()?
            .into_iter()
            .filter(|t| awaits_restore(&t.state))
            .map(|t| t.setup.stepper)
            .collect();
        steppers.sort();
        steppers.dedup();
        for stepper in steppers {
            self.lock_cleared(&stepper).await?;
        }
        Ok(())
    }

    async fn lock_cleared(&self, stepper: &str) -> Result<(), TuneError> {
        let _held = self.hold(stepper).await;
        if self.engine.lock_state(stepper).is_some() {
            return Ok(());
        }
        for mut tune in self.store.scan_tunes()? {
            if tune.setup.stepper == stepper && awaits_restore(&tune.state) {
                tune.state = TuneState::Abandoned;
                tune.notes.push(TuneNote::now(
                    "the board reported the starting gains; the restore lock cleared",
                ));
                self.save(&tune)?;
            }
        }
        Ok(())
    }

    /// Persists `tune`, then announces it to the subscribers.
    fn save(&self, tune: &Tune) -> io::Result<()> {
        self.store.write_tune(tune)?;
        let _ = self.changes.send(Arc::new(tune.clone()));
        Ok(())
    }

    fn read(&self, tune: &TuneId) -> Result<Tune, TuneError> {
        self.store.read_tune(tune).map_err(|e| match e {
            StoreError::Io(io) if io.kind() == io::ErrorKind::NotFound => {
                TuneError::NotFound(tune.clone())
            }
            other => TuneError::Store(other.to_string()),
        })
    }

    fn refuse_locked(&self, stepper: &str) -> Result<(), TuneError> {
        match self.engine.lock_state(stepper) {
            Some(lock) => Err(Precondition::TuneLocked(lock.reason).into()),
            None => Ok(()),
        }
    }

    async fn read_control_state(&self, stepper: &str) -> Result<ControlSnapshot, TuneError> {
        self.engine
            .read_control_state(stepper)
            .await
            .map_err(|e| match e {
                CommandError::Precondition(p) => TuneError::Precondition(p),
                other => TuneError::NoControlState(other.to_string()),
            })
    }

    fn create_sessions(
        &self,
        tune: &Tune,
        number: usize,
        base: (Gains, Filters),
    ) -> io::Result<StepSessions> {
        let setup = &tune.setup;
        let create = |label: &str, kind: RunKind| {
            self.store.new_session(&Session {
                format_version: FORMAT_VERSION,
                name: format!("{} step {number} {label}", tune.id),
                created_unix_s: now_s(),
                stepper: setup.stepper.clone(),
                kind,
                pattern: setup.pattern,
                gains: base.0,
                filters: base.1,
                ladder: None,
            })
        };
        Ok(StepSessions {
            probe: create("probe", probe_kind(setup))?,
            tracking: create(
                "tracking",
                RunKind::Tracking {
                    speeds_mm_s: setup.speeds_mm_s.clone(),
                    strokes: setup.tracking_strokes,
                },
            )?,
            standstill: create("standstill", standstill_kind(setup))?,
        })
    }

    /// The tune, if it is still `Running` the step and start `ctx` describes.
    fn running(&self, ctx: &StepContext) -> Result<Option<Tune>, TuneError> {
        let tune = self.read(&ctx.tune)?;
        let current = tune.state == TuneState::Running
            && tune.steps.len() == ctx.index + 1
            && tune.steps[ctx.index].starts == ctx.start;
        Ok(current.then_some(tune))
    }

    /// Applies `change` to the running tune under its stepper's mutex and persists it.
    /// Returns `None`, changing nothing, once the tune has left `Running`.
    async fn update<R>(
        &self,
        ctx: &StepContext,
        change: impl FnOnce(&mut Tune) -> Result<R, TuneError>,
    ) -> Result<Option<R>, TuneError> {
        let _held = self.hold(&ctx.setup.stepper).await;
        let Some(mut tune) = self.running(ctx)? else {
            return Ok(None);
        };
        let out = change(&mut tune)?;
        self.save(&tune)?;
        Ok(Some(out))
    }

    /// Submits `request` only while the tune is still `Running` this step, holding the
    /// stepper's mutex across the check and the submit so no transition can come between.
    async fn submit(&self, ctx: &StepContext, request: RunRequest) -> Result<Submitted, TuneError> {
        let _held = self.hold(&ctx.setup.stepper).await;
        if self.running(ctx)?.is_none() {
            return Ok(Submitted::Superseded);
        }
        let events = self.engine.events();
        Ok(match self.engine.submit(request).await {
            Ok(id) => Submitted::Run(id, events),
            Err(p) => Submitted::Refused(p),
        })
    }

    /// Runs the step on a task of its own. An error or a panic in it records the step as
    /// failed and leaves the tune awaiting a choice, so the stepper is never held by a dead
    /// task.
    fn spawn_step(self: &Arc<Self>, ctx: StepContext, order: Vec<usize>, drift: bool) {
        let (done, watcher) = watch::channel(false);
        self.tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(ctx.tune.clone(), watcher.clone());
        let shared = self.clone();
        tokio::spawn(async move {
            let (worker, step) = (shared.clone(), ctx.clone());
            let outcome =
                tokio::spawn(async move { worker.execute_step(&step, order, drift).await }).await;
            let failure = match outcome {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(format!("the step task ended unexpectedly: {e}")),
            };
            if let Some(reason) = failure {
                tracing::warn!(tune = %ctx.tune, error = %reason, "tune step stopped");
                shared.fail_running(&ctx, reason).await;
            }
            {
                let mut tasks = shared.tasks.lock().unwrap_or_else(PoisonError::into_inner);
                if tasks
                    .get(&ctx.tune)
                    .is_some_and(|w| w.same_channel(&watcher))
                {
                    tasks.remove(&ctx.tune);
                }
            }
            let _ = done.send(true);
        });
    }

    async fn fail_running(&self, ctx: &StepContext, reason: String) {
        let _held = self.hold(&ctx.setup.stepper).await;
        let written = self.running(ctx).and_then(|record| {
            let Some(mut record) = record else {
                return Ok(());
            };
            conclude(&mut record, ctx.index, Some(reason));
            Ok(self.save(&record)?)
        });
        if let Err(e) = written {
            tracing::error!(tune = %ctx.tune, error = %e, "failed step not recorded");
        }
    }

    async fn execute_step(
        &self,
        ctx: &StepContext,
        order: Vec<usize>,
        drift: bool,
    ) -> Result<(), TuneError> {
        let failed = match self.run_rungs(ctx, &order).await? {
            Ended::Superseded | Ended::Paused => return Ok(()),
            Ended::Stopped(reason) => Some(reason),
            Ended::Halted => None,
            Ended::Completed if drift => match self.run_drift(ctx).await? {
                Ended::Superseded | Ended::Paused => return Ok(()),
                Ended::Stopped(reason) => Some(reason),
                Ended::Completed | Ended::Halted => None,
            },
            Ended::Completed => None,
        };
        self.update(ctx, |tune| {
            conclude(tune, ctx.index, failed);
            Ok(())
        })
        .await?;
        Ok(())
    }

    /// Runs the rungs in `order`, all repeats of one before the next, and assesses the ladder
    /// after each rung. A rung that cannot be assessed pauses the tune and ends the loop.
    async fn run_rungs(&self, ctx: &StepContext, order: &[usize]) -> Result<Ended, TuneError> {
        for &index in order {
            let value = self
                .update(ctx, |t| {
                    let rung = rung_mut(step_mut(t, ctx.index)?, index)?;
                    rung.started_unix_ms = Some(now_ms());
                    Ok((rung.shown, rung.raw))
                })
                .await?;
            let Some((shown, raw)) = value else {
                return Ok(Ended::Superseded);
            };
            for _ in 0..ctx.repeats {
                match self
                    .run_repeat(ctx, Target::Rung(index), shown, raw)
                    .await?
                {
                    Ended::Completed => {}
                    other => return Ok(other),
                }
            }
            let ended = self
                .update(ctx, |t| {
                    let setup = t.setup.clone();
                    let step = step_mut(t, ctx.index)?;
                    rung_mut(step, index)?.ended_unix_ms = Some(now_ms());
                    let assessment = assess(step, &setup);
                    step.assessment = Some(assessment.clone());
                    if let Some((at, h)) = assessment.halt {
                        let reason = format!("not run: {}", halt_text(h, &setup.gates));
                        for rung in step.rungs.iter_mut().skip(at + 1) {
                            if rung.run_ids.is_empty() && rung.skipped.is_none() {
                                rung.skipped = Some(reason.clone());
                            }
                        }
                        return Ok(Some(Ended::Halted));
                    }
                    Ok(pause_if_unassessable(t, ctx.index, assessment).then_some(Ended::Paused))
                })
                .await?;
            match ended {
                None => return Ok(Ended::Superseded),
                Some(Some(end)) => return Ok(end),
                Some(None) => {}
            }
        }
        Ok(Ended::Completed)
    }

    /// Runs the seed rung once more and records each metric's drift from its first median.
    async fn run_drift(&self, ctx: &StepContext) -> Result<Ended, TuneError> {
        let repeats = ctx.repeats as usize;
        let seed = self
            .update(ctx, |t| {
                let step = step_mut(t, ctx.index)?;
                let measured = seed_index(step).and_then(|i| {
                    let rung = step.rungs.get(i)?;
                    (rung.repeat_values.len() >= repeats).then_some((i, rung.shown, rung.raw))
                });
                if let Some((_, shown, raw)) = measured {
                    let mut record = rung_record(shown, raw);
                    record.started_unix_ms = Some(now_ms());
                    step.drift_rung = Some(record);
                }
                Ok(measured)
            })
            .await?;
        let Some(seed) = seed else {
            return Ok(Ended::Superseded);
        };
        let Some((seed, shown, raw)) = seed else {
            return Ok(Ended::Completed);
        };
        match self.run_repeat(ctx, Target::Drift, shown, raw).await? {
            Ended::Completed => {}
            other => return Ok(other),
        }
        let recorded = self
            .update(ctx, |t| {
                let setup = t.setup.clone();
                let step = step_mut(t, ctx.index)?;
                let first = assess(step, &setup);
                let drift = step.drift_rung.as_mut().map(|rung| {
                    rung.ended_unix_ms = Some(now_ms());
                    rung.repeat_values
                        .first()
                        .map(|rerun| drift_records(&first, seed, rerun, &setup.speeds_mm_s))
                });
                if let Some(Some(records)) = drift {
                    step.drift = records;
                }
                Ok(())
            })
            .await?;
        Ok(match recorded {
            Some(()) => Ended::Completed,
            None => Ended::Superseded,
        })
    }

    /// Submits one repeat: the probe, one tracking run per chosen speed, then the standstill
    /// run, each after the previous one finished. Files every run and the repeat's result in
    /// `target`.
    async fn run_repeat(
        &self,
        ctx: &StepContext,
        target: Target,
        shown: f64,
        raw: i32,
    ) -> Result<Ended, TuneError> {
        let setup = &ctx.setup;
        let mut kinds = vec![probe_kind(setup)];
        kinds.extend(setup.speeds_mm_s.iter().map(|&speed| RunKind::Tracking {
            speeds_mm_s: vec![speed],
            strokes: setup.tracking_strokes,
        }));
        kinds.push(standstill_kind(setup));
        let mut runs = Vec::with_capacity(kinds.len());
        for kind in kinds {
            let started = now_ms();
            let (id, mut events) = match self.submit(ctx, ctx.request(kind, shown, raw)).await? {
                Submitted::Run(id, events) => (id, events),
                Submitted::Refused(p) => return Ok(Ended::Stopped(format!("run refused: {p}"))),
                Submitted::Superseded => return Ok(Ended::Superseded),
            };
            let run = self.wait_finished(&id, &mut events).await?;
            let time = RunTime {
                id: run.id.clone(),
                started_unix_ms: started,
                ended_unix_ms: now_ms(),
            };
            let filed = self
                .update(ctx, |t| {
                    let rung = target_rung(step_mut(t, ctx.index)?, target)?;
                    rung.run_ids.push(time.id.clone());
                    rung.run_times.push(time);
                    Ok(())
                })
                .await?;
            if filed.is_none() {
                return Ok(Ended::Superseded);
            }
            if let Some(reason) = stop_text(&run) {
                return Ok(Ended::Stopped(reason));
            }
            runs.push(run);
        }
        let speeds = setup.speeds_mm_s.len();
        let tracking: Vec<Option<&Run>> = runs[1..=speeds].iter().map(Some).collect();
        let result = repeat_result(runs.first(), &tracking, runs.get(speeds + 1));
        let filed = self
            .update(ctx, |t| {
                target_rung(step_mut(t, ctx.index)?, target)?
                    .repeat_values
                    .push(result);
                Ok(())
            })
            .await?;
        Ok(match filed {
            Some(()) => Ended::Completed,
            None => Ended::Superseded,
        })
    }

    /// Waits for `run`'s final state. Falls back to polling the store when the event stream
    /// lagged past it.
    async fn wait_finished(
        &self,
        run: &RunId,
        events: &mut broadcast::Receiver<EngineEvent>,
    ) -> Result<Run, TuneError> {
        loop {
            match events.recv().await {
                Ok(EngineEvent::Finished(done)) if done.id == *run => return Ok(*done),
                Ok(_) => {}
                Err(RecvError::Lagged(_)) => return self.poll_finished(run).await,
                Err(RecvError::Closed) => {
                    return Err(TuneError::Engine("the engine stopped".to_owned()));
                }
            }
        }
    }

    async fn poll_finished(&self, run: &RunId) -> Result<Run, TuneError> {
        loop {
            if let Some(session) = self.store.find_run(run)?
                && let Ok(found) = self.store.read_run(&session, run)
                && found.state.is_terminal()
            {
                return Ok(found);
            }
            tokio::time::sleep(STORE_POLL).await;
        }
    }
}

/// Ends the step at `index`: rungs never run are skipped when the step failed, rungs left open
/// are closed, the ladder is assessed and, unless the step failed, a gain recommended; the tune
/// then awaits the operator's choice.
fn conclude(tune: &mut Tune, index: usize, failed: Option<String>) {
    let setup = tune.setup.clone();
    if let Some(step) = tune.steps.get_mut(index) {
        let now = now_ms();
        for rung in step.rungs.iter_mut().chain(step.drift_rung.as_mut()) {
            if let Some(reason) = &failed
                && rung.run_ids.is_empty()
                && rung.skipped.is_none()
            {
                rung.skipped = Some(format!("not run: {reason}"));
            }
            if rung.started_unix_ms.is_some() && rung.ended_unix_ms.is_none() {
                rung.ended_unix_ms = Some(now);
            }
        }
        let assessment = assess(step, &setup);
        for (s, d) in step
            .drift
            .iter_mut()
            .filter(|d| d.metric.starts_with("p99 "))
            .enumerate()
        {
            let spread = assessment.spreads.get(s).copied().flatten();
            d.verdict = Some(drift_verdict(d.first, d.rerun, spread));
        }
        let shown: Vec<f64> = step.rungs.iter().map(|r| r.shown).collect();
        step.recommendation = failed.is_none().then(|| recommend(&assessment, &shown));
        step.assessment = Some(assessment);
        step.failed = failed;
        step.ended_unix_ms = Some(now);
    }
    tune.state = TuneState::AwaitingChoice;
}

fn missing(what: String) -> TuneError {
    TuneError::Store(format!("the running step's record has no {what}"))
}

fn step_mut(tune: &mut Tune, index: usize) -> Result<&mut TuneStep, TuneError> {
    tune.steps
        .get_mut(index)
        .ok_or_else(|| missing(format!("step {index}")))
}

fn rung_mut(step: &mut TuneStep, index: usize) -> Result<&mut RungRecord, TuneError> {
    step.rungs
        .get_mut(index)
        .ok_or_else(|| missing(format!("rung {index}")))
}

fn target_rung(step: &mut TuneStep, target: Target) -> Result<&mut RungRecord, TuneError> {
    match target {
        Target::Rung(i) => rung_mut(step, i),
        Target::Drift => step
            .drift_rung
            .as_mut()
            .ok_or_else(|| missing("drift rung".to_owned())),
    }
}

fn probe_kind(setup: &TuneSetup) -> RunKind {
    RunKind::Probe {
        speed_mm_s: setup.probe.speed_mm_s,
        high_gain: setup.probe.high_gain,
        segments_between_toggles: setup.probe.segments_between_toggles,
        strokes: setup.probe.strokes,
    }
}

fn standstill_kind(setup: &TuneSetup) -> RunKind {
    RunKind::Standstill {
        rests: setup.standstill.rests,
        dwell_s: setup.standstill.dwell_s,
    }
}
