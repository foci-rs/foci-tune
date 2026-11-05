//! JSON API over HTTP and the embedded UI.
//!
//! Handlers check requests and translate between JSON and the engine and store; the run
//! sequence and its rules stay in [`crate::engine`]. Every error is a JSON body
//! `{"error": "<reason meant for display>"}`, with `"field"` naming the offending request
//! field on a 400.

use std::collections::BTreeMap;
use std::io;
use std::ops::Deref;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use bytes::Bytes;
use foci_tune_analysis::ladder::PID_GAIN_MAX_RAW;
use foci_tune_analysis::recommend::Extension;
use foci_tune_analysis::signals::corner_hz;
use foci_tune_analysis::units::POSITION_UNITS_PER_REV;
use foci_tune_analysis::verdict::Gates;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{broadcast, watch};
use tokio::time::MissedTickBehavior;

use crate::assets;
use crate::config_block;
use crate::engine::{
    CommandError, Engine, LadderRejected, LadderRequest, Precondition, TraceModeCommand,
};
use crate::live::{LiveState, TraceMode};
use crate::model::{
    ControlSnapshot, FORMAT_VERSION, Filters, Gains, LadderDef, Run, RunId, RunRequest, RunState,
    Session, SessionId,
};
use crate::moonraker::{KlippyState, McuVersion, MoonrakerApi};
use crate::pattern::{Pattern, PatternError, propose_pattern};
use crate::steppers::{BedLimits, Stepper};
use crate::stimulus::{RunKind, StimulusError, compose};
use crate::store::{SessionSummary, Store, StoreError};
use crate::tune::{
    Choice, PauseDecision, StepPlan, StepRequest, Tune, TuneError, TuneId, TuneScheduler, TuneSetup,
};
use crate::ws;

const SNAPSHOT_PERIOD: Duration = Duration::from_secs(1);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(5);
/// A snapshot that differs only in free space or measured cadence is republished at most
/// this often.
const VOLATILE_PERIOD: Duration = Duration::from_secs(5);
const DEFAULT_REPEATS: u32 = 2;
const DEFAULT_FLOOR: f64 = 0.05;
const DEFAULT_MARGINAL: f64 = 0.08;

/// Everything the handlers share.
pub struct Api<M> {
    /// The run engine.
    pub engine: Arc<Engine<M>>,
    /// The Moonraker client.
    pub moonraker: Arc<M>,
    /// Live trace state per board.
    pub live: Arc<Mutex<LiveState>>,
    /// The trace reader's broadcast of `(serial, frame)`.
    pub frames: broadcast::Sender<(String, Bytes)>,
    /// The data directory.
    pub store: Store,
    /// The guided tune scheduler.
    pub tunes: TuneScheduler<M>,
}

impl<M> Clone for Api<M> {
    fn clone(&self) -> Self {
        Self {
            engine: self.engine.clone(),
            moonraker: self.moonraker.clone(),
            live: self.live.clone(),
            frames: self.frames.clone(),
            store: self.store.clone(),
            tunes: self.tunes.clone(),
        }
    }
}

/// The latest printer snapshot, `None` until the first one is built.
pub type PrinterFeed = watch::Receiver<Option<Arc<PrinterView>>>;

/// Router state: the shared handles plus the printer snapshot feed.
pub struct App<M> {
    api: Api<M>,
    printer: PrinterFeed,
}

impl<M> Clone for App<M> {
    fn clone(&self) -> Self {
        Self {
            api: self.api.clone(),
            printer: self.printer.clone(),
        }
    }
}

impl<M> Deref for App<M> {
    type Target = Api<M>;

    fn deref(&self) -> &Api<M> {
        &self.api
    }
}

impl<M> App<M> {
    /// A receiver of the shared printer snapshot.
    pub fn printer_feed(&self) -> PrinterFeed {
        self.printer.clone()
    }
}

/// An error response: a status, a reason meant for display and, for a bad request, the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    status: StatusCode,
    message: String,
    field: Option<String>,
}

impl ApiError {
    /// An error with `status` and the displayable `message`.
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            field: None,
        }
    }

    /// A 400 naming the request field at fault.
    pub fn bad_field(field: &str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
            field: Some(field.to_owned()),
        }
    }

    /// The HTTP status.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// The reason meant for display.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The request field at fault, for a 400.
    pub fn field(&self) -> Option<&str> {
        self.field.as_deref()
    }
}

impl From<Precondition> for ApiError {
    fn from(p: Precondition) -> Self {
        Self::new(StatusCode::CONFLICT, p.to_string())
    }
}

impl From<LadderRejected> for ApiError {
    fn from(e: LadderRejected) -> Self {
        match e {
            LadderRejected::Precondition(p) => p.into(),
            LadderRejected::UnknownAxis(_) => Self::bad_field("axis", e.to_string()),
            LadderRejected::Empty
            | LadderRejected::OutOfRange(_)
            | LadderRejected::TooManyRuns { .. } => Self::bad_field("values", e.to_string()),
            LadderRejected::TooLarge { parameter, .. } => Self::bad_field(parameter, e.to_string()),
            LadderRejected::NoControlState => Self::new(StatusCode::CONFLICT, e.to_string()),
        }
    }
}

impl From<TuneError> for ApiError {
    fn from(e: TuneError) -> Self {
        match e {
            TuneError::Precondition(p) => p.into(),
            TuneError::NotFound(_) => Self::new(StatusCode::NOT_FOUND, e.to_string()),
            TuneError::InvalidRequest(_) => Self::new(StatusCode::BAD_REQUEST, e.to_string()),
            TuneError::SeedRefused(_) => Self::bad_field("seed", e.to_string()),
            TuneError::Store(_) => Self::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            TuneError::NoControlState(_)
            | TuneError::StepperBusy(_)
            | TuneError::OutOfOrder(_)
            | TuneError::NotAnnounced
            | TuneError::Engine(_)
            | TuneError::NoExtension(_) => Self::new(StatusCode::CONFLICT, e.to_string()),
        }
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        let status = match &e {
            StoreError::Io(io) if io.kind() == io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
            StoreError::Capture(_) => StatusCode::UNPROCESSABLE_ENTITY,
            StoreError::Io(_) | StoreError::Json(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self::new(status, e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = json!({"error": self.message});
        if let Some(field) = self.field {
            body["field"] = json!(field);
        }
        (self.status, Json(body)).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn io_error(e: io::Error) -> ApiError {
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("store I/O error: {e}"),
    )
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> ApiResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("store task failed: {e}"),
        )
    })
}

fn run_id(raw: &str) -> ApiResult<RunId> {
    RunId::parse(raw).map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))
}

