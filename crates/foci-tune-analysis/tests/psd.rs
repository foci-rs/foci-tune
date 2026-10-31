use core::f64::consts::TAU;

use foci_tune_analysis::psd::welch_peak_hz;

struct Gaussian(u64);

impl Gaussian {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn sample(&mut self) -> f64 {
        let (u1, u2) = (self.uniform(), self.uniform());
        (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
    }
}

#[test]
fn welch_peak_finds_a_200_hz_tone_in_noise() {
    let fs = 1000.0;
    for seed in [1u64, 0xdead_beef, 0x9e37_79b9_7f4a_7c15] {
        let mut rng = Gaussian(seed);
        let x: Vec<f64> = (0..20_000)
            .map(|k| (TAU * 200.0 * k as f64 / fs).sin() + 0.5 * rng.sample())
            .collect();

        let peak = welch_peak_hz(&x, fs, (60.0, 490.0));

        assert!((peak - 200.0).abs() <= 4.0, "seed {seed}: {peak}");
    }
}

#[test]
fn welch_peak_of_short_or_nan_input_is_nan() {
    assert!(welch_peak_hz(&[], 1000.0, (60.0, 490.0)).is_nan());
    assert!(welch_peak_hz(&[1.0; 100], 1000.0, (60.0, 490.0)).is_nan());
    assert!(welch_peak_hz(&[f64::NAN; 1000], 1000.0, (60.0, 490.0)).is_nan());
}
