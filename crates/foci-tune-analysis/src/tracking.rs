//! Tracking-error metrics for a plain move, following the reference script.

use crate::signals::ChannelSeries;
use crate::units::Regime;

const MOVING_RPM: f64 = 10.0;
const CRUISE_BAND: f64 = 0.05;
const REVERSAL_HALF_WINDOW: usize = 30;

/// Tracking-error summary of one channel over a move.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackingMetrics {
    /// Mean `|error|` over cruise samples, in micrometres.
    pub cruise_mean_abs_um: f64,
    /// Population standard deviation of the error over moving samples, in micrometres.
    pub sd_um: f64,
    /// 99th percentile of `|error|` over moving samples, in micrometres.
    pub p99_um: f64,
    /// Mean over reversals of the peak `|error|` near each reversal; NaN with no reversals.
    pub reversal_peak_um: f64,
    /// Number of velocity sign reversals.
    pub reversal_count: usize,
    /// Number of moving samples.
    pub moving_samples: usize,
}

fn mean(xs: &[f64]) -> f64 {
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// Linear-interpolated percentile (`q` in 0..=100) of an unsorted slice; NaN when empty.
fn percentile(xs: &[f64], q: f64) -> f64 {
    if xs.is_empty() {
        return f64::NAN;
    }
    let mut sorted = xs.to_vec();
    sorted.sort_by(f64::total_cmp);
    let pos = q / 100.0 * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

fn sign(x: f64) -> i8 {
    if x > 0.0 {
        1
    } else if x < 0.0 {
        -1
    } else {
        0
    }
}

/// Computes tracking metrics for one channel.
///
/// Moving samples exceed 10 rpm; cruise samples are moving samples within 5% of the 75th
/// percentile of moving speed. With no moving samples the statistics are NaN.
pub fn tracking_metrics(s: &ChannelSeries, regime: &Regime) -> TrackingMetrics {
    let rpm_scale = 60.0 / regime.motor_rotation_distance_mm;
    let moving: Vec<usize> = (0..s.velocity_mm_s.len())
        .filter(|&k| (s.velocity_mm_s[k] * rpm_scale).abs() > MOVING_RPM)
        .collect();
    let speeds: Vec<f64> = moving.iter().map(|&k| s.velocity_mm_s[k].abs()).collect();
    let errors: Vec<f64> = moving.iter().map(|&k| s.position_error_um[k]).collect();
    let abs_errors: Vec<f64> = errors.iter().map(|e| e.abs()).collect();

    let v75 = percentile(&speeds, 75.0);
    let cruise: Vec<f64> = moving
        .iter()
        .filter(|&&k| (s.velocity_mm_s[k].abs() - v75).abs() < CRUISE_BAND * v75)
        .map(|&k| s.position_error_um[k].abs())
        .collect();

    let mu = mean(&errors);
    let sd_um = (errors.iter().map(|e| (e - mu).powi(2)).sum::<f64>() / errors.len() as f64).sqrt();

    let n = s.velocity_mm_s.len();
    let peaks: Vec<f64> = (1..n)
        .filter(|&k| {
            let (a, b) = (sign(s.velocity_mm_s[k - 1]), sign(s.velocity_mm_s[k]));
            a != 0 && b != 0 && a != b
        })
        .map(|k| {
            let i = k - 1;
            let lo = i.saturating_sub(REVERSAL_HALF_WINDOW);
            let hi = (i + REVERSAL_HALF_WINDOW).min(n);
            s.position_error_um[lo..hi]
                .iter()
                .fold(0.0_f64, |m, e| m.max(e.abs()))
        })
        .collect();

    TrackingMetrics {
        cruise_mean_abs_um: mean(&cruise),
        sd_um,
        p99_um: percentile(&abs_errors, 99.0),
        reversal_peak_um: mean(&peaks),
        reversal_count: peaks.len(),
        moving_samples: moving.len(),
    }
}

/// Splits a series into speed blocks `[start, end)` in time order, using the commanded
/// position: a block is a maximal span of commanded motion between holds of at least
/// `min_gap_s`, ending on the sample that reaches the hold. Leading and trailing holds of any
/// length are not part of a block.
///
/// The measured position cannot find the holds: at rest it jitters by a few encoder counts,
/// each worth about 10 mm/s of finite-difference velocity at 1 kHz.
pub fn split_speed_segments(s: &ChannelSeries, min_gap_s: f64) -> Vec<(usize, usize)> {
    let p = &s.step_position;
    let n = p.len().min(s.t_s.len());
    let mut spans = Vec::new();
    let mut open: Option<usize> = None;
    let mut k = 0;
    while k < n {
        let last = (k..n).take_while(|&j| p[j] == p[k]).last().unwrap_or(k);
        let is_gap = k == 0 || last + 1 == n || s.t_s[last + 1] - s.t_s[k] >= min_gap_s;
        match (is_gap, open) {
            (true, Some(start)) => {
                spans.push((start, k + 1));
                open = None;
            }
            (false, None) => open = Some(k),
            _ => {}
        }
        k = last + 1;
    }
    spans
}