fn session_id(raw: &str) -> ApiResult<SessionId> {
    SessionId::parse(raw).map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Moonraker and Klipper as the header shows them.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MoonrakerView {
    /// Whether the Moonraker socket is up.
    pub connected: bool,
    /// Klipper state: `ready`, `startup`, `shutdown`, `error` or `disconnected`.
    pub klippy: &'static str,
    /// Homed axes.
    pub homed_axes: String,
    /// `print_stats.state`.
    pub print_state: String,
    /// `idle_timeout.state`.
    pub idle_state: String,
    /// Toolhead `max_velocity`, mm/s.
    pub max_velocity: f64,
    /// Toolhead `max_accel`, mm/s^2.
    pub max_accel: f64,
}

fn klippy_name(k: KlippyState) -> &'static str {
    match k {
        KlippyState::Ready => "ready",
        KlippyState::Startup => "startup",
        KlippyState::Shutdown => "shutdown",
        KlippyState::Error => "error",
        KlippyState::Disconnected => "disconnected",
    }
}

fn trace_mode_name(m: TraceMode) -> &'static str {
    match m {
        TraceMode::Unknown => "unknown",
        TraceMode::Dormant => "dormant",
        TraceMode::Streaming => "streaming",
    }
}

/// The proposed stroke pattern for a stepper, or why there is none.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProposalView {
    /// The centred 100 mm pattern along the isolating direction.
    pub pattern: Option<Pattern>,
    /// Why no pattern is proposed.
    pub refusal: Option<String>,
}

/// One row of the stepper table.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepperView {
    /// Every derived fact.
    #[serde(flatten)]
    pub stepper: Stepper,
    /// Pattern proposal.
    pub proposal: ProposalView,
}

/// The latest control-state frame of one channel.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChannelView {
    /// Source index.
    pub channel: u8,
    /// The frame's values, raw.
    pub control: ControlSnapshot,
    /// Velocity I as its integrator corner in Hz.
    pub velocity_i_corner_hz: Option<f64>,
    /// One encoder count in position units.
    pub quantum_units: Option<f64>,
}

/// One trace interface.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardView {
    /// USB serial.
    pub serial: String,
    /// Whether the interface is open.
    pub open: bool,
    /// Protocol version of the latest frame.
    pub protocol_version: Option<u8>,
    /// Why runs cannot use this board.
    pub unusable_reason: Option<String>,
    /// `unknown`, `dormant` or `streaming`.
    pub trace_mode: &'static str,
    /// Measured critical cadence per channel in Hz.
    pub critical_hz_measured: f64,
    /// Firmware identifier from the metadata frame.
    pub firmware_id: Option<String>,
    /// Product string from the metadata frame.
    pub product: Option<String>,
    /// Firmware version Klipper reports for the MCU this board belongs to.
    pub mcu_version: Option<String>,
    /// Firmware build components Klipper reports for that MCU.
    pub mcu_build: Option<String>,
    /// Boot identifier changes seen.
    pub boot_changes: u64,
    /// Frames that could not be decoded.
    pub undecodable_frames: u64,
    /// Channels by source index.
    pub channels: Vec<ChannelView>,
}

/// Whether a run could start on a stepper now.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PreconditionView {
    /// Stepper section name.
    pub stepper: String,
    /// True when every precondition holds.
    pub ok: bool,
    /// Every failing precondition, in the order a run request would report them.
    pub failing: Vec<String>,
}

/// The run in flight.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CurrentRunView {
    /// Owning session.
    pub session: SessionId,
    /// The run.
    pub run: RunId,
    /// Its state, flattened to `state` and `reason`.
    #[serde(flatten)]
    pub state: RunState,
}

/// What the engine is doing.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EngineView {
    /// The run in flight.
    pub current: Option<CurrentRunView>,
    /// Queued runs in order.
    pub queue: Vec<RunId>,
    /// The run whose emergency stop is unconfirmed: motion may continue.
    pub unconfirmed_kill: Option<RunId>,
}

/// `GET /api/printer` and the `printer` event.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrinterView {
    /// Moonraker and Klipper.
    pub moonraker: MoonrakerView,
    /// The stepper table.
    pub steppers: Vec<StepperView>,
    /// Why the table could not be read from Moonraker.
    pub settings_error: Option<String>,
    /// Trace interfaces by serial.
    pub boards: Vec<BoardView>,
    /// Run preconditions per stepper.
    pub preconditions: Vec<PreconditionView>,
    /// Free bytes on the data directory.
    pub free_bytes: Option<u64>,
    /// Version of the running foci-tune service.
    pub version: &'static str,
    /// Engine state.
    pub engine: EngineView,
}

/// A run as listed in a session.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunEntry {
    /// Run id.
    pub id: RunId,
    /// State, flattened to `state` and `reason`; `failed` for an incomplete directory.
    #[serde(flatten)]
    pub state: RunState,
    /// The stored record, absent for an incomplete directory.
    pub run: Option<Run>,
}

/// A session with its runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionView {
    /// Session id.
    pub id: SessionId,
    /// `session.json`, absent when unreadable.
    pub session: Option<Session>,
    /// Why `session.json` is unreadable.
    pub error: Option<String>,
    /// Runs in id order.
    pub runs: Vec<RunEntry>,
    /// The tune and step the session was made for; absent for a session made by hand.
    pub tune: Option<TuneRef>,
}

/// The guided tune and step a listed session was made for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TuneRef {
    /// Id of the tune whose step made the session.
    pub id: TuneId,
    /// The step, counted from 1.
    pub step: usize,
}

