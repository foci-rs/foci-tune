//! Welch power spectral density peak search.

use core::f64::consts::TAU;

use realfft::RealFftPlanner;

const SEGMENT: usize = 256;
const HOP: usize = SEGMENT / 2;

/// Subtracts a centred `window`-sample moving mean, treating samples outside the
/// series as zero.
pub(crate) fn moving_mean_detrend(x: &[f64], window: usize) -> Vec<f64> {
    let half = window / 2;
    (0..x.len())
        .map(|k| {
            let lo = k.saturating_sub(half);
            let hi = (k + window - half).min(x.len());
            x[k] - x[lo..hi].iter().sum::<f64>() / window as f64
        })
        .collect()
}

/// Frequency in Hz of the largest Welch PSD bin strictly inside `band`.
///
/// Uses 256-sample periodic Hann segments with 50% overlap, each mean-removed.
/// Returns NaN when `x` is shorter than one segment or no finite bin lies in `band`.
pub fn welch_peak_hz(x: &[f64], fs_hz: f64, band: (f64, f64)) -> f64 {
    if x.len() < SEGMENT {
        return f64::NAN;
    }
    let window: Vec<f64> = (0..SEGMENT)
        .map(|k| 0.5 - 0.5 * (TAU * k as f64 / SEGMENT as f64).cos())
        .collect();
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(SEGMENT);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let mut power = vec![0.0; spectrum.len()];
    for start in (0..=x.len() - SEGMENT).step_by(HOP) {
        let seg = &x[start..start + SEGMENT];
        let mean = seg.iter().sum::<f64>() / SEGMENT as f64;
        for ((slot, v), w) in input.iter_mut().zip(seg).zip(&window) {
            *slot = (v - mean) * w;
        }
        if fft.process(&mut input, &mut spectrum).is_err() {
            return f64::NAN;
        }
        for (p, c) in power.iter_mut().zip(&spectrum) {
            *p += c.norm_sqr();
        }
    }
    let bin_hz = fs_hz / SEGMENT as f64;
    power
        .iter()
        .enumerate()
        .map(|(k, &p)| (k as f64 * bin_hz, p))
        .filter(|&(f, p)| f > band.0 && f < band.1 && p.is_finite())
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(f64::NAN, |(f, _)| f)
}
