//! G-code stimulus composition for tracking, probe and standstill runs.

use serde::{Deserialize, Serialize};

use crate::pattern::{Pattern, PatternError, validate_pattern};
use crate::steppers::BedLimits;

/// Rest offsets from the pattern start, in mm of stepper travel along the isolating direction.
pub const REST_OFFSETS_MM: [f64; 8] = [0.0, 0.007, 0.013, 0.021, 0.034, 0.055, 0.089, 0.144];

/// Most strokes a tracking (per speed) or probe run may ask for.
pub const MAX_STROKES: usize = 50;
/// Most speeds a tracking run may ask for.
pub const MAX_SPEEDS: usize = 10;
/// Slowest stepper speed a tracking or probe run may ask for, in mm/s.
pub const MIN_SPEED_MM_S: f64 = 1.0;
/// Slowest feed, in mm/min, a stroke may be sent with after rounding.
pub const MIN_FEED_MM_MIN: u32 = 60;
/// Longest predicted script, in seconds. Abort waits for the script in flight, so this also
/// bounds how long an abort can take.
pub const MAX_SCRIPT_S: f64 = 900.0;
/// Shortest time between two probe gain toggles, in seconds. The probe's edge detector needs
/// 75 samples (15 before, 60 after, at 1 kHz) of constant feedforward offset around an edge;
/// the rest is margin for the planner's segment-boundary changes.
pub const MIN_TOGGLE_SPACING_S: f64 = 0.1;

const SEGMENT_MM: f64 = 5.0;
const SETTLE_LINE: &str = "G4 P500";
const SETTLE_S: f64 = 0.5;
const APPROACH_FEED: u32 = 3000;
const MAX_DWELL_S: f64 = 60.0;
const REST_FEED: u32 = 600;
/// Absorbs rounding in the spacing estimate, so a spacing of exactly the minimum passes.
const SPACING_TOLERANCE_S: f64 = 1e-9;

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
    /// Predicted script duration in seconds: the strokes at their sent feed, the settle dwells
    /// and the rests. The approach to the pattern is not counted.
    pub predicted_s: f64,
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
    /// A size parameter exceeds its ceiling.
    #[error("{parameter} must be at most {max}")]
    TooLarge {
        /// Parameter name as in [`RunKind`].
        parameter: &'static str,
        /// The ceiling.
        max: usize,
    },
    /// A speed is below [`MIN_SPEED_MM_S`], or its feed rounds below [`MIN_FEED_MM_MIN`].
    #[error(
        "{parameter}: {speed_mm_s} mm/s is too slow; speeds must be at least {MIN_SPEED_MM_S} \
         mm/s and give a feed of at least F{MIN_FEED_MM_MIN}"
    )]
    TooSlow {
        /// Parameter name as in [`RunKind`].
        parameter: &'static str,
        /// The speed, after clamping to `max_velocity`.
        speed_mm_s: f64,
    },
    /// The predicted script duration exceeds [`MAX_SCRIPT_S`].
    #[error(
        "{parameter}: the script would take {predicted_s:.1} s, more than the {max_s} s allowed"
    )]
    TooLong {
        /// Parameter name as in [`RunKind`].
        parameter: &'static str,
        /// Predicted duration in seconds.
        predicted_s: f64,
        /// The ceiling, [`MAX_SCRIPT_S`].
        max_s: f64,
    },
    /// Probe gain toggles would come closer than [`MIN_TOGGLE_SPACING_S`].
    #[error(
        "{parameter}: toggle spacing {:.0} ms at {speed_mm_s:.0} mm/s is below the {:.0} ms the \
         edge detector needs; use at least {min_segments} segments between toggles",
        .spacing_s * 1000.0,
        MIN_TOGGLE_SPACING_S * 1000.0
    )]
    TogglesTooClose {
        /// Parameter name as in [`RunKind`].
        parameter: &'static str,
        /// Predicted time between toggles in seconds.
        spacing_s: f64,
        /// The applied stepper speed in mm/s.
        speed_mm_s: f64,
        /// Fewest segments between toggles that reach the minimum at this speed.
        min_segments: usize,
    },
}

