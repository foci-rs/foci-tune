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

const UNITS_PER_MM: f64 = 65536.0 / 40.0;
const JITTER_UNITS: [i32; 4] = [0, 66, 33, 49];

/// Commanded positions at 1 kHz: a hold of `samples` frames, or a move at `mm_s`.
enum Piece {
    Hold(usize),
    Move(usize, f64),
}

/// A series whose measured position jitters by up to 4 encoder counts around the command,
/// with velocity differentiated from the measured position as the series builder does.
fn commanded(pieces: &[Piece]) -> ChannelSeries {
    let mut step = Vec::new();
    let mut at = 0.0_f64;
    for piece in pieces {
        let (n, mm_s) = match *piece {
            Piece::Hold(n) => (n, 0.0),
            Piece::Move(n, mm_s) => (n, mm_s),
        };
        for _ in 0..n {
            at += mm_s * UNITS_PER_MM / 1000.0;
            step.push(at.round() as i32);
        }
    }
    let tmc: Vec<i32> = step
        .iter()
        .enumerate()
        .map(|(k, p)| p + JITTER_UNITS[k % JITTER_UNITS.len()])
        .collect();
    let velocity = (0..tmc.len())
        .map(|k| match k {
            0 => 0.0,
            _ => f64::from(tmc[k] - tmc[k - 1]) / UNITS_PER_MM * 1000.0,
        })
        .collect();
    ChannelSeries {
        t_s: (0..step.len()).map(|k| k as f64 * 1e-3).collect(),
        position_error_um: vec![0.0; step.len()],
        step_position: step,
        tmc_position: tmc,
        velocity_mm_s: velocity,
        ..ChannelSeries::default()
    }
}

#[test]
fn a_dwell_splits_speed_blocks_despite_encoder_jitter() {
    let s = commanded(&[
        Piece::Hold(300),
        Piece::Move(1000, 50.0),
        Piece::Hold(500),
        Piece::Move(1000, 195.0),
        Piece::Hold(300),
    ]);

    assert_eq!(
        split_speed_segments(&s, 0.4),
        vec![(300, 1300), (1800, 2800)]
    );
}

#[test]
fn back_to_back_speeds_without_a_dwell_are_one_block() {
    let s = commanded(&[
        Piece::Hold(300),
        Piece::Move(1000, 50.0),
        Piece::Move(1000, 195.0),
        Piece::Hold(300),
    ]);

    assert_eq!(split_speed_segments(&s, 0.4), vec![(300, 2300)]);
}

#[test]
fn a_hold_shorter_than_the_gap_does_not_split() {
    let s = commanded(&[
        Piece::Hold(300),
        Piece::Move(1000, 50.0),
        Piece::Hold(300),
        Piece::Move(1000, 195.0),
        Piece::Hold(300),
    ]);

    assert_eq!(split_speed_segments(&s, 0.4), vec![(300, 2600)]);
}

#[test]
fn leading_and_trailing_rests_are_not_blocks() {
    let s = commanded(&[Piece::Hold(200), Piece::Move(1000, 50.0), Piece::Hold(900)]);

    assert_eq!(split_speed_segments(&s, 0.4), vec![(200, 1200)]);
}
