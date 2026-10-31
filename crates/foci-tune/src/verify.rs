//! `verify-archive`: runs the analysis over archived captures and compares it with the
//! values the archived hand scripts produced.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use foci_tune_analysis::fit::{DampedFit, FitError, fit_damped};
use foci_tune_analysis::probe::{ensemble, locate_edges, probe_metrics};
use foci_tune_analysis::protocol::{
    ControlState, Critical, FRAME_HEADER_LEN, Frame, FrameHeader, Metadata, PROTOCOL_VERSION,
    decode_frame,
};
use foci_tune_analysis::signals::{ChannelSeries, channel_series};
use foci_tune_analysis::standstill::standstill_metrics;
use foci_tune_analysis::tracking::tracking_metrics;
use foci_tune_analysis::units::{POSITION_UNITS_PER_REV, Regime};

use crate::capture_read::{CaptureError, read_capture};

/// Which analysis to verify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Kind {
    /// Feedforward-step probe captures.
    Probe,
    /// Plain-move tracking captures.
    Tracking,
    /// Standstill rest captures.
    Standstill,
}

/// Channel the reference scripts analysed.
const CHANNEL: u8 = 0;
const ROTATION_DISTANCE_MM: f64 = 40.0;
/// Samples dropped at the start of each rest before measuring.
pub(crate) const STANDSTILL_SETTLE: usize = 500;
/// Samples a rest must keep after the settle prefix.
pub(crate) const STANDSTILL_MIN_LEN: usize = 200;
const PSD_BIN_HZ: f64 = 1000.0 / 256.0;
const ARCHIVE_PROTOCOL_VERSION: u8 = 24;
const FRAME_TYPE_CRITICAL: u8 = 1;
const FRAME_TYPE_METADATA: u8 = 5;
const PACKED_TORQUE_OFFSET: usize = 64;

struct ProbeRef {
    prefix: &'static str,
    vel_hz: f64,
    vel_zeta: f64,
    edges: usize,
    psd_peak_hz: f64,
}

const PROBE: [ProbeRef; 4] = [
    ProbeRef {
        prefix: "foci527-meter1-bq500-posp141-ffstep195-p1500",
        vel_hz: 319.66,
        vel_zeta: 0.1096,
        edges: 44,
        psd_peak_hz: 62.50,
    },
    ProbeRef {
        prefix: "foci527-meter1-bq500-posp200-ffstep195-p1500",
        vel_hz: 328.06,
        vel_zeta: 0.0558,
        edges: 44,
        psd_peak_hz: 62.50,
    },
    ProbeRef {
        prefix: "foci527-meter1-bq500-posp240-ffstep195-p1500",
        vel_hz: 334.17,
        vel_zeta: 0.0215,
        edges: 44,
        psd_peak_hz: 62.50,
    },
    ProbeRef {
        prefix: "foci527-meter1-posp141-ffstep195-p1100",
        vel_hz: 238.15,
        vel_zeta: 0.0292,
        edges: 44,
        psd_peak_hz: 62.50,
    },
];

struct TrackingRef {
    prefix: &'static str,
    sd: f64,
    p99: f64,
    cruise: f64,
    rev_peak: f64,
    n_rev: usize,
}

const fn tr(
    prefix: &'static str,
    sd: f64,
    p99: f64,
    cruise: f64,
    rev_peak: f64,
    n_rev: usize,
) -> TrackingRef {
    TrackingRef {
        prefix,
        sd,
        p99,
        cruise,
        rev_peak,
        n_rev,
    }
}