impl SessionView {
    fn owned_by(mut self, owners: &BTreeMap<SessionId, (TuneId, usize)>) -> Self {
        self.tune = owners.get(&self.id).map(|(id, step)| TuneRef {
            id: id.clone(),
            step: *step,
        });
        self
    }
}

impl From<SessionSummary> for SessionView {
    fn from(s: SessionSummary) -> Self {
        Self {
            id: s.id,
            session: s.session,
            error: s.error,
            runs: s
                .runs
                .into_iter()
                .map(|r| RunEntry {
                    id: r.id,
                    state: r.state,
                    run: r.run,
                })
                .collect(),
            tune: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct GainsBody {
    velocity_p: i32,
    velocity_i: i32,
    position_p: i32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct TrackingParams {
    speeds_mm_s: Vec<f64>,
    strokes: usize,
}

impl Default for TrackingParams {
    fn default() -> Self {
        Self {
            speeds_mm_s: vec![50.0, 195.0, 400.0],
            strokes: 8,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct ProbeParams {
    speed_mm_s: f64,
    high_gain: f64,
    segments_between_toggles: usize,
    strokes: usize,
}

impl Default for ProbeParams {
    fn default() -> Self {
        Self {
            speed_mm_s: 195.0,
            high_gain: 1.3,
            segments_between_toggles: 3,
            strokes: 8,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct StandstillParams {
    rests: usize,
    dwell_s: f64,
}

impl Default for StandstillParams {
    fn default() -> Self {
        Self {
            rests: 8,
            dwell_s: 3.0,
        }
    }
}

/// A run request after its JSON has been checked.
struct RunInput {
    session: Option<SessionId>,
    name: Option<String>,
    stepper: String,
    kind: RunKind,
    pattern: Pattern,
    gains: Gains,
    filters: Filters,
}

fn parse_value<T: DeserializeOwned>(name: &str, v: Value) -> ApiResult<T> {
    serde_json::from_value(v).map_err(|e| ApiError::bad_field(name, format!("{name}: {e}")))
}

fn required<T: DeserializeOwned>(body: &Value, name: &str) -> ApiResult<T> {
    match body.get(name) {
        None | Some(Value::Null) => Err(ApiError::bad_field(name, format!("{name} is required"))),
        Some(v) => parse_value(name, v.clone()),
    }
}

fn optional<T: DeserializeOwned>(body: &Value, name: &str) -> ApiResult<Option<T>> {
    match body.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => parse_value(name, v.clone()).map(Some),
    }
}

fn params<T: DeserializeOwned + Default>(body: &Value) -> ApiResult<T> {
    Ok(optional(body, "parameters")?.unwrap_or_default())
}

fn parse_kind(body: &Value) -> ApiResult<RunKind> {
    let kind: String = required(body, "type")?;
    Ok(match kind.as_str() {
        "tracking" => {
            let p: TrackingParams = params(body)?;
            RunKind::Tracking {
                speeds_mm_s: p.speeds_mm_s,
                strokes: p.strokes,
            }
        }
        "probe" => {
            let p: ProbeParams = params(body)?;
            RunKind::Probe {
                speed_mm_s: p.speed_mm_s,
                high_gain: p.high_gain,
                segments_between_toggles: p.segments_between_toggles,
                strokes: p.strokes,
            }
        }
        "standstill" => {
            let p: StandstillParams = params(body)?;
            RunKind::Standstill {
                rests: p.rests,
                dwell_s: p.dwell_s,
            }
        }
        other => {
            return Err(ApiError::bad_field(
                "type",
                format!("type must be tracking, probe or standstill, not {other}"),
            ));
        }
    })
}

fn gain_in_range(name: &str, raw: i32) -> ApiResult<i32> {
    if (0..=PID_GAIN_MAX_RAW).contains(&raw) {
        Ok(raw)
    } else {
        Err(ApiError::bad_field(
            name,
            format!("{name} {raw} is outside 0..={PID_GAIN_MAX_RAW}"),
        ))
    }
}

fn parse_run(body: &Value) -> ApiResult<RunInput> {
    if !body.is_object() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "the request body must be a JSON object",
        ));
    }
    let session = optional::<String>(body, "session")?
        .map(|s| SessionId::parse(&s).map_err(|e| ApiError::bad_field("session", e.to_string())))
        .transpose()?;
    let gains: GainsBody = required(body, "gains")?;
    Ok(RunInput {
        session,
        name: optional(body, "name")?,
        stepper: required(body, "stepper")?,
        kind: parse_kind(body)?,
        pattern: required(body, "pattern")?,
        gains: Gains {
            velocity_p: gain_in_range("gains.velocity_p", gains.velocity_p)?,
            velocity_i: gain_in_range("gains.velocity_i", gains.velocity_i)?,
            position_p: gain_in_range("gains.position_p", gains.position_p)?,
            position_i: 0,
        },
        filters: required(body, "filters")?,
    })
}

fn parse_ladder(body: &Value) -> ApiResult<LadderDef> {
    Ok(LadderDef {
        axis: required(body, "axis")?,
        values: required(body, "values")?,
        repeats: optional(body, "repeats")?.unwrap_or(DEFAULT_REPEATS),
        floor: optional(body, "floor")?.unwrap_or(DEFAULT_FLOOR),
        marginal: optional(body, "marginal")?.unwrap_or(DEFAULT_MARGINAL),
    })
}

fn json_body(body: Result<Json<Value>, JsonRejection>) -> ApiResult<Value> {
    body.map(|Json(v)| v)
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.body_text()))
}

fn kind_name(kind: &RunKind) -> &'static str {
    match kind {
        RunKind::Tracking { .. } => "tracking",
        RunKind::Probe { .. } => "probe",
        RunKind::Standstill { .. } => "standstill",
    }
}

fn stimulus_error(e: StimulusError) -> ApiError {
    match e {
        StimulusError::Pattern(PatternError::UnsupportedKinematics(k)) => ApiError::new(
            StatusCode::CONFLICT,
            format!("kinematics {k} is not supported by the stimulus composer"),
        ),
        StimulusError::Pattern(p @ PatternError::UnsupportedStepper(_)) => {
            ApiError::new(StatusCode::CONFLICT, p.to_string())
        }
        StimulusError::Pattern(p @ (PatternError::OutsideLimits | PatternError::ZeroLength)) => {
            ApiError::bad_field("pattern", p.to_string())
        }
        e @ StimulusError::InvalidParameter(_) => ApiError::bad_field("parameters", e.to_string()),
        e @ (StimulusError::TooLarge { parameter, .. }
        | StimulusError::TooSlow { parameter, .. }
        | StimulusError::TooLong { parameter, .. }
        | StimulusError::TogglesTooClose { parameter, .. }) => {
            ApiError::bad_field(&format!("parameters.{parameter}"), e.to_string())
        }
    }
}

fn setup_mismatch(session: &Session, input: &RunInput) -> Option<ApiError> {
    let field = if session.stepper != input.stepper {
        "stepper"
    } else if kind_name(&session.kind) != kind_name(&input.kind) {
        "type"
    } else if session.kind != input.kind {
        "parameters"
    } else if session.pattern != input.pattern {
        "pattern"
    } else {
        return None;
    };
    Some(ApiError::bad_field(
        field,
        format!(
            "the session's {field} is fixed; start a new session to change it (session {})",
            session.name
        ),
    ))
}

struct Prepared {
    session: SessionId,
    created: bool,
}

impl<M: MoonrakerApi + 'static> Api<M> {
    /// Builds the printer snapshot: Moonraker state, the stepper table, live boards,
    /// preconditions per stepper and the engine state.
    pub async fn printer_view(&self) -> PrinterView {
        let printer = self.moonraker.state().borrow().clone();
        let (steppers, settings_error) = match self.engine.steppers().await {
            Ok(s) => (s, None),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
        let versions = self.engine.mcu_versions(&steppers).await;
        let limits = BedLimits::from_steppers(&steppers);
        let mut preconditions = Vec::new();
        for s in &steppers {
            let failing = self.engine.preconditions(&s.name).await;
            preconditions.push(PreconditionView {
                stepper: s.name.clone(),
                ok: failing.is_empty(),
                failing: failing.iter().map(ToString::to_string).collect(),
            });
        }
        let boards = self.boards(&steppers, &versions);
        let steppers = steppers
            .into_iter()
            .map(|stepper| {
                let proposal = match &limits {
                    Some(l) => match propose_pattern(&stepper.kinematics, &stepper.name, l) {
                        Ok(p) => ProposalView {
                            pattern: Some(p),
                            refusal: None,
                        },
                        Err(e) => ProposalView {
                            pattern: None,
                            refusal: Some(e.to_string()),
                        },
                    },
                    None => ProposalView {
                        pattern: None,
                        refusal: Some(
                            "bed limits need stepper_x and stepper_y in the config".to_owned(),
                        ),
                    },
                };
                StepperView { stepper, proposal }
            })
            .collect();
        let store = self.store.clone();
        let free_bytes = blocking(move || store.free_bytes().ok())
            .await
            .ok()
            .flatten();
        let status = self.engine.status();
        PrinterView {
            moonraker: MoonrakerView {
                connected: printer.klippy != KlippyState::Disconnected,
                klippy: klippy_name(printer.klippy),
                homed_axes: printer.homed_axes,
                print_state: printer.print_state,
                idle_state: printer.idle_state,
                max_velocity: printer.max_velocity,
                max_accel: printer.max_accel,
            },
            steppers,
            settings_error,
            boards,
            preconditions,
            free_bytes,
            version: env!("CARGO_PKG_VERSION"),
            engine: EngineView {
                current: status.current.map(|(session, run, state)| CurrentRunView {
                    session,
                    run,
                    state,
                }),
                queue: status.queue,
                unconfirmed_kill: status.unconfirmed_kill,
            },
        }
    }

    fn boards(
        &self,
        steppers: &[Stepper],
        versions: &BTreeMap<String, McuVersion>,
    ) -> Vec<BoardView> {
        let live = lock(&self.live);
        let mut boards: Vec<BoardView> = live
            .boards
            .iter()
            .map(|(serial, b)| {
                let version = steppers
                    .iter()
                    .find(|s| &s.serial == serial)
                    .and_then(|s| versions.get(&s.mcu));
                let mut channels: Vec<ChannelView> = b
                    .channels
                    .iter()
                    .map(|(&channel, c)| ChannelView {
                        channel,
                        control: ControlSnapshot::from(c),
                        velocity_i_corner_hz: (c.pwm_hz > 0)
                            .then(|| corner_hz(c.velocity_i, c.pwm_hz)),
                        quantum_units: (c.encoder_counts_per_rev > 0)
                            .then(|| POSITION_UNITS_PER_REV / f64::from(c.encoder_counts_per_rev)),
                    })
                    .collect();
                channels.sort_by_key(|c| c.channel);
                BoardView {
                    serial: serial.clone(),
                    open: b.open,
                    protocol_version: b.protocol_version,
                    unusable_reason: b.unusable_reason().map(str::to_owned),
                    trace_mode: trace_mode_name(b.trace_mode),
                    critical_hz_measured: b.critical_hz_measured,
                    firmware_id: b.metadata.as_ref().map(|m| m.firmware_id.clone()),
                    product: b.metadata.as_ref().map(|m| m.product.clone()),
                    mcu_version: version.map(|v| v.version.clone()),
                    mcu_build: version.map(|v| v.build.clone()),
                    boot_changes: b.boot_changes,
                    undecodable_frames: b.undecodable_frames,
                    channels,
                }
            })
            .collect();
        boards.sort_by(|a, b| a.serial.cmp(&b.serial));
        boards
    }

    /// Checks the preconditions and the stimulus, then finds or creates the session.
    async fn prepare(&self, input: &RunInput, ladder: Option<&LadderDef>) -> ApiResult<Prepared> {
        self.engine.check(&input.stepper).await?;
        let steppers = self.engine.steppers().await?;
        let stepper = steppers
            .iter()
            .find(|s| s.name == input.stepper)
            .ok_or_else(|| Precondition::UnknownStepper(input.stepper.clone()))?;
        let limits = BedLimits::from_steppers(&steppers).ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                "bed limits need stepper_x and stepper_y in the config",
            )
        })?;
        let max_velocity = self.moonraker.state().borrow().max_velocity;
        compose(
            &input.kind,
            &input.pattern,
            &stepper.name,
            &stepper.kinematics,
            &limits,
            max_velocity,
        )
        .map_err(stimulus_error)?;

        if let Some(id) = &input.session {
            let store = self.store.clone();
            let lookup = id.clone();
            let session = blocking(move || store.read_session(&lookup))
                .await?
                .map_err(|_| Precondition::UnknownSession(id.to_string()))?;
            if let Some(e) = setup_mismatch(&session, input) {
                return Err(e);
            }
            return Ok(Prepared {
                session: id.clone(),
                created: false,
            });
        }

        let control = lock(&self.live)
            .boards
            .get(&stepper.serial)
            .and_then(|b| b.channels.get(&stepper.channel))
            .cloned()
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::CONFLICT,
                    "no control-state frame for the stepper's channel yet",
                )
            })?;
        let session = Session {
            format_version: FORMAT_VERSION,
            name: input
                .name
                .clone()
                .unwrap_or_else(|| format!("{} {}", input.stepper, kind_name(&input.kind))),
            created_unix_s: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            stepper: input.stepper.clone(),
            kind: input.kind.clone(),
            pattern: input.pattern,
            gains: Gains {
                velocity_p: control.velocity_p,
                velocity_i: control.velocity_i,
                position_p: control.position_p,
                position_i: control.position_i,
            },
            filters: Filters {
                velocity_hz: control.velocity_filter_hz,
                torque_hz: control.torque_filter_hz,
                position_hz: control.position_filter_hz,
                flux_hz: control.flux_filter_hz,
            },
            ladder: ladder.cloned(),
        };
        let store = self.store.clone();
        let id = blocking(move || store.new_session(&session))
            .await?
            .map_err(io_error)?;
        Ok(Prepared {
            session: id,
            created: true,
        })
    }

    async fn roll_back(&self, prepared: &Prepared) {
        if !prepared.created {
            return;
        }
        let store = self.store.clone();
        let id = prepared.session.clone();
        if let Ok(Err(e)) = blocking(move || store.remove_empty_session(&id)).await {
            tracing::warn!(session = %prepared.session, error = %e, "cannot remove refused session");
        }
    }

    async fn locate(&self, run: &RunId) -> ApiResult<SessionId> {
        let store = self.store.clone();
        let id = run.clone();
        blocking(move || store.find_run(&id))
            .await?
            .map_err(io_error)?
            .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("run {run} not found")))
    }

    async fn read_run(&self, run: &RunId) -> ApiResult<Run> {
        let session = self.locate(run).await?;
        let store = self.store.clone();
        let id = run.clone();
        Ok(blocking(move || store.read_run(&session, &id)).await??)
    }

    fn request(input: RunInput, session: SessionId) -> RunRequest {
        RunRequest {
            session,
            stepper: input.stepper,
            kind: input.kind,
            pattern: input.pattern,
            gains: input.gains,
            filters: input.filters,
            ladder_value: None,
        }
    }
}

