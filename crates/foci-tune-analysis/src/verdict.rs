//! Per-rung gate verdicts and the worst-speed score.

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
    /// Worst-speed ratio; `Some` only for score-eligible rungs. Lower is better.
    pub score: Option<f64>,
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
/// ratio, over speeds, of its median p99 to the lowest median p99 of that speed among
/// score-eligible rungs. The standstill baseline and gate apply only when the gates carry a
/// `standstill_factor` and the waivers leave the gate on.
pub fn assess_ladder_with(
    rungs: &[Vec<RepeatResult>],
    repeats: usize,
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
    let floors: Vec<f64> = (0..n_speeds)
        .map(|s| {
            eligible
                .iter()
                .filter_map(|&i| medians[i].p99_by_speed[s])
                .fold(f64::INFINITY, f64::min)
        })
        .collect();
    for &i in &eligible {
        let worst = medians[i]
            .p99_by_speed
            .iter()
            .zip(&floors)
            .filter_map(|(p, f)| p.map(|p| p / f))
            .fold(f64::NEG_INFINITY, f64::max);
        verdicts[i].score = Some(worst);
    }

    LadderAssessment {
        verdicts,
        medians,
        halt,
        pause,
    }
}
