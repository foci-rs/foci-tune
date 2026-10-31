use foci_tune_analysis::protocol::{Critical, FrameHeader};
use foci_tune_analysis::signals::{
    ChannelSeries, channel_series, corner_hz, i_gain_param, p_gain_param, velocity_i_raw,
};
use foci_tune_analysis::units::{POSITION_UNITS_PER_REV, Regime};

fn regime(rd: f64, cpr: u32) -> Regime {
    Regime {
        motor_rotation_distance_mm: rd,
        encoder_counts_per_rev: cpr,
        pwm_hz: 50_000,
        clock_freq: 260_000_000,
    }
}

fn header(sequence: u32, ticks: u64, flags: u8) -> FrameHeader {
    FrameHeader {
        protocol_version: 25,
        frame_type: 1,
        flags,
        payload_len: 88,
        source_kind: 0,
        source_index: 0,
        sequence,
        timestamp_ticks: ticks,
        boot_id: 1,
    }
}

fn critical(pos: i32) -> Critical {
    Critical {
        step_position: pos,
        tmc_position_actual: pos,
        position_error: 0,
        target_velocity: 0,
        pid_velocity_actual: 0,
        cached_pid_velocity_offset: 0,
        velocity_error: 0,
        sample_flags: 0,
        interim_pidin_target_torque: 0,
    }
}

#[test]
fn position_conversion_uses_65536_per_rev_for_any_encoder() {
    assert_eq!(POSITION_UNITS_PER_REV, 65536.0);
    for cpr in [4000, 16384, 65536] {
        let r = regime(40.0, cpr);
        assert!((r.position_um(65536) - 40_000.0).abs() < 1e-9);
        assert!((r.position_um(-32768) + 20_000.0).abs() < 1e-9);
    }
}

#[test]
fn gear_ratio_is_already_folded_into_motor_rotation_distance() {
    let r = regime(40.0 / 5.0, 4000);
    assert!((r.position_um(65536) - 8_000.0).abs() < 1e-9);
}

#[test]
fn quantum_is_65536_over_counts_per_rev() {
    let r = regime(40.0, 4000);
    assert!((r.quantum_units() - 16.384).abs() < 1e-12);
    assert!((r.quantum_um() - r.position_um(1) * 16.384).abs() < 1e-9);
    assert!((r.quantum_um() - 10.0).abs() < 1e-9);
}

#[test]
fn velocity_from_a_linear_ramp_is_constant() {
    // 16384 units = 10 mm at rd 40; one frame per 0.1 s = 100 mm/s.
    let r = regime(40.0, 4000);
    let frames: Vec<_> = (0..1000u32)
        .map(|k| {
            (
                header(k, 5_000_000_000 + u64::from(k) * 26_000_000, 0),
                critical(k as i32 * 16384),
            )
        })
        .collect();
    let s: ChannelSeries = channel_series(&frames, &r);
    assert_eq!(s.t_s.len(), 1000);
    assert_eq!(s.t_s[0], 0.0);
    assert!((s.t_s[999] - 99.9).abs() < 1e-9);
    assert_eq!(s.velocity_mm_s[0], 0.0);
    for v in &s.velocity_mm_s[1..] {
        assert!((v - 100.0).abs() < 1e-6, "{v}");
    }
}

#[test]
fn series_maps_fields_and_marks_drop_gaps() {
    let r = regime(40.0, 4000);
    let mut c = critical(100);
    c.position_error = 65536;
    c.cached_pid_velocity_offset = -7;
    c.interim_pidin_target_torque = -300;
    let frames = vec![(header(0, 0, 0), critical(0)), (header(5, 260_000, 1), c)];
    let s = channel_series(&frames, &r);
    assert_eq!(s.drop_gap, vec![false, true]);
    assert_eq!(s.step_position, vec![0, 100]);
    assert_eq!(s.tmc_position, vec![0, 100]);
    assert!((s.position_error_um[1] - 40_000.0).abs() < 1e-9);
    assert_eq!(s.ff_offset, vec![0, -7]);
    assert_eq!(s.torque, vec![0, -300]);
}

#[test]
fn corner_round_trips_through_raw() {
    assert!((corner_hz(412, 50_000) - 50.03).abs() < 0.01);
    assert_eq!(velocity_i_raw(75.0, 50_000), 618);
    assert_eq!(velocity_i_raw(75.0, 25_000), 1235);
    for raw in [1, 412, 618, 1235] {
        assert_eq!(velocity_i_raw(corner_hz(raw, 50_000), 50_000), raw);
    }
}

#[test]
fn gain_params_round_trip_exactly() {
    assert_eq!(p_gain_param(841), "3.28515625");
    let p: f64 = p_gain_param(841).parse().unwrap();
    assert_eq!((p * 256.0).round() as i32, 841);
    let i: f64 = i_gain_param(412).parse().unwrap();
    assert_eq!((i * 4096.0).round() as i32, 412);
    assert_eq!(i_gain_param(1), "0.000244140625");
}
