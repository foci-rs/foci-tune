use foci_tune_analysis::ladder::RungOutcome;
use foci_tune_analysis::verdict::{
    DriftVerdict, Gates, Halt, InadmissibleReason, LadderAssessment, Pause, RepeatResult,
    RungVerdict, UnscoredReason, Waivers, assess_ladder, assess_ladder_at, assess_ladder_with,
    drift_verdict, pair_tolerance,
};

fn gated() -> Gates {
    Gates {
        standstill_factor: Some(2.0),
        ..Gates::default()
    }
}

fn rep(zeta: f64, p99: &[Option<f64>], sd: Option<f64>) -> RepeatResult {
    RepeatResult {
        zeta: RungOutcome::Assessed(zeta),
        p99_by_speed: p99.to_vec(),
        worst_torque_sd: sd,
    }
}

fn rung(zeta: f64, p99: [f64; 2], sd: f64, n: usize) -> Vec<RepeatResult> {
    (0..n)
        .map(|_| rep(zeta, &[Some(p99[0]), Some(p99[1])], Some(sd)))
        .collect()
}

#[test]
fn score_is_the_rungs_largest_median_p99() {
    let rungs = vec![
        rung(0.2, [60.0, 66.0], 1.0, 3),
        rung(0.2, [36.0, 80.0], 1.0, 3),
    ];
    let a = assess_ladder(&rungs, 3, &Gates::default(), false);
    assert_eq!(a.verdicts[0].score, Some(66.0));
    assert_eq!(a.verdicts[1].score, Some(80.0));
    assert_eq!(a.verdicts[0].deciding_speed, Some(1));
    assert_eq!(a.verdicts[1].deciding_speed, Some(1));
    assert_eq!(a.best, Some(0));
    assert_eq!(a.runner_up, Some(1));
}

#[test]
fn a_rung_that_hunts_at_the_slow_speed_loses_despite_winning_the_fast_one() {
    let rungs = vec![
        rung(0.2, [43.0, 66.0], 1.0, 3),
        rung(0.2, [90.0, 55.0], 1.0, 3),
    ];
    let a = assess_ladder(&rungs, 3, &Gates::default(), false);
    assert_eq!(a.verdicts[1].score, Some(90.0));
    assert_eq!(a.verdicts[1].deciding_speed, Some(0));
    assert_eq!(a.best, Some(0));
}

#[test]
fn equal_largest_medians_record_the_faster_speed() {
    let rungs = vec![rung(0.2, [70.0, 70.0], 1.0, 3)];
    let a = assess_ladder(&rungs, 3, &Gates::default(), false);
    assert_eq!(a.verdicts[0].deciding_speed, Some(1));
    let unsorted = assess_ladder_at(
        &rungs,
        3,
        &[400.0, 50.0],
        &Gates::default(),
        false,
        &Waivers::default(),
    );
    assert_eq!(unsorted.verdicts[0].deciding_speed, Some(0));
}

#[test]
fn equal_lowest_scores_make_the_lower_gain_best() {
    let rungs = vec![
        rung(0.2, [50.0, 70.0], 1.0, 3),
        rung(0.2, [60.0, 70.0], 1.0, 3),
    ];
    let a = assess_ladder(&rungs, 3, &Gates::default(), false);
    assert_eq!(a.best, Some(0));
    assert_eq!(a.runner_up, Some(1));
}

#[test]
fn equal_second_lowest_scores_make_the_lower_gain_the_runner_up() {
    let rungs = vec![
        rung(0.2, [40.0, 60.0], 1.0, 3),
        rung(0.2, [70.0, 50.0], 1.0, 3),
        rung(0.2, [40.0, 70.0], 1.0, 3),
    ];
    let a = assess_ladder(&rungs, 3, &Gates::default(), false);
    assert_eq!(a.best, Some(0));
    assert_eq!(a.runner_up, Some(1));
    assert_eq!(a.verdicts[1].deciding_speed, Some(0));
    assert_eq!(a.verdicts[2].deciding_speed, Some(1));
}

