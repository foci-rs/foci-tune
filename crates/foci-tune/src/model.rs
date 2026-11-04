//! Persisted data model: sessions, runs, metrics and plot series.
//!
//! Every float that analysis can leave non-finite is an `Option<f64>` here, `None` written as
//! JSON `null`, so `run.json` and `series.json` always read back.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use foci_tune_analysis::fit::{DampedFit, FitError};
use foci_tune_analysis::probe::{EdgeDamping, EdgeDampingError, ProbeMetrics};
use foci_tune_analysis::protocol::{ControlState, Metadata};
use foci_tune_analysis::standstill::StandstillMetrics;
use foci_tune_analysis::tracking::TrackingMetrics;
use foci_tune_analysis::units::Regime;
use rand::RngExt;
use serde::{Deserialize, Serialize};

use crate::pattern::Pattern;
use crate::stimulus::RunKind;

/// `format_version` written into `session.json` and `run.json`.
pub const FORMAT_VERSION: u32 = 1;

/// `format_version` written into `series.json`; a stored series of another version is rebuilt
/// from the capture.
pub const SERIES_FORMAT_VERSION: u32 = 2;

const ID_SUFFIX_LEN: usize = 4;
const ID_SUFFIX_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const MAX_ID_LEN: usize = 64;

/// Builds an id: the UTC time to the second, then four random characters, for example
/// `20261030T174512Z-k3q9`. Ids sort by creation time.
pub fn new_id(now: SystemTime) -> String {
    let secs = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days as i64);
    let mut rng = rand::rng();
    let suffix: String = (0..ID_SUFFIX_LEN)
        .map(|_| char::from(ID_SUFFIX_ALPHABET[rng.random_range(0..ID_SUFFIX_ALPHABET.len())]))
        .collect();
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z-{suffix}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// An id that is not usable as a single path component.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid id `{0}`")]
pub struct InvalidId(pub String);

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Generates a fresh id for `now`.
            pub fn generate(now: SystemTime) -> Self {
                Self(new_id(now))
            }

            /// Accepts ASCII letters, digits and `-`, so the id is safe as a directory name.
            ///
            /// # Errors
            /// [`InvalidId`] for anything else, or an empty or over-long string.
            pub fn parse(s: &str) -> Result<Self, InvalidId> {
                let ok = !s.is_empty()
                    && s.len() <= MAX_ID_LEN
                    && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
                if ok { Ok(Self(s.to_owned())) } else { Err(InvalidId(s.to_owned())) }
            }

            /// The id as text.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                Self::parse(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

id_type!(
    /// Identifier of a session; its directory name.
    SessionId
);
id_type!(
    /// Identifier of a run; its directory name inside the session.
    RunId
);
id_type!(
    /// Identifier of a tune; its file name under `tunes/`.
    TuneId
);

fn finite(v: f64) -> Option<f64> {
    v.is_finite().then_some(v)
}

/// The four outer-loop gains as raw register values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gains {
    /// Velocity P.
    pub velocity_p: i32,
    /// Velocity I.
    pub velocity_i: i32,
    /// Position P.
    pub position_p: i32,
    /// Position I, held from the session's baseline frame.
    pub position_i: i32,
}

/// The four filter cutoffs in Hz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filters {
    /// Velocity filter.
    pub velocity_hz: u16,
    /// Torque filter.
    pub torque_hz: u16,
    /// Position filter.
    pub position_hz: u16,
    /// Flux filter.
    pub flux_hz: u16,
}

/// A stepper whose gains must be restored before it runs again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lock {
    /// Why the stepper is locked, meant for display.
    pub reason: String,
    /// The gains and filters whose confirmed restore, or appearance in a control-state frame,
    /// clears the lock.
    pub snapshot: (Gains, Filters),
}

/// A ladder definition: one axis varied over a value list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LadderDef {
    /// Axis name: `velocity_p`, `velocity_i_corner_hz` or `position_p`.
    pub axis: String,
    /// Values in the axis' display unit.
    pub values: Vec<f64>,
    /// Repeats per value.
    pub repeats: u32,
    /// Damping floor of the stop rule.
    pub floor: f64,
    /// Threshold below which a value is marked marginal.
    pub marginal: f64,
}