const TRACKING: [TrackingRef; 18] = [
    tr("foci527-plain-50-i0-r1", 75.8, 107.4, 71.1, 62.5, 257),
    tr("foci527-plain-50-i0-r2", 75.7, 108.0, 68.4, 38.6, 54),
    tr("foci527-plain-50-i206-r1", 21.6, 81.8, 15.4, 15.2, 69),
    tr("foci527-plain-50-i206-r2", 21.5, 80.0, 15.7, 24.7, 26),
    tr("foci527-plain-50-i412-r1", 18.7, 49.3, 14.7, 25.6, 30),
    tr("foci527-plain-50-i412-r2", 19.1, 51.9, 15.1, 21.4, 38),
    tr("foci527-plain-195-i0-r1", 94.8, 117.2, 94.4, 82.2, 38),
    tr("foci527-plain-195-i0-r2", 94.0, 115.4, 93.5, 116.7, 13),
    tr("foci527-plain-195-i206-r1", 31.4, 124.5, 14.8, 65.4, 36),
    tr("foci527-plain-195-i206-r2", 30.9, 123.3, 14.6, 64.0, 33),
    tr("foci527-plain-195-i412-r1", 17.8, 84.8, 7.6, 47.3, 34),
    tr("foci527-plain-195-i412-r2", 18.3, 86.7, 7.7, 52.3, 43),
    tr("foci527-plain-400-i0-r1", 114.7, 158.7, 117.6, 66.0, 142),
    tr("foci527-plain-400-i0-r2", 112.5, 157.5, 116.4, 34.8, 213),
    tr("foci527-plain-400-i206-r1", 44.8, 148.9, 22.4, 95.6, 50),
    tr("foci527-plain-400-i206-r2", 44.7, 149.5, 22.5, 103.6, 54),
    tr("foci527-plain-400-i412-r1", 23.8, 100.1, 8.7, 70.3, 55),
    tr("foci527-plain-400-i412-r2", 23.6, 97.7, 8.8, 103.6, 29),
];

struct RestRef {
    samples: usize,
    torque_sd: f64,
    pos_pp_units: f64,
}

const fn rr(samples: usize, torque_sd: f64, pos_pp_units: f64) -> RestRef {
    RestRef {
        samples,
        torque_sd,
        pos_pp_units,
    }
}

const STANDSTILL_PREFIX: &str = "foci527-rest-m0-p841";
const STANDSTILL: [RestRef; 8] = [
    rr(2491, 2.871, 16.0),
    rr(2502, 0.000, 0.0),
    rr(2496, 0.389, 0.0),
    rr(2500, 0.000, 0.0),
    rr(2495, 0.419, 0.0),
    rr(2491, 0.447, 0.0),
    rr(2491, 3.162, 16.0),
    rr(5652, 2.726, 16.0),
];

/// Outcome counts of one `verify-archive` run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// Captures whose every compared metric is within tolerance.
    pub passed: usize,
    /// Captures with at least one metric outside tolerance.
    pub breached: usize,
    /// Captures with no reference entry.
    pub skipped: usize,
    /// Captures that could not be read or analysed.
    pub failed: usize,
}

impl Summary {
    /// True when nothing breached and nothing failed.
    pub fn ok(&self) -> bool {
        self.breached == 0 && self.failed == 0
    }
}

#[derive(Debug, thiserror::Error)]
enum LoadError {
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error("no critical frames for channel {CHANNEL}")]
    NoCriticals,
    #[error("no metadata frame")]
    NoMetadata,
}

/// Relabels a protocol 24 critical or metadata frame as protocol 25.
///
/// Archived captures predate protocol 25, whose only change is the new control-state frame;
/// critical and metadata payloads are byte-identical across the two. Other frames pass
/// through untouched and fail the version check as before.
fn as_protocol_25(bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let is_archive_frame = bytes.get(4) == Some(&ARCHIVE_PROTOCOL_VERSION)
        && matches!(
            bytes.get(6),
            Some(&(FRAME_TYPE_CRITICAL | FRAME_TYPE_METADATA))
        );
    if !is_archive_frame {
        return std::borrow::Cow::Borrowed(bytes);
    }
    let mut owned = bytes.to_vec();
    owned[4] = PROTOCOL_VERSION;
    std::borrow::Cow::Owned(owned)
}

struct Loaded {
    criticals: Vec<(FrameHeader, Critical)>,
    series: ChannelSeries,
    regime: Regime,
    packed_torque_target: Vec<i32>,
    protocol_24: bool,
}

