//! Ladder value lists, ordering and the damping stop rule.

use serde::{Deserialize, Serialize};

use crate::signals::velocity_i_raw;

/// Largest raw gain `FOCI_SET_GAINS` accepts.
pub const PID_GAIN_MAX_RAW: i32 = 0x7FFF;

const DEFAULT_STEPS: i32 = 6;
const CORNER_DEFAULTS_HZ: [f64; 4] = [25.0, 50.0, 75.0, 100.0];

/// The single axis a ladder varies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LadderAxis {
    /// Velocity P gain, entered as a raw value.
    VelocityP,
    /// Velocity I gain, entered as a corner frequency in Hz.
    VelocityICornerHz,
    /// Position P gain, entered as a raw value.
    PositionP,
}

/// A ladder value as the operator sees it and as it is written.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LadderValue {
    /// Value in the axis' display unit.
    pub shown: f64,
    /// Raw gain written to the board.
    pub raw: i32,
}

/// A ladder value that cannot be written.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum LadderError {
    /// The raw value falls outside `0..=PID_GAIN_MAX_RAW`.
    #[error("{shown} converts to raw {raw}, outside 0..={PID_GAIN_MAX_RAW}")]
    OutOfRange {
        /// Value as entered.
        shown: f64,
        /// Converted raw value.
        raw: i32,
    },
}

/// Converts a shown value to raw and checks it against the `FOCI_SET_GAINS` range.
///
/// # Errors
///
/// Returns [`LadderError::OutOfRange`] when the raw value is negative, above
/// [`PID_GAIN_MAX_RAW`], or the input is not finite.
pub fn to_raw(axis: LadderAxis, shown: f64, pwm_hz: u32) -> Result<LadderValue, LadderError> {
    let raw = match axis {
        LadderAxis::VelocityP | LadderAxis::PositionP => shown.round() as i32,
        LadderAxis::VelocityICornerHz => velocity_i_raw(shown, pwm_hz),
    };
    if shown.is_finite() && (0..=PID_GAIN_MAX_RAW).contains(&raw) {
        Ok(LadderValue { shown, raw })
    } else {
        Err(LadderError::OutOfRange { shown, raw })
    }
}

/// Default ladder values for an axis, with every out-of-range rung omitted.
pub fn default_values(axis: LadderAxis, current_raw: i32, pwm_hz: u32) -> Vec<LadderValue> {
    let current = f64::from(current_raw);
    let shown: Vec<f64> = match axis {
        LadderAxis::VelocityP => (0..DEFAULT_STEPS)
            .map(|k| current + f64::from(100 * k))
            .collect(),
        LadderAxis::PositionP => (0..DEFAULT_STEPS)
            .map(|k| current - 20.0 + f64::from(20 * k))
            .collect(),
        LadderAxis::VelocityICornerHz => CORNER_DEFAULTS_HZ.to_vec(),
    };
    shown
        .into_iter()
        .filter_map(|s| to_raw(axis, s, pwm_hz).ok())
        .collect()
}

/// Queue order as `(value_index, repeat)`: value by value, repeat by repeat (A B C A B C).
pub fn interleave(values: usize, repeats: usize) -> Vec<(usize, usize)> {
    (0..repeats)
        .flat_map(|r| (0..values).map(move |v| (v, r)))
        .collect()
}

/// Headline damping of one finished run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RungOutcome {
    /// Velocity zeta of the fit.
    Assessed(f64),
    /// No zeta: the fit failed or the run is not comparable.
    Unassessable,
}

/// What a probe ladder does after a value's runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StopDecision {
    /// Advance to the next value.
    Continue,
    /// The median zeta is below the floor.
    StopBelowFloor {
        /// Median zeta over the value's repeats.
        median: f64,
    },
    /// A repeat has no zeta; the value could not be assessed.
    StopUnassessable,
}

/// Applies the probe-ladder stop rule to the outcomes of exactly one value.
///
/// The queue is interleaved (A B C A B C), so one value's runs are not
/// contiguous in global order: the caller gathers `outcomes` by value index, in
/// repeat order. Returns `Continue` until `repeats` outcomes exist; only the
/// first `repeats` are evaluated, so the median is never taken over a wider set.
pub fn probe_stop(outcomes: &[RungOutcome], repeats: usize, floor: f64) -> StopDecision {
    if repeats == 0 || outcomes.len() < repeats {
        return StopDecision::Continue;
    }
    let outcomes = &outcomes[..repeats];
    let mut zetas = Vec::with_capacity(outcomes.len());
    for outcome in outcomes {
        match outcome {
            RungOutcome::Assessed(z) if z.is_finite() => zetas.push(*z),
            RungOutcome::Assessed(_) | RungOutcome::Unassessable => {
                return StopDecision::StopUnassessable;
            }
        }
    }
    let m = median(&zetas);
    if m < floor {
        StopDecision::StopBelowFloor { median: m }
    } else {
        StopDecision::Continue
    }
}

/// Median of the values; NaN for an empty slice.
pub fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    }
}

/// True when zeta is below the marginal threshold.
pub fn is_marginal(zeta: f64, marginal: f64) -> bool {
    zeta < marginal
}
