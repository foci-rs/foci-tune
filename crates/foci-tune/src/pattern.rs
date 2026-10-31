//! Two-point stroke patterns proposed from the stepper's kinematics.

use serde::{Deserialize, Serialize};

use crate::steppers::BedLimits;

const SPAN_MM: f64 = 100.0;
const MARGIN_MM: f64 = 10.0;
const MIN_LENGTH_MM: f64 = 1.0;

/// A stroke between two bed points; strokes run start to end and back.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pattern {
    /// Start point `(x, y)` in mm.
    pub start: (f64, f64),
    /// End point `(x, y)` in mm.
    pub end: (f64, f64),
}

/// Why a pattern could not be proposed or accepted.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PatternError {
    /// Only cartesian and corexy have a known isolating direction.
    #[error("kinematics `{0}` has no known isolating direction")]
    UnsupportedKinematics(String),
    /// Only `stepper_x` and `stepper_y` have a known isolating direction.
    #[error("stepper `{0}` has no known isolating direction")]
    UnsupportedStepper(String),
    /// A point lies outside the axis limits minus the margin.
    #[error("pattern lies outside the bed limits minus the {MARGIN_MM} mm margin")]
    OutsideLimits,
    /// Start and end are closer than 1 mm, so there is no stroke direction.
    #[error("pattern is shorter than {MIN_LENGTH_MM} mm")]
    ZeroLength,
}

/// Proposes a centred 100 mm pattern along the stepper's isolating direction.
///
/// # Errors
/// Refuses other kinematics or steppers, and beds too small for the span plus margins.
pub fn propose_pattern(
    kinematics: &str,
    stepper: &str,
    limits: &BedLimits,
) -> Result<Pattern, PatternError> {
    let corexy = match kinematics {
        "corexy" => true,
        "cartesian" => false,
        other => return Err(PatternError::UnsupportedKinematics(other.to_owned())),
    };
    let centre = |(lo, hi): (f64, f64)| (lo + hi) / 2.0;
    let (cx, cy) = (centre(limits.x), centre(limits.y));
    let h = SPAN_MM / 2.0;
    let pattern = match (stepper, corexy) {
        ("stepper_x", true) => Pattern {
            start: (cx - h, cy - h),
            end: (cx + h, cy + h),
        },
        ("stepper_y", true) => Pattern {
            start: (cx - h, cy + h),
            end: (cx + h, cy - h),
        },
        ("stepper_x", false) => Pattern {
            start: (cx - h, cy),
            end: (cx + h, cy),
        },
        ("stepper_y", false) => Pattern {
            start: (cx, cy - h),
            end: (cx, cy + h),
        },
        (other, _) => return Err(PatternError::UnsupportedStepper(other.to_owned())),
    };
    validate_pattern(&pattern, limits)?;
    Ok(pattern)
}

/// Checks both points against the axis limits shrunk by the 10 mm margin.
///
/// # Errors
/// [`PatternError::OutsideLimits`] when any coordinate falls outside, or is not finite;
/// [`PatternError::ZeroLength`] when start and end are closer than 1 mm.
pub fn validate_pattern(p: &Pattern, limits: &BedLimits) -> Result<(), PatternError> {
    let inside = |v: f64, (lo, hi): (f64, f64)| v >= lo + MARGIN_MM && v <= hi - MARGIN_MM;
    let ok = [p.start, p.end]
        .iter()
        .all(|&(x, y)| inside(x, limits.x) && inside(y, limits.y));
    if !ok {
        return Err(PatternError::OutsideLimits);
    }
    if (p.end.0 - p.start.0).hypot(p.end.1 - p.start.1) < MIN_LENGTH_MM {
        return Err(PatternError::ZeroLength);
    }
    Ok(())
}
