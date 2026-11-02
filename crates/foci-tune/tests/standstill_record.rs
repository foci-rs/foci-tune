use foci_tune::model::StandstillRecord;
use foci_tune_analysis::standstill::{RestMetrics, StandstillMetrics};

fn rest(index: usize, torque_sd: f64) -> RestMetrics {
    RestMetrics {
        index,
        samples: 300,
        torque_sd,
        position_pp_um: 1.0,
    }
}

#[test]
fn a_standstill_run_without_a_qualifying_rest_has_no_worst_torque_sd() {
    let none = StandstillMetrics {
        rests: Vec::new(),
        worst_torque_sd: 0.0,
    };

    assert_eq!(StandstillRecord::from(&none).worst_torque_sd, None);
}

#[test]
fn the_worst_torque_sd_is_kept_when_rests_qualified() {
    let some = StandstillMetrics {
        rests: vec![rest(0, 2.0), rest(1, 5.0)],
        worst_torque_sd: 5.0,
    };

    assert_eq!(StandstillRecord::from(&some).worst_torque_sd, Some(5.0));
}
