//! Per-rung gate verdicts and the absolute score.

use serde::{Deserialize, Serialize};

use crate::ladder::{RungOutcome, median};

/// Measurements of one repeat of one rung.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepeatResult {
    /// Velocity damping of the run.
    pub zeta: RungOutcome,
    /// Tracking p99 per chosen speed; `None` for a missing or not-comparable result.
    pub p99_by_speed: Vec<Option<f64>>,
    /// Worst per-rest torque sd of the standstill run; `None` when unusable.
    pub worst_torque_sd: Option<f64>,
}

/// Thresholds of the rung gates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Gates {
    /// Median zeta below this makes a rung inadmissible.
    pub damping_floor: f64,
    /// Median zeta below this (and at or above the floor) makes a rung marginal.
    pub marginal: f64,
    /// Standstill median above this multiple of the lowest rung's median is inadmissible; `None`
    /// leaves the standstill gate off.
    #[serde(default)]
    pub standstill_factor: Option<f64>,
}

impl Default for Gates {
    fn default() -> Self {
        Self {
            damping_floor: 0.05,
            marginal: 0.08,
            standstill_factor: None,
        }
    }
}

/// Why a rung is excluded from scoring for lack of data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnscoredReason {
    /// A requested repeat of a chosen speed has no comparable tracking result.
    TrackingMissing,
    /// A requested standstill repeat produced no valid metric.
    StandstillMissing,
    /// The operator continued past a rung whose damping could not be assessed.
    DampingWaived,
}

/// Why a rung failed a gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InadmissibleReason {
    /// Median zeta is below the damping floor.
    DampingBelowFloor,
    /// Standstill median exceeds the allowed multiple of the baseline.
    StandstillHigh,
}

/// Verdict of one rung.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct RungVerdict {
    /// Gate failure, if any.
    pub inadmissible: Option<InadmissibleReason>,
    /// Median zeta is between the floor and the marginal threshold.
    pub marginal: bool,
    /// Missing data that keeps the rung from being scored.
    pub unscored: Option<UnscoredReason>,
    /// Largest median p99 over the speeds, in um; `Some` only for score-eligible rungs. Lower is
    /// better.
    pub score: Option<f64>,
    /// Index into the speed vector of the speed that sets `score`; `Some` iff `score` is.
    pub deciding_speed: Option<usize>,
    /// Score minus the best rung's score, in um; `Some` iff `score` is, and `Some(0.0)` for the
    /// best rung.
    pub gap: Option<f64>,
    /// Two-sigma tie tolerance of the comparison against the best rung, in um; `None` for the
    /// best rung and when the spreads are unknown.
    pub tolerance: Option<f64>,
}

/// Why assessment of the ladder stopped.
///
/// Only `BelowFloor` is emitted by [`assess_ladder_with`]; the two unassessable variants are kept
/// for records written before the pause existed and for callers of the old entry point.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Halt {
    /// Median zeta fell below the floor.
    BelowFloor {
        /// Median zeta of the rung.
        median: f64,
    },
    /// A repeat has no velocity zeta.
    DampingUnassessable,
    /// The lowest rung has no usable standstill result.
    StandstillBaselineUnassessable,
}

/// Why assessment paused for an operator decision rather than ending the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Pause {
    /// A repeat has no velocity zeta.
    DampingUnassessable,
    /// The lowest rung has no usable standstill result.
    StandstillBaselineUnassessable,
}

/// Operator decisions that let assessment continue past a pause.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Waivers {
    /// Per rung index, whether the operator continued past unassessable damping. A missing index
    /// counts as `false`. A waiver covers damping only.
    pub continued: Vec<bool>,
    /// Skip the standstill baseline and the `StandstillHigh` gate.
    pub standstill_gate_off: bool,
}