#[test]
fn a_score_does_not_change_when_a_later_rung_finishes() {
    let two = vec![
        rung(0.2, [60.0, 66.0], 1.0, 3),
        rung(0.2, [36.0, 80.0], 1.0, 3),
    ];
    let mut three = two.clone();
    three.push(rung(0.2, [30.0, 50.0], 1.0, 3));
    let a = assess_ladder(&two, 3, &Gates::default(), false);
    let b = assess_ladder(&three, 3, &Gates::default(), false);
    assert_eq!(a.verdicts[0].score, b.verdicts[0].score);
    assert_eq!(b.best, Some(2));
}

#[test]
fn one_speed_is_decided_by_index_zero() {
    let rungs = vec![vec![rep(0.2, &[Some(40.0)], Some(1.0)); 3]];
    let a = assess_ladder(&rungs, 3, &Gates::default(), false);
    assert_eq!(a.verdicts[0].deciding_speed, Some(0));
    assert_eq!(a.runner_up, None);
}

#[test]
fn median_uses_exactly_requested_repeats() {
    let rungs = vec![
        rung(0.2, [80.0, 80.0], 1.0, 3),
        rung(0.2, [70.0, 70.0], 1.0, 2),
    ];

    let a = assess_ladder(&rungs, 3, &Gates::default(), false);

    assert_eq!(a.verdicts[1], Default::default());
    assert_eq!(a.medians[1].zeta, None);
    assert!(a.halt.is_none());
    assert_eq!(a.verdicts[0].score, Some(80.0));
}

#[test]
fn damping_below_floor_halts() {
    let rungs = vec![
        rung(0.2, [80.0, 80.0], 1.0, 3),
        rung(0.04, [70.0, 70.0], 1.0, 3),
        rung(0.2, [60.0, 60.0], 1.0, 3),
    ];

    let a = assess_ladder(&rungs, 3, &Gates::default(), false);

    assert_eq!(a.halt, Some((1, Halt::BelowFloor { median: 0.04 })));
    assert_eq!(
        a.verdicts[1].inadmissible,
        Some(InadmissibleReason::DampingBelowFloor)
    );
    assert_eq!(a.verdicts[2], Default::default());
    assert_eq!(a.medians[2].zeta, None);
}

#[test]
fn unassessable_damping_halts() {
    let mut r1 = rung(0.2, [70.0, 70.0], 1.0, 3);
    r1[1].zeta = RungOutcome::Unassessable;
    let rungs = vec![rung(0.2, [80.0, 80.0], 1.0, 3), r1];

    let a = assess_ladder(&rungs, 3, &Gates::default(), false);

    assert_eq!(a.halt, Some((1, Halt::DampingUnassessable)));
}

#[test]
fn marginal_excluded_by_default() {
    let rungs = vec![
        rung(0.2, [80.0, 80.0], 1.0, 3),
        rung(0.06, [40.0, 40.0], 1.0, 3),
    ];

    let excluded = assess_ladder(&rungs, 3, &Gates::default(), false);
    assert!(excluded.verdicts[1].marginal);
    assert_eq!(excluded.verdicts[1].score, None);
    assert_eq!(excluded.verdicts[0].score, Some(80.0));

    let included = assess_ladder(&rungs, 3, &Gates::default(), true);
    assert!(included.verdicts[1].marginal);
    assert_eq!(included.verdicts[1].score, Some(40.0));
    assert_eq!(included.verdicts[0].score, Some(80.0));
}

#[test]
fn standstill_gate_marks_inadmissible_without_halting() {
    let rungs = vec![
        rung(0.2, [80.0, 80.0], 1.0, 3),
        rung(0.2, [70.0, 70.0], 2.5, 3),
        rung(0.2, [75.0, 75.0], 1.5, 3),
    ];

    let a = assess_ladder(&rungs, 3, &gated(), false);

    assert_eq!(
        a.verdicts[1].inadmissible,
        Some(InadmissibleReason::StandstillHigh)
    );
    assert_eq!(a.verdicts[1].score, None);
    assert!(a.halt.is_none());
    assert_eq!(a.verdicts[2].inadmissible, None);
    assert!(a.verdicts[2].score.is_some());
}

