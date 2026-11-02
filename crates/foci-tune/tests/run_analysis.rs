use std::collections::HashMap;

use foci_tune::run_analysis::{AnalysisInput, StimulusWindow, analyze, stimulus_window};
use foci_tune::stimulus::RunKind;
use foci_tune_analysis::protocol::{Critical, Frame, FrameHeader};
use foci_tune_analysis::units::Regime;

const CLOCK: u32 = 260_000_000;
const TICKS_PER_MS: u64 = 260_000;

fn regime() -> Regime {
    Regime {
        motor_rotation_distance_mm: 40.0,
        encoder_counts_per_rev: 4000,
        pwm_hz: 50_000,
        clock_freq: CLOCK,
    }
}

fn regimes() -> HashMap<u8, Regime> {
    HashMap::from([(0, regime()), (1, regime())])
}

fn critical(channel: u8, ms: usize, step: i32, tmc: i32, torque: i16) -> Frame {
    Frame::Critical(
        FrameHeader {
            protocol_version: 25,
            frame_type: 1,
            flags: 0,
            payload_len: 88,
            source_kind: 1,
            source_index: channel,
            sequence: ms as u32,
            timestamp_ticks: ms as u64 * TICKS_PER_MS,
            boot_id: 7,
        },
        Critical {
            step_position: step,
            tmc_position_actual: tmc,
            position_error: 0,
            target_velocity: 0,
            pid_velocity_actual: 0,
            cached_pid_velocity_offset: 0,
            velocity_error: 0,
            sample_flags: 0,
            torque_target: torque,
            torque_actual: 0,
        },
    )
}

/// One millisecond piece of the script: channel 0 commanded steps per ms and channel 1
/// commanded steps per ms, for `ms` samples.
struct Piece {
    ms: usize,
    declared: i32,
    other: i32,
}

const fn hold(ms: usize) -> Piece {
    Piece {
        ms,
        declared: 0,
        other: 0,
    }
}

const fn strokes(ms: usize) -> Piece {
    Piece {
        ms,
        declared: 20,
        other: 0,
    }
}

/// An approach along a direction that also drives the idle motor.
const fn approach(ms: usize) -> Piece {
    Piece {
        ms,
        declared: -20,
        other: 3,
    }
}

/// Frames at 1 kHz for both channels; actual positions follow the command ten to one.
fn frames(pieces: &[Piece]) -> Vec<Frame> {
    frames_with_torque(pieces, |_| 0)
}

/// As `frames`, with the commanded torque of both channels given per millisecond.
fn frames_with_torque(pieces: &[Piece], torque: impl Fn(usize) -> i16) -> Vec<Frame> {
    let (mut a, mut b, mut ms) = (0, 0, 0);
    let mut out = Vec::new();
    for piece in pieces {
        for _ in 0..piece.ms {
            a += piece.declared;
            b += piece.other;
            out.push(critical(0, ms, a, a * 10, torque(ms)));
            out.push(critical(1, ms, b, b * 10, torque(ms)));
            ms += 1;
        }
    }
    out
}

fn tracking(speeds: usize) -> RunKind {
    RunKind::Tracking {
        speeds_mm_s: vec![50.0; speeds],
        strokes: 4,
    }
}

fn input<'a>(
    kind: &'a RunKind,
    frames: &'a [Frame],
    regimes: &'a HashMap<u8, Regime>,
    speeds: &'a [f64],
) -> AnalysisInput<'a> {
    AnalysisInput {
        kind,
        declared: 0,
        frames,
        regimes,
        applied_speeds: speeds,
    }
}

fn ticks(ms: usize) -> u64 {
    ms as u64 * TICKS_PER_MS
}