/// Medians behind a rung's verdict, for display.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RungMedians {
    /// Median zeta, when every requested repeat has one.
    pub zeta: Option<f64>,
    /// Median p99 per speed, `None` where any requested repeat lacks it.
    pub p99_by_speed: Vec<Option<f64>>,
    /// Median worst torque sd, when every requested repeat has one.
    pub worst_torque_sd: Option<f64>,
}

/// Verdicts for every rung, in the order given.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LadderAssessment {
    /// One verdict per rung; rungs after a halt or pause, or without all repeats, are default.
    pub verdicts: Vec<RungVerdict>,
    /// One set of medians per rung.
    pub medians: Vec<RungMedians>,
    /// Index of the rung where assessment stopped and the reason.
    pub halt: Option<(usize, Halt)>,
    /// Index of the rung where assessment paused for an operator decision and the reason.
    #[serde(default)]
    pub pause: Option<(usize, Pause)>,
    /// Index of the score-eligible rung with the lowest score; ties go to the lower index.
    pub best: Option<usize>,
    /// Index of the score-eligible rung with the second-lowest score; ties go to the lower index.
    pub runner_up: Option<usize>,
    /// Ascending indices of the rungs statistically tied with the best, the best included; empty
    /// when no rung is score-eligible.
    pub tie: Vec<usize>,
    /// Relative spread (sample sd over median) of the p99 per speed, pooled by RMS over the
    /// score-eligible rungs; `None` per speed with fewer than two repeats or no eligible rung.
    pub spreads: Vec<Option<f64>>,
}

/// Two-sigma tolerance of the difference between two single measurements.
///
/// `s_b` and `s_r` are relative spreads and `score_b` and `score_r` the scores they apply to.
pub fn pair_tolerance(s_b: f64, score_b: f64, s_r: f64, score_r: f64) -> f64 {
    2.0 * (s_b * score_b).hypot(s_r * score_r)
}

fn relative_spreads(
    rungs: &[Vec<RepeatResult>],
    repeats: usize,
    eligible: &[usize],
    n_speeds: usize,
) -> Vec<Option<f64>> {
    if repeats < 2 || eligible.is_empty() {
        return vec![None; n_speeds];
    }
    (0..n_speeds)
        .map(|s| {
            let sum_sq: f64 = eligible
                .iter()
                .map(|&i| {
                    let values: Vec<f64> = rungs[i][..repeats]
                        .iter()
                        .filter_map(|r| r.p99_by_speed.get(s).copied().flatten())
                        .collect();
                    let mean = values.iter().sum::<f64>() / values.len() as f64;
                    let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>()
                        / (values.len() - 1) as f64;
                    (var.sqrt() / median(&values)).powi(2)
                })
                .sum();
            Some((sum_sq / eligible.len() as f64).sqrt())
        })
        .collect()
}

fn all_some(values: impl Iterator<Item = Option<f64>>) -> Option<Vec<f64>> {
    values.collect()
}

fn rung_medians(repeats: &[RepeatResult], n_speeds: usize) -> RungMedians {
    let zeta = all_some(repeats.iter().map(|r| match r.zeta {
        RungOutcome::Assessed(z) if z.is_finite() => Some(z),
        RungOutcome::Assessed(_) | RungOutcome::Unassessable => None,
    }))
    .map(|v| median(&v));
    let p99_by_speed = (0..n_speeds)
        .map(|s| {
            all_some(repeats.iter().map(|r| {
                r.p99_by_speed
                    .get(s)
                    .copied()
                    .flatten()
                    .filter(|p| p.is_finite() && *p > 0.0)
            }))
            .map(|v| median(&v))
        })
        .collect();
    let worst_torque_sd = all_some(
        repeats
            .iter()
            .map(|r| r.worst_torque_sd.filter(|sd| sd.is_finite())),
    )
    .map(|v| median(&v));
    RungMedians {
        zeta,
        p99_by_speed,
        worst_torque_sd,
    }
}