#[test]
fn missing_tracking_repeat_makes_rung_unscored() {
    let mut r1 = rung(0.2, [10.0, 10.0], 1.0, 3);
    r1[2].p99_by_speed[1] = None;
    let rungs = vec![
        rung(0.2, [80.0, 80.0], 1.0, 3),
        r1,
        rung(0.06, [5.0, 5.0], 1.0, 3),
        rung(0.2, [100.0, 100.0], 9.0, 3),
    ];

    let a = assess_ladder(&rungs, 3, &Gates::default(), false);

    assert_eq!(
        a.verdicts[1].unscored,
        Some(UnscoredReason::TrackingMissing)
    );
    assert_eq!(a.verdicts[1].score, None);
    assert_eq!(a.verdicts[0].score, Some(80.0));
}

#[test]
fn missing_standstill_on_later_rung_is_unscored() {
    let mut r1 = rung(0.2, [70.0, 70.0], 1.0, 3);
    r1[0].worst_torque_sd = None;
    let rungs = vec![rung(0.2, [80.0, 80.0], 1.0, 3), r1];

    let a = assess_ladder(&rungs, 3, &gated(), false);

    assert_eq!(
        a.verdicts[1].unscored,
        Some(UnscoredReason::StandstillMissing)
    );
    assert_eq!(a.verdicts[1].score, None);
    assert!(a.halt.is_none());
}

#[test]
fn unusable_standstill_baseline_halts() {
    let mut r0 = rung(0.2, [80.0, 80.0], 1.0, 3);
    r0[1].worst_torque_sd = None;
    let rungs = vec![r0, rung(0.2, [70.0, 70.0], 1.0, 3)];

    let a = assess_ladder(&rungs, 3, &gated(), false);

    assert_eq!(a.halt, Some((0, Halt::StandstillBaselineUnassessable)));
    assert_eq!(a.verdicts[0].score, None);
    assert_eq!(a.verdicts[1], Default::default());
}

#[test]
fn non_finite_standstill_is_unusable_not_a_passed_gate() {
    let mut bad_base = rung(0.2, [80.0, 80.0], 1.0, 3);
    bad_base[0].worst_torque_sd = Some(f64::NAN);
    let rungs = vec![bad_base, rung(0.2, [70.0, 70.0], 1.0, 3)];
    let a = assess_ladder(&rungs, 3, &gated(), false);
    assert_eq!(a.halt, Some((0, Halt::StandstillBaselineUnassessable)));

    let mut bad_later = rung(0.2, [70.0, 70.0], 50.0, 3);
    bad_later[2].worst_torque_sd = Some(f64::INFINITY);
    let rungs = vec![rung(0.2, [80.0, 80.0], 1.0, 3), bad_later];
    let a = assess_ladder(&rungs, 3, &gated(), false);
    assert_eq!(
        a.verdicts[1].unscored,
        Some(UnscoredReason::StandstillMissing)
    );
    assert_eq!(a.verdicts[1].score, None);
}

#[test]
fn non_positive_or_non_finite_p99_is_missing_tracking() {
    let mut r1 = rung(0.2, [70.0, 70.0], 1.0, 3);
    r1[0].p99_by_speed[0] = Some(0.0);
    let mut r2 = rung(0.2, [70.0, 70.0], 1.0, 3);
    r2[1].p99_by_speed[1] = Some(f64::NAN);
    let rungs = vec![rung(0.2, [80.0, 80.0], 1.0, 3), r1, r2];

    let a = assess_ladder(&rungs, 3, &Gates::default(), false);

    assert_eq!(
        a.verdicts[1].unscored,
        Some(UnscoredReason::TrackingMissing)
    );
    assert_eq!(
        a.verdicts[2].unscored,
        Some(UnscoredReason::TrackingMissing)
    );
    assert_eq!(a.verdicts[0].score, Some(80.0));
}

