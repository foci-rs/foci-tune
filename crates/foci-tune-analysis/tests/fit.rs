use core::f64::consts::TAU;

use foci_tune_analysis::fit::{FitError, fit_damped};

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

#[test]
fn fitted_curve_is_the_curve_the_residual_is_reported_for() {
    let mean = damped(150.0, 0.06, 0.3, 0.2, 0.05, 0.02);

    let fit = fit_damped(&mean, DT).expect("clean damped cosine fits");

    let squares: Vec<f64> = (2..55)
        .map(|k| (fit.value_at(k as f64 * DT) - mean[k]).powi(2))
        .collect();
    let rms = (squares.iter().sum::<f64>() / squares.len() as f64).sqrt();
    assert!(
        (rms - fit.residual_rms).abs() < 1e-6 * fit.residual_rms.max(1e-9),
        "{rms} vs {fit:?}"
    );
}

#[test]
fn trap_shape_fits_from_the_start_set() {
    let mean = damped(145.0, 0.058, -0.37, 0.9, 0.19, 0.0056);
    let expected_hz = 145.0 * (1.0f64 - 0.058 * 0.058).sqrt();

    let fit = fit_damped(&mean, DT).expect("ringing response fits");

    assert!(
        (fit.damped_hz - expected_hz).abs() < 0.02 * expected_hz,
        "{}",
        fit.damped_hz
    );
    assert!((fit.zeta - 0.058).abs() < 0.01, "{}", fit.zeta);
    assert!(fit.admissible_starts >= 1);
}

#[test]
fn shape_grid_fits_at_every_point() {
    for sign in [1.0, -1.0] {
        for zeta in [0.05, 0.075, 0.10] {
            for f in [120.0, 140.0, 160.0] {
                for phase in [0.8, 1.0, 1.2] {
                    for b in [0.15, 0.30] {
                        for tau in [0.004, 0.007] {
                            let mean = damped(f, zeta, sign * 0.37, phase, b, tau);
                            let fit = fit_damped(&mean, DT).unwrap_or_else(|e| {
                                panic!("{sign} {zeta} {f} {phase} {b} {tau}: {e}")
                            });
                            let expected_hz = f * (1.0f64 - zeta * zeta).sqrt();
                            assert!((fit.damped_hz - expected_hz).abs() < 0.02 * expected_hz);
                            assert!(
                                (fit.zeta - zeta).abs() < 0.01,
                                "{sign} {zeta} {f} {phase} {b} {tau}: {fit:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn non_ringing_shapes_report_failure() {
    let t = |k: usize| k as f64 * DT;
    let pure_exp = |tau: f64| {
        (0..60)
            .map(|k| 0.1 * (-t(k) / tau).exp())
            .collect::<Vec<_>>()
    };
    let const_exp = |tau: f64| {
        (0..60)
            .map(|k| 0.3 + 0.2 * (-t(k) / tau).exp())
            .collect::<Vec<_>>()
    };
    let critical = |tau: f64| {
        (0..60)
            .map(|k| 0.5 * (1.0 - (1.0 + t(k) / tau) * (-t(k) / tau).exp()))
            .collect::<Vec<_>>()
    };
    let overdamped = (0..60)
        .map(|k| {
            let (w, z) = (TAU * 150.0, 1.5f64);
            let (s1, s2) = (
                -w * (z - (z * z - 1.0).sqrt()),
                -w * (z + (z * z - 1.0).sqrt()),
            );
            0.5 * (1.0 - (s2 * (s1 * t(k)).exp() - s1 * (s2 * t(k)).exp()) / (s2 - s1))
        })
        .collect::<Vec<_>>();
    let cases: Vec<(&str, Vec<f64>)> = [0.005, 0.01, 0.02, 0.05]
        .iter()
        .map(|&tau| ("pure", pure_exp(tau)))
        .chain(
            [0.003, 0.008, 0.02, 0.05]
                .iter()
                .map(|&tau| ("const", const_exp(tau))),
        )
        .chain(
            [0.003, 0.008, 0.02]
                .iter()
                .map(|&tau| ("critical", critical(tau))),
        )
        .chain([("overdamped", overdamped)])
        .collect();
    for (name, mean) in cases {
        let err = fit_damped(&mean, DT).unwrap_err();
        match name {
            "pure" => assert!(matches!(err, FitError::SingularJacobian), "{name}: {err:?}"),
            // Which rule a degenerate candidate breaks is numerically arbitrary.
            _ => assert!(
                matches!(
                    err,
                    FitError::ZetaOutOfRange(_)
                        | FitError::FrequencyOffBand(_)
                        | FitError::AmplitudeExcessive(_)
                        | FitError::NoConvergence
                ),
                "{name}: {err:?}"
            ),
        }
    }
}

#[test]
fn fit_is_deterministic() {
    let mean = damped(145.0, 0.058, -0.37, 0.9, 0.19, 0.0056);
    assert_eq!(
        fit_damped(&mean, DT).unwrap(),
        fit_damped(&mean, DT).unwrap()
    );
}
