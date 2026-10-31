//! G-code stimulus composition for tracking, probe and standstill runs.

use serde::{Deserialize, Serialize};

use crate::pattern::{Pattern, PatternError, validate_pattern};
use crate::steppers::BedLimits;

/// Rest offsets from the pattern start, in mm of stepper travel along the isolating direction.
pub const REST_OFFSETS_MM: [f64; 8] = [0.0, 0.007, 0.013, 0.021, 0.034, 0.055, 0.089, 0.144];

const SEGMENT_MM: f64 = 5.0;
const APPROACH_FEED: u32 = 3000;
const MAX_DWELL_S: f64 = 60.0;
const REST_FEED: u32 = 600;

/// The kind of run a stimulus drives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RunKind {
    /// Plain strokes at gain 1.0 over a speed set, slowest first.
    Tracking {
        /// Requested speeds in mm/s.
        speeds_mm_s: Vec<f64>,
        /// Strokes per speed.
        strokes: usize,
    },
    /// Strokes with the feedforward gain toggled between `high_gain` and 1.0.
    Probe {
        /// Requested speed in mm/s.
        speed_mm_s: f64,
        /// Gain for the odd toggles.
        high_gain: f64,
        /// Segments between gain toggles.
        segments_between_toggles: usize,
        /// Number of strokes.
        strokes: usize,
    },
    /// Rests at sub-step offsets from the pattern start.
    Standstill {
        /// Number of rests, at most the length of [`REST_OFFSETS_MM`].
        rests: usize,
        /// Dwell per rest in seconds.
        dwell_s: f64,
    },
}

/// A composed script and the speeds that were reduced to `max_velocity`.
#[derive(Debug, Clone, PartialEq)]
pub struct Stimulus {
    /// G-code lines joined by newlines.
    pub gcode: String,
    /// `(requested, applied)` mm/s for every speed that was clamped.
    pub clamped_speeds: Vec<(f64, f64)>,
    /// Applied stepper speeds in script block order, slowest first, duplicates kept.
    pub applied_speeds: Vec<f64>,
}