/// The router: the API under `/api`, the event stream at `/api/events`, and the embedded UI
/// for every other path. A path that names a file (under `assets/`, or with an extension) and
/// is not embedded is a 404; any other path gets `index.html` so client-side routes load.
///
/// Spawns the one task that builds the printer snapshot for every event-stream client, so it
/// must be called inside a tokio runtime.
pub fn router<M: MoonrakerApi + 'static>(api: Api<M>) -> Router {
    let feed = spawn_printer_feed(api.clone());
    let app = App { api, printer: feed };
    Router::new()
        .route("/api/printer", get(printer::<M>))
        .route("/api/trace-mode", post(trace_mode::<M>))
        .route("/api/runs", post(create_run::<M>))
        .route("/api/ladders", post(create_ladder::<M>))
        .route("/api/runs/{id}", get(read_run::<M>))
        .route("/api/runs/{id}/series", get(series::<M>))
        .route("/api/runs/{id}/capture", get(capture::<M>))
        .route("/api/runs/{id}/config-block", get(config_block_text::<M>))
        .route("/api/runs/{id}/abort", post(abort::<M>))
        .route("/api/kill", post(kill::<M>))
        .route("/api/sessions", get(sessions::<M>))
        .route("/api/sessions/{id}", get(session::<M>))
        .route("/api/sessions/{id}/revert", post(revert::<M>))
        .route("/api/firmware-restart", post(firmware_restart::<M>))
        .route("/api/home", post(home::<M>))
        .route("/api/tunes", get(list_tunes::<M>).post(create_tune::<M>))
        .route("/api/tunes/{id}", get(read_tune::<M>))
        .route("/api/tunes/{id}/setup", put(update_tune_setup::<M>))
        .route("/api/tunes/{id}/steps", post(prepare_tune_step::<M>))
        .route(
            "/api/tunes/{id}/steps/confirm",
            post(confirm_tune_step::<M>),
        )
        .route("/api/tunes/{id}/steps/cancel", post(cancel_tune_step::<M>))
        .route("/api/tunes/{id}/extension", post(tune_extension::<M>))
        .route("/api/tunes/{id}/choice", post(tune_choice::<M>))
        .route("/api/tunes/{id}/decision", post(tune_decision::<M>))
        .route("/api/tunes/{id}/apply", post(apply_tune_result::<M>))
        .route("/api/tunes/{id}/abandon", post(abandon_tune::<M>))
        .route("/api/tunes/{id}/restore", post(restore_tune::<M>))
        .route("/api/events", get(ws::events::<M>))
        .fallback(static_or_index)
        .with_state(app)
}

