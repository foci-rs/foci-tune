use foci_tune_analysis::protocol::{
    DecodeError, FLAG_DROP_GAP_BEFORE_THIS_FRAME, Frame, decode_frame,
};
use foci_tune_analysis::signals::channel_series;
use foci_tune_analysis::units::Regime;

fn header(
    version: u8,
    frame_type: u8,
    flags: u8,
    payload_len: usize,
    source_index: u8,
    seq: u32,
    ts: u64,
) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&0x4352_5446u32.to_le_bytes());
    v.extend_from_slice(&[version, 32, frame_type, flags]);
    v.extend_from_slice(&(payload_len as u16).to_le_bytes());
    v.extend_from_slice(&[1, source_index]);
    v.extend_from_slice(&seq.to_le_bytes());
    v.extend_from_slice(&ts.to_le_bytes());
    v.extend_from_slice(&7u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v
}

fn frame(
    frame_type: u8,
    flags: u8,
    payload: &[u8],
    source_index: u8,
    seq: u32,
    ts: u64,
) -> Vec<u8> {
    let mut v = header(25, frame_type, flags, payload.len(), source_index, seq, ts);
    v.extend_from_slice(payload);
    v
}

fn control_state_payload(seq: u32) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&seq.to_le_bytes());
    p.extend_from_slice(&123_456u64.to_le_bytes());
    p.extend_from_slice(&[1, 0, 0, 0]);
    p.extend_from_slice(&4000u32.to_le_bytes());
    p.extend_from_slice(&50_000u32.to_le_bytes());
    p.extend_from_slice(&[2, 0, 0, 0]);
    for hz in [11u16, 22, 33, 44] {
        p.extend_from_slice(&hz.to_le_bytes());
    }
    for g in [841i32, 256, 141, 0, 1200, 300, 1200, 300] {
        p.extend_from_slice(&g.to_le_bytes());
    }
    p.extend_from_slice(&[1, 0]);
    p.extend_from_slice(&1000u16.to_le_bytes());
    p.extend_from_slice(&[1, 0]);
    p.extend_from_slice(&77u16.to_le_bytes());
    p.extend_from_slice(&88u16.to_le_bytes());
    p.extend_from_slice(&1_500u32.to_le_bytes());
    p.extend_from_slice(&0x5A81u16.to_le_bytes());
    p.resize(88, 0);
    p
}

fn critical_payload(pidin_torque: u32, torque_target_actual: u32) -> Vec<u8> {
    let mut p = Vec::new();
    for v in [100i32, 200, -300, 400, 500, 600, 700] {
        p.extend_from_slice(&v.to_le_bytes());
    }
    p.extend_from_slice(&[0, 0x5A]);
    p.extend_from_slice(&0u16.to_le_bytes());
    p.extend_from_slice(&0u32.to_le_bytes());
    p.extend_from_slice(&0i32.to_le_bytes());
    p.extend_from_slice(&0u64.to_le_bytes());
    p.extend_from_slice(&0u64.to_le_bytes());
    p.extend_from_slice(&0u32.to_le_bytes());
    p.extend_from_slice(&pidin_torque.to_le_bytes());
    p.extend_from_slice(&torque_target_actual.to_le_bytes());
    p.resize(88, 0);
    p
}

fn push_str(p: &mut Vec<u8>, s: &str) {
    p.push(s.len() as u8);
    p.extend_from_slice(s.as_bytes());
}

fn metadata_payload() -> Vec<u8> {
    let mut p = vec![1, 0, 2, 0];
    for v in [260_000_000u32, 1000, 100, 10, 0, 0, 0, 0] {
        p.extend_from_slice(&v.to_le_bytes());
    }
    push_str(&mut p, "fw");
    push_str(&mut p, "FOCI Ouroboros");
    push_str(&mut p, "2600abcd");
    for (kind, idx, label) in [(1u8, 0u8, "channel0"), (1, 1, "channel1")] {
        p.extend_from_slice(&[kind, idx, label.len() as u8]);
        p.extend_from_slice(label.as_bytes());
    }
    p
}

#[test]
fn control_state_decodes_every_field_at_its_offset() {
    let bytes = frame(10, 0, &control_state_payload(9), 0, 5, 42);
    let Frame::ControlState(h, c) = decode_frame(&bytes).unwrap() else {
        panic!("expected control state");
    };
    assert_eq!(
        (h.protocol_version, h.frame_type, h.payload_len),
        (25, 10, 88)
    );
    assert_eq!((h.sequence, h.timestamp_ticks, h.boot_id), (5, 42, 7));
    assert_eq!(c.change_sequence, 9);
    assert_eq!(c.applied_tick, 123_456);
    assert_eq!(c.applied_request, 1);
    assert_eq!(c.encoder_counts_per_rev, 4000);
    assert_eq!(c.pwm_hz, 50_000);
    assert_eq!(c.velocity_meter, 2);
    assert_eq!(
        (
            c.velocity_filter_hz,
            c.torque_filter_hz,
            c.position_filter_hz,
            c.flux_filter_hz
        ),
        (11, 22, 33, 44)
    );
    assert_eq!(
        (c.velocity_p, c.velocity_i, c.position_p, c.position_i),
        (841, 256, 141, 0)
    );
    assert_eq!(
        (c.torque_p, c.torque_i, c.flux_p, c.flux_i),
        (1200, 300, 1200, 300)
    );
    assert!(c.velocity_ff_enable);
    assert_eq!(c.velocity_ff_permille, 1000);
    assert!(c.accel_ff_enable);
    assert_eq!((c.accel_ff_gain, c.decel_ff_gain), (77, 88));
    assert_eq!(c.run_current, 1_500);
    assert_eq!(c.voltage_limit, 0x5A81);
}