#[test]
fn no_scored_rung_is_representable() {
    let mut r0 = rung(0.2, [80.0, 80.0], 1.0, 3);
    r0[0].p99_by_speed[0] = None;
    let mut r1 = rung(0.2, [70.0, 70.0], 1.0, 3);
    r1[0].p99_by_speed[1] = None;

    let a = assess_ladder(&[r0, r1], 3, &Gates::default(), false);

    assert!(a.verdicts.iter().all(|v| v.score.is_none()));
    assert!(a.halt.is_none());
}

#[test]
fn unassessable_damping_pauses_the_new_entry_point() {
    let mut r1 = rung(0.2, [70.0, 70.0], 1.0, 3);
    r1[1].zeta = RungOutcome::Unassessable;
    let rungs = vec![
        rung(0.2, [80.0, 80.0], 1.0, 3),
        r1,
        rung(0.2, [60.0, 60.0], 1.0, 3),
    ];

    let a = assess_ladder_with(&rungs, 3, &Gates::default(), false, &Waivers::default());

    assert_eq!(a.pause, Some((1, Pause::DampingUnassessable)));
    assert_eq!(a.halt, None);
    assert_eq!(
        a.verdicts[2],
        RungVerdict::default(),
        "rungs above a pause are not assessed"
    );
}

#[test]
fn a_waived_rung_is_unscored_and_assessment_continues_above_it() {
    let mut r1 = rung(0.2, [70.0, 70.0], 1.0, 3);
    r1[1].zeta = RungOutcome::Unassessable;
    let rungs = vec![
        rung(0.2, [80.0, 80.0], 1.0, 3),
        r1,
        rung(0.2, [60.0, 60.0], 1.0, 3),
    ];
    let waivers = Waivers {
        continued: vec![false, true],
        standstill_gate_off: false,
    };

    let a = assess_ladder_with(&rungs, 3, &Gates::default(), false, &waivers);

    assert_eq!(a.pause, None);
    assert_eq!(a.verdicts[1].unscored, Some(UnscoredReason::DampingWaived));
    assert_eq!(a.verdicts[1].score, None);
    assert!(
        a.verdicts[2].score.is_some(),
        "the rung above the waived one is scored"
    );
}

#[test]
fn a_waived_rung_zero_still_provides_the_standstill_baseline() {
    let mut r0 = rung(0.2, [80.0, 80.0], 1.0, 3);
    r0[0].zeta = RungOutcome::Unassessable;
    let rungs = vec![r0, rung(0.2, [70.0, 70.0], 5.0, 3)];
    let waivers = Waivers {
        continued: vec![true],
        standstill_gate_off: false,
    };

    let a = assess_ladder_with(&rungs, 3, &gated(), false, &waivers);

    assert_eq!(a.pause, None);
    assert_eq!(a.verdicts[0].unscored, Some(UnscoredReason::DampingWaived));
    assert_eq!(
        a.verdicts[1].inadmissible,
        Some(InadmissibleReason::StandstillHigh),
        "5.0 > 2 x 1.0"
    );
}

#[test]
fn a_waived_rung_zero_without_a_usable_standstill_still_pauses_for_the_baseline() {
    let mut r0 = vec![rep(0.2, &[Some(80.0), Some(80.0)], None); 3];
    r0[0].zeta = RungOutcome::Unassessable;
    let rungs = vec![r0, rung(0.2, [70.0, 70.0], 1.0, 3)];
    let waivers = Waivers {
        continued: vec![true],
        standstill_gate_off: false,
    };

    let a = assess_ladder_with(&rungs, 3, &gated(), false, &waivers);

    assert_eq!(a.pause, Some((0, Pause::StandstillBaselineUnassessable)));
}