/// The snapshot with the fields that change on every build cleared, for change detection.
fn stable_part(view: &PrinterView) -> PrinterView {
    let mut stable = view.clone();
    stable.free_bytes = None;
    for board in &mut stable.boards {
        board.critical_hz_measured = 0.0;
    }
    stable
}

/// Rebuilds the printer snapshot once a second, each build bounded so a hung Moonraker only
/// delays the snapshot, and publishes it when it changed: at once for a change other than free
/// space or cadence, at most every [`VOLATILE_PERIOD`] for those.
fn spawn_printer_feed<M: MoonrakerApi + 'static>(api: Api<M>) -> PrinterFeed {
    let (tx, rx) = watch::channel(None);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SNAPSHOT_PERIOD);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut published: Option<(PrinterView, PrinterView, tokio::time::Instant)> = None;
        loop {
            tick.tick().await;
            if tx.is_closed() {
                return;
            }
            let Ok(view) = tokio::time::timeout(SNAPSHOT_TIMEOUT, api.printer_view()).await else {
                tracing::warn!("printer snapshot took longer than {SNAPSHOT_TIMEOUT:?}");
                continue;
            };
            let stable = stable_part(&view);
            let now = tokio::time::Instant::now();
            let publish = match &published {
                None => true,
                Some((full, key, at)) => {
                    *key != stable || (*full != view && now - *at >= VOLATILE_PERIOD)
                }
            };
            if publish {
                tx.send_replace(Some(Arc::new(view.clone())));
                published = Some((view, stable, now));
            }
        }
    });
    rx
}