/// Torque series a standstill comparison measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TorqueSource {
    /// `interim_pidin_target_torque`, the signal the analysis binds.
    PidinTarget,
    /// Signed upper half of `interim_pid_torque_target_actual`, the signal `standstill.py`
    /// measured; protocol 24 captures carry no PID-input torque.
    PackedTarget,
}

impl TorqueSource {
    fn for_capture(loaded: &Loaded) -> Self {
        if loaded.protocol_24 {
            Self::PackedTarget
        } else {
            Self::PidinTarget
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::PidinTarget => "interim_pidin_target_torque",
            Self::PackedTarget => "interim_pid_torque_target_actual>>16",
        }
    }
}

/// Signed upper half of the critical frame's `interim_pid_torque_target_actual`, the
/// torque the standstill reference script measured.
fn packed_torque_target(frame: &[u8]) -> i32 {
    let at = FRAME_HEADER_LEN + PACKED_TORQUE_OFFSET;
    frame
        .get(at..at + 4)
        .and_then(|b| b.try_into().ok())
        .map_or(0, |b| {
            i32::from((u32::from_le_bytes(b) >> 16) as u16 as i16)
        })
}

fn load(path: &Path) -> Result<Loaded, LoadError> {
    let capture = read_capture(path)?;
    let mut rows = Vec::new();
    let mut protocol_24 = false;
    let mut control: Option<ControlState> = None;
    let mut metadata: Option<Metadata> = None;
    let embedded = capture
        .records
        .iter()
        .filter_map(|r| r.device_table_metadata());
    let frames = capture.firmware_frames().map(|r| r.payload.as_slice());
    for bytes in embedded.chain(frames) {
        protocol_24 |= bytes.get(4) == Some(&ARCHIVE_PROTOCOL_VERSION);
        match decode_frame(&as_protocol_25(bytes)) {
            Ok(Frame::Critical(h, c)) if h.source_index == CHANNEL => {
                rows.push((h, c, packed_torque_target(bytes)));
            }
            Ok(Frame::ControlState(h, c)) if h.source_index == CHANNEL && control.is_none() => {
                control = Some(c);
            }
            Ok(Frame::Metadata(_, m)) if metadata.is_none() => metadata = Some(m),
            _ => {}
        }
    }
    if rows.is_empty() {
        return Err(LoadError::NoCriticals);
    }
    let metadata = metadata.ok_or(LoadError::NoMetadata)?;
    // The reference scripts sort rows by timestamp before any analysis.
    rows.sort_by_key(|(h, _, _)| h.timestamp_ticks);
    let packed_torque_target = rows.iter().map(|(_, _, t)| *t).collect();
    let criticals: Vec<(FrameHeader, Critical)> =
        rows.into_iter().map(|(h, c, _)| (h, c)).collect();
    // Protocol 24 carries no control-state frame; no verified analysis reads these two.
    let regime = Regime {
        motor_rotation_distance_mm: ROTATION_DISTANCE_MM,
        encoder_counts_per_rev: control.as_ref().map_or(0, |c| c.encoder_counts_per_rev),
        pwm_hz: control.as_ref().map_or(0, |c| c.pwm_hz),
        clock_freq: metadata.clock_freq,
    };
    let series = channel_series(&criticals, &regime);
    Ok(Loaded {
        criticals,
        series,
        regime,
        packed_torque_target,
        protocol_24,
    })
}

#[derive(Clone, Copy)]
enum Tol {
    Rel(f64),
    Abs(f64),
}

/// Compares `ours` with a reference printed to `decimals` places, allowing the
/// reference's own rounding on top of the tolerance.
fn within(ours: f64, reference: f64, tol: Tol, decimals: i32) -> bool {
    let rounding = 0.5 * 10f64.powi(-decimals);
    let allowed = match tol {
        Tol::Rel(r) => r * reference.abs(),
        Tol::Abs(a) => a,
    }
    .max(rounding);
    (ours - reference).abs() <= allowed + 1e-12
}

