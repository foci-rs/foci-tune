//! Feedforward-step probe: edge location, ensemble response and per-signal fits.

use crate::fit::{DampedFit, FitError, fit_damped};
use crate::ladder::median;
use crate::psd::{Psd, moving_mean_detrend, welch_peak_hz, welch_psd};
use crate::signals::ChannelSeries;

/// Samples before an edge used for the flatness check and the baseline.
pub const PRE: usize = 15;
/// Samples from an edge onward kept as the response.
pub const POST: usize = 60;
/// Half-open sample range of the response passed to the damping fit.
pub const FIT_RANGE: (usize, usize) = (2, 55);

const PSD_FS_HZ: f64 = 1000.0;
pub(crate) const PSD_BAND_HZ: (f64, f64) = (60.0, 490.0);
const DETREND_WINDOW: usize = 25;
const FALLBACK_DT_S: f64 = 1e-3;

fn span(xs: &[i32]) -> i64 {
    let max = xs.iter().copied().max().unwrap_or(0);
    let min = xs.iter().copied().min().unwrap_or(0);
    i64::from(max) - i64::from(min)
}

/// Indices of clean feedforward steps: a 5-50% same-sign jump with the `PRE` samples
/// before and the `POST` samples from the edge each flat within 2% of their level.
pub fn locate_edges(ff_offset: &[i32]) -> Vec<usize> {
    let Some(last) = ff_offset.len().checked_sub(POST + 2) else {
        return Vec::new();
    };
    (PRE + 2..last)
        .filter(|&i| {
            let (a, b) = (ff_offset[i - 1], ff_offset[i]);
            if a == 0 || b == 0 || (a > 0) != (b > 0) {
                return false;
            }
            let (a, b) = (f64::from(a), f64::from(b));
            let jump = ((b - a) / a).abs();
            0.05 < jump
                && jump < 0.5
                && span(&ff_offset[i - PRE..i]) as f64 <= 0.02 * a.abs()
                && span(&ff_offset[i..i + POST]) as f64 <= 0.02 * b.abs()
        })
        .collect()
}

/// Ensemble-mean step response of one signal.
#[derive(Debug, Clone, PartialEq)]
pub struct Ensemble {
    /// Mean normalised response, `POST` samples long; empty with no usable edges.
    pub mean: Vec<f64>,
    /// Standard error of the mean at its largest sample; NaN with fewer than two edges.
    pub stderr_at_peak: f64,
    /// Number of edges averaged.
    pub edges: usize,
}

/// Baselined, step-normalised response of `signal` at each usable edge, in edge order.
///
/// Each response is `(x - mean(x[i-PRE..i])) / (ff[i] - ff[i-1])` over `x[i..i+POST]`;
/// edges whose window does not fit inside both slices are skipped.
pub fn edge_responses(signal: &[f64], ff_offset: &[i32], edges: &[usize]) -> Vec<Vec<f64>> {
    let len = signal.len().min(ff_offset.len());
    edges
        .iter()
        .filter(|&&i| i >= PRE && i + POST <= len)
        .map(|&i| {
            let step = f64::from(ff_offset[i]) - f64::from(ff_offset[i - 1]);
            let base = signal[i - PRE..i].iter().sum::<f64>() / PRE as f64;
            signal[i..i + POST]
                .iter()
                .map(|x| (x - base) / step)
                .collect()
        })
        .collect()
}

/// Averages the [`edge_responses`] of `signal` over `edges`.
pub fn ensemble(signal: &[f64], ff_offset: &[i32], edges: &[usize]) -> Ensemble {
    let responses = edge_responses(signal, ff_offset, edges);
    let n = responses.len();
    if n == 0 {
        return Ensemble {
            mean: Vec::new(),
            stderr_at_peak: f64::NAN,
            edges: 0,
        };
    }
    let mean: Vec<f64> = (0..POST)
        .map(|k| responses.iter().map(|r| r[k]).sum::<f64>() / n as f64)
        .collect();
    let peak = mean
        .iter()
        .enumerate()
        .filter(|(_, v)| !v.is_nan())
        .max_by(|(_, x), (_, y)| x.total_cmp(y))
        .map(|(k, _)| k);
    let stderr_at_peak = match peak {
        Some(k) if n > 1 => {
            let var = responses
                .iter()
                .map(|r| (r[k] - mean[k]).powi(2))
                .sum::<f64>()
                / (n - 1) as f64;
            (var / n as f64).sqrt()
        }
        _ => f64::NAN,
    };
    Ensemble {
        mean,
        stderr_at_peak,
        edges: n,
    }
}

/// Fewest admissible per-edge fits that make a damping assessment.
pub const MIN_FITTED_EDGES: usize = 12;
/// Largest scaled MAD of the per-edge damped frequency, in Hz, that still counts as one ring.
pub const MAX_HZ_SPREAD: f64 = 3.0;

const MAD_TO_SIGMA: f64 = 1.4826;

/// Damping assessed from independent per-edge fits.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeDamping {
    /// Median damping ratio over the fitted edges.
    pub zeta: f64,
    /// Median damped frequency over the fitted edges, in Hz.
    pub damped_hz: f64,
    /// Scaled median absolute deviation of the per-edge damped frequency, in Hz.
    pub hz_spread: f64,
    /// Edges whose fit was admissible.
    pub fitted: usize,
    /// Edge responses offered to the fit.
    pub edges: usize,
}