async fn printer<M: MoonrakerApi + 'static>(State(api): State<App<M>>) -> Json<PrinterView> {
    Json(api.printer_view().await)
}

async fn create_run<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    body: Result<Json<Value>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let input = parse_run(&json_body(body)?)?;
    let prepared = api.prepare(&input, None).await?;
    let session = prepared.session.clone();
    match api
        .engine
        .submit(Api::<M>::request(input, session.clone()))
        .await
    {
        Ok(run) => Ok(Json(json!({"session": session, "run": run}))),
        Err(p) => {
            api.roll_back(&prepared).await;
            Err(p.into())
        }
    }
}

async fn create_ladder<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    body: Result<Json<Value>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = json_body(body)?;
    if body.get("session").is_some() {
        return Err(ApiError::bad_field(
            "session",
            "a ladder always opens its own session; leave session out",
        ));
    }
    let input = parse_run(&body)?;
    let ladder = parse_ladder(&body)?;
    let prepared = api.prepare(&input, Some(&ladder)).await?;
    let session = prepared.session.clone();
    let request = LadderRequest {
        run: Api::<M>::request(input, session.clone()),
        ladder,
    };
    match api.engine.submit_ladder(request).await {
        Ok(runs) => Ok(Json(json!({"session": session, "runs": runs}))),
        Err(e) => {
            api.roll_back(&prepared).await;
            Err(e.into())
        }
    }
}

async fn read_run<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Run>> {
    Ok(Json(api.read_run(&run_id(&id)?).await?))
}

async fn series<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let run = run_id(&id)?;
    let session = api.locate(&run).await?;
    let store = api.store.clone();
    let series = blocking(move || store.series(&session, &run)).await??;
    Ok(Json(series).into_response())
}

async fn capture<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let run = run_id(&id)?;
    let session = api.locate(&run).await?;
    let bytes = tokio::fs::read(api.store.capture_path(&session, &run))
        .await
        .map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => {
                ApiError::new(StatusCode::NOT_FOUND, format!("run {run} has no capture"))
            }
            _ => io_error(e),
        })?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/zstd".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{run}.bin.zst\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

async fn config_block_text<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let run = api.read_run(&run_id(&id)?).await?;
    Ok((
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        config_block::for_run(&run),
    )
        .into_response())
}

async fn abort<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let run = run_id(&id)?;
    let status = api.engine.status();
    let current = status.current.as_ref().map(|(_, id, _)| id);
    if current != Some(&run) && !status.queue.contains(&run) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("run {run} is not in flight or queued"),
        ));
    }
    api.engine.abort().await;
    Ok(Json(json!({"run": run, "aborted": status.queue})))
}

async fn kill<M: MoonrakerApi + 'static>(State(api): State<App<M>>) -> Json<Value> {
    let outcome = api.engine.kill().await;
    Json(json!({"run": outcome.run, "state": outcome.state.name()}))
}

async fn sessions<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
) -> ApiResult<Json<Vec<SessionView>>> {
    let store = api.store.clone();
    let (scanned, owners) =
        blocking(move || Ok::<_, std::io::Error>((store.scan()?, store.tune_sessions()?)))
            .await?
            .map_err(io_error)?;
    Ok(Json(
        scanned
            .into_iter()
            .map(|s| SessionView::from(s).owned_by(&owners))
            .collect(),
    ))
}

async fn session<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    Path(id): Path<String>,
) -> ApiResult<Json<SessionView>> {
    let id = session_id(&id)?;
    let store = api.store.clone();
    let lookup = id.clone();
    let (summary, owners) = blocking(move || {
        Ok::<_, std::io::Error>((store.session_summary(&lookup)?, store.tune_sessions()?))
    })
    .await?
    .map_err(io_error)?;
    summary
        .map(|s| Json(SessionView::from(s).owned_by(&owners)))
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("session {id} not found")))
}

async fn revert<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let id = session_id(&id)?;
    let store = api.store.clone();
    let lookup = id.clone();
    blocking(move || store.read_session(&lookup))
        .await?
        .map_err(|e| match ApiError::from(e) {
            e if e.status == StatusCode::NOT_FOUND => {
                ApiError::new(StatusCode::NOT_FOUND, format!("session {id} not found"))
            }
            e => e,
        })?;
    api.engine
        .revert(&id)
        .await
        .map_err(|e| ApiError::new(StatusCode::CONFLICT, e))?;
    Ok(Json(json!({"session": id, "reverted": true})))
}

async fn firmware_restart<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
) -> ApiResult<Json<Value>> {
    let klippy = api.moonraker.state().borrow().klippy;
    if !matches!(klippy, KlippyState::Shutdown | KlippyState::Error) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!(
                "firmware restart is offered only once klipper reports shutdown or error; \
                 klipper is {}",
                klippy_name(klippy)
            ),
        ));
    }
    api.moonraker
        .firmware_restart()
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(Json(json!({"ok": true})))
}