/// Why a stimulus could not be composed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StimulusError {
    /// The pattern or kinematics is not usable.
    #[error(transparent)]
    Pattern(#[from] PatternError),
    /// A run parameter is non-finite or out of range.
    #[error("invalid run parameter: {0}")]
    InvalidParameter(&'static str),
}

fn num(v: f64) -> String {
    let s = format!("{v:.5}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn feed(stepper_speed_mm_s: f64, ratio: f64) -> String {
    format!("{:.0}", stepper_speed_mm_s * 60.0 * ratio)
}

fn positive(v: f64, what: &'static str) -> Result<f64, StimulusError> {
    if v.is_finite() && v > 0.0 {
        Ok(v)
    } else {
        Err(StimulusError::InvalidParameter(what))
    }
}

fn clamp_speed(
    speed: f64,
    max_velocity: f64,
    ratio: f64,
    clamped: &mut Vec<(f64, f64)>,
) -> Result<f64, StimulusError> {
    positive(speed, "speed must be finite and positive")?;
    let limit = max_velocity / ratio;
    if speed > limit {
        clamped.push((speed, limit));
        Ok(limit)
    } else {
        Ok(speed)
    }
}

struct Script<'a> {
    lines: Vec<String>,
    stepper: &'a str,
    pattern: &'a Pattern,
    at_start: bool,
}

impl<'a> Script<'a> {
    fn new(stepper: &'a str, pattern: &'a Pattern) -> Self {
        let mut s = Self {
            lines: vec!["G90".to_owned()],
            stepper,
            pattern,
            at_start: true,
        };
        s.goto(pattern.start, &APPROACH_FEED.to_string());
        s.push("M400");
        s.push("G4 P500");
        s
    }

    fn push(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    fn gain(&mut self, gain: &str) {
        let line = format!(
            "FOCI_SET_VELOCITY_FEEDFORWARD STEPPER={} ENABLE=1 GAIN={gain}",
            self.stepper
        );
        self.push(line);
    }

    fn goto(&mut self, (x, y): (f64, f64), feed: &str) {
        self.push(format!("G1 X{} Y{} F{feed}", num(x), num(y)));
    }

    fn finish(mut self) -> String {
        self.gain("1.0");
        self.push("M400");
        self.gain("1.0");
        self.lines.join("\n")
    }

    fn stroke(&mut self, feed: &str, on_segment: &mut dyn FnMut(&mut Self)) {
        let (from, to) = if self.at_start {
            (self.pattern.start, self.pattern.end)
        } else {
            (self.pattern.end, self.pattern.start)
        };
        let reach = (to.0 - from.0).abs().max((to.1 - from.1).abs());
        let n = (reach / SEGMENT_MM).ceil().max(1.0) as usize;
        for i in 1..=n {
            let t = i as f64 / n as f64;
            self.goto(
                (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t),
                feed,
            );
            on_segment(self);
        }
        self.at_start = !self.at_start;
    }
}

/// Composes the script for `kind`.
///
/// Speeds are stepper (motor) linear speeds. The feed sent to Klipper is the path speed, which
/// on the corexy isolating diagonal is `1/sqrt(2)` of the stepper speed, and speeds are clamped
/// so the path speed stays within `max_velocity` before the script is built.
///
/// # Errors
/// Unsupported kinematics, a pattern outside `limits` minus margin, or any non-finite or
/// out-of-range run parameter.
pub fn compose(
    kind: &RunKind,
    pattern: &Pattern,
    stepper: &str,
    kinematics: &str,
    limits: &BedLimits,
    max_velocity: f64,
) -> Result<Stimulus, StimulusError> {
    let ratio = match kinematics {
        "corexy" => std::f64::consts::FRAC_1_SQRT_2,
        "cartesian" => 1.0,
        other => return Err(PatternError::UnsupportedKinematics(other.to_owned()).into()),
    };
    validate_pattern(pattern, limits)?;
    positive(max_velocity, "max_velocity must be finite and positive")?;
    let mut clamped = Vec::new();
    let mut applied = Vec::new();
    let mut s = Script::new(stepper, pattern);
    match kind {
        RunKind::Tracking {
            speeds_mm_s,
            strokes,
        } => {
            if speeds_mm_s.is_empty() {
                return Err(StimulusError::InvalidParameter("speed list is empty"));
            }
            if *strokes == 0 {
                return Err(StimulusError::InvalidParameter(
                    "strokes must be at least 1",
                ));
            }
            for &v in speeds_mm_s {
                applied.push(clamp_speed(v, max_velocity, ratio, &mut clamped)?);
            }
            applied.sort_by(f64::total_cmp);
            s.lines.insert(1, feedforward_reset_line(stepper));
            for (i, speed) in applied.iter().enumerate() {
                if i > 0 {
                    s.push("G4 P500");
                }
                let f = feed(*speed, ratio);
                for _ in 0..*strokes {
                    s.stroke(&f, &mut |_| {});
                }
            }
        }
        RunKind::Probe {
            speed_mm_s,
            high_gain,
            segments_between_toggles,
            strokes,
        } => {
            positive(*high_gain, "high_gain must be finite and positive")?;
            if *segments_between_toggles == 0 {
                return Err(StimulusError::InvalidParameter(
                    "segments_between_toggles must be at least 1",
                ));
            }
            if *strokes == 0 {
                return Err(StimulusError::InvalidParameter(
                    "strokes must be at least 1",
                ));
            }
            let speed = clamp_speed(*speed_mm_s, max_velocity, ratio, &mut clamped)?;
            applied.push(speed);
            let f = feed(speed, ratio);
            let every = *segments_between_toggles;
            let high = format!("{high_gain:?}");
            s.lines.insert(1, feedforward_reset_line(stepper));
            let (mut seg, mut toggles) = (0usize, 0usize);
            for _ in 0..*strokes {
                s.stroke(&f, &mut |sc| {
                    seg += 1;
                    if seg % every == 0 {
                        sc.gain(if toggles % 2 == 0 { &high } else { "1.0" });
                        toggles += 1;
                    }
                });
            }
        }
        RunKind::Standstill { rests, dwell_s } => {
            if *rests == 0 || *rests > REST_OFFSETS_MM.len() {
                return Err(StimulusError::InvalidParameter(
                    "rests must be between 1 and 8",
                ));
            }
            if !dwell_s.is_finite() || *dwell_s <= 0.0 || *dwell_s > MAX_DWELL_S {
                return Err(StimulusError::InvalidParameter(
                    "dwell must be in (0, 60] s",
                ));
            }
            let (dx, dy) = (
                pattern.end.0 - pattern.start.0,
                pattern.end.1 - pattern.start.1,
            );
            let len = dx.hypot(dy);
            let (ux, uy) = (dx / len, dy / len);
            if !(ux.is_finite() && uy.is_finite()) {
                return Err(StimulusError::InvalidParameter(
                    "pattern direction is not finite",
                ));
            }
            let dwell_ms = (dwell_s * 1000.0).round() as u64;
            for off in REST_OFFSETS_MM.iter().take(*rests) {
                let path = off * ratio;
                s.goto(
                    (pattern.start.0 + path * ux, pattern.start.1 + path * uy),
                    &REST_FEED.to_string(),
                );
                s.push(format!("G4 P{dwell_ms}"));
                s.push("M400");
            }
            return Ok(Stimulus {
                gcode: s.lines.join("\n"),
                clamped_speeds: clamped,
                applied_speeds: applied,
            });
        }
    }
    Ok(Stimulus {
        gcode: s.finish(),
        clamped_speeds: clamped,
        applied_speeds: applied,
    })
}

/// The feedforward line the tracking and probe scripts start and end with: enabled at gain
/// 1.0. Sent on its own to restore the gain after a script that did not finish.
pub fn feedforward_reset_line(stepper: &str) -> String {
    format!("FOCI_SET_VELOCITY_FEEDFORWARD STEPPER={stepper} ENABLE=1 GAIN=1.0")
}