/// Judges each rung on medians over exactly `repeats` results and scores the eligible ones.
///
/// Behaves as [`assess_ladder_with`] with default waivers, reporting a pause as the matching
/// [`Halt`] variant.
pub fn assess_ladder(
    rungs: &[Vec<RepeatResult>],
    repeats: usize,
    gates: &Gates,
    include_marginal: bool,
) -> LadderAssessment {
    let mut a = assess_ladder_with(rungs, repeats, gates, include_marginal, &Waivers::default());
    if let Some((i, pause)) = a.pause.take() {
        let halt = match pause {
            Pause::DampingUnassessable => Halt::DampingUnassessable,
            Pause::StandstillBaselineUnassessable => Halt::StandstillBaselineUnassessable,
        };
        a.halt = Some((i, halt));
    }
    a
}

/// Judges each rung on medians over exactly `repeats` results and scores the eligible ones.
///
/// `rungs` holds the completed repeats of each rung in ascending gain order. A rung with fewer
/// than `repeats` results has no verdict yet. Assessment halts at the first rung whose damping is
/// below the floor, and pauses at the first rung whose damping is unassessable (unless waived) or
/// whose standstill baseline (rung 0) is unusable; later rungs keep default verdicts. A waived
/// rung is unscored but still contributes its standstill result. A rung's score is the largest
/// median p99 over its speeds. The standstill baseline and gate apply only when the gates carry a
/// `standstill_factor` and the waivers leave the gate on.
pub fn assess_ladder_with(
    rungs: &[Vec<RepeatResult>],
    repeats: usize,
    gates: &Gates,
    include_marginal: bool,
    waivers: &Waivers,
) -> LadderAssessment {
    assess_ladder_at(rungs, repeats, &[], gates, include_marginal, waivers)
}