#[test]
fn below_floor_still_halts_under_the_new_entry_point() {
    let rungs = vec![
        rung(0.2, [80.0, 80.0], 1.0, 3),
        rung(0.04, [70.0, 70.0], 1.0, 3),
    ];
    let a = assess_ladder_with(&rungs, 3, &Gates::default(), false, &Waivers::default());
    assert_eq!(a.halt, Some((1, Halt::BelowFloor { median: 0.04 })));
    assert_eq!(a.pause, None);
}

#[test]
fn an_unusable_standstill_baseline_pauses_and_the_disabled_gate_scores_without_one() {
    let rungs = vec![
        vec![rep(0.2, &[Some(80.0), Some(80.0)], None); 3],
        rung(0.2, [70.0, 70.0], 5.0, 3),
    ];

    let paused = assess_ladder_with(&rungs, 3, &gated(), false, &Waivers::default());
    assert_eq!(
        paused.pause,
        Some((0, Pause::StandstillBaselineUnassessable))
    );

    let off = Waivers {
        continued: vec![],
        standstill_gate_off: true,
    };
    let a = assess_ladder_with(&rungs, 3, &gated(), false, &off);
    assert_eq!(a.pause, None);
    assert_eq!(a.verdicts[0].unscored, None);
    assert!(
        a.verdicts[0].score.is_some(),
        "rung 0 is scored without its standstill"
    );
    assert_eq!(
        a.verdicts[1].inadmissible, None,
        "no baseline, no StandstillHigh"
    );
    assert!(a.verdicts[1].score.is_some());
}

#[test]
fn the_old_entry_point_folds_a_pause_into_a_halt() {
    let mut r1 = rung(0.2, [70.0, 70.0], 1.0, 3);
    r1[1].zeta = RungOutcome::Unassessable;
    let rungs = vec![rung(0.2, [80.0, 80.0], 1.0, 3), r1];
    let a = assess_ladder(&rungs, 3, &Gates::default(), false);
    assert_eq!(a.halt, Some((1, Halt::DampingUnassessable)));
    assert_eq!(a.pause, None);
}

#[test]
fn an_assessment_without_a_pause_field_deserialises() {
    let v = serde_json::json!({"verdicts": [], "medians": [], "halt": [1, "DampingUnassessable"], "best": null, "runner_up": null, "tie": [], "spreads": []});
    let a: LadderAssessment = serde_json::from_value(v).unwrap();
    assert_eq!(a.halt, Some((1, Halt::DampingUnassessable)));
    assert_eq!(a.pause, None);
}

#[test]
fn a_later_rung_without_a_standstill_metric_is_scored_when_the_gate_is_off() {
    let rungs = vec![
        rung(0.2, [80.0, 80.0], 1.0, 3),
        vec![rep(0.2, &[Some(70.0), Some(70.0)], None); 3],
    ];
    let off = Waivers {
        continued: vec![],
        standstill_gate_off: true,
    };

    let a = assess_ladder_with(&rungs, 3, &gated(), false, &off);

    assert_eq!(a.verdicts[1].unscored, None);
    assert!(a.verdicts[1].score.is_some());
}

#[test]
fn the_default_gates_leave_the_standstill_gate_off() {
    assert_eq!(Gates::default().standstill_factor, None);
}

#[test]
fn an_absent_factor_scores_a_ladder_whose_baseline_is_missing() {
    let rungs = vec![
        vec![rep(0.2, &[Some(80.0), Some(80.0)], None); 3],
        rung(0.2, [70.0, 70.0], 50.0, 3),
    ];
    let a = assess_ladder_with(&rungs, 3, &Gates::default(), false, &Waivers::default());
    assert_eq!(a.pause, None);
    assert_eq!(a.verdicts[0].unscored, None);
    assert!(a.verdicts[0].score.is_some());
    assert_eq!(a.verdicts[1].inadmissible, None);
    assert!(a.verdicts[1].score.is_some());
}

