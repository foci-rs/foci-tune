use foci_tune::model::StandstillRecord;
use foci_tune_analysis::standstill::{RestMetrics, StandstillMetrics};

fn rest(index: usize, torque_sd: f64, rate: f64, large: f64) -> RestMetrics {
    RestMetrics {
        index,
        samples: 300,
        torque_sd,
        position_pp_um: 1.0,
        transitions_per_s: rate,
        large_transitions_per_s: large,
    }
}

#[test]
fn a_standstill_run_without_a_qualifying_rest_has_no_worst_torque_sd() {
    let none = StandstillMetrics {
        rests: Vec::new(),
        worst_torque_sd: 0.0,
        worst_large_transitions_per_s: 0.0,
    };

    assert_eq!(StandstillRecord::from(&none).worst_torque_sd, None);
}

#[test]
fn the_worst_torque_sd_is_kept_when_rests_qualified() {
    let some = StandstillMetrics {
        rests: vec![rest(0, 2.0, 0.0, 0.0), rest(1, 5.0, 0.0, 0.0)],
        worst_torque_sd: 5.0,
        worst_large_transitions_per_s: 0.0,
    };

    assert_eq!(StandstillRecord::from(&some).worst_torque_sd, Some(5.0));
}

#[test]
fn the_rates_are_recorded_per_rest_and_the_worst_large_rate_per_run() {
    let some = StandstillMetrics {
        rests: vec![
            rest(0, 2.0, 950.0, 0.0),
            rest(1, 5.0, 4.0, 4.0),
            rest(2, 0.0, 0.0, 0.0),
        ],
        worst_torque_sd: 5.0,
        worst_large_transitions_per_s: 4.0,
    };

    let r = StandstillRecord::from(&some);

    assert_eq!(r.rests[0].transitions_per_s, Some(950.0));
    assert_eq!(r.rests[0].large_transitions_per_s, Some(0.0));
    assert_eq!(r.rests[1].large_transitions_per_s, Some(4.0));
    assert_eq!(r.worst_large_transitions_per_s, Some(4.0));
}

#[test]
fn a_run_without_a_qualifying_rest_has_no_worst_large_rate() {
    let none = StandstillMetrics {
        rests: Vec::new(),
        worst_torque_sd: 0.0,
        worst_large_transitions_per_s: 0.0,
    };

    assert_eq!(
        StandstillRecord::from(&none).worst_large_transitions_per_s,
        None
    );
}

#[test]
fn a_rest_record_without_rate_fields_loads_with_absent_rates() {
    let v = serde_json::json!({
        "rests": [{"index": 0, "samples": 2750, "torque_sd": 0.49, "position_pp_um": 4.88}],
        "worst_torque_sd": 0.49
    });

    let r: StandstillRecord = serde_json::from_value(v).unwrap();

    assert_eq!(r.rests[0].transitions_per_s, None);
    assert_eq!(r.rests[0].large_transitions_per_s, None);
    assert_eq!(r.worst_large_transitions_per_s, None);
}