async fn home<M: MoonrakerApi + 'static>(State(api): State<App<M>>) -> ApiResult<Json<Value>> {
    let printer = api.moonraker.state().borrow().clone();
    if printer.klippy != KlippyState::Ready {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!(
                "klipper is {}; homing needs it ready",
                klippy_name(printer.klippy)
            ),
        ));
    }
    if matches!(printer.print_state.as_str(), "printing" | "paused") {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "a print is in progress".to_owned(),
        ));
    }
    if !api.engine.is_idle() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "a run is in progress".to_owned(),
        ));
    }
    api.moonraker
        .script("G28")
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(Json(json!({"ok": true})))
}

async fn trace_mode<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    body: Result<Json<Value>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = json_body(body)?;
    let board: String = required(&body, "board")?;
    let raw: String = required(&body, "mode")?;
    let mode = TraceModeCommand::parse(&raw).ok_or_else(|| {
        ApiError::bad_field(
            "mode",
            format!("mode must be dormant, critical or diagnostic, not {raw}"),
        )
    })?;
    let steppers = api.engine.steppers().await?;
    let stepper = steppers.iter().find(|s| s.serial == board).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            format!("no FOCI stepper on board {board}"),
        )
    })?;
    api.engine
        .set_trace_mode(&stepper.name, mode)
        .await
        .map_err(|e| match e {
            CommandError::Precondition(p) => ApiError::from(p),
            CommandError::Gcode(message) => ApiError::new(StatusCode::BAD_GATEWAY, message),
            CommandError::NoControlState(message) => {
                ApiError::new(StatusCode::GATEWAY_TIMEOUT, message)
            }
        })?;
    Ok(Json(json!({"board": board, "mode": mode.name()})))
}

async fn static_or_index(uri: Uri) -> Response {
    let path = uri.path();
    if path == "/api" || path.starts_with("/api/") {
        return ApiError::new(StatusCode::NOT_FOUND, format!("no API route {path}"))
            .into_response();
    }
    let file = path.trim_start_matches('/');
    if !file.is_empty()
        && let Some(asset) = assets::get(file)
    {
        return ([(header::CONTENT_TYPE, asset.mime)], asset.data).into_response();
    }
    let last = file.rsplit('/').next().unwrap_or_default();
    if file.starts_with("assets/") || last.contains('.') {
        return (StatusCode::NOT_FOUND, format!("no file {path}")).into_response();
    }
    match assets::index_html() {
        Some(index) => (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned())],
            index.data,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "the web UI is not embedded").into_response(),
    }
}

fn tune_id(raw: &str) -> ApiResult<TuneId> {
    TuneId::parse(raw).map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))
}

fn from_body<T: DeserializeOwned>(body: Value) -> ApiResult<T> {
    serde_json::from_value(body).map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, e.to_string()))
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ChoiceBody {
    Recommended,
    Rung(usize),
    Value(f64),
}

#[derive(Deserialize)]
struct ChoiceRequest {
    choice: ChoiceBody,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Direction {
    Below,
    Above,
}

#[derive(Deserialize)]
struct ExtensionRequest {
    direction: Direction,
}

/// One row of the tune list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TuneSummary {
    /// Tune id.
    pub id: TuneId,
    /// Declared stepper section name.
    pub stepper: String,
    /// Life-cycle state, as in the tune record.
    pub state: crate::tune::TuneState,
    /// Creation time, seconds since the UNIX epoch.
    pub created: u64,
    /// Axis of the step in progress or last worked on; `None` before the first step.
    pub current_step: Option<foci_tune_analysis::ladder::LadderAxis>,
}

/// A tune record with the config block and lock state read alongside it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TuneDetail {
    /// The record, flattened into the response.
    #[serde(flatten)]
    pub tune: Tune,
    /// The `[foci <stepper>]` block once every step has a chosen value.
    pub config_block: Option<String>,
    /// Why the tune's stepper is locked, if it is.
    pub lock: Option<String>,
}

impl From<&Tune> for TuneSummary {
    fn from(t: &Tune) -> Self {
        Self {
            id: t.id.clone(),
            stepper: t.setup.stepper.clone(),
            state: t.state.clone(),
            created: t.created_unix_s,
            current_step: t.steps.last().map(|s| s.axis),
        }
    }
}

impl<M: MoonrakerApi + 'static> Api<M> {
    /// Checks a tune setup beyond its shape: the probe stimulus must compose for the stepper
    /// and pattern, which covers the pattern limits and the toggle spacing.
    async fn check_tune_setup(&self, setup: &TuneSetup) -> ApiResult<()> {
        let steppers = self.engine.steppers().await?;
        let stepper = steppers
            .iter()
            .find(|s| s.name == setup.stepper)
            .ok_or_else(|| Precondition::UnknownStepper(setup.stepper.clone()))?;
        let limits = BedLimits::from_steppers(&steppers).ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                "bed limits need stepper_x and stepper_y in the config",
            )
        })?;
        let max_velocity = self.moonraker.state().borrow().max_velocity;
        let probe = RunKind::Probe {
            speed_mm_s: setup.probe.speed_mm_s,
            high_gain: setup.probe.high_gain,
            segments_between_toggles: setup.probe.segments_between_toggles,
            strokes: setup.probe.strokes,
        };
        compose(
            &probe,
            &setup.pattern,
            &stepper.name,
            &stepper.kinematics,
            &limits,
            max_velocity,
        )
        .map_err(|e| {
            let mut e = stimulus_error(e);
            e.field = e.field.map(|f| f.replace("parameters.", "probe."));
            e
        })?;
        Ok(())
    }
}

/// Refuses gate values that would let a rung through unchecked.
fn check_gates(gates: &Gates) -> ApiResult<()> {
    if !(gates.damping_floor.is_finite() && gates.damping_floor >= 0.0) {
        return Err(ApiError::bad_field(
            "gates.damping_floor",
            "the damping floor must be a finite number of at least 0",
        ));
    }
    if !(gates.marginal.is_finite() && gates.marginal > gates.damping_floor) {
        return Err(ApiError::bad_field(
            "gates.marginal",
            "the marginal threshold must be finite and above the damping floor",
        ));
    }
    if let Some(factor) = gates.standstill_factor
        && !(factor.is_finite() && factor >= 1.0)
    {
        return Err(ApiError::bad_field(
            "gates.standstill_factor",
            "the standstill factor must be a finite number of at least 1",
        ));
    }
    Ok(())
}