#[test]
fn critical_splits_offset_64_into_signed_torque_target_and_actual() {
    let bytes = frame(1, 0, &critical_payload(0, 0xFFC9_0123), 1, 1, 1);
    let Frame::Critical(_, c) = decode_frame(&bytes).unwrap() else {
        panic!("expected critical");
    };
    assert_eq!((c.torque_target, c.torque_actual), (-55, 0x0123));
    assert_eq!(c.position_error, -300);
    assert_eq!(
        (
            c.step_position,
            c.tmc_position_actual,
            c.target_velocity,
            c.pid_velocity_actual
        ),
        (100, 200, 400, 500)
    );
    assert_eq!((c.cached_pid_velocity_offset, c.velocity_error), (600, 700));
    assert_eq!(c.sample_flags, 0x5A);
}

#[test]
fn metadata_decodes_strings_and_sources() {
    let bytes = frame(5, 0, &metadata_payload(), 0, 1, 1);
    let Frame::Metadata(_, m) = decode_frame(&bytes).unwrap() else {
        panic!("expected metadata");
    };
    assert_eq!(m.clock_freq, 260_000_000);
    assert_eq!(
        (m.critical_hz, m.control_detail_hz, m.status_hz),
        (1000, 100, 10)
    );
    assert_eq!(m.firmware_id, "fw");
    assert_eq!(m.product, "FOCI Ouroboros");
    assert_eq!(m.serial, "2600abcd");
    assert_eq!(
        m.sources,
        vec![
            (1, 0, "channel0".to_string()),
            (1, 1, "channel1".to_string())
        ]
    );
}

#[test]
fn protocol_24_is_rejected_with_the_version() {
    let mut bytes = header(24, 1, 0, 0, 0, 0, 0);
    bytes.extend_from_slice(&critical_payload(0, 0));
    assert!(matches!(
        decode_frame(&bytes),
        Err(DecodeError::UnsupportedVersion(24))
    ));
}

#[test]
fn truncated_payload_is_truncated_not_panic() {
    let mut bytes = header(25, 1, 0, 88, 0, 0, 0);
    bytes.extend_from_slice(&critical_payload(0, 0)[..40]);
    assert!(matches!(decode_frame(&bytes), Err(DecodeError::Truncated)));
    assert!(matches!(
        decode_frame(&bytes[..10]),
        Err(DecodeError::Truncated)
    ));
}

#[test]
fn drop_flag_is_exposed_on_the_header() {
    let bytes = frame(1, 1, &critical_payload(0, 0), 0, 0, 0);
    let Frame::Critical(h, _) = decode_frame(&bytes).unwrap() else {
        panic!("expected critical");
    };
    assert_ne!(h.flags & FLAG_DROP_GAP_BEFORE_THIS_FRAME, 0);
}

#[test]
fn unknown_frame_types_decode_as_other() {
    let bytes = frame(8, 0, &[1, 2, 3], 0, 0, 0);
    assert!(matches!(decode_frame(&bytes), Ok(Frame::Other(h)) if h.frame_type == 8));
}

#[test]
fn bad_magic_is_rejected() {
    let mut bytes = frame(1, 0, &critical_payload(0, 0), 0, 0, 0);
    bytes[0] = 0;
    assert!(matches!(decode_frame(&bytes), Err(DecodeError::BadMagic)));
}

#[test]
fn torque_actual_is_the_signed_low_half() {
    let bytes = frame(1, 0, &critical_payload(0, 0x0037_FF9C), 0, 0, 0);
    let Frame::Critical(_, c) = decode_frame(&bytes).unwrap() else {
        panic!("expected critical");
    };
    assert_eq!((c.torque_target, c.torque_actual), (55, -100));
}

#[test]
fn torque_series_comes_from_the_target_half_not_the_pidin_field() {
    let frames: Vec<_> = [0x0000_0000u32, 0xFF38_0010, 0x00C8_FFF0]
        .iter()
        .enumerate()
        .map(|(k, &packed)| {
            let bytes = frame(
                1,
                0,
                &critical_payload(0, packed),
                0,
                k as u32,
                k as u64 * 260_000,
            );
            match decode_frame(&bytes).unwrap() {
                Frame::Critical(h, c) => (h, c),
                other => panic!("expected critical, got {other:?}"),
            }
        })
        .collect();
    let regime = Regime {
        motor_rotation_distance_mm: 40.0,
        encoder_counts_per_rev: 4000,
        pwm_hz: 50_000,
        clock_freq: 260_000_000,
    };

    let s = channel_series(&frames, &regime);

    assert_eq!(s.torque, vec![0, -200, 200]);
}