struct Line {
    text: String,
    breaches: Vec<String>,
}

impl Line {
    fn new(name: &str) -> Self {
        Self {
            text: name.to_owned(),
            breaches: Vec::new(),
        }
    }

    fn check(&mut self, label: &str, ours: f64, reference: f64, tol: Tol, decimals: i32) {
        let d = usize::try_from(decimals).unwrap_or(0);
        self.text
            .push_str(&format!("  {label} {ours:.d$} (ref {reference:.d$})"));
        if !within(ours, reference, tol, decimals) {
            self.breaches.push(label.to_owned());
        }
    }

    fn note(&mut self, text: &str) {
        self.text.push_str("  ");
        self.text.push_str(text);
    }

    fn finish(mut self, out: &mut dyn Write) -> io::Result<bool> {
        let ok = self.breaches.is_empty();
        if ok {
            self.text.push_str("  -- ok");
        } else {
            self.text
                .push_str(&format!("  -- BREACH: {}", self.breaches.join(", ")));
        }
        writeln!(out, "{}", self.text)?;
        Ok(ok)
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn matches_prefix(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '.']))
}

fn describe_fit(fit: &Result<DampedFit, FitError>) -> String {
    match fit {
        Ok(f) => format!("{:.2}Hz z={:.4}", f.damped_hz, f.zeta),
        Err(e) => format!("failed ({e})"),
    }
}

fn verify_probe(name: &str, loaded: &Loaded, out: &mut dyn Write) -> io::Result<Option<bool>> {
    let Some(reference) = PROBE.iter().find(|r| matches_prefix(name, r.prefix)) else {
        return Ok(None);
    };
    let m = probe_metrics(&loaded.series);
    let mut line = Line::new(reference.prefix);
    match &meter_velocity_fit(loaded) {
        Ok(f) => {
            line.check("vel_hz", f.damped_hz, reference.vel_hz, Tol::Rel(0.02), 2);
            line.check("vel_zeta", f.zeta, reference.vel_zeta, Tol::Abs(0.01), 4);
        }
        Err(e) => {
            line.note(&format!(
                "meter-vel fit failed ({e}) (ref {:.2}Hz z={:.4})",
                reference.vel_hz, reference.vel_zeta
            ));
            line.breaches.push("vel_fit".to_owned());
        }
    }
    line.check(
        "edges",
        m.edges as f64,
        reference.edges as f64,
        Tol::Abs(0.0),
        0,
    );
    line.check(
        "psd_peak_hz",
        m.torque_psd_peak_hz,
        reference.psd_peak_hz,
        Tol::Abs(PSD_BIN_HZ),
        2,
    );
    line.note(&format!("| trq {}", describe_fit(&m.torque)));
    line.note(&format!("| perr {}", describe_fit(&m.position_error)));
    line.note(&format!(
        "| info, not gated: fd-vel {}",
        describe_fit(&m.velocity)
    ));
    line.finish(out).map(Some)
}

/// The series with its torque taken from `source`.
fn with_torque(loaded: &Loaded, source: TorqueSource) -> ChannelSeries {
    match source {
        TorqueSource::PidinTarget => loaded.series.clone(),
        TorqueSource::PackedTarget => ChannelSeries {
            torque: loaded.packed_torque_target.clone(),
            ..loaded.series.clone()
        },
    }
}

/// The damping fit of the chip's `pid_velocity_actual`, the signal `fit.py` fitted.
fn meter_velocity_fit(loaded: &Loaded) -> Result<DampedFit, FitError> {
    let meter: Vec<f64> = loaded
        .criticals
        .iter()
        .map(|(_, c)| f64::from(c.pid_velocity_actual))
        .collect();
    let ff = &loaded.series.ff_offset;
    let e = ensemble(&meter, ff, &locate_edges(ff));
    fit_damped(&e.mean, 1e-3)
}

