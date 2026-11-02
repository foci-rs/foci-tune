use core::f64::consts::TAU;

use foci_tune_analysis::fit::fit_damped;
use foci_tune_analysis::probe::{
    EdgeDampingError, MAX_HZ_SPREAD, POST, edge_damping, probe_metrics,
};
use foci_tune_analysis::signals::ChannelSeries;

const DT: f64 = 1e-3;

fn response(hz: f64, zeta: f64) -> Vec<f64> {
    let w = TAU * hz / (1.0 - zeta * zeta).sqrt();
    let wd = TAU * hz;
    (0..POST)
        .map(|k| {
            let t = k as f64 * DT;
            0.5 * (-zeta * w * t).exp() * (wd * t + 1.3).cos() + 0.25 * (-t / 0.0045).exp()
        })
        .collect()
}

fn ramp(from: f64, to: f64, n: usize) -> Vec<f64> {
    (0..n)
        .map(|k| from + (to - from) * k as f64 / (n - 1) as f64)
        .collect()
}

fn synthetic_probe_series_with_gaps(hz_per_edge: &[Option<f64>], zeta: f64) -> ChannelSeries {
    let ff_offset: Vec<i32> = (0..=hz_per_edge.len())
        .flat_map(|b| std::iter::repeat_n(if b % 2 == 0 { 1000 } else { 1300 }, 100))
        .collect();
    let n = ff_offset.len();
    let mut velocity: Vec<f64> = ff_offset.iter().map(|&v| 0.01 * f64::from(v)).collect();
    for (e, hz) in hz_per_edge.iter().enumerate() {
        let i = 100 * (e + 1);
        let step = f64::from(ff_offset[i] - ff_offset[i - 1]);
        let ring = hz.map(|hz| response(hz, zeta));
        for k in 0..POST.min(n - i) {
            velocity[i + k] += match &ring {
                Some(r) => step * r[k],
                None => step * 0.1 * (-(k as f64 * DT) / 0.01).exp(),
            };
        }
    }
    ChannelSeries {
        t_s: (0..n).map(|k| k as f64 * DT).collect(),
        velocity_mm_s: velocity,
        torque: vec![0; n],
        position_error_um: vec![0.0; n],
        ff_offset,
        ..ChannelSeries::default()
    }
}

fn synthetic_probe_series(hz_per_edge: &[f64], zeta: f64) -> ChannelSeries {
    let hz: Vec<Option<f64>> = hz_per_edge.iter().map(|&h| Some(h)).collect();
    synthetic_probe_series_with_gaps(&hz, zeta)
}

#[test]
fn coherent_edges_recover_the_edge_zeta() {
    let responses: Vec<Vec<f64>> = ramp(150.0, 154.0, 33)
        .iter()
        .map(|&hz| response(hz, 0.034))
        .collect();

    let d = edge_damping(&responses, DT).expect("33 coherent edges assess");

    assert!((d.zeta - 0.034).abs() < 0.003, "{d:?}");
    assert!((d.damped_hz - 152.0).abs() < 1.0, "{d:?}");
    assert!(d.hz_spread < MAX_HZ_SPREAD, "{d:?}");
    assert_eq!((d.fitted, d.edges), (33, 33));
}

#[test]
fn frequency_drift_across_edges_is_unassessable_where_the_ensemble_fit_is_merely_wrong() {
    let responses: Vec<Vec<f64>> = ramp(147.0, 173.0, 33)
        .iter()
        .map(|&hz| response(hz, 0.034))
        .collect();
    let mean: Vec<f64> = (0..POST)
        .map(|k| responses.iter().map(|r| r[k]).sum::<f64>() / 33.0)
        .collect();

    let ensemble = fit_damped(&mean, DT);
    let d = edge_damping(&responses, DT);

    // The decohered mean either over-reports damping or does not fit at all; both are
    // what the per-edge gate protects against.
    assert!(
        !ensemble.as_ref().is_ok_and(|f| f.zeta <= 0.05),
        "{ensemble:?}"
    );
    assert!(
        matches!(d, Err(EdgeDampingError::Incoherent { hz_spread }) if hz_spread > MAX_HZ_SPREAD),
        "{d:?}"
    );
}