#[test]
fn an_absent_factor_admits_a_rung_ten_times_the_baseline() {
    let rungs = vec![
        rung(0.2, [80.0, 80.0], 1.0, 3),
        rung(0.2, [70.0, 70.0], 10.0, 3),
    ];
    let a = assess_ladder_with(&rungs, 3, &Gates::default(), false, &Waivers::default());
    assert_eq!(a.verdicts[1].inadmissible, None);
    let g = assess_ladder_with(&rungs, 3, &gated(), false, &Waivers::default());
    assert_eq!(
        g.verdicts[1].inadmissible,
        Some(InadmissibleReason::StandstillHigh)
    );
}

#[test]
fn an_absent_factor_scores_a_rung_with_one_repeat_lacking_a_standstill() {
    let mut r1 = rung(0.2, [70.0, 70.0], 1.0, 3);
    r1[2].worst_torque_sd = None;
    let rungs = vec![rung(0.2, [80.0, 80.0], 1.0, 3), r1];
    let a = assess_ladder_with(&rungs, 3, &Gates::default(), false, &Waivers::default());
    assert_eq!(a.verdicts[1].unscored, None);
    assert!(a.verdicts[1].score.is_some());
}

#[test]
fn an_absent_factor_and_the_step_waiver_give_the_same_verdicts() {
    let rungs = vec![
        vec![rep(0.2, &[Some(80.0), Some(80.0)], None); 3],
        rung(0.2, [70.0, 70.0], 50.0, 3),
        rung(0.2, [75.0, 75.0], 0.5, 3),
    ];
    let off = Waivers {
        continued: vec![],
        standstill_gate_off: true,
    };
    let by_setup = assess_ladder_with(&rungs, 3, &Gates::default(), false, &Waivers::default());
    let by_waiver = assess_ladder_with(&rungs, 3, &gated(), false, &off);
    assert_eq!(by_setup, by_waiver);
}

#[test]
fn gates_without_a_standstill_factor_deserialise_as_off() {
    let base = serde_json::json!({"damping_floor": 0.05, "marginal": 0.08});
    let g: Gates = serde_json::from_value(base.clone()).unwrap();
    assert_eq!(g.standstill_factor, None);
    let mut with_null = base.clone();
    with_null["standstill_factor"] = serde_json::Value::Null;
    let g: Gates = serde_json::from_value(with_null).unwrap();
    assert_eq!(g.standstill_factor, None);
    let mut with_value = base;
    with_value["standstill_factor"] = serde_json::json!(2.0);
    let g: Gates = serde_json::from_value(with_value).unwrap();
    assert_eq!(g.standstill_factor, Some(2.0));
}

fn spread_rung(p99_fast: f64, e: f64) -> Vec<RepeatResult> {
    [-e, 0.0, e]
        .iter()
        .map(|o| rep(0.2, &[Some(40.0), Some(p99_fast * (1.0 + o))], Some(1.0)))
        .collect()
}

#[test]
fn pair_tolerance_is_two_sigma_of_the_difference() {
    let t = pair_tolerance(0.02, 100.0, 0.01, 200.0);
    assert!((t - 2.0 * (2.0f64.powi(2) + 2.0f64.powi(2)).sqrt()).abs() < 1e-12);
}

#[test]
fn a_gap_inside_the_pair_tolerance_is_a_tie() {
    let a = assess_ladder(
        &[spread_rung(100.0, 0.01), spread_rung(101.0, 0.01)],
        3,
        &Gates::default(),
        false,
    );
    assert!((a.spreads[1].unwrap() - 0.01).abs() < 1e-12);
    assert_eq!(a.verdicts[0].gap, Some(0.0));
    assert_eq!(a.verdicts[0].tolerance, None);
    assert!((a.verdicts[1].gap.unwrap() - 1.0).abs() < 1e-9);
    assert!((a.verdicts[1].tolerance.unwrap() - 2.0 * 1.0f64.hypot(1.01)).abs() < 1e-9);
    assert_eq!(a.tie, vec![0, 1]);
}

