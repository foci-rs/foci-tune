mod fake_moonraker;
mod rig;

use foci_tune::model::RunState;
use foci_tune::stimulus::RunKind;
use foci_tune::tune::repeat_result;
use foci_tune_analysis::ladder::RungOutcome;
use rig::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeat_result_drops_not_comparable_and_missing_runs() {
    let rig = rig().await;
    rig.a.sim(|s| {
        s.lag_by_velocity_p.insert(841, 40);
    });
    let probe_run = rig.run_once(probe()).await;
    let tracking = |speed| RunKind::Tracking {
        speeds_mm_s: vec![speed],
        strokes: 1,
    };
    let slow = rig.run_once(tracking(50.0)).await;
    let mut fast = rig.run_once(tracking(120.0)).await;
    fast.state = RunState::NotComparable("channel 1 moved".into());
    let standstill = rig
        .run_once(RunKind::Standstill {
            rests: 2,
            dwell_s: 0.2,
        })
        .await;

    let result = repeat_result(
        Some(&probe_run),
        &[Some(&slow), Some(&fast), None],
        Some(&standstill),
    );

    assert!(matches!(result.zeta, RungOutcome::Assessed(z) if (z - 0.2).abs() < 0.02));
    assert!(result.p99_by_speed[0].is_some_and(|p| p > 0.0));
    assert_eq!(result.p99_by_speed[1..], [None, None]);
    assert!(result.worst_torque_sd.is_some());

    let mut unassessable = probe_run.clone();
    unassessable.state = RunState::NotComparable("12 edges".into());
    let result = repeat_result(Some(&unassessable), &[Some(&slow)], None);
    assert_eq!(result.zeta, RungOutcome::Unassessable);
    assert_eq!(result.worst_torque_sd, None);
}