#[test]
fn twelve_fitted_edges_is_the_minimum() {
    let twelve: Vec<Vec<f64>> = (0..12).map(|_| response(152.0, 0.034)).collect();
    assert!(edge_damping(&twelve, DT).is_ok());

    let eleven = &twelve[..11];
    assert_eq!(
        edge_damping(eleven, DT),
        Err(EdgeDampingError::TooFewFits {
            fitted: 11,
            edges: 11
        })
    );
}

#[test]
fn inadmissible_edge_fits_do_not_count() {
    let pure_exp: Vec<f64> = (0..POST)
        .map(|k| 0.1 * (-(k as f64 * DT) / 0.01).exp())
        .collect();
    let mut responses: Vec<Vec<f64>> = (0..12).map(|_| response(152.0, 0.034)).collect();
    responses.extend((0..5).map(|_| pure_exp.clone()));

    let d = edge_damping(&responses, DT).expect("12 admissible fits assess");
    assert_eq!((d.fitted, d.edges), (12, 17));
    assert!((d.zeta - 0.034).abs() < 0.003, "{d:?}");

    let mut short = responses.clone();
    short.remove(0);
    assert_eq!(
        edge_damping(&short, DT),
        Err(EdgeDampingError::TooFewFits {
            fitted: 11,
            edges: 16
        })
    );
}

#[test]
fn headline_is_the_median_not_the_mean() {
    let mut responses: Vec<Vec<f64>> = (0..12).map(|_| response(152.0, 0.034)).collect();
    responses.push(response(152.0, 0.3));

    let d = edge_damping(&responses, DT).unwrap();

    assert!((d.zeta - 0.034).abs() < 0.003, "{d:?}");
}

#[test]
fn one_outlier_frequency_does_not_trip_the_spread_limit() {
    let mut responses: Vec<Vec<f64>> = (0..20).map(|_| response(152.0, 0.034)).collect();
    responses.push(response(246.0, 0.034));

    let d = edge_damping(&responses, DT).unwrap();

    assert!(d.hz_spread < 1.0, "{d:?}");
    assert!((d.damped_hz - 152.0).abs() < 1.0, "{d:?}");
    assert_eq!(d.fitted, 21);
}

#[test]
fn no_edges_and_nan_edges_do_not_panic() {
    assert_eq!(
        edge_damping(&[], DT),
        Err(EdgeDampingError::TooFewFits {
            fitted: 0,
            edges: 0
        })
    );
    let nan = vec![vec![f64::NAN; POST]; 13];
    assert_eq!(
        edge_damping(&nan, DT),
        Err(EdgeDampingError::TooFewFits {
            fitted: 0,
            edges: 13
        })
    );
}

#[test]
fn probe_metrics_reports_damping_from_the_velocity_edges() {
    let s = synthetic_probe_series(&ramp(150.0, 154.0, 14), 0.034);

    let m = probe_metrics(&s);

    assert_eq!(m.edges, 14);
    let d = m.damping.expect("14 coherent edges assess");
    assert_eq!(d.fitted, 14);
    assert!((d.zeta - 0.034).abs() < 0.003, "{d:?}");
}

#[test]
fn probe_metrics_counts_only_admissible_edges_of_a_verifiable_run() {
    // `None` edges carry a pure exponential instead of a ring, so their fits are
    // inadmissible.
    let mut hz: Vec<Option<f64>> = vec![Some(152.0); 12];
    hz.extend(std::iter::repeat_n(None, 8));
    let m = probe_metrics(&synthetic_probe_series_with_gaps(&hz, 0.034));
    assert_eq!(m.edges, 20);
    let d = m.damping.expect("12 admissible fits among 20 edges assess");
    assert_eq!((d.fitted, d.edges), (12, 20));

    hz[11] = None;
    let m = probe_metrics(&synthetic_probe_series_with_gaps(&hz, 0.034));
    assert_eq!(m.edges, 20);
    assert_eq!(
        m.damping,
        Err(EdgeDampingError::TooFewFits {
            fitted: 11,
            edges: 20
        })
    );
}
