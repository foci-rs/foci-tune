//! Two-point stroke patterns proposed from the stepper's kinematics.

use serde::{Deserialize, Serialize};

use crate::steppers::BedLimits;

/// Longest stroke proposed, as toolhead path length in mm.
const DEFAULT_STROKE_MM: f64 = 200.0;
/// Shortest stroke worth proposing. Below it the probe finds too few clean edges.
const MIN_STROKE_MM: f64 = 50.0;
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
    /// The axis limits minus the margins leave no room for the shortest stroke.
    #[error(
        "the bed leaves less than {MIN_STROKE_MM} mm for a stroke inside the {MARGIN_MM} mm margins"
    )]
    BedTooSmall,
    /// A point lies outside the axis limits minus the margin.
    #[error("pattern lies outside the bed limits minus the {MARGIN_MM} mm margin")]
    OutsideLimits,
    /// Start and end are closer than 1 mm, so there is no stroke direction.
    #[error("pattern is shorter than {MIN_LENGTH_MM} mm")]
    ZeroLength,
}

/// Proposes a centred stroke of up to 200 mm along the stepper's isolating direction, shortened
/// to fit inside the axis limits minus the margins.
///
/// A corexy diagonal moves both axes by the same distance, so the usable travel of the smaller
/// axis sets its length. A cartesian stroke runs along the stepper's own axis only.
///
/// # Errors
/// Refuses other kinematics or steppers, and beds that leave less than 50 mm for a stroke.
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
    if stepper != "stepper_x" && stepper != "stepper_y" {
        return Err(PatternError::UnsupportedStepper(stepper.to_owned()));
    }
    let centre = |(lo, hi): (f64, f64)| (lo + hi) / 2.0;
    let usable = |(lo, hi): (f64, f64)| (hi - lo - 2.0 * MARGIN_MM).floor();
    let (cx, cy) = (centre(limits.x), centre(limits.y));
    let (tx, ty) = (usable(limits.x), usable(limits.y));
    let (delta, stroke_per_delta) = match (corexy, stepper) {
        (true, _) => (
            tx.min(ty)
                .min((DEFAULT_STROKE_MM / std::f64::consts::SQRT_2).floor()),
            std::f64::consts::SQRT_2,
        ),
        (false, "stepper_x") if ty >= 0.0 => (tx.min(DEFAULT_STROKE_MM), 1.0),
        (false, "stepper_y") if tx >= 0.0 => (ty.min(DEFAULT_STROKE_MM), 1.0),
        (false, _) => return Err(PatternError::BedTooSmall),
    };
    if delta * stroke_per_delta < MIN_STROKE_MM {
        return Err(PatternError::BedTooSmall);
    }
    let h = delta / 2.0;
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
        _ => Pattern {
            start: (cx, cy - h),
            end: (cx, cy + h),
        },
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
