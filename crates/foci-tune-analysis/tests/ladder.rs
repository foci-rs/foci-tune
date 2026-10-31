use foci_tune_analysis::ladder::{
    LadderAxis, LadderError, PID_GAIN_MAX_RAW, RungOutcome, StopDecision, default_values,
    interleave, is_marginal, median, probe_stop, to_raw,
};

fn raws(axis: LadderAxis, current: i32) -> Vec<i32> {
    default_values(axis, current, 50_000)
        .iter()
        .map(|v| v.raw)
        .collect()
}

#[test]
fn position_p_defaults_omit_the_negative_rung() {
    assert_eq!(raws(LadderAxis::PositionP, 10), vec![10, 30, 50, 70, 90]);
}

#[test]
fn velocity_p_defaults_omit_values_above_the_maximum() {
    assert_eq!(raws(LadderAxis::VelocityP, 32_700), vec![32_700]);
    assert_eq!(
        raws(LadderAxis::VelocityP, 1000),
        vec![1000, 1100, 1200, 1300, 1400, 1500]
    );
}

#[test]
fn corner_defaults_convert_through_pwm_rate() {
    let shown: Vec<f64> = default_values(LadderAxis::VelocityICornerHz, 0, 50_000)
        .iter()
        .map(|v| v.shown)
        .collect();
    assert_eq!(shown, vec![25.0, 50.0, 75.0, 100.0]);
}

#[test]
fn corner_75_hz_at_50_khz_is_618() {
    let v = to_raw(LadderAxis::VelocityICornerHz, 75.0, 50_000).unwrap();
    assert_eq!(v.raw, 618);
    assert_eq!(v.shown, 75.0);
}

#[test]
fn typed_values_outside_the_range_are_rejected_with_the_value() {
    let hi = to_raw(LadderAxis::VelocityICornerHz, 5000.0, 50_000).unwrap_err();
    let LadderError::OutOfRange { shown, raw } = hi;
    assert_eq!(shown, 5000.0);
    assert!(raw > PID_GAIN_MAX_RAW);
    assert!(to_raw(LadderAxis::PositionP, -1.0, 50_000).is_err());
    assert!(to_raw(LadderAxis::VelocityP, f64::NAN, 50_000).is_err());
    assert_eq!(
        to_raw(LadderAxis::VelocityP, 32_767.0, 50_000).unwrap().raw,
        PID_GAIN_MAX_RAW
    );
    assert!(to_raw(LadderAxis::VelocityP, 32_768.0, 50_000).is_err());
}

#[test]
fn interleave_runs_value_by_value_repeat_by_repeat() {
    assert_eq!(
        interleave(3, 2),
        vec![(0, 0), (1, 0), (2, 0), (0, 1), (1, 1), (2, 1)]
    );
    assert!(interleave(0, 2).is_empty());
}

fn assessed(v: &[f64]) -> Vec<RungOutcome> {
    v.iter().map(|z| RungOutcome::Assessed(*z)).collect()
}

#[test]
fn probe_stop_stops_only_strictly_below_the_floor() {
    match probe_stop(&assessed(&[0.06, 0.03]), 2, 0.05) {
        StopDecision::StopBelowFloor { median } => assert!((median - 0.045).abs() < 1e-12),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        probe_stop(&assessed(&[0.0625, 0.0375]), 2, 0.05),
        StopDecision::Continue
    );
    assert_eq!(
        probe_stop(&assessed(&[0.1, 0.09]), 2, 0.05),
        StopDecision::Continue
    );
}

#[test]
fn probe_stop_flags_unassessable_and_waits_for_all_repeats() {
    let mixed = [RungOutcome::Assessed(0.08), RungOutcome::Unassessable];
    assert_eq!(probe_stop(&mixed, 2, 0.05), StopDecision::StopUnassessable);
    assert_eq!(
        probe_stop(&assessed(&[0.01]), 2, 0.05),
        StopDecision::Continue
    );
    assert_eq!(
        probe_stop(&assessed(&[f64::NAN, 0.1]), 2, 0.05),
        StopDecision::StopUnassessable
    );
}

#[test]
fn median_and_marginal() {
    assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
    assert_eq!(median(&[4.0, 1.0]), 2.5);
    assert!(median(&[]).is_nan());
    assert!(is_marginal(0.07, 0.08));
    assert!(!is_marginal(0.08, 0.08));
}

#[test]
fn probe_stop_evaluates_only_the_first_repeats() {
    assert_eq!(
        probe_stop(&assessed(&[0.1, 0.09, 0.01]), 2, 0.05),
        StopDecision::Continue
    );
    let tail_unassessable = [
        RungOutcome::Assessed(0.06),
        RungOutcome::Assessed(0.03),
        RungOutcome::Unassessable,
    ];
    match probe_stop(&tail_unassessable, 2, 0.05) {
        StopDecision::StopBelowFloor { median } => assert!((median - 0.045).abs() < 1e-12),
        other => panic!("{other:?}"),
    }
}
