//! Seed validation and geometric (third-octave) rung values.

use crate::ladder::{LadderAxis, LadderError, LadderValue, to_raw};
use crate::signals::corner_hz;

/// Corner frequency in Hz that seeds the velocity I ladder when the live gain is zero.
pub const SEED_CORNER_HZ: f64 = 50.0;
/// Default lowest rung exponent: the seed times `2^(k/3)`.
pub const DEFAULT_LOWER_K: i32 = -3;
/// Default highest rung exponent.
pub const DEFAULT_UPPER_K: i32 = 3;

/// A displayed seed that cannot start a ladder.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum SeedRefused {
    /// A zero seed would scale to zero at every rung.
    #[error("a zero seed cannot be scaled into rungs")]
    Zero,
    /// The seed is nonzero but its raw value rounds to zero.
    #[error("the seed rounds to a raw value of zero")]
    RoundsToZero,
    /// The seed converts outside the writable range.
    #[error(transparent)]
    OutOfRange(LadderError),
}

/// Why a rung is absent from [`Rungs::values`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RungNote {
    /// The shown value converts outside the writable range.
    OutOfRange(f64),
    /// The shown value rounds to a raw value of zero.
    RoundedToZero(f64),
    /// The shown value rounds to the raw value of an earlier rung.
    Merged {
        /// Omitted rung, in the axis' display unit.
        shown: f64,
        /// Kept rung with the same raw value.
        into: f64,
    },
}

/// Ascending rung values and the reasons any rung was left out.
#[derive(Debug, Clone, PartialEq)]
pub struct Rungs {
    /// Writable rungs, ascending by raw value.
    pub values: Vec<LadderValue>,
    /// One note per omitted rung.
    pub notes: Vec<RungNote>,
}

/// Converts a displayed seed (raw for velocity P and position P, Hz for the velocity I
/// corner) into a validated raw seed.
///
/// A velocity I corner of exactly zero is the live-zero case and returns `0`.
///
/// # Errors
///
/// Returns [`SeedRefused`] for a zero or zero-rounding seed, or one outside the writable range.
pub fn seed_from(axis: LadderAxis, shown: f64, pwm_hz: u32) -> Result<i32, SeedRefused> {
    let is_corner = axis == LadderAxis::VelocityICornerHz;
    if shown == 0.0 {
        return if is_corner {
            Ok(0)
        } else {
            Err(SeedRefused::Zero)
        };
    }
    let raw = to_raw(axis, shown, pwm_hz)
        .map_err(SeedRefused::OutOfRange)?
        .raw;
    if raw == 0 {
        Err(SeedRefused::RoundsToZero)
    } else {
        Ok(raw)
    }
}

/// Third-octave rung values `seed * 2^(k/3)` for `k` in `lower_k..=upper_k`.
///
/// `seed_raw` must come from [`seed_from`]. Velocity I scales in Hz, with a zero seed standing
/// in as [`SEED_CORNER_HZ`]. Rungs that fall outside the writable range, round to zero, or
/// repeat an earlier raw value are omitted and noted.
pub fn rung_values(
    axis: LadderAxis,
    seed_raw: i32,
    pwm_hz: u32,
    lower_k: i32,
    upper_k: i32,
) -> Rungs {
    let base = match axis {
        LadderAxis::VelocityP | LadderAxis::PositionP => f64::from(seed_raw),
        LadderAxis::VelocityICornerHz if seed_raw == 0 => SEED_CORNER_HZ,
        LadderAxis::VelocityICornerHz => corner_hz(seed_raw, pwm_hz),
    };
    let mut rungs = Rungs {
        values: Vec::new(),
        notes: Vec::new(),
    };
    for k in lower_k..=upper_k {
        let shown = base * 2f64.powf(f64::from(k) / 3.0);
        let Ok(value) = to_raw(axis, shown, pwm_hz) else {
            rungs.notes.push(RungNote::OutOfRange(shown));
            continue;
        };
        if value.raw == 0 {
            rungs.notes.push(RungNote::RoundedToZero(shown));
        } else if let Some(kept) = rungs.values.iter().find(|v| v.raw == value.raw) {
            rungs.notes.push(RungNote::Merged {
                shown,
                into: kept.shown,
            });
        } else {
            rungs.values.push(value);
        }
    }
    rungs
}
