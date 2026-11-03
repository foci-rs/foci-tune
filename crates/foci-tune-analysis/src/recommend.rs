//! Gain recommendation from scored rungs, interpolated in log gain.

use serde::{Deserialize, Serialize};

use crate::verdict::LadderAssessment;

/// Why no gain could be recommended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoRecommendation {
    /// No rung carries a score.
    NoScoredRung,
}

/// The recommended gain.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Recommendation {
    /// Nothing to recommend.
    None(NoRecommendation),
    /// A measured rung is the recommendation.
    Measured {
        /// Index of the rung.
        rung: usize,
    },
    /// The fitted optimum lies between two adjacent rungs.
    Interpolated {
        /// Recommended gain.
        shown: f64,
        /// Adjacent rung indices whose gains bracket `shown`.
        between: (usize, usize),
    },
}

/// Direction in which extending the ladder may find a better gain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Extension {
    /// The best rung is the lowest of the ladder.
    Below,
    /// The best rung is the highest of the ladder.
    Above,
}

/// A recommendation plus an optional offer to extend the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Recommended {
    /// The recommended gain.
    pub choice: Recommendation,
    /// Extension worth offering; never acted on automatically.
    pub extend: Option<Extension>,
}

const SAME_GAIN: f64 = 1e-9;

/// Recommends a gain from the scored rungs.
///
/// `shown` holds the rung gains in ascending order, aligned with `assessment.verdicts`. A tie
/// recommends the lowest gain of the tie set as measured. Otherwise the best rung is refined by
/// a quadratic fit in `ln(gain)` through it and its two neighbours, only when all three share
/// one deciding speed; the result is clamped to their span, so it never extrapolates.
pub fn recommend(assessment: &LadderAssessment, shown: &[f64]) -> Recommended {
    let scores: Vec<Option<f64>> = assessment.verdicts.iter().map(|v| v.score).collect();
    let best = assessment.best;

    let Some(best) = best else {
        return Recommended {
            choice: Recommendation::None(NoRecommendation::NoScoredRung),
            extend: None,
        };
    };

    let extend = if best == 0 {
        Some(Extension::Below)
    } else if best + 1 >= shown.len() {
        Some(Extension::Above)
    } else {
        None
    };

    let choice = match assessment.tie.first() {
        Some(&lowest) if assessment.tie.len() > 1 => Recommendation::Measured { rung: lowest },
        _ => refine(assessment, &scores, shown, best),
    };
    Recommended { choice, extend }
}

fn refine(
    assessment: &LadderAssessment,
    scores: &[Option<f64>],
    shown: &[f64],
    best: usize,
) -> Recommendation {
    let measured = Recommendation::Measured { rung: best };
    if best == 0 || best + 1 >= shown.len() {
        return measured;
    }
    let speed = |i: usize| assessment.verdicts[i].deciding_speed;
    match (speed(best - 1), speed(best), speed(best + 1)) {
        (Some(a), Some(b), Some(c)) if a == b && b == c => {}
        _ => return measured,
    }
    let (Some(s0), Some(s1), Some(s2)) = (scores[best - 1], scores[best], scores[best + 1]) else {
        return measured;
    };

    let x1 = shown[best].ln();
    let (d0, d2) = (shown[best - 1].ln() - x1, shown[best + 1].ln() - x1);
    let a = ((s2 - s1) / d2 - (s0 - s1) / d0) / (d2 - d0);
    let b = (s2 - s1) / d2 - a * d2;
    if a.is_nan() || a <= 0.0 {
        return measured;
    }

    let vertex = (-b / (2.0 * a)).clamp(d0, d2);
    let gain = (x1 + vertex).exp();
    if (gain - shown[best]).abs() <= SAME_GAIN * shown[best] {
        return measured;
    }
    let between = if gain < shown[best] {
        (best - 1, best)
    } else {
        (best, best + 1)
    };
    Recommendation::Interpolated {
        shown: gain,
        between,
    }
}
