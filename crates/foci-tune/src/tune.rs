//! Tune records: the plan, per-step progress and per-rung results of an automated tune.

use foci_tune_analysis::ladder::{LadderAxis, RungOutcome};
use foci_tune_analysis::recommend::Recommended;
use foci_tune_analysis::verdict::{Gates, LadderAssessment, RepeatResult};
use serde::{Deserialize, Serialize};

pub use crate::model::TuneId;
use crate::model::{ChannelMetrics, Filters, FitOutcome, Gains, Run, RunId, RunState, SessionId};
use crate::pattern::Pattern;

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
    /// Lowest rung exponent: rungs are the seed times `2^(k/3)`.
    pub lower_k: i32,
    /// Highest rung exponent.
    pub upper_k: i32,
    /// PWM frequency the velocity I corners were converted with.
    pub pwm_hz: u32,
    /// Gains and filters every run starts from, the swept axis replaced per rung.
    pub base: (Gains, Filters),
    /// Sessions this step's runs are filed under.
    pub sessions: StepSessions,
    /// Rungs in ladder order.
    pub rungs: Vec<RungRecord>,
    /// Raw value the operator chose.
    pub chosen: Option<i32>,
    /// Wall-clock start, milliseconds since the UNIX epoch.
    pub started_unix_ms: Option<u64>,
    /// Wall-clock end, milliseconds since the UNIX epoch.
    pub ended_unix_ms: Option<u64>,
    /// Why the step stopped early on a run that did not finish, if it did.
    #[serde(default)]
    pub failed: Option<String>,
    /// Verdicts over the measured rungs, set when the step ends.
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
    let zeta = match done_channel(probe).and_then(|c| c.probe.as_ref()) {
        Some(p) => match p.velocity {
            FitOutcome::Fit { zeta, .. } => RungOutcome::Assessed(zeta),
            FitOutcome::Failed { .. } => RungOutcome::Unassessable,
        },
        None => RungOutcome::Unassessable,
    };
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
