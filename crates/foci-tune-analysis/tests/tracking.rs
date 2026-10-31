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

const COUNT_UNITS: f64 = 65536.0 / 4096.0;

/// Commanded positions in mm at 1 kHz for `strokes` alternating strokes, each accelerating to
/// 50 mm/s over 100 samples, cruising 400 and decelerating over 100; the sample index of each
/// turning point comes with them.
fn stroke_path(strokes: usize) -> (Vec<f64>, Vec<usize>) {
    let profile: Vec<f64> = (0..600)
        .map(|k| match k {
            0..100 => 50.0 * k as f64 / 100.0,
            100..500 => 50.0,
            _ => 50.0 * (600 - k) as f64 / 100.0,
        })
        .collect();
    let (mut mm, mut at, mut turns) = (Vec::new(), 0.0, Vec::new());
    for stroke in 0..strokes {
        let dir = if stroke % 2 == 0 { 1.0 } else { -1.0 };
        for v in &profile {
            at += dir * v / 1000.0;
            mm.push(at);
        }
        turns.push(mm.len() - 1);
    }
    turns.pop();
    (mm, turns)
}

/// A series over `mm`: the measured position lags the command by 5 samples, is quantised to
/// encoder counts and gains one count on alternate samples while slower than 15 mm/s, so the
/// finite-difference velocity holds runs of zeros and flips sign near every reversal.
fn quantised(mm: &[f64], error_um: Vec<f64>) -> ChannelSeries {
    let step: Vec<i32> = mm
        .iter()
        .map(|p| (p * UNITS_PER_MM).round() as i32)
        .collect();
    let tmc: Vec<i32> = (0..mm.len())
        .map(|k| {
            let lagged = mm[k.saturating_sub(5)];
            let slow = k > 0 && (mm[k] - mm[k - 1]).abs() * 1000.0 < 15.0;
            let counts = (lagged * UNITS_PER_MM / COUNT_UNITS).round()
                + f64::from(u8::from(slow && k % 2 == 1));
            (counts * COUNT_UNITS).round() as i32
        })
        .collect();
    let velocity = (0..tmc.len())
        .map(|k| match k {
            0 => 0.0,
            _ => f64::from(tmc[k] - tmc[k - 1]) / UNITS_PER_MM * 1000.0,
        })
        .collect();
    ChannelSeries {
        t_s: (0..mm.len()).map(|k| k as f64 * 1e-3).collect(),
        position_error_um: error_um,
        step_position: step,
        tmc_position: tmc,
        velocity_mm_s: velocity,
        ..ChannelSeries::default()
    }
}

#[test]
fn reversals_come_from_the_commanded_path_despite_velocity_quantisation() {
    let (mut mm, turns) = stroke_path(4);
    mm[250] -= 2.0 / UNITS_PER_MM;
    let mut error = vec![10.0; mm.len()];
    for (&turn, peak) in turns.iter().zip([80.0, 120.0, 100.0]) {
        error[turn + 25] = peak;
    }

    let m = tracking_metrics(&quantised(&mm, error), &regime());

    assert_eq!(m.reversal_count, 3);
    assert!(
        (m.reversal_peak_um - 100.0).abs() < 1e-9,
        "{}",
        m.reversal_peak_um
    );
}

#[test]
fn a_monotonic_move_has_no_reversal() {
    let (mm, _) = stroke_path(1);
    let error = vec![10.0; mm.len()];

    let m = tracking_metrics(&quantised(&mm, error), &regime());

    assert_eq!(m.reversal_count, 0);
    assert!(m.reversal_peak_um.is_nan());
}
