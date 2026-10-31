//! G-code stimulus composition for tracking, probe and standstill runs.

use serde::{Deserialize, Serialize};

use crate::pattern::Pattern;

/// Rest offsets from the pattern start along the stroke direction, in mm.
pub const REST_OFFSETS_MM: [f64; 8] = [0.0, 0.007, 0.013, 0.021, 0.034, 0.055, 0.089, 0.144];

const SEGMENT_MM: f64 = 5.0;
const APPROACH_FEED: u32 = 3000;
const STANDSTILL_APPROACH_FEED: u32 = 6000;
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
}

fn num(v: f64) -> String {
    let s = format!("{v:.5}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn feed(speed_mm_s: f64) -> String {
    num(speed_mm_s * 60.0)
}

fn clamp_speed(speed: f64, max: f64, clamped: &mut Vec<(f64, f64)>) -> f64 {
    if speed > max {
        clamped.push((speed, max));
        max
    } else {
        speed
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
        Self {
            lines: vec!["G90".to_owned()],
            stepper,
            pattern,
            at_start: true,
        }
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

    fn approach_start(&mut self) {
        let start = self.pattern.start;
        self.goto(start, &APPROACH_FEED.to_string());
        self.push("M400");
        self.push("G4 P500");
    }

    fn finish(mut self) -> String {
        self.gain("1.0");
        self.push("M400");
        self.gain("1.0");
        self.lines.join("\n")
    }

    /// One stroke towards the far end; `on_segment` runs after each segment move.
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

/// Composes the script for `kind`, clamping speeds to `max_velocity` first.
///
/// The pattern is not validated here; callers run `validate_pattern` first.
pub fn compose(kind: &RunKind, pattern: &Pattern, stepper: &str, max_velocity: f64) -> Stimulus {
    let mut clamped = Vec::new();
    let mut s = Script::new(stepper, pattern);
    match kind {
        RunKind::Tracking {
            speeds_mm_s,
            strokes,
        } => {
            let mut speeds: Vec<f64> = speeds_mm_s
                .iter()
                .map(|&v| clamp_speed(v, max_velocity, &mut clamped))
                .collect();
            speeds.sort_by(f64::total_cmp);
            s.gain("1.0");
            s.approach_start();
            for (i, speed) in speeds.iter().enumerate() {
                if i > 0 {
                    s.push("G4 P500");
                }
                let f = feed(*speed);
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
            let speed = clamp_speed(*speed_mm_s, max_velocity, &mut clamped);
            let f = feed(speed);
            let every = (*segments_between_toggles).max(1);
            let high = format!("{high_gain:?}");
            s.gain("1.0");
            s.approach_start();
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
            s.goto(pattern.start, &STANDSTILL_APPROACH_FEED.to_string());
            s.push("M400");
            let (dx, dy) = (
                pattern.end.0 - pattern.start.0,
                pattern.end.1 - pattern.start.1,
            );
            let len = dx.hypot(dy);
            let (ux, uy) = if len > 0.0 {
                (dx / len, dy / len)
            } else {
                (0.0, 0.0)
            };
            let dwell_ms = (dwell_s * 1000.0).round() as u64;
            for off in REST_OFFSETS_MM.iter().take(*rests) {
                s.goto(
                    (pattern.start.0 + off * ux, pattern.start.1 + off * uy),
                    &REST_FEED.to_string(),
                );
                s.push(format!("G4 P{dwell_ms}"));
                s.push("M400");
            }
            return Stimulus {
                gcode: s.lines.join("\n"),
                clamped_speeds: clamped,
            };
        }
    }
    Stimulus {
        gcode: s.finish(),
        clamped_speeds: clamped,
    }
}
