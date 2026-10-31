use std::collections::HashMap;

use foci_tune::model::VerificationCheck;
use foci_tune::verify_run::{Window, first_failure, verify};
use foci_tune_analysis::protocol::{ControlState, Critical, Frame, FrameHeader};

const COUNTS_PER_REV: u32 = 4096;
const Q: i32 = 16;

fn header(source_index: u8, sequence: u32, flags: u8) -> FrameHeader {
    FrameHeader {
        protocol_version: 25,
        frame_type: 1,
        flags,
        payload_len: 88,
        source_kind: 1,
        source_index,
        sequence,
        timestamp_ticks: u64::from(sequence) * 1000,
        boot_id: 7,
    }
}

fn critical(channel: u8, sequence: u32, step: i32, tmc: i32) -> Frame {
    Frame::Critical(
        header(channel, sequence, 0),
        Critical {
            step_position: step,
            tmc_position_actual: tmc,
            position_error: 0,
            target_velocity: 0,
            pid_velocity_actual: 0,
            cached_pid_velocity_offset: 0,
            velocity_error: 0,
            sample_flags: 0,
            torque_target: 0,
            torque_actual: 0,
        },
    )
}

fn control(change_sequence: u32, applied_request: u8) -> ControlState {
    ControlState {
        change_sequence,
        applied_tick: 0,
        applied_request,
        encoder_counts_per_rev: COUNTS_PER_REV,
        pwm_hz: 50_000,
        velocity_meter: 0,
        velocity_filter_hz: 0,
        torque_filter_hz: 0,
        position_filter_hz: 0,
        flux_filter_hz: 0,
        velocity_p: 0,
        velocity_i: 0,
        position_p: 0,
        position_i: 0,
        torque_p: 0,
        torque_i: 0,
        flux_p: 0,
        flux_i: 0,
        velocity_ff_enable: false,
        velocity_ff_permille: 0,
        accel_ff_enable: false,
        accel_ff_gain: 0,
        decel_ff_gain: 0,
        run_current: 0,
        voltage_limit: 0,
    }
}

fn control_frame(channel: u8, sequence: u32, state: ControlState) -> Frame {
    Frame::ControlState(header(channel, sequence, 0), state)
}

fn baselines() -> HashMap<u8, ControlState> {
    HashMap::from([(0, control(5, 0)), (1, control(9, 0))])
}

/// Frames where channel 0's step position spans `declared` and channel 1's actual
/// position spans `other`.
fn motion(declared: i32, other: i32) -> Vec<Frame> {
    let mut frames = vec![
        control_frame(0, 1, control(5, 0)),
        control_frame(1, 2, control(9, 0)),
    ];
    frames.push(critical(0, 3, 1000, 1000));
    frames.push(critical(1, 4, 500, 500));
    frames.push(critical(0, 5, 1000 + declared, 1000 + declared));
    frames.push(critical(1, 6, 500, 500 + other));
    frames
}

fn outcome(
    frames: &[Frame],
    edges: Option<usize>,
) -> (Vec<(VerificationCheck, bool)>, Option<String>) {
    let base = baselines();
    let v = verify(&Window {
        declared: 0,
        frames,
        baselines: &base,
        probe_edges: edges,
    });
    (
        v.iter().map(|o| (o.check, o.passed)).collect(),
        first_failure(&v).map(str::to_owned),
    )
}

#[test]
fn declared_channel_must_move_more_than_one_count() {
    let (_, at_q) = outcome(&motion(Q, 0), None);
    let (_, above_q) = outcome(&motion(Q + 1, 0), None);

    assert_eq!(at_q.as_deref(), Some("channel 0 did not move"));
    assert_eq!(above_q, None);
}

fn channel_moved(frames: &[Frame]) -> (bool, String) {
    let base = baselines();
    let v = verify(&Window {
        declared: 0,
        frames,
        baselines: &base,
        probe_edges: None,
    });
    let check = v
        .into_iter()
        .find(|c| c.check == VerificationCheck::ChannelMoved)
        .unwrap();
    (check.passed, check.detail)
}