fn verify_tracking(name: &str, loaded: &Loaded, out: &mut dyn Write) -> io::Result<Option<bool>> {
    let Some(reference) = TRACKING.iter().find(|r| matches_prefix(name, r.prefix)) else {
        return Ok(None);
    };
    let m = tracking_metrics(&loaded.series, &loaded.regime);
    let mut line = Line::new(reference.prefix);
    line.check("sd", m.sd_um, reference.sd, Tol::Rel(0.01), 1);
    line.check("p99", m.p99_um, reference.p99, Tol::Rel(0.01), 1);
    line.check(
        "cruise",
        m.cruise_mean_abs_um,
        reference.cruise,
        Tol::Rel(0.01),
        1,
    );
    line.check(
        "rev_peak",
        m.reversal_peak_um,
        reference.rev_peak,
        Tol::Rel(0.01),
        1,
    );
    line.check(
        "n_rev",
        m.reversal_count as f64,
        reference.n_rev as f64,
        Tol::Abs(0.0),
        0,
    );
    line.finish(out).map(Some)
}

fn verify_standstill(name: &str, loaded: &Loaded, out: &mut dyn Write) -> io::Result<Option<bool>> {
    if !matches_prefix(name, STANDSTILL_PREFIX) {
        return Ok(None);
    }
    let source = TorqueSource::for_capture(loaded);
    let m = standstill_metrics(
        &with_torque(loaded, source),
        &loaded.regime,
        STANDSTILL_SETTLE,
        STANDSTILL_MIN_LEN,
    );
    let um_per_unit = ROTATION_DISTANCE_MM / POSITION_UNITS_PER_REV * 1000.0;
    let mut line = Line::new(STANDSTILL_PREFIX);
    line.note(&format!("torque source {}", source.label()));
    line.check(
        "rests",
        m.rests.len() as f64,
        STANDSTILL.len() as f64,
        Tol::Abs(0.0),
        0,
    );
    for (rest, reference) in m.rests.iter().zip(&STANDSTILL) {
        let k = rest.index;
        line.text.push_str("\n   ");
        line.check(
            &format!("r{k}.samples"),
            rest.samples as f64,
            reference.samples as f64,
            Tol::Abs(0.0),
            0,
        );
        line.check(
            &format!("r{k}.torque_sd"),
            rest.torque_sd,
            reference.torque_sd,
            Tol::Rel(0.01),
            3,
        );
        line.check(
            &format!("r{k}.pos_pp"),
            rest.position_pp_um / um_per_unit,
            reference.pos_pp_units,
            Tol::Abs(0.0),
            0,
        );
    }
    line.finish(out).map(Some)
}

/// Runs `kind`'s analysis over each capture in `paths`, writing one line per capture
/// (standstill adds one indented line per rest), and returns the outcome counts.
///
/// # Errors
///
/// Only when writing to `out` fails; unreadable captures are counted in
/// [`Summary::failed`].
pub fn verify_archive(kind: Kind, paths: &[PathBuf], out: &mut dyn Write) -> io::Result<Summary> {
    let mut summary = Summary::default();
    for path in paths {
        let name = file_name(path);
        let loaded = match load(path) {
            Ok(l) => l,
            Err(e) => {
                writeln!(out, "{name}  -- ERROR: {e}")?;
                summary.failed += 1;
                continue;
            }
        };
        let outcome = match kind {
            Kind::Probe => verify_probe(&name, &loaded, out)?,
            Kind::Tracking => verify_tracking(&name, &loaded, out)?,
            Kind::Standstill => verify_standstill(&name, &loaded, out)?,
        };
        match outcome {
            Some(true) => summary.passed += 1,
            Some(false) => summary.breached += 1,
            None => {
                writeln!(out, "{name}  -- skipped: no reference entry")?;
                summary.skipped += 1;
            }
        }
    }
    writeln!(
        out,
        "summary: {} within tolerance, {} breached, {} skipped, {} failed",
        summary.passed, summary.breached, summary.skipped, summary.failed
    )?;
    Ok(summary)
}