#[test]
fn a_gap_outside_the_pair_tolerance_is_not_a_tie() {
    let a = assess_ladder(
        &[spread_rung(100.0, 0.01), spread_rung(110.0, 0.01)],
        3,
        &Gates::default(),
        false,
    );
    assert!((a.verdicts[1].tolerance.unwrap() - 2.0 * 1.0f64.hypot(1.1)).abs() < 1e-9);
    assert_eq!(a.tie, vec![0]);
    assert_eq!(a.runner_up, Some(1));
}

fn two_speed_rung(p0: f64, e0: f64, p1: f64, e1: f64) -> Vec<RepeatResult> {
    [-1.0, 0.0, 1.0]
        .iter()
        .map(|k| {
            rep(
                0.2,
                &[Some(p0 * (1.0 + k * e0)), Some(p1 * (1.0 + k * e1))],
                Some(1.0),
            )
        })
        .collect()
}

#[test]
fn noise_at_a_speed_that_decides_neither_rung_leaves_the_tie_alone() {
    let quiet = vec![
        two_speed_rung(40.0, 0.0, 100.0, 0.01),
        two_speed_rung(40.0, 0.0, 104.0, 0.01),
    ];
    let noisy = vec![
        two_speed_rung(40.0, 0.5, 100.0, 0.01),
        two_speed_rung(40.0, 0.5, 104.0, 0.01),
    ];
    let a = assess_ladder(&quiet, 3, &Gates::default(), false);
    let b = assess_ladder(&noisy, 3, &Gates::default(), false);
    assert_eq!(b.verdicts[1].deciding_speed, Some(1));
    assert_eq!(a.verdicts[1].tolerance, b.verdicts[1].tolerance);
    assert_eq!(a.tie, b.tie);
    assert!(b.spreads[0].unwrap() > a.spreads[0].unwrap());
}

#[test]
fn noise_at_a_deciding_speed_widens_the_tolerance() {
    let a = assess_ladder(
        &[spread_rung(100.0, 0.01), spread_rung(104.0, 0.01)],
        3,
        &Gates::default(),
        false,
    );
    let b = assess_ladder(
        &[spread_rung(100.0, 0.03), spread_rung(104.0, 0.03)],
        3,
        &Gates::default(),
        false,
    );
    assert!(b.verdicts[1].tolerance.unwrap() > a.verdicts[1].tolerance.unwrap());
    assert_eq!(a.tie, vec![0]);
    assert_eq!(b.tie, vec![0, 1]);
}

#[test]
fn rungs_decided_by_different_speeds_use_each_speeds_spread() {
    let r0 = spread_rung(100.0, 0.03);
    let r1: Vec<RepeatResult> = (0..3)
        .map(|_| rep(0.2, &[Some(103.0), Some(50.0)], Some(1.0)))
        .collect();
    let a = assess_ladder(&[r0, r1], 3, &Gates::default(), false);
    let s1 = a.spreads[1].unwrap();
    let expected = pair_tolerance(s1, 100.0, a.spreads[0].unwrap(), 103.0);
    assert!((a.verdicts[1].tolerance.unwrap() - expected).abs() < 1e-12);
}

#[test]
fn a_tie_set_of_three_is_ascending_and_starts_at_the_best() {
    let a = assess_ladder(
        &[
            spread_rung(100.5, 0.02),
            spread_rung(100.0, 0.02),
            spread_rung(101.0, 0.02),
        ],
        3,
        &Gates::default(),
        false,
    );
    assert_eq!(a.best, Some(1));
    assert_eq!(a.tie, vec![0, 1, 2]);
}