#[test]
fn an_approach_before_the_speed_blocks_is_not_a_block() {
    let f = frames(&[
        hold(250),
        approach(1000),
        hold(600),
        strokes(1500),
        hold(500),
        strokes(800),
        hold(300),
    ]);
    let kind = tracking(2);
    let speeds = [50.0, 195.0];
    let r = regimes();
    let input = input(&kind, &f, &r, &speeds);

    let window = stimulus_window(&input);
    let metrics = analyze(&input, &window, &mut Vec::new());

    assert_eq!(
        window,
        StimulusWindow::Strokes {
            blocks: vec![(ticks(1850), ticks(3349)), (ticks(3850), ticks(4649))],
        }
    );
    let declared = &metrics.channels[0].tracking;
    assert_eq!(declared.len(), 2);
    assert_eq!(declared[0].speed_mm_s, 50.0);
    assert!(declared[0].moving_samples > 1400, "{declared:?}");
    assert_eq!(declared[1].speed_mm_s, 195.0);
    assert!(declared[1].moving_samples < 900, "{declared:?}");
}

#[test]
fn without_an_approach_every_block_is_a_speed() {
    let f = frames(&[hold(850), strokes(1500), hold(500), strokes(800), hold(300)]);
    let kind = tracking(2);
    let speeds = [50.0, 195.0];
    let r = regimes();
    let input = input(&kind, &f, &r, &speeds);

    let window = stimulus_window(&input);
    let metrics = analyze(&input, &window, &mut Vec::new());

    assert_eq!(
        window,
        StimulusWindow::Strokes {
            blocks: vec![(ticks(850), ticks(2349)), (ticks(2850), ticks(3649))],
        }
    );
    let declared = &metrics.channels[0].tracking;
    assert_eq!(declared[0].speed_mm_s, 50.0);
    assert!(declared[0].moving_samples > 1400, "{declared:?}");
    assert_eq!(declared[1].speed_mm_s, 195.0);
    assert!(declared[1].moving_samples < 900, "{declared:?}");
}

#[test]
fn a_block_count_that_fits_no_script_is_unmatched() {
    let f = frames(&[
        hold(250),
        approach(1000),
        hold(600),
        strokes(1500),
        hold(500),
        strokes(800),
        hold(500),
        strokes(800),
        hold(300),
    ]);
    let kind = tracking(1);
    let speeds = [50.0];
    let r = regimes();
    let input = input(&kind, &f, &r, &speeds);

    let window = stimulus_window(&input);
    let metrics = analyze(&input, &window, &mut Vec::new());

    assert_eq!(
        window,
        StimulusWindow::Unmatched("found 4 motion blocks for 1 speed".to_owned())
    );
    assert!(metrics.channels[0].tracking.is_empty());
}

#[test]
fn standstill_rests_start_after_the_approach() {
    let f = frames(&[
        hold(250),
        approach(1000),
        hold(3500),
        Piece {
            ms: 1,
            declared: 1,
            other: 0,
        },
        hold(3000),
    ]);
    let kind = RunKind::Standstill {
        rests: 2,
        dwell_s: 3.0,
    };
    let r = regimes();
    let input = input(&kind, &f, &r, &[]);

    let window = stimulus_window(&input);

    assert_eq!(
        window,
        StimulusWindow::Rests {
            from: ticks(1249),
            to: ticks(7750),
        }
    );
}

#[test]
fn an_analysed_standstill_run_records_the_rates_per_rest_and_per_run() {
    let f = frames_with_torque(
        &[
            hold(250),
            approach(1000),
            hold(3500),
            Piece {
                ms: 1,
                declared: 1,
                other: 0,
            },
            hold(3000),
        ],
        |ms| if ms % 2 == 0 { -2 } else { -3 },
    );
    let kind = RunKind::Standstill {
        rests: 2,
        dwell_s: 3.0,
    };
    let r = regimes();
    let input = input(&kind, &f, &r, &[]);

    let window = stimulus_window(&input);
    let metrics = analyze(&input, &window, &mut Vec::new());

    let standstill = metrics.channels[0].standstill.as_ref().unwrap();
    assert_eq!(standstill.rests.len(), 2);
    for rest in &standstill.rests {
        let rate = rest.transitions_per_s.unwrap();
        assert!((rate - 1000.0).abs() < 1e-6, "{rest:?}");
        assert_eq!(rest.large_transitions_per_s, Some(0.0));
    }
    assert_eq!(standstill.worst_large_transitions_per_s, Some(0.0));
}