#[test]
fn a_short_declared_move_keeps_the_sixteen_count_floor() {
    let (at_floor, detail) = channel_moved(&motion(1000, 16 * Q));
    let (below_floor, _) = channel_moved(&motion(1000, 16 * Q - 1));

    assert!(!at_floor);
    assert_eq!(
        detail,
        "channel 1 moved 16.0 counts against a limit of 16.0 (declared channel 62.5)"
    );
    assert!(below_floor);
}

#[test]
fn a_long_declared_move_allows_two_percent_of_its_range() {
    let declared = 500 * 100 * Q;
    let (at_limit, detail) = channel_moved(&motion(declared, 1000 * Q));
    let (below_limit, _) = channel_moved(&motion(declared, 1000 * Q - 1));

    assert!(!at_limit);
    assert_eq!(
        detail,
        "channel 1 moved 1000.0 counts against a limit of 1000.0 (declared channel 50000.0)"
    );
    assert!(below_limit);
}

#[test]
fn belt_coupling_deflection_of_the_idle_corexy_motor_passes() {
    let (passed, detail) = channel_moved(&motion(19_988 * Q, 18 * Q));

    assert!(passed, "{detail}");
    assert!(
        detail.contains(
            "channel 1 moved 18.0 counts against a limit of 399.8 (declared channel 19988.0), \
             margin 381.8"
        ),
        "{detail}"
    );
}

#[test]
fn feedforward_writes_advance_the_sequence_one_at_a_time() {
    let mut frames = motion(1000, 0);
    frames.push(control_frame(0, 7, control(6, 7)));
    frames.push(control_frame(0, 8, control(7, 7)));
    frames.push(control_frame(0, 9, control(7, 0)));

    let (checks, reason) = outcome(&frames, None);

    assert!(checks.contains(&(VerificationCheck::BaselineContiguous, true)));
    assert_eq!(reason, None);
}

#[test]
fn a_sequence_jump_of_two_fails_contiguity() {
    let mut frames = motion(1000, 0);
    frames.push(control_frame(0, 7, control(7, 7)));

    let (checks, reason) = outcome(&frames, None);

    assert!(checks.contains(&(VerificationCheck::BaselineContiguous, false)));
    assert_eq!(
        reason.as_deref(),
        Some("change_sequence on channel 0 advanced by 2")
    );
}

#[test]
fn a_write_the_tool_did_not_make_fails_contiguity() {
    let mut frames = motion(1000, 0);
    frames.push(control_frame(0, 7, control(6, 1)));

    let (_, reason) = outcome(&frames, None);

    assert_eq!(
        reason.as_deref(),
        Some("channel 0 changed by request 1 from outside the tool")
    );
}

#[test]
fn another_channels_writes_do_not_count() {
    let mut frames = motion(1000, 0);
    frames.push(control_frame(1, 7, control(12, 1)));

    let (_, reason) = outcome(&frames, None);

    assert_eq!(reason, None);
}

#[test]
fn a_drop_flag_anywhere_in_the_window_fails() {
    let mut frames = motion(1000, 0);
    if let Frame::Critical(h, _) = &mut frames[4] {
        h.flags = 1;
    }

    let (checks, reason) = outcome(&frames, None);

    assert!(checks.contains(&(VerificationCheck::NoDropGaps, false)));
    assert_eq!(
        reason.as_deref(),
        Some("drop gap before frame 5 of source 0")
    );
}

#[test]
fn probe_runs_need_twenty_edges() {
    let frames = motion(1000, 0);

    let (checks, twelve) = outcome(&frames, Some(12));
    let (_, twenty) = outcome(&frames, Some(20));
    let (no_probe, _) = outcome(&frames, None);

    assert!(checks.contains(&(VerificationCheck::EdgeCount, false)));
    assert_eq!(twelve.as_deref(), Some("12 edges"));
    assert_eq!(twenty, None);
    assert!(
        !no_probe
            .iter()
            .any(|(c, _)| *c == VerificationCheck::EdgeCount)
    );
}
