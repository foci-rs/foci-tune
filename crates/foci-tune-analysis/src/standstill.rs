//! Torque and position dither measured while the commanded position is constant.

use crate::signals::{ChannelSeries, sample_period_s};
use crate::units::Regime;

/// Smallest torque change in counts between adjacent samples that counts as a large step.
pub const LARGE_STEP: i32 = 2;

/// Dither metrics for one standstill span.
#[derive(Debug, Clone, PartialEq)]
pub struct RestMetrics {
    /// Position of this rest among the qualifying rests, counting from 0.
    pub index: usize,
    /// Samples remaining after the settle prefix is dropped.
    pub samples: usize,
    /// Population standard deviation of torque over the retained samples.
    pub torque_sd: f64,
    /// Adjacent-sample torque changes of any size per second over the retained span.
    pub transitions_per_s: f64,
    /// Adjacent-sample torque changes of [`LARGE_STEP`] counts or more per second.
    pub large_transitions_per_s: f64,
    /// Peak-to-peak position error in micrometres over the retained samples.
    pub position_pp_um: f64,
}

/// Standstill metrics for a whole capture.
#[derive(Debug, Clone, PartialEq)]
pub struct StandstillMetrics {
    /// Qualifying rests in capture order.
    pub rests: Vec<RestMetrics>,
    /// Largest `torque_sd` among the rests, 0 when there are none.
    pub worst_torque_sd: f64,
    /// Largest `large_transitions_per_s` among the rests, 0 when there are none.
    pub worst_large_transitions_per_s: f64,
}

/// Finds rests and measures their dither.
///
/// A rest is a maximal run of constant `step_position` at least `settle + min_len` samples long;
/// its first `settle` samples are dropped before measuring. `regime` is unused because
/// `position_error_um` is already in micrometres.
pub fn standstill_metrics(
    s: &ChannelSeries,
    _regime: &Regime,
    settle: usize,
    min_len: usize,
) -> StandstillMetrics {
    let n = s
        .step_position
        .len()
        .min(s.torque.len())
        .min(s.position_error_um.len());
    let mut rests = Vec::new();
    let mut start = 0;
    for i in 1..=n {
        if i < n && s.step_position[i] == s.step_position[start] {
            continue;
        }
        if i - start >= settle + min_len {
            rests.push(measure(s, rests.len(), start + settle, i));
        }
        start = i;
    }
    let worst_torque_sd = rests.iter().map(|r| r.torque_sd).fold(0.0, f64::max);
    let worst_large_transitions_per_s = rests
        .iter()
        .map(|r| r.large_transitions_per_s)
        .fold(0.0, f64::max);
    StandstillMetrics {
        rests,
        worst_torque_sd,
        worst_large_transitions_per_s,
    }
}

fn measure(s: &ChannelSeries, index: usize, from: usize, to: usize) -> RestMetrics {
    let torque = &s.torque[from..to];
    let count = torque.len() as f64;
    let mean = torque.iter().map(|&t| f64::from(t)).sum::<f64>() / count;
    let var = torque
        .iter()
        .map(|&t| (f64::from(t) - mean).powi(2))
        .sum::<f64>()
        / count;
    let steps = || (from + 1..to).map(|k| (s.torque[k] - s.torque[k - 1]).abs());
    let transitions = steps().filter(|&d| d != 0).count();
    let large = steps().filter(|&d| d >= LARGE_STEP).count();
    let span = (to - from - 1) as f64 * sample_period_s(&s.t_s);
    let err = &s.position_error_um[from..to];
    let hi = err.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let lo = err.iter().copied().fold(f64::INFINITY, f64::min);
    RestMetrics {
        index,
        samples: to - from,
        torque_sd: var.sqrt(),
        transitions_per_s: transitions as f64 / span,
        large_transitions_per_s: large as f64 / span,
        position_pp_um: hi - lo,
    }
}