async fn parse_tune_setup<M: MoonrakerApi + 'static>(
    api: &App<M>,
    body: Result<Json<Value>, JsonRejection>,
) -> ApiResult<TuneSetup> {
    let body = json_body(body)?;
    let speeds = body
        .get("speeds_mm_s")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if !(1..=3).contains(&speeds) {
        return Err(ApiError::bad_field(
            "speeds_mm_s",
            "a tune needs one to three tracking speeds",
        ));
    }
    if body.pointer("/probe/speed_mm_s").is_none_or(Value::is_null) {
        return Err(ApiError::bad_field(
            "probe.speed_mm_s",
            "the probe speed must be given explicitly",
        ));
    }
    let setup: TuneSetup = from_body(body)?;
    check_gates(&setup.gates)?;
    api.check_tune_setup(&setup).await?;
    Ok(setup)
}

async fn create_tune<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    body: Result<Json<Value>, JsonRejection>,
) -> ApiResult<Json<Tune>> {
    let setup = parse_tune_setup(&api, body).await?;
    let id = api.tunes.create(setup).await?;
    Ok(Json(read_tune_record(&api, &id).await?))
}

async fn update_tune_setup<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    Path(id): Path<String>,
    body: Result<Json<Value>, JsonRejection>,
) -> ApiResult<Json<Tune>> {
    let id = tune_id(&id)?;
    let setup = parse_tune_setup(&api, body).await?;
    api.tunes.update_setup(&id, setup).await?;
    Ok(Json(read_tune_record(&api, &id).await?))
}

async fn read_tune_record<M>(api: &Api<M>, id: &TuneId) -> ApiResult<Tune> {
    let store = api.store.clone();
    let lookup = id.clone();
    blocking(move || store.read_tune(&lookup))
        .await?
        .map_err(|e| match ApiError::from(e) {
            e if e.status == StatusCode::NOT_FOUND => {
                ApiError::new(StatusCode::NOT_FOUND, format!("unknown tune {id}"))
            }
            e => e,
        })
}

async fn list_tunes<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
) -> ApiResult<Json<Vec<TuneSummary>>> {
    let store = api.store.clone();
    let tunes = blocking(move || store.scan_tunes())
        .await?
        .map_err(io_error)?;
    Ok(Json(tunes.iter().map(TuneSummary::from).collect()))
}

async fn read_tune<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    Path(id): Path<String>,
) -> ApiResult<Json<TuneDetail>> {
    let id = tune_id(&id)?;
    let tune = read_tune_record(&api, &id).await?;
    let lock = api.engine.lock_state(&tune.setup.stepper).map(|l| l.reason);
    Ok(Json(TuneDetail {
        tune,
        config_block: api.tunes.config_block(&id),
        lock,
    }))
}

async fn prepare_tune_step<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    Path(id): Path<String>,
    body: Result<Json<Value>, JsonRejection>,
) -> ApiResult<Json<StepPlan>> {
    let id = tune_id(&id)?;
    let request: StepRequest = from_body(json_body(body)?)?;
    Ok(Json(api.tunes.prepare_step(&id, request).await?))
}

async fn confirm_tune_step<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let id = tune_id(&id)?;
    api.tunes.confirm_step(&id).await?;
    Ok(Json(json!({"tune": id, "confirmed": true})))
}

async fn tune_extension<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    Path(id): Path<String>,
    body: Result<Json<Value>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let id = tune_id(&id)?;
    let request: ExtensionRequest = from_body(json_body(body)?)?;
    let ext = match request.direction {
        Direction::Below => Extension::Below,
        Direction::Above => Extension::Above,
    };
    api.tunes.confirm_extension(&id, ext).await?;
    Ok(Json(json!({"tune": id, "confirmed": true})))
}

async fn tune_choice<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    Path(id): Path<String>,
    body: Result<Json<Value>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let id = tune_id(&id)?;
    let request: ChoiceRequest = from_body(json_body(body)?)?;
    let choice = match request.choice {
        ChoiceBody::Recommended => Choice::Recommended,
        ChoiceBody::Rung(index) => Choice::Rung(index),
        ChoiceBody::Value(value) => Choice::Value(value),
    };
    api.tunes.choose(&id, choice).await?;
    Ok(Json(json!({"tune": id, "chosen": true})))
}

#[derive(Deserialize)]
struct DecisionRequest {
    decision: PauseDecision,
}

async fn tune_decision<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    Path(id): Path<String>,
    body: Result<Json<Value>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let id = tune_id(&id)?;
    let request: DecisionRequest = from_body(json_body(body)?)?;
    api.tunes.decide(&id, request.decision).await?;
    Ok(Json(json!({"tune": id, "decided": true})))
}

async fn cancel_tune_step<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Tune>> {
    let id = tune_id(&id)?;
    api.tunes.cancel_step(&id).await?;
    Ok(Json(read_tune_record(&api, &id).await?))
}

async fn apply_tune_result<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let id = tune_id(&id)?;
    api.tunes.apply_result(&id).await?;
    Ok(Json(json!({"tune": id, "applied": true})))
}

async fn abandon_tune<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let id = tune_id(&id)?;
    api.tunes.abandon(&id).await?;
    Ok(Json(json!({"tune": id, "abandoned": true})))
}

async fn restore_tune<M: MoonrakerApi + 'static>(
    State(api): State<App<M>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let id = tune_id(&id)?;
    api.tunes.restore(&id).await?;
    Ok(Json(json!({"tune": id, "restored": true})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_finite_standstill_factor_is_rejected_by_the_shared_validation() {
        let g = Gates {
            standstill_factor: Some(f64::NAN),
            ..Gates::default()
        };
        assert!(check_gates(&g).is_err());
        let g = Gates {
            standstill_factor: Some(f64::INFINITY),
            ..Gates::default()
        };
        assert!(check_gates(&g).is_err());
        assert!(check_gates(&Gates::default()).is_ok());
    }
}
