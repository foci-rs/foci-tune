use foci_tune_analysis::signals::ChannelSeries;
use foci_tune_analysis::tracking::{split_speed_segments, tracking_metrics};
use foci_tune_analysis::units::Regime;

fn regime() -> Regime {
    Regime {
        motor_rotation_distance_mm: 40.0,
        encoder_counts_per_rev: 4096,
        pwm_hz: 50_000,
        clock_freq: 260_000_000,
    }
}

fn series(velocity: Vec<f64>, error: Vec<f64>) -> ChannelSeries {
    let n = velocity.len();
    ChannelSeries {
        t_s: (0..n).map(|k| k as f64 * 0.01).collect(),
        position_error_um: error,
        velocity_mm_s: velocity,
        ..ChannelSeries::default()
    }
}

fn stroke_series(speed_mm_s: f64, error_um: &[f64]) -> ChannelSeries {
    series(vec![speed_mm_s; error_um.len()], error_um.to_vec())
}

#[test]
fn cruise_sd_and_p99_match_hand_values() {
    let mut e = vec![100.0; 980];
    e.extend(vec![300.0; 20]);
    let m = tracking_metrics(&stroke_series(50.0, &e), &regime());
    assert!((m.sd_um - 28.0).abs() < 1e-9, "sd {}", m.sd_um);
    assert!((m.p99_um - 300.0).abs() < 1e-9, "p99 {}", m.p99_um);
    assert!(
        (m.cruise_mean_abs_um - 104.0).abs() < 1e-9,
        "mean {}",
        m.cruise_mean_abs_um
    );
    assert_eq!(m.moving_samples, 1000);
}

#[test]
fn reversal_peak_is_the_mean_of_per_reversal_maxima() {
    let mut v = vec![50.0; 60];
    v.extend(vec![-50.0; 80]);
    v.extend(vec![50.0; 60]);
    let mut e = vec![0.0; 200];
    e[60] = -50.0;
    e[140] = 150.0;
    let m = tracking_metrics(&series(v, e), &regime());
    assert_eq!(m.reversal_count, 2);
    assert!(
        (m.reversal_peak_um - 100.0).abs() < 1e-9,
        "{}",
        m.reversal_peak_um
    );
}

#[test]
fn low_speed_samples_are_not_moving() {
    let mut v = vec![50.0; 10];
    v.extend(vec![40.0 * 5.0 / 60.0; 10]);
    let mut e = vec![10.0, 30.0, 10.0, 30.0, 10.0, 30.0, 10.0, 30.0, 10.0, 30.0];
    e.extend(vec![1000.0; 10]);
    let m = tracking_metrics(&series(v, e), &regime());
    assert_eq!(m.moving_samples, 10);
    assert!((m.sd_um - 10.0).abs() < 1e-9, "sd {}", m.sd_um);
}

#[test]
fn nan_velocity_does_not_panic() {
    let v = vec![f64::NAN, f64::INFINITY, 50.0, 50.0, 50.0];
    let m = tracking_metrics(&series(v, vec![1.0; 5]), &regime());
    assert!(m.moving_samples >= 3);
}

#[test]
fn speed_segments_split_on_standstill_gaps_in_order() {
    let mut v = vec![50.0; 100];
    v.extend(vec![0.0; 50]);
    v.extend(vec![195.0; 100]);
    v.extend(vec![0.0; 50]);
    v.extend(vec![400.0; 100]);
    let n = v.len();
    let s = series(v, vec![0.0; n]);
    assert_eq!(
        split_speed_segments(&s, 0.4),
        vec![(0, 100), (150, 250), (300, 400)]
    );
}

#[test]
fn short_stops_do_not_split_a_segment() {
    let mut v = vec![50.0; 100];
    v.extend(vec![0.0; 10]);
    v.extend(vec![50.0; 100]);
    let n = v.len();
    let s = series(v, vec![0.0; n]);
    assert_eq!(split_speed_segments(&s, 0.4), vec![(0, 210)]);
}
