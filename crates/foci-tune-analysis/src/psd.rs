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

/// A Welch power spectrum: one power value per frequency bin, averaged over segments.
#[derive(Debug, Clone, PartialEq)]
pub struct Psd {
    /// Bin centre frequencies in Hz, from 0 to `fs / 2`.
    pub freq_hz: Vec<f64>,
    /// Mean power per bin, in squared signal units.
    pub power: Vec<f64>,
}

/// Welch spectrum of `x` from 256-sample periodic Hann segments with 50% overlap, each
/// mean-removed. `None` when `x` is shorter than one segment.
pub fn welch_psd(x: &[f64], fs_hz: f64) -> Option<Psd> {
    if x.len() < SEGMENT {
        return None;
    }
    let window: Vec<f64> = (0..SEGMENT)
        .map(|k| 0.5 - 0.5 * (TAU * k as f64 / SEGMENT as f64).cos())
        .collect();
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(SEGMENT);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let mut power = vec![0.0; spectrum.len()];
    let mut segments = 0usize;
    for start in (0..=x.len() - SEGMENT).step_by(HOP) {
        let seg = &x[start..start + SEGMENT];
        let mean = seg.iter().sum::<f64>() / SEGMENT as f64;
        for ((slot, v), w) in input.iter_mut().zip(seg).zip(&window) {
            *slot = (v - mean) * w;
        }
        fft.process(&mut input, &mut spectrum).ok()?;
        for (p, c) in power.iter_mut().zip(&spectrum) {
            *p += c.norm_sqr();
        }
        segments += 1;
    }
    let bin_hz = fs_hz / SEGMENT as f64;
    Some(Psd {
        freq_hz: (0..power.len()).map(|k| k as f64 * bin_hz).collect(),
        power: power.into_iter().map(|p| p / segments as f64).collect(),
    })
}

/// Frequency in Hz of the largest Welch PSD bin strictly inside `band`.
///
/// Uses [`welch_psd`]. Equal bins resolve to the lowest frequency, as numpy's `argmax` in the
/// reference script does. Returns NaN when `x` is shorter than one segment or no finite bin
/// lies in `band`.
pub fn welch_peak_hz(x: &[f64], fs_hz: f64, band: (f64, f64)) -> f64 {
    let Some(psd) = welch_psd(x, fs_hz) else {
        return f64::NAN;
    };
    psd.freq_hz
        .iter()
        .zip(&psd.power)
        .map(|(&f, &p)| (f, p))
        .filter(|&(f, p)| f > band.0 && f < band.1 && p.is_finite())
        .reduce(|best, bin| if bin.1 > best.1 { bin } else { best })
        .map_or(f64::NAN, |(f, _)| f)
}