/// `session.json`: the setup fixed for a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// [`FORMAT_VERSION`].
    pub format_version: u32,
    /// Operator-visible name.
    pub name: String,
    /// Creation time, seconds since the UNIX epoch.
    pub created_unix_s: u64,
    /// Declared stepper section name.
    pub stepper: String,
    /// Run type and its parameters.
    pub kind: RunKind,
    /// Stroke pattern.
    pub pattern: Pattern,
    /// Gains in effect when the session started, restored by Revert.
    pub gains: Gains,
    /// Filters in effect when the session started, restored by Revert.
    pub filters: Filters,
    /// Ladder definition, if the session is a ladder.
    pub ladder: Option<LadderDef>,
}

/// What a run was asked to do; travels with the run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRequest {
    /// Owning session.
    pub session: SessionId,
    /// Declared stepper section name.
    pub stepper: String,
    /// Run type and its parameters.
    pub kind: RunKind,
    /// Stroke pattern.
    pub pattern: Pattern,
    /// Gains written before the stimulus.
    pub gains: Gains,
    /// Filters written before the stimulus.
    pub filters: Filters,
    /// Ladder value in the axis' display unit, for ladder runs.
    pub ladder_value: Option<f64>,
}

/// Lifecycle state of a run.
///
/// Serialised adjacently tagged: `{"state": "skipped", "reason": "..."}`, and just
/// `{"state": "queued"}` for variants without a reason. Inside [`Run`] the two keys are
/// flattened to the run's top level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum RunState {
    /// Waiting in the queue.
    Queued,
    /// Preconditions being checked.
    Preparing,
    /// Gains and filters being written and confirmed.
    WritingGains,
    /// Waiting for the baseline control-state frame.
    Baseline,
    /// Capture recording, stimulus not yet sent.
    Recording,
    /// Stimulus executing.
    Moving,
    /// Capture draining after motion.
    Draining,
    /// Verification running.
    Verifying,
    /// Analysis running.
    Analyzing,
    /// Finished and verified.
    Done,
    /// Finished, but a verification check failed; data kept.
    NotComparable(String),
    /// Ended by an error.
    Failed(String),
    /// Stopped by the operator after the current run.
    Aborted,
    /// Not run, for example after a ladder stop rule.
    Skipped(String),
    /// Emergency stop posted, not yet confirmed.
    Killing,
    /// Emergency stop could not be confirmed; motion may continue.
    KillUnconfirmed,
    /// Emergency stop confirmed by Klipper shutdown.
    Killed,
}

/// Snapshot of a channel's control-state frame.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlSnapshot {
    /// Change sequence counter.
    pub change_sequence: u32,
    /// Tick at which the last change was applied.
    pub applied_tick: u64,
    /// Applied request code.
    pub applied_request: u8,
    /// Encoder counts per revolution.
    pub encoder_counts_per_rev: u32,
    /// PWM frequency in Hz.
    pub pwm_hz: u32,
    /// Velocity meter selector.
    pub velocity_meter: u8,
    /// Velocity filter corner in Hz.
    pub velocity_filter_hz: u16,
    /// Torque filter corner in Hz.
    pub torque_filter_hz: u16,
    /// Position filter corner in Hz.
    pub position_filter_hz: u16,
    /// Flux filter corner in Hz.
    pub flux_filter_hz: u16,
    /// Velocity P gain.
    pub velocity_p: i32,
    /// Velocity I gain.
    pub velocity_i: i32,
    /// Position P gain.
    pub position_p: i32,
    /// Position I gain.
    pub position_i: i32,
    /// Torque P gain.
    pub torque_p: i32,
    /// Torque I gain.
    pub torque_i: i32,
    /// Flux P gain.
    pub flux_p: i32,
    /// Flux I gain.
    pub flux_i: i32,
    /// Velocity feedforward enabled.
    pub velocity_ff_enable: bool,
    /// Velocity feedforward in permille.
    pub velocity_ff_permille: u16,
    /// Acceleration feedforward enabled.
    pub accel_ff_enable: bool,
    /// Acceleration feedforward gain.
    pub accel_ff_gain: u16,
    /// Deceleration feedforward gain.
    pub decel_ff_gain: u16,
    /// Run current setting.
    pub run_current: u32,
    /// Voltage limit setting.
    pub voltage_limit: u16,
}