fn num(v: f64) -> String {
    let s = format!("{v:.5}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn at_most(value: usize, max: usize, parameter: &'static str) -> Result<(), StimulusError> {
    if value > max {
        Err(StimulusError::TooLarge { parameter, max })
    } else {
        Ok(())
    }
}

fn feed(stepper_speed_mm_s: f64, ratio: f64) -> String {
    format!("{:.0}", stepper_speed_mm_s * 60.0 * ratio)
}

/// The feed [`feed`] formats, as a number: `{:.0}` rounds ties to even.
fn feed_mm_min(stepper_speed_mm_s: f64, ratio: f64) -> f64 {
    (stepper_speed_mm_s * 60.0 * ratio).round_ties_even()
}

fn strokes_s(strokes: usize, length_mm: f64, feed_mm_min: f64) -> f64 {
    strokes as f64 * length_mm * 60.0 / feed_mm_min
}

fn within_budget(predicted_s: f64, parameter: &'static str) -> Result<(), StimulusError> {
    if predicted_s > MAX_SCRIPT_S {
        Err(StimulusError::TooLong {
            parameter,
            predicted_s,
            max_s: MAX_SCRIPT_S,
        })
    } else {
        Ok(())
    }
}

/// Stepper travel of one stroke segment, in mm: the path length of a segment over the path to
/// stepper speed ratio, so on the corexy diagonal a 5 mm per-axis segment is 10 mm.
fn segment_travel_mm(pattern: &Pattern, length_mm: f64, ratio: f64) -> f64 {
    let (dx, dy) = (
        pattern.end.0 - pattern.start.0,
        pattern.end.1 - pattern.start.1,
    );
    let n = segments_per_stroke(dx.abs().max(dy.abs()));
    length_mm / n as f64 / ratio
}

fn segments_per_stroke(reach_mm: f64) -> usize {
    (reach_mm / SEGMENT_MM).ceil().max(1.0) as usize
}

fn toggles_apart(travel_mm: f64, every: usize, speed: f64) -> Result<(), StimulusError> {
    let spacing_s = travel_mm * every as f64 / speed;
    if spacing_s + SPACING_TOLERANCE_S >= MIN_TOGGLE_SPACING_S {
        return Ok(());
    }
    let needed = MIN_TOGGLE_SPACING_S * speed / travel_mm - SPACING_TOLERANCE_S;
    Err(StimulusError::TogglesTooClose {
        parameter: "segments_between_toggles",
        spacing_s,
        speed_mm_s: speed,
        min_segments: needed.ceil().max(1.0) as usize,
    })
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
    parameter: &'static str,
    clamped: &mut Vec<(f64, f64)>,
) -> Result<f64, StimulusError> {
    positive(speed, "speed must be finite and positive")?;
    let limit = max_velocity / ratio;
    let applied = if speed > limit {
        clamped.push((speed, limit));
        limit
    } else {
        speed
    };
    if applied < MIN_SPEED_MM_S || feed_mm_min(applied, ratio) < f64::from(MIN_FEED_MM_MIN) {
        return Err(StimulusError::TooSlow {
            parameter,
            speed_mm_s: applied,
        });
    }
    Ok(applied)
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
        s.push(SETTLE_LINE);
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
        let n = segments_per_stroke(reach);
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
/// The predicted duration counts the strokes at their sent feed, the settle dwells and, for a
/// standstill, the rests and the moves between them; the approach from wherever the toolhead
/// stands is not counted.
///
/// # Errors
/// Unsupported kinematics, a pattern outside `limits` minus margin, any non-finite or
/// out-of-range run parameter, a speed below [`MIN_SPEED_MM_S`] or [`MIN_FEED_MM_MIN`], probe
/// toggles closer than [`MIN_TOGGLE_SPACING_S`] at the applied speed, or a predicted duration
/// above [`MAX_SCRIPT_S`].
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
    let length = (pattern.end.0 - pattern.start.0).hypot(pattern.end.1 - pattern.start.1);
    let mut s = Script::new(stepper, pattern);
    let predicted_s;
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
            at_most(speeds_mm_s.len(), MAX_SPEEDS, "speeds_mm_s")?;
            at_most(*strokes, MAX_STROKES, "strokes")?;
            for &v in speeds_mm_s {
                applied.push(clamp_speed(
                    v,
                    max_velocity,
                    ratio,
                    "speeds_mm_s",
                    &mut clamped,
                )?);
            }
            applied.sort_by(f64::total_cmp);
            let predicted = applied.iter().fold(0.0, |total, &v| {
                total + SETTLE_S + strokes_s(*strokes, length, feed_mm_min(v, ratio))
            });
            within_budget(predicted, "speeds_mm_s")?;
            predicted_s = predicted;
            s.lines.insert(1, feedforward_reset_line(stepper));
            for (i, speed) in applied.iter().enumerate() {
                if i > 0 {
                    s.push(SETTLE_LINE);
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
            at_most(*strokes, MAX_STROKES, "strokes")?;
            let speed = clamp_speed(*speed_mm_s, max_velocity, ratio, "speed_mm_s", &mut clamped)?;
            toggles_apart(
                segment_travel_mm(pattern, length, ratio),
                *segments_between_toggles,
                speed,
            )?;
            let predicted = SETTLE_S + strokes_s(*strokes, length, feed_mm_min(speed, ratio));
            within_budget(predicted, "speed_mm_s")?;
            predicted_s = predicted;
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
            let (ux, uy) = (dx / length, dy / length);
            let offsets = &REST_OFFSETS_MM[..*rests];
            let moves_mm: f64 = offsets.windows(2).map(|w| (w[1] - w[0]) * ratio).sum();
            let predicted =
                SETTLE_S + *rests as f64 * dwell_s + moves_mm * 60.0 / f64::from(REST_FEED);
            within_budget(predicted, "dwell_s")?;
            let dwell_ms = (dwell_s * 1000.0).round() as u64;
            for off in offsets {
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
                predicted_s: predicted,
            });
        }
    }
    Ok(Stimulus {
        gcode: s.finish(),
        clamped_speeds: clamped,
        applied_speeds: applied,
        predicted_s,
    })
}

/// The feedforward line the tracking and probe scripts start and end with: enabled at gain
/// 1.0. Sent on its own to restore the gain after a script that did not finish.
pub fn feedforward_reset_line(stepper: &str) -> String {
    format!("FOCI_SET_VELOCITY_FEEDFORWARD STEPPER={stepper} ENABLE=1 GAIN=1.0")
}