#[test]
fn one_repeat_has_no_spread_no_tolerance_and_no_tie() {
    let rungs = vec![
        rung(0.2, [40.0, 100.0], 1.0, 1),
        rung(0.2, [40.0, 100.1], 1.0, 1),
    ];
    let a = assess_ladder(&rungs, 1, &Gates::default(), false);
    assert_eq!(a.spreads, vec![None, None]);
    assert_eq!(a.tie, vec![0]);
    assert!((a.verdicts[1].gap.unwrap() - 0.1).abs() < 1e-9);
    assert_eq!(a.verdicts[1].tolerance, None);
    assert_eq!(a.runner_up, Some(1));
}

#[test]
fn identical_repeats_give_zero_spread_and_tie_only_on_equal_scores() {
    let a = assess_ladder(
        &[
            rung(0.2, [40.0, 100.0], 1.0, 3),
            rung(0.2, [40.0, 100.0], 1.0, 3),
        ],
        3,
        &Gates::default(),
        false,
    );
    assert_eq!(a.spreads[1], Some(0.0));
    assert_eq!(a.tie, vec![0, 1]);
    let b = assess_ladder(
        &[
            rung(0.2, [40.0, 100.0], 1.0, 3),
            rung(0.2, [40.0, 100.001], 1.0, 3),
        ],
        3,
        &Gates::default(),
        false,
    );
    assert_eq!(b.tie, vec![0]);
}

#[test]
fn no_eligible_rung_gives_no_spread_no_best_and_an_empty_tie_set() {
    let rungs = vec![
        rung(0.06, [40.0, 100.0], 1.0, 3),
        rung(0.07, [40.0, 101.0], 1.0, 3),
    ];
    let a = assess_ladder(&rungs, 3, &Gates::default(), false);
    assert_eq!(a.best, None);
    assert_eq!(a.runner_up, None);
    assert_eq!(a.tie, Vec::<usize>::new());
    assert_eq!(a.spreads, vec![None, None]);
}

#[test]
fn ineligible_rungs_enter_neither_the_spread_nor_the_tie_set() {
    let marginal: Vec<RepeatResult> = [-30.0, 0.0, 30.0]
        .iter()
        .map(|o| rep(0.06, &[Some(40.0), Some(60.0 + o)], Some(1.0)))
        .collect();
    let rungs = vec![spread_rung(100.0, 0.01), marginal, spread_rung(101.0, 0.01)];
    let a = assess_ladder(&rungs, 3, &Gates::default(), false);
    assert_eq!(a.verdicts[1].score, None);
    assert_eq!(a.tie, vec![0, 2]);
    let b = assess_ladder(&rungs, 3, &Gates::default(), true);
    assert_eq!(b.best, Some(1));
    assert!(b.spreads[1].unwrap() > a.spreads[1].unwrap());
}

#[test]
fn drift_is_judged_against_twice_the_speeds_spread() {
    assert_eq!(
        drift_verdict(Some(100.0), Some(103.0), Some(0.02)),
        DriftVerdict::Steady
    );
    assert_eq!(
        drift_verdict(Some(100.0), Some(104.0), Some(0.02)),
        DriftVerdict::Steady
    );
    assert_eq!(
        drift_verdict(Some(100.0), Some(104.1), Some(0.02)),
        DriftVerdict::Drifted
    );
    assert_eq!(
        drift_verdict(Some(100.0), Some(95.9), Some(0.02)),
        DriftVerdict::Drifted
    );
    assert_eq!(
        drift_verdict(Some(100.0), Some(100.0), Some(0.0)),
        DriftVerdict::Steady
    );
}

#[test]
fn drift_without_data_is_unassessed_not_steady() {
    assert_eq!(
        drift_verdict(None, Some(100.0), Some(0.02)),
        DriftVerdict::Unassessed
    );
    assert_eq!(
        drift_verdict(Some(100.0), None, Some(0.02)),
        DriftVerdict::Unassessed
    );
    assert_eq!(
        drift_verdict(Some(100.0), Some(100.0), None),
        DriftVerdict::Unassessed
    );
}
