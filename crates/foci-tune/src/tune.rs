//! Tune records: the plan, per-step progress and per-rung results of an automated tune.

use foci_tune_analysis::ladder::LadderAxis;
use foci_tune_analysis::verdict::{Gates, RepeatResult};
use serde::{Deserialize, Serialize};

pub use crate::model::TuneId;
use crate::model::{Filters, Gains, RunId, SessionId};
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
