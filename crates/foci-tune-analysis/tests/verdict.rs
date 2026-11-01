use foci_tune_analysis::ladder::RungOutcome;
use foci_tune_analysis::verdict::{
    Gates, Halt, InadmissibleReason, RepeatResult, UnscoredReason, assess_ladder,
};

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
fn score_is_worst_speed_ratio() {
    let rungs = vec![
        rung(0.2, [85.0, 100.0], 1.0, 3),
        rung(0.2, [70.0, 145.0], 1.0, 3),
        rung(0.2, [80.0, 110.0], 1.0, 3),
    ];

    let a = assess_ladder(&rungs, 3, &Gates::default(), false);

    let scores: Vec<f64> = a.verdicts.iter().map(|v| v.score.unwrap()).collect();
    assert!((scores[0] - 85.0 / 70.0).abs() < 1e-12);
    assert!((scores[1] - 1.45).abs() < 1e-12);
    assert!((scores[2] - 80.0 / 70.0).abs() < 1e-12);
    let best = scores
        .iter()
        .enumerate()
        .min_by(|x, y| x.1.total_cmp(y.1))
        .unwrap()
        .0;
    assert_eq!(best, 2);
    let mean_ratio = |i: usize| -> f64 {
        let p = [[85.0, 100.0], [70.0, 145.0], [80.0, 110.0]][i];
        (p[0] / 70.0 + p[1] / 100.0) / 2.0
    };
    let mean_best = (0..3)
        .min_by(|x, y| mean_ratio(*x).total_cmp(&mean_ratio(*y)))
        .unwrap();
    assert_ne!(best, mean_best);
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
    assert_eq!(a.verdicts[0].score, Some(1.0));
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
    assert_eq!(excluded.verdicts[0].score, Some(1.0));

    let included = assess_ladder(&rungs, 3, &Gates::default(), true);
    assert!(included.verdicts[1].marginal);
    assert_eq!(included.verdicts[1].score, Some(1.0));
    assert_eq!(included.verdicts[0].score, Some(2.0));
}

#[test]
fn standstill_gate_marks_inadmissible_without_halting() {
    let rungs = vec![
        rung(0.2, [80.0, 80.0], 1.0, 3),
        rung(0.2, [70.0, 70.0], 2.5, 3),
        rung(0.2, [75.0, 75.0], 1.5, 3),
    ];

    let a = assess_ladder(&rungs, 3, &Gates::default(), false);

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
    assert_eq!(a.verdicts[0].score, Some(1.0));
}

#[test]
fn missing_standstill_on_later_rung_is_unscored() {
    let mut r1 = rung(0.2, [70.0, 70.0], 1.0, 3);
    r1[0].worst_torque_sd = None;
    let rungs = vec![rung(0.2, [80.0, 80.0], 1.0, 3), r1];

    let a = assess_ladder(&rungs, 3, &Gates::default(), false);

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

    let a = assess_ladder(&rungs, 3, &Gates::default(), false);

    assert_eq!(a.halt, Some((0, Halt::StandstillBaselineUnassessable)));
    assert_eq!(a.verdicts[0].score, None);
    assert_eq!(a.verdicts[1], Default::default());
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
    assert_eq!(a.verdicts[0].score, Some(1.0));
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
