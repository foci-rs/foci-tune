use foci_tune_analysis::signals::ChannelSeries;
use foci_tune_analysis::standstill::standstill_metrics;
use foci_tune_analysis::units::Regime;

fn regime() -> Regime {
    Regime {
        motor_rotation_distance_mm: 40.0,
        encoder_counts_per_rev: 4096,
        pwm_hz: 50_000,
        clock_freq: 260_000_000,
    }
}

fn span(s: &mut ChannelSeries, pos: i32, n: usize, torque_sd: i32, err_amp: f64, spike: bool) {
    for k in 0..n {
        let sign = if k % 2 == 0 { 1 } else { -1 };
        let hot = spike && k == 10;
        s.step_position.push(pos);
        s.torque.push(if hot { 9000 } else { sign * torque_sd });
        s.position_error_um.push(if hot {
            500.0
        } else {
            f64::from(sign) * err_amp
        });
    }
}

fn capture(spike_in_settle: bool) -> ChannelSeries {
    let mut s = ChannelSeries::default();
    span(&mut s, 100, 700, 0, 1.0, spike_in_settle);
    span(&mut s, 200, 300, 40, 40.0, false);
    span(&mut s, 300, 800, 2, 3.0, spike_in_settle);
    span(&mut s, 400, 900, 5, 0.5, spike_in_settle);
    s
}

#[test]
fn rests_are_delimited_by_constant_step_position_and_length() {
    let m = standstill_metrics(&capture(false), &regime(), 500, 200);

    assert_eq!(m.rests.len(), 3);
    assert_eq!(
        m.rests.iter().map(|r| r.index).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(
        m.rests.iter().map(|r| r.samples).collect::<Vec<_>>(),
        vec![200, 300, 400]
    );
}

#[test]
fn torque_sd_is_population_sd_and_worst_is_the_max() {
    let m = standstill_metrics(&capture(false), &regime(), 500, 200);

    assert!((m.rests[0].torque_sd - 0.0).abs() < 1e-12);
    assert!((m.rests[1].torque_sd - 2.0).abs() < 1e-12);
    assert!((m.rests[2].torque_sd - 5.0).abs() < 1e-12);
    assert!((m.worst_torque_sd - 5.0).abs() < 1e-12);
    assert!((m.rests[1].position_pp_um - 6.0).abs() < 1e-12);
    assert!((m.rests[2].position_pp_um - 1.0).abs() < 1e-12);
}

#[test]
fn settle_samples_are_excluded_from_metrics() {
    let clean = standstill_metrics(&capture(false), &regime(), 500, 200);
    let spiked = standstill_metrics(&capture(true), &regime(), 500, 200);

    assert_eq!(clean, spiked);
}

#[test]
fn empty_series_has_no_rests() {
    let m = standstill_metrics(&ChannelSeries::default(), &regime(), 500, 200);

    assert!(m.rests.is_empty());
    assert_eq!(m.worst_torque_sd, 0.0);
}