/// As [`assess_ladder_with`], with the speed of each p99 index in `speeds_mm_s`.
///
/// When several speeds share a rung's largest median p99, the rung's `deciding_speed` is the
/// fastest of them. An index with no entry in `speeds_mm_s` ranks by its position, so with an
/// empty slice the largest index wins.
pub fn assess_ladder_at(
    rungs: &[Vec<RepeatResult>],
    repeats: usize,
    speeds_mm_s: &[f64],
    gates: &Gates,
    include_marginal: bool,
    waivers: &Waivers,
) -> LadderAssessment {
    let n_speeds = rungs
        .iter()
        .flat_map(|r| r.iter().map(|x| x.p99_by_speed.len()))
        .max()
        .unwrap_or(0);
    let mut verdicts = vec![RungVerdict::default(); rungs.len()];
    let mut medians = vec![RungMedians::default(); rungs.len()];
    let mut halt = None;
    let mut pause = None;
    let mut baseline: Option<f64> = None;
    let standstill_gate = gates
        .standstill_factor
        .filter(|_| !waivers.standstill_gate_off);

    for (i, results) in rungs.iter().enumerate() {
        if repeats == 0 || results.len() < repeats {
            continue;
        }
        let m = rung_medians(&results[..repeats], n_speeds);
        medians[i] = m.clone();
        let waived = match m.zeta {
            Some(zeta) => {
                if zeta < gates.damping_floor {
                    verdicts[i].inadmissible = Some(InadmissibleReason::DampingBelowFloor);
                    halt = Some((i, Halt::BelowFloor { median: zeta }));
                    break;
                }
                verdicts[i].marginal = zeta < gates.marginal;
                false
            }
            None if waivers.continued.get(i).copied().unwrap_or(false) => {
                verdicts[i].unscored = Some(UnscoredReason::DampingWaived);
                true
            }
            None => {
                pause = Some((i, Pause::DampingUnassessable));
                break;
            }
        };

        if i == 0 && standstill_gate.is_some() {
            match m.worst_torque_sd {
                Some(sd) => baseline = Some(sd),
                None => {
                    if !waived {
                        verdicts[i].unscored = Some(UnscoredReason::StandstillMissing);
                    }
                    pause = Some((i, Pause::StandstillBaselineUnassessable));
                    break;
                }
            }
        }

        if !waived {
            let tracking_complete = n_speeds > 0 && m.p99_by_speed.iter().all(Option::is_some);
            if !tracking_complete {
                verdicts[i].unscored = Some(UnscoredReason::TrackingMissing);
            } else if m.worst_torque_sd.is_none() && standstill_gate.is_some() {
                verdicts[i].unscored = Some(UnscoredReason::StandstillMissing);
            }
        }
        if let (Some(sd), Some(base), Some(factor)) = (m.worst_torque_sd, baseline, standstill_gate)
            && sd > factor * base
        {
            verdicts[i].inadmissible = Some(InadmissibleReason::StandstillHigh);
        }
    }

    let eligible: Vec<usize> = (0..rungs.len())
        .filter(|&i| {
            let v = &verdicts[i];
            v.inadmissible.is_none()
                && v.unscored.is_none()
                && (!v.marginal || include_marginal)
                && medians[i].zeta.is_some()
        })
        .collect();
    for &i in &eligible {
        let largest = medians[i]
            .p99_by_speed
            .iter()
            .flatten()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let deciding = medians[i]
            .p99_by_speed
            .iter()
            .enumerate()
            .filter(|(_, p)| **p == Some(largest))
            .map(|(s, _)| s)
            .max_by(|&x, &y| match (speeds_mm_s.get(x), speeds_mm_s.get(y)) {
                (Some(sx), Some(sy)) => sx.total_cmp(sy),
                _ => x.cmp(&y),
            });
        verdicts[i].score = Some(largest);
        verdicts[i].deciding_speed = deciding;
    }

    let mut ranked: Vec<(f64, usize)> = eligible
        .iter()
        .filter_map(|&i| verdicts[i].score.map(|s| (s, i)))
        .collect();
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let best = ranked.first().map(|r| r.1);
    let runner_up = ranked.get(1).map(|r| r.1);

    let spreads = relative_spreads(rungs, repeats, &eligible, n_speeds);
    let mut tie = Vec::new();
    if let Some(b) = best {
        let score_b = ranked[0].0;
        let dec_b = verdicts[b].deciding_speed;
        for &(score_r, r) in &ranked {
            if r == b {
                verdicts[r].gap = Some(0.0);
                tie.push(r);
                continue;
            }
            let gap = score_r - score_b;
            let spread_at = |d: Option<usize>| d.and_then(|d| spreads.get(d).copied().flatten());
            let tolerance = spread_at(dec_b)
                .zip(spread_at(verdicts[r].deciding_speed))
                .map(|(sb, sr)| pair_tolerance(sb, score_b, sr, score_r));
            verdicts[r].gap = Some(gap);
            verdicts[r].tolerance = tolerance;
            if tolerance.is_some_and(|t| gap <= t) {
                tie.push(r);
            }
        }
        tie.sort_unstable();
    }

    LadderAssessment {
        verdicts,
        medians,
        halt,
        pause,
        best,
        runner_up,
        tie,
        spreads,
    }
}

/// Whether the seed rung's score moved between the first run and the rerun.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriftVerdict {
    /// The change is within twice the speed's relative spread.
    Steady,
    /// The change exceeds twice the speed's relative spread.
    Drifted,
    /// A score or the spread is missing, or the first score is not positive.
    Unassessed,
}

/// Compares the seed rung's first and rerun scores against twice the relative `spread`.
pub fn drift_verdict(first: Option<f64>, rerun: Option<f64>, spread: Option<f64>) -> DriftVerdict {
    match (first, rerun, spread) {
        (Some(first), Some(rerun), Some(spread)) if first > 0.0 => {
            if (rerun - first).abs() / first > 2.0 * spread {
                DriftVerdict::Drifted
            } else {
                DriftVerdict::Steady
            }
        }
        _ => DriftVerdict::Unassessed,
    }
}