impl From<&ControlState> for ControlSnapshot {
    fn from(c: &ControlState) -> Self {
        Self {
            change_sequence: c.change_sequence,
            applied_tick: c.applied_tick,
            applied_request: c.applied_request,
            encoder_counts_per_rev: c.encoder_counts_per_rev,
            pwm_hz: c.pwm_hz,
            velocity_meter: c.velocity_meter,
            velocity_filter_hz: c.velocity_filter_hz,
            torque_filter_hz: c.torque_filter_hz,
            position_filter_hz: c.position_filter_hz,
            flux_filter_hz: c.flux_filter_hz,
            velocity_p: c.velocity_p,
            velocity_i: c.velocity_i,
            position_p: c.position_p,
            position_i: c.position_i,
            torque_p: c.torque_p,
            torque_i: c.torque_i,
            flux_p: c.flux_p,
            flux_i: c.flux_i,
            velocity_ff_enable: c.velocity_ff_enable,
            velocity_ff_permille: c.velocity_ff_permille,
            accel_ff_enable: c.accel_ff_enable,
            accel_ff_gain: c.accel_ff_gain,
            decel_ff_gain: c.decel_ff_gain,
            run_current: c.run_current,
            voltage_limit: c.voltage_limit,
        }
    }
}

/// The baseline control-state frame of one channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelBaseline {
    /// Source index of the channel.
    pub channel: u8,
    /// Its baseline frame.
    pub control: ControlSnapshot,
}

/// Strings and rates from the stream's metadata frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamInfo {
    /// Trace protocol version.
    pub protocol_version: u8,
    /// Firmware identifier.
    pub firmware_id: String,
    /// Product string.
    pub product: String,
    /// Serial string.
    pub serial: String,
    /// MCU clock frequency in Hz.
    pub clock_freq: u32,
    /// Critical sample rate in Hz.
    pub critical_hz: u32,
}

impl StreamInfo {
    /// Builds the record from a decoded metadata frame and its protocol version.
    pub fn new(protocol_version: u8, m: &Metadata) -> Self {
        Self {
            protocol_version,
            firmware_id: m.firmware_id.clone(),
            product: m.product.clone(),
            serial: m.serial.clone(),
            clock_freq: m.clock_freq,
            critical_hz: m.critical_hz,
        }
    }
}

/// The conversion constants the series need, recorded so `series.json` can be regenerated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRegime {
    /// Motor rotation distance in mm.
    pub motor_rotation_distance_mm: f64,
    /// Encoder counts per revolution from the baseline frame.
    pub encoder_counts_per_rev: u32,
    /// PWM frequency in Hz from the baseline frame.
    pub pwm_hz: u32,
    /// MCU clock frequency in Hz from the metadata frame.
    pub clock_freq: u32,
}

impl From<&RunRegime> for Regime {
    fn from(r: &RunRegime) -> Self {
        Self {
            motor_rotation_distance_mm: r.motor_rotation_distance_mm,
            encoder_counts_per_rev: r.encoder_counts_per_rev,
            pwm_hz: r.pwm_hz,
            clock_freq: r.clock_freq,
        }
    }
}

/// When a sequence step started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepTime {
    /// Step name, such as `writing_gains` or `moving`.
    pub step: String,
    /// Start time, milliseconds since the UNIX epoch.
    pub unix_ms: u64,
}

/// Size of the stored capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureInfo {
    /// Firmware frames written.
    pub frames: u64,
    /// Compressed file size.
    pub file_bytes: u64,
}

/// The verification checks of the run sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationCheck {
    /// The declared channel moved and no other channel did.
    ChannelMoved,
    /// `change_sequence` advanced only by the tool's own writes.
    BaselineContiguous,
    /// No frame carries the drop flag.
    NoDropGaps,
    /// Probe runs only: enough edges were located.
    EdgeCount,
}

/// One verification outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verification {
    /// Which check.
    pub check: VerificationCheck,
    /// Whether it passed.
    pub passed: bool,
    /// Measured values or the reason it failed.
    pub detail: String,
}

/// A damping fit or the reason there is none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FitOutcome {
    /// A converged fit.
    Fit {
        /// Damped frequency in Hz.
        damped_hz: f64,
        /// Damping ratio.
        zeta: f64,
        /// Cosine amplitude.
        amplitude: f64,
        /// Cosine phase in radians.
        phase: f64,
        /// Exponential term amplitude.
        offset: f64,
        /// Exponential time constant in seconds.
        tau_s: f64,
        /// Residual RMS.
        residual_rms: f64,
        /// How many multi-start seeds produced an admissible fit; zero in records
        /// written before it was tracked.
        #[serde(default)]
        admissible_starts: usize,
    },
    /// The fit failed or produced non-finite parameters.
    Failed {
        /// Why.
        reason: String,
    },
}

