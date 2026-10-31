use core::f64::consts::TAU;

use foci_tune_analysis::fit::fit_damped;

const DT: f64 = 1e-3;

fn damped(f_hz: f64, zeta: f64, amplitude: f64, phase: f64, b: f64, tau: f64) -> Vec<f64> {
    let w = TAU * f_hz;
    (0..60)
        .map(|k| {
            let t = k as f64 * DT;
            amplitude * (-zeta * w * t).exp() * (w * (1.0 - zeta * zeta).sqrt() * t + phase).cos()
                + b * (-t / tau).exp()
        })
        .collect()
}

#[test]
fn fit_recovers_a_known_damped_cosine() {
    let mean = damped(150.0, 0.06, 0.3, 0.2, 0.05, 0.02);
    let expected_hz = 150.0 * (1.0f64 - 0.06 * 0.06).sqrt();

    let fit = fit_damped(&mean, DT).expect("clean damped cosine fits");

    assert!(
        (fit.damped_hz - expected_hz).abs() < 0.02 * expected_hz,
        "{}",
        fit.damped_hz
    );
    assert!((fit.zeta - 0.06).abs() < 0.01, "{}", fit.zeta);
}

#[test]
fn fit_handles_heavy_and_light_damping() {
    for zeta in [0.4, 0.01] {
        let mean = damped(150.0, zeta, 0.3, 0.2, 0.05, 0.02);

        let fit = fit_damped(&mean, DT).expect("clean damped cosine fits");

        assert!(
            (fit.zeta - zeta).abs() < 0.01,
            "zeta {zeta}: got {}",
            fit.zeta
        );
    }
}

#[test]
fn pure_exponential_reports_failure_not_a_zeta() {
    let mean: Vec<f64> = (0..60)
        .map(|k| 0.1 * (-(k as f64 * DT) / 0.01).exp())
        .collect();

    let result = fit_damped(&mean, DT);

    assert!(result.is_err(), "{result:?}");
}

#[test]
fn short_or_nan_input_fails_without_panicking() {
    assert!(fit_damped(&[], DT).is_err());
    assert!(fit_damped(&[0.1; 20], DT).is_err());
    assert!(fit_damped(&[f64::NAN; 60], DT).is_err());
}