/// Reasons a per-edge damping assessment is withheld.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EdgeDampingError {
    /// Too few edges produced an admissible fit.
    #[error("{fitted} of {edges} edges fitted, fewer than {MIN_FITTED_EDGES}")]
    TooFewFits {
        /// Edges whose fit was admissible.
        fitted: usize,
        /// Edge responses offered to the fit.
        edges: usize,
    },
    /// The edges do not share one ring frequency, so no single damping ratio applies.
    #[error("ring frequency spread {hz_spread:.1} Hz exceeds {MAX_HZ_SPREAD} Hz")]
    Incoherent {
        /// Scaled MAD of the per-edge damped frequency, in Hz.
        hz_spread: f64,
    },
}

/// Fits each edge response on its own and reports the median damping ratio.
///
/// Averaging edges whose ring frequency differs decoheres the mean and inflates the fitted
/// damping, so the fit is done per edge and the edges must agree on frequency.
///
/// # Errors
///
/// [`EdgeDampingError::TooFewFits`] with fewer than [`MIN_FITTED_EDGES`] admissible fits;
/// [`EdgeDampingError::Incoherent`] when the scaled MAD of the damped frequencies exceeds
/// [`MAX_HZ_SPREAD`].
pub fn edge_damping(responses: &[Vec<f64>], dt_s: f64) -> Result<EdgeDamping, EdgeDampingError> {
    let fits: Vec<DampedFit> = responses
        .iter()
        .filter_map(|r| fit_damped(r, dt_s).ok())
        .collect();
    let (fitted, edges) = (fits.len(), responses.len());
    if fitted < MIN_FITTED_EDGES {
        return Err(EdgeDampingError::TooFewFits { fitted, edges });
    }
    let hz: Vec<f64> = fits.iter().map(|f| f.damped_hz).collect();
    let zeta: Vec<f64> = fits.iter().map(|f| f.zeta).collect();
    let damped_hz = median(&hz);
    let deviations: Vec<f64> = hz.iter().map(|h| (h - damped_hz).abs()).collect();
    let hz_spread = MAD_TO_SIGMA * median(&deviations);
    if hz_spread > MAX_HZ_SPREAD {
        return Err(EdgeDampingError::Incoherent { hz_spread });
    }
    Ok(EdgeDamping {
        zeta: median(&zeta),
        damped_hz,
        hz_spread,
        fitted,
        edges,
    })
}

/// Probe results for one channel.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeMetrics {
    /// Per-edge damping of the velocity response: the gated headline. [`Self::velocity`] is
    /// the ensemble fit kept for display.
    pub damping: Result<EdgeDamping, EdgeDampingError>,
    /// Damping fit of the velocity ensemble response.
    pub velocity: Result<DampedFit, FitError>,
    /// Damping fit of the target-torque response.
    pub torque: Result<DampedFit, FitError>,
    /// Damping fit of the position-error response.
    pub position_error: Result<DampedFit, FitError>,
    /// Number of feedforward edges found.
    pub edges: usize,
    /// Standard error at the peak of the velocity ensemble.
    pub stderr_at_peak: f64,
    /// Peak of the detrended torque Welch PSD within 60-490 Hz; NaN when too short.
    pub torque_psd_peak_hz: f64,
}

fn sample_period_s(t_s: &[f64]) -> f64 {
    let mut dt: Vec<f64> = t_s.windows(2).map(|w| w[1] - w[0]).collect();
    dt.sort_by(f64::total_cmp);
    dt.get(dt.len() / 2)
        .copied()
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(FALLBACK_DT_S)
}

/// The three ensemble responses of one channel and their sample period.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeEnsembles {
    /// Median sample period in seconds.
    pub dt_s: f64,
    /// Velocity response.
    pub velocity: Ensemble,
    /// Target-torque response.
    pub torque: Ensemble,
    /// Position-error response.
    pub position_error: Ensemble,
}

/// Locates feedforward edges in `s` and averages the velocity, torque and position-error
/// responses over them.
pub fn probe_ensembles(s: &ChannelSeries) -> ProbeEnsembles {
    let edges = locate_edges(&s.ff_offset);
    let torque: Vec<f64> = s.torque.iter().map(|&v| f64::from(v)).collect();
    ProbeEnsembles {
        dt_s: sample_period_s(&s.t_s),
        velocity: ensemble(&s.velocity_mm_s, &s.ff_offset, &edges),
        torque: ensemble(&torque, &s.ff_offset, &edges),
        position_error: ensemble(&s.position_error_um, &s.ff_offset, &edges),
    }
}

fn detrended_torque(s: &ChannelSeries) -> Vec<f64> {
    let torque: Vec<f64> = s.torque.iter().map(|&v| f64::from(v)).collect();
    moving_mean_detrend(&torque, DETREND_WINDOW)
}

/// Welch spectrum of the detrended torque, the spectrum whose in-band peak
/// [`ProbeMetrics::torque_psd_peak_hz`] reports; `None` when too short.
pub fn torque_psd(s: &ChannelSeries) -> Option<Psd> {
    welch_psd(&detrended_torque(s), PSD_FS_HZ)
}

/// Locates feedforward edges in `s` and fits the velocity, torque and position-error
/// ensembles, plus the torque PSD peak.
pub fn probe_metrics(s: &ChannelSeries) -> ProbeMetrics {
    let e = probe_ensembles(s);
    let edges = locate_edges(&s.ff_offset);
    let responses = edge_responses(&s.velocity_mm_s, &s.ff_offset, &edges);
    ProbeMetrics {
        damping: edge_damping(&responses, e.dt_s),
        velocity: fit_damped(&e.velocity.mean, e.dt_s),
        torque: fit_damped(&e.torque.mean, e.dt_s),
        position_error: fit_damped(&e.position_error.mean, e.dt_s),
        edges: e.velocity.edges,
        stderr_at_peak: e.velocity.stderr_at_peak,
        torque_psd_peak_hz: welch_peak_hz(&detrended_torque(s), PSD_FS_HZ, PSD_BAND_HZ),
    }
}