impl From<&Result<DampedFit, FitError>> for FitOutcome {
    fn from(r: &Result<DampedFit, FitError>) -> Self {
        match r {
            Ok(f)
                if [
                    f.damped_hz,
                    f.zeta,
                    f.amplitude,
                    f.phase,
                    f.offset,
                    f.tau_s,
                    f.residual_rms,
                ]
                .iter()
                .all(|v| v.is_finite()) =>
            {
                Self::Fit {
                    damped_hz: f.damped_hz,
                    zeta: f.zeta,
                    amplitude: f.amplitude,
                    phase: f.phase,
                    offset: f.offset,
                    tau_s: f.tau_s,
                    residual_rms: f.residual_rms,
                    admissible_starts: f.admissible_starts,
                }
            }
            Ok(_) => Self::Failed {
                reason: "fit parameters are not finite".into(),
            },
            Err(e) => Self::Failed {
                reason: e.to_string(),
            },
        }
    }
}

/// Tracking metrics for one speed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackingRecord {
    /// Applied stepper speed in mm/s.
    pub speed_mm_s: f64,
    /// Mean `|error|` over cruise samples in um.
    pub cruise_mean_abs_um: Option<f64>,
    /// Error standard deviation over moving samples in um.
    pub sd_um: Option<f64>,
    /// 99th percentile of `|error|` in um.
    pub p99_um: Option<f64>,
    /// Mean peak error near reversals in um; `None` with no reversals.
    pub reversal_peak_um: Option<f64>,
    /// Number of reversals.
    pub reversal_count: usize,
    /// Number of moving samples.
    pub moving_samples: usize,
}

impl TrackingRecord {
    /// Converts analysis output, mapping non-finite values to `None`.
    pub fn new(speed_mm_s: f64, m: &TrackingMetrics) -> Self {
        Self {
            speed_mm_s,
            cruise_mean_abs_um: finite(m.cruise_mean_abs_um),
            sd_um: finite(m.sd_um),
            p99_um: finite(m.p99_um),
            reversal_peak_um: finite(m.reversal_peak_um),
            reversal_count: m.reversal_count,
            moving_samples: m.moving_samples,
        }
    }
}

/// A per-edge damping assessment or the reason there is none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DampingOutcome {
    /// The edges share one ring frequency and give a median damping ratio.
    Assessed {
        /// Median damping ratio over the fitted edges.
        zeta: f64,
        /// Median damped frequency in Hz.
        damped_hz: f64,
        /// Scaled MAD of the per-edge damped frequency in Hz.
        hz_spread: f64,
        /// Edges whose fit was admissible.
        fitted: usize,
        /// Edge responses offered to the fit.
        edges: usize,
    },
    /// No single damping ratio applies.
    Unassessable {
        /// Why.
        reason: String,
    },
}

impl From<&Result<EdgeDamping, EdgeDampingError>> for DampingOutcome {
    fn from(r: &Result<EdgeDamping, EdgeDampingError>) -> Self {
        match r {
            Ok(d)
                if [d.zeta, d.damped_hz, d.hz_spread]
                    .iter()
                    .all(|v| v.is_finite()) =>
            {
                Self::Assessed {
                    zeta: d.zeta,
                    damped_hz: d.damped_hz,
                    hz_spread: d.hz_spread,
                    fitted: d.fitted,
                    edges: d.edges,
                }
            }
            Ok(_) => Self::Unassessable {
                reason: "damping values are not finite".to_owned(),
            },
            Err(e) => Self::Unassessable {
                reason: e.to_string(),
            },
        }
    }
}

/// Probe metrics for one channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeRecord {
    /// Velocity fit.
    pub velocity: FitOutcome,
    /// Torque fit.
    pub torque: FitOutcome,
    /// Position-error fit.
    pub position_error: FitOutcome,
    /// Edges located.
    pub edges: usize,
    /// Standard error at the ensemble peak.
    pub stderr_at_peak: Option<f64>,
    /// Welch PSD peak of torque in Hz.
    pub torque_psd_peak_hz: Option<f64>,
    /// Per-edge damping; absent in records written before it was tracked.
    #[serde(default)]
    pub damping: Option<DampingOutcome>,
}

impl ProbeRecord {
    /// The damping ratio rungs are gated on.
    ///
    /// The per-edge assessment when the record has one; otherwise the ensemble velocity fit.
    pub fn headline_zeta(&self) -> Option<f64> {
        match &self.damping {
            Some(DampingOutcome::Assessed { zeta, .. }) => Some(*zeta),
            Some(DampingOutcome::Unassessable { .. }) => None,
            None => match self.velocity {
                FitOutcome::Fit { zeta, .. } => Some(zeta),
                FitOutcome::Failed { .. } => None,
            },
        }
    }
}

impl From<&ProbeMetrics> for ProbeRecord {
    fn from(m: &ProbeMetrics) -> Self {
        Self {
            velocity: (&m.velocity).into(),
            torque: (&m.torque).into(),
            position_error: (&m.position_error).into(),
            edges: m.edges,
            stderr_at_peak: finite(m.stderr_at_peak),
            torque_psd_peak_hz: finite(m.torque_psd_peak_hz),
            damping: Some((&m.damping).into()),
        }
    }
}

/// Dither metrics for one rest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RestRecord {
    /// Index among the qualifying rests.
    pub index: usize,
    /// Samples measured.
    pub samples: usize,
    /// Torque standard deviation.
    pub torque_sd: Option<f64>,
    /// Peak-to-peak position error in um.
    pub position_pp_um: Option<f64>,
    /// Torque-target value changes per second.
    #[serde(default)]
    pub transitions_per_s: Option<f64>,
    /// Torque-target changes of more than one count per second.
    #[serde(default)]
    pub large_transitions_per_s: Option<f64>,
}

/// Standstill metrics for one channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StandstillRecord {
    /// Rests in capture order.
    pub rests: Vec<RestRecord>,
    /// Largest torque standard deviation.
    pub worst_torque_sd: Option<f64>,
    /// Largest large-transition rate over the rests that carry one.
    #[serde(default)]
    pub worst_large_transitions_per_s: Option<f64>,
}

impl From<&StandstillMetrics> for StandstillRecord {
    fn from(m: &StandstillMetrics) -> Self {
        let rests: Vec<RestRecord> = m
            .rests
            .iter()
            .map(|r| RestRecord {
                index: r.index,
                samples: r.samples,
                torque_sd: finite(r.torque_sd),
                position_pp_um: finite(r.position_pp_um),
                transitions_per_s: finite(r.transitions_per_s),
                large_transitions_per_s: finite(r.large_transitions_per_s),
            })
            .collect();
        let worst_large_transitions_per_s = rests
            .iter()
            .filter_map(|r| r.large_transitions_per_s)
            .fold(None, |m, v| Some(m.map_or(v, |m: f64| m.max(v))));
        Self {
            rests,
            worst_torque_sd: if m.rests.is_empty() {
                None
            } else {
                finite(m.worst_torque_sd)
            },
            worst_large_transitions_per_s,
        }
    }
}

/// All metrics of one channel; empty members for run types that do not apply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelMetrics {
    /// Source index of the channel.
    pub channel: u8,
    /// Whether this is the declared channel.
    pub declared: bool,
    /// Tracking metrics per speed.
    pub tracking: Vec<TrackingRecord>,
    /// Probe metrics.
    pub probe: Option<ProbeRecord>,
    /// Standstill metrics.
    pub standstill: Option<StandstillRecord>,
}

/// Metrics for every channel, the declared channel first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    /// Per-channel metrics.
    pub channels: Vec<ChannelMetrics>,
}

impl Metrics {
    /// Metrics holding one declared channel with tracking and probe results.
    pub fn for_tracking_and_probe(
        channel: u8,
        tracking: Vec<TrackingRecord>,
        probe: Option<ProbeRecord>,
    ) -> Self {
        Self {
            channels: vec![ChannelMetrics {
                channel,
                declared: true,
                tracking,
                probe,
                standstill: None,
            }],
        }
    }
}

/// `run.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// [`FORMAT_VERSION`].
    pub format_version: u32,
    /// Run id.
    pub id: RunId,
    /// The request, including the pattern.
    pub request: RunRequest,
    /// Lifecycle state; its `state` and `reason` keys sit at the top level of the document.
    #[serde(flatten)]
    pub state: RunState,
    /// Creation time, seconds since the UNIX epoch.
    pub created_unix_s: u64,
    /// Source index of the declared channel.
    pub declared_channel: u8,
    /// Conversion constants, recorded once the baseline and metadata frames are known.
    pub regime: Option<RunRegime>,
    /// Baseline control-state frame per channel.
    pub baselines: Vec<ChannelBaseline>,
    /// Metadata strings and protocol version.
    pub stream: Option<StreamInfo>,
    /// Start time of each sequence step.
    pub steps: Vec<StepTime>,
    /// `(requested, applied)` mm/s for every speed reduced to `max_velocity`.
    pub clamped_speeds: Vec<(f64, f64)>,
    /// Applied stepper speeds in script block order.
    pub applied_speeds: Vec<f64>,
    /// Predicted duration of the stimulus script in seconds; absent in runs recorded before it
    /// was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicted_s: Option<f64>,
    /// Stored capture size.
    pub capture: Option<CaptureInfo>,
    /// Verification outcomes.
    pub verification: Vec<Verification>,
    /// Analysis results.
    pub metrics: Option<Metrics>,
    /// Operator-visible remarks that are not errors, such as a trace mode left unrestored.
    #[serde(default)]
    pub notes: Vec<String>,
}

impl RunState {
    /// True for states a run never leaves on its own: finished, stopped or not run.
    ///
    /// `KillUnconfirmed` counts as terminal: only a confirmed emergency stop changes it.
    pub fn is_terminal(&self) -> bool {
        match self {
            Self::Done
            | Self::NotComparable(_)
            | Self::Failed(_)
            | Self::Aborted
            | Self::Skipped(_)
            | Self::KillUnconfirmed
            | Self::Killed => true,
            Self::Queued
            | Self::Preparing
            | Self::WritingGains
            | Self::Baseline
            | Self::Recording
            | Self::Moving
            | Self::Draining
            | Self::Verifying
            | Self::Analyzing
            | Self::Killing => false,
        }
    }

    /// The state's `snake_case` name, as `run.json` stores it.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Preparing => "preparing",
            Self::WritingGains => "writing_gains",
            Self::Baseline => "baseline",
            Self::Recording => "recording",
            Self::Moving => "moving",
            Self::Draining => "draining",
            Self::Verifying => "verifying",
            Self::Analyzing => "analyzing",
            Self::Done => "done",
            Self::NotComparable(_) => "not_comparable",
            Self::Failed(_) => "failed",
            Self::Aborted => "aborted",
            Self::Skipped(_) => "skipped",
            Self::Killing => "killing",
            Self::KillUnconfirmed => "kill_unconfirmed",
            Self::Killed => "killed",
        }
    }
}

/// Decimated plot series of one channel; `None` marks a non-finite sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelPlot {
    /// Source index of the channel.
    pub channel: u8,
    /// Seconds since the first frame.
    pub t_s: Vec<Option<f64>>,
    /// Position error in um.
    pub position_error_um: Vec<Option<f64>>,
    /// Velocity in mm/s.
    pub velocity_mm_s: Vec<Option<f64>>,
    /// Target torque, raw.
    pub torque: Vec<Option<f64>>,
    /// Feedforward offset, raw.
    pub ff_offset: Vec<Option<f64>>,
    /// Step-response ensembles and their fits, for probe runs.
    #[serde(default)]
    pub probe: Option<ProbePlot>,
    /// Welch spectrum of the detrended torque, for probe runs long enough to have one.
    #[serde(default)]
    pub torque_psd: Option<PsdPlot>,
}

/// One signal's ensemble-mean step response and its fitted curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnsemblePlot {
    /// Edges averaged.
    pub edges: usize,
    /// Mean response normalised by the feedforward step, one value per [`ProbePlot::t_s`].
    pub mean: Vec<Option<f64>>,
    /// The fitted model at the same instants, `None` outside the fit range; absent when the
    /// fit failed.
    pub fit: Option<Vec<Option<f64>>>,
}

/// The step-response ensembles of one channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbePlot {
    /// Seconds after the edge.
    pub t_s: Vec<f64>,
    /// Velocity response.
    pub velocity: EnsemblePlot,
    /// Target-torque response.
    pub torque: EnsemblePlot,
    /// Position-error response.
    pub position_error: EnsemblePlot,
}

/// A power spectrum; `None` marks a non-finite bin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PsdPlot {
    /// Bin frequencies in Hz.
    pub freq_hz: Vec<f64>,
    /// Mean power per bin.
    pub power: Vec<Option<f64>>,
}

/// `series.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Series {
    /// [`SERIES_FORMAT_VERSION`].
    pub format_version: u32,
    /// Every `decimation`-th frame is kept.
    pub decimation: u32,
    /// One entry per channel with critical frames.
    pub channels: Vec<ChannelPlot>,
}
