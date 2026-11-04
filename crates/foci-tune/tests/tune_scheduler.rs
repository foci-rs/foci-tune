mod fake_moonraker;
mod rig;

use std::sync::Arc;
use std::time::{Duration, Instant};

use foci_tune::engine::{EngineEvent, LadderRejected, Precondition};
use foci_tune::model::{DampingOutcome, Filters, FitOutcome, Gains, Run, RunId, RunState};
use foci_tune::moonraker::MoonrakerClient;
use foci_tune::stimulus::RunKind;
use foci_tune::tune::{
    Choice, PauseDecision, ProbeSettings, Seed, StandstillSettings, StepRequest, Tune, TuneError,
    TuneId, TuneScheduler, TuneSetup, TuneState, repeat_result,
};
use foci_tune_analysis::ladder::{LadderAxis, RungOutcome, to_raw};
use foci_tune_analysis::recommend::{Extension, Recommendation};
use foci_tune_analysis::rungs::{SeedRefused, rung_values};
use foci_tune_analysis::verdict::{
    DriftVerdict, Gates, Halt, InadmissibleReason, Pause, UnscoredReason, Waivers, assess_ladder_at,
};
use rig::*;
use serde_json::json;
use tokio::sync::Notify;

const BASE: (Gains, Filters) = (
    Gains {
        velocity_p: 841,
        velocity_i: 412,
        position_p: 141,
        position_i: 0,
    },
    Filters {
        velocity_hz: 500,
        torque_hz: 1000,
        position_hz: 500,
        flux_hz: 1000,
    },
);

fn setup(speeds: &[f64]) -> TuneSetup {
    TuneSetup {
        stepper: "stepper_x".into(),
        speeds_mm_s: speeds.to_vec(),
        pattern: pattern(),
        probe: ProbeSettings {
            speed_mm_s: 195.0,
            high_gain: 1.3,
            segments_between_toggles: 3,
            strokes: 2,
        },
        tracking_strokes: 1,
        standstill: StandstillSettings {
            rests: 2,
            dwell_s: 0.2,
        },
        gates: Gates::default(),
        include_marginal: false,
    }
}

fn gated_setup() -> TuneSetup {
    let mut s = setup(&[50.0]);
    s.gates.standstill_factor = Some(2.0);
    s
}

fn scheduler(rig: &Rig) -> TuneScheduler<MoonrakerClient> {
    TuneScheduler::new(rig.engine.clone(), rig.store.clone())
}

fn step(lower_k: i32, upper_k: i32, repeats: u32) -> StepRequest {
    StepRequest {
        seed: Seed::Live,
        repeats,
        lower_k,
        upper_k,
        octave_divisions: 3,
    }
}

async fn wait_state(rig: &Rig, id: &TuneId, state: TuneState) -> Tune {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let tune = rig.store.read_tune(id).unwrap();
        if tune.state == state && rig.engine.is_idle() {
            return tune;
        }
        assert!(
            Instant::now() < deadline,
            "tune stuck in {:?}, wanted {state:?}",
            tune.state
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn run_step(rig: &Rig, sched: &TuneScheduler<MoonrakerClient>, req: StepRequest) -> Tune {
    let id = sched.create(setup(&[50.0])).await.unwrap();
    sched.prepare_step(&id, req).await.unwrap();
    sched.confirm_step(&id).await.unwrap();
    wait_state(rig, &id, TuneState::AwaitingChoice).await
}

fn read(rig: &Rig, id: &RunId) -> Run {
    let session = rig.store.find_run(id).unwrap().unwrap();
    rig.store.read_run(&session, id).unwrap()
}

fn written_velocity_p(rig: &Rig) -> Vec<i32> {
    rig.scripts()
        .iter()
        .filter(|s| s.starts_with("FOCI_SET_GAINS"))
        .map(|s| (float(s, "VELOCITY_P") * 256.0).round() as i32)
        .collect()
}

fn refused<T: std::fmt::Debug>(a: Result<T, TuneError>, b: Result<T, TuneError>) -> TuneError {
    match (a, b) {
        (Ok(_), Err(e)) | (Err(e), Ok(_)) => e,
        other => panic!("expected exactly one success: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_tune_is_ready_and_confirm_is_refused_outside_announced() {
    let rig = rig().await;
    let sched = scheduler(&rig);

    let id = sched.create(setup(&[50.0])).await.unwrap();

    assert_eq!(rig.store.read_tune(&id).unwrap().state, TuneState::Ready);
    assert_eq!(sched.confirm_step(&id).await, Err(TuneError::NotAnnounced));
    assert_eq!(rig.stimuli(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn second_tune_for_a_stepper_is_refused_across_restart() {
    let rig = rig().await;
    let first = scheduler(&rig).create(setup(&[50.0])).await.unwrap();

    let restarted = scheduler(&rig);
    let mut blocking = rig.store.read_tune(&first).unwrap();
    for state in [
        TuneState::Ready,
        TuneState::RestorePending,
        TuneState::RestoreFailed("no response".into()),
    ] {
        blocking.state = state;
        rig.store.write_tune(&blocking).unwrap();
        let refused = restarted.create(setup(&[50.0])).await;
        assert!(
            matches!(&refused, Err(TuneError::StepperBusy(t)) if *t == first),
            "{refused:?}"
        );
    }

    blocking.state = TuneState::Done;
    rig.store.write_tune(&blocking).unwrap();
    let second = restarted.create(setup(&[50.0])).await.unwrap();
    let mut abandoned = rig.store.read_tune(&second).unwrap();
    abandoned.state = TuneState::Abandoned;
    rig.store.write_tune(&abandoned).unwrap();
    assert!(restarted.create(setup(&[50.0])).await.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_snapshots_the_live_control_state() {
    let rig = rig().await;
    let sched = scheduler(&rig);

    let tune = run_step(&rig, &sched, step(-1, 0, 1)).await;

    assert_eq!(tune.snapshot, BASE);
    assert!(written_velocity_p(&rig).contains(&668));

    let failing = rig_with(Options::default()).await;
    failing.a.sim(|s| s.baseline = false);
    let err = scheduler(&failing).create(setup(&[50.0])).await;
    assert!(matches!(err, Err(TuneError::NoControlState(_))), "{err:?}");
    assert!(failing.store.scan_tunes().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_transitions_admit_exactly_one() {
    let rig = rig().await;
    let sched = scheduler(&rig);

    let (a, b) = tokio::join!(sched.create(setup(&[50.0])), sched.create(setup(&[50.0])));
    let id = match (&a, &b) {
        (Ok(id), _) | (_, Ok(id)) => id.clone(),
        _ => panic!("no create succeeded: {a:?} {b:?}"),
    };
    assert!(matches!(refused(a, b), TuneError::StepperBusy(_)));

    let (a, b) = tokio::join!(
        sched.prepare_step(&id, step(0, 0, 1)),
        sched.prepare_step(&id, step(0, 0, 1))
    );
    assert!(matches!(refused(a, b), TuneError::OutOfOrder(_)));

    let (a, b) = tokio::join!(sched.confirm_step(&id), sched.confirm_step(&id));
    assert_eq!(refused(a, b), TuneError::NotAnnounced);

    let tune = wait_state(&rig, &id, TuneState::AwaitingChoice).await;
    assert_eq!(tune.steps.len(), 1);
    assert_eq!(tune.steps[0].rungs[0].run_ids.len(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn steps_follow_the_fixed_order() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[50.0])).await.unwrap();

    let plan = sched.prepare_step(&id, step(0, 0, 1)).await.unwrap();
    assert_eq!(plan.axis, LadderAxis::VelocityP);
    let duplicate = sched.prepare_step(&id, step(0, 0, 1)).await;
    assert!(matches!(duplicate, Err(TuneError::OutOfOrder(_))));

    sched.confirm_step(&id).await.unwrap();
    let active = sched.prepare_step(&id, step(0, 0, 1)).await;
    assert!(matches!(active, Err(TuneError::OutOfOrder(_))));

    let mut tune = wait_state(&rig, &id, TuneState::AwaitingChoice).await;
    let unchosen = sched.prepare_step(&id, step(0, 0, 1)).await;
    assert!(matches!(unchosen, Err(TuneError::OutOfOrder(_))));

    tune.state = TuneState::Ready;
    rig.store.write_tune(&tune).unwrap();
    let unchosen = sched.prepare_step(&id, step(0, 0, 1)).await;
    assert!(matches!(unchosen, Err(TuneError::OutOfOrder(_))));

    tune.steps[0].chosen = Some(841);
    rig.store.write_tune(&tune).unwrap();
    let plan = sched.prepare_step(&id, step(0, 0, 1)).await.unwrap();
    assert_eq!(plan.axis, LadderAxis::VelocityICornerHz);

    let mut tune = rig.store.read_tune(&id).unwrap();
    tune.steps[1].chosen = Some(412);
    tune.state = TuneState::Ready;
    rig.store.write_tune(&tune).unwrap();
    let plan = sched.prepare_step(&id, step(0, 0, 1)).await.unwrap();
    assert_eq!(plan.axis, LadderAxis::PositionP);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_seed_reads_fresh_state_at_step_start() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[50.0])).await.unwrap();

    rig.a.sim(|s| s.baseline = false);
    let err = sched.prepare_step(&id, step(-1, 1, 1)).await;
    assert!(matches!(err, Err(TuneError::NoControlState(_))), "{err:?}");
    assert_eq!(rig.store.read_tune(&id).unwrap().state, TuneState::Ready);

    rig.a.sim(|s| {
        s.baseline = true;
        s.channels[0].gains[0] = 1000;
    });
    let plan = sched.prepare_step(&id, step(-1, 1, 1)).await.unwrap();

    let raws: Vec<i32> = plan.rungs.values.iter().map(|v| v.raw).collect();
    assert_eq!(raws, vec![794, 1000, 1260]);
    assert_eq!(rig.store.read_tune(&id).unwrap().steps[0].seed_raw, 1000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_run_before_confirm() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[50.0])).await.unwrap();

    let plan = sched.prepare_step(&id, step(-1, 1, 1)).await.unwrap();

    assert_eq!(
        rig.store.read_tune(&id).unwrap().state,
        TuneState::Announced
    );
    let status = rig.engine.status();
    assert!(status.current.is_none() && status.queue.is_empty());
    assert_eq!(rig.stimuli(), 0);
    assert_eq!(plan.run_count, 12);
    assert!(plan.announcement.contains("stepper_x"));

    sched.confirm_step(&id).await.unwrap();
    wait_state(&rig, &id, TuneState::AwaitingChoice).await;
    assert_eq!(rig.stimuli(), 12);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_locked_stepper_refuses_confirm() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[50.0])).await.unwrap();
    sched.prepare_step(&id, step(0, 0, 1)).await.unwrap();

    rig.engine
        .lock_stepper("stepper_x", "restore failed".into(), BASE)
        .unwrap();
    let err = sched.confirm_step(&id).await;

    assert!(
        matches!(
            err,
            Err(TuneError::Precondition(Precondition::TuneLocked(_)))
        ),
        "{err:?}"
    );
    assert_eq!(
        rig.store.read_tune(&id).unwrap().state,
        TuneState::Announced
    );
    assert_eq!(rig.stimuli(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rungs_run_ascending_one_at_a_time() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let speeds = [50.0, 120.0];
    let id = sched.create(setup(&speeds)).await.unwrap();
    sched.prepare_step(&id, step(-1, 1, 2)).await.unwrap();
    let mut events = rig.engine.events();

    sched.confirm_step(&id).await.unwrap();
    let tune = wait_state(&rig, &id, TuneState::AwaitingChoice).await;

    let step = &tune.steps[0];
    let raws: Vec<i32> = step.rungs.iter().map(|r| r.raw).collect();
    assert_eq!(raws, vec![668, 841, 1060]);
    for rung in &step.rungs {
        assert_eq!(rung.run_ids.len(), 8);
        for (n, run_id) in rung.run_ids.iter().enumerate() {
            let run = read(&rig, run_id);
            assert_eq!(run.request.gains.velocity_p, rung.raw);
            assert_eq!(run.request.gains.velocity_i, BASE.0.velocity_i);
            assert_eq!(run.request.filters, BASE.1);
            let session = match (n % 4, &run.request.kind) {
                (0, RunKind::Probe { .. }) => &step.sessions.probe,
                (1, RunKind::Tracking { speeds_mm_s, .. }) if speeds_mm_s == &[50.0] => {
                    &step.sessions.tracking
                }
                (2, RunKind::Tracking { speeds_mm_s, .. }) if speeds_mm_s == &[120.0] => {
                    &step.sessions.tracking
                }
                (3, RunKind::Standstill { .. }) => &step.sessions.standstill,
                (k, kind) => panic!("run {k} of a repeat is {kind:?}"),
            };
            assert_eq!(&run.request.session, session);
        }
    }
    let mut expected = Vec::new();
    for raw in [668, 841, 1060] {
        expected.extend([raw; 8]);
    }
    expected.extend([841; 4]);
    assert_eq!(written_velocity_p(&rig), expected);
    let mut longest_queue = 0;
    while let Ok(event) = events.try_recv() {
        if let EngineEvent::Queue(q) = event {
            longest_queue = longest_queue.max(q.len());
        }
    }
    assert_eq!(longest_queue, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn damping_halt_leaves_higher_rungs_unqueued() {
    let rig = rig().await;
    rig.a.sim(|s| {
        s.zeta_by_velocity_p.insert(841, 0.03);
    });
    let sched = scheduler(&rig);

    let tune = run_step(&rig, &sched, step(-1, 1, 1)).await;

    let step = &tune.steps[0];
    let assessment = step.assessment.as_ref().unwrap();
    assert!(matches!(
        assessment.halt,
        Some((1, Halt::BelowFloor { .. }))
    ));
    assert!(step.rungs[1].skipped.is_none());
    let skipped = step.rungs[2].skipped.as_deref().unwrap();
    assert!(skipped.contains("floor"), "{skipped}");
    assert!(step.rungs[2].run_ids.is_empty());
    assert!(!written_velocity_p(&rig).contains(&1060));
    assert!(step.drift_rung.is_none());
    assert!(step.failed.is_none());

    let rig = rig_with(Options::default()).await;
    rig.a.sim(|s| {
        let dropped = Profile {
            drop_flag: true,
            ..Profile::default()
        };
        s.profile_by_script.push(("G4 P200".into(), dropped));
    });
    let sched = scheduler(&rig);
    let id = sched.create(gated_setup()).await.unwrap();
    sched.prepare_step(&id, crate::step(0, 1, 1)).await.unwrap();
    sched.confirm_step(&id).await.unwrap();

    let tune = wait_state(&rig, &id, TuneState::Paused).await;

    let step = &tune.steps[0];
    assert_eq!(
        step.assessment.as_ref().unwrap().pause,
        Some((0, Pause::StandstillBaselineUnassessable))
    );
    assert!(step.rungs[1].run_ids.is_empty());
    assert!(step.drift_rung.is_none());
    assert_eq!(rig.stimuli(), 3);
}

async fn wait_stimuli(rig: &Rig, n: usize) {
    let deadline = Instant::now() + WAIT;
    while rig.stimuli() < n {
        assert!(Instant::now() < deadline, "stimulus {n} not sent");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_submit_after_the_tune_leaves_running() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[50.0])).await.unwrap();
    sched.prepare_step(&id, step(-1, 1, 1)).await.unwrap();
    let gate = rig.hold_stimulus();
    sched.confirm_step(&id).await.unwrap();
    wait_stimuli(&rig, 1).await;

    let mut left = rig.store.read_tune(&id).unwrap();
    left.state = TuneState::Abandoned;
    rig.store.write_tune(&left).unwrap();
    rig.a.sim(|s| s.gate = None);
    gate.notify_one();
    sched.wait_idle(&id).await;

    assert_eq!(rig.stimuli(), 1);
    assert!(rig.engine.is_idle());
    assert_eq!(rig.store.read_tune(&id).unwrap(), left);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scheduler_error_ends_the_step_awaiting_a_choice() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[50.0])).await.unwrap();
    sched.prepare_step(&id, step(-1, 1, 1)).await.unwrap();
    let gate = rig.hold_stimulus();
    sched.confirm_step(&id).await.unwrap();
    wait_stimuli(&rig, 1).await;

    let mut broken = rig.store.read_tune(&id).unwrap();
    broken.steps[0].rungs.clear();
    rig.store.write_tune(&broken).unwrap();
    rig.a.sim(|s| s.gate = None);
    gate.notify_one();
    sched.wait_idle(&id).await;

    let tune = rig.store.read_tune(&id).unwrap();
    assert_eq!(tune.state, TuneState::AwaitingChoice);
    let failed = tune.steps[0].failed.as_deref().unwrap();
    assert!(failed.contains("rung 0"), "{failed}");
    assert_eq!(rig.stimuli(), 1);
    assert!(
        sched.create(setup(&[50.0])).await.is_err(),
        "the tune still holds the stepper until it is chosen or abandoned"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_run_stops_the_step() {
    let rig = rig().await;
    rig.a.sim(|s| s.unconfirmed_velocity_p.push(841));
    let sched = scheduler(&rig);

    let tune = run_step(&rig, &sched, step(-1, 1, 1)).await;

    let step = &tune.steps[0];
    let failed = step.failed.as_deref().unwrap();
    assert!(failed.contains("failed"), "{failed}");
    assert_eq!(step.rungs[0].repeat_values.len(), 1);
    assert_eq!(step.rungs[1].run_ids.len(), 1);
    assert!(step.rungs[2].skipped.as_deref().unwrap().contains("failed"));
    assert!(step.rungs[2].run_ids.is_empty());
    assert!(!written_velocity_p(&rig).contains(&1060));
    assert!(step.drift_rung.is_none());
    assert!(step.ended_unix_ms.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seed_rung_is_rerun_at_the_end_for_drift() {
    let rig = rig().await;
    rig.a.sim(|s| {
        s.lag_by_velocity_p.insert(668, 60);
        s.lag_by_velocity_p.insert(841, 40);
        s.lag_by_velocity_p.insert(1060, 50);
    });
    let sched = scheduler(&rig);

    let tune = run_step(&rig, &sched, step(-1, 1, 1)).await;

    let step = &tune.steps[0];
    let drift = step.drift_rung.as_ref().unwrap();
    assert_eq!(drift.raw, 841);
    assert_eq!(drift.run_ids.len(), 3);
    assert_eq!(drift.repeat_values.len(), 1);
    let metrics: Vec<&str> = step.drift.iter().map(|d| d.metric.as_str()).collect();
    assert_eq!(metrics, vec!["zeta", "p99 50 mm/s", "worst torque sd"]);
    assert!(
        step.drift
            .iter()
            .all(|d| d.first.is_some() && d.rerun.is_some())
    );
    let p99 = step
        .drift
        .iter()
        .find(|d| d.metric == "p99 50 mm/s")
        .unwrap();
    assert_eq!(p99.verdict, Some(DriftVerdict::Unassessed));
    assert!(
        step.drift
            .iter()
            .filter(|d| !d.metric.starts_with("p99"))
            .all(|d| d.verdict.is_none())
    );
    let main: Vec<_> = step.rungs.iter().map(|r| r.repeat_values.clone()).collect();
    assert_eq!(
        step.assessment.as_ref().unwrap(),
        &assess_ladder_at(
            &main,
            1,
            &[50.0],
            &Gates::default(),
            false,
            &Waivers::default()
        )
    );
}

fn three_lags(rig: &Rig) {
    rig.a.sim(|s| {
        s.lag_by_velocity_p.insert(668, 60);
        s.lag_by_velocity_p.insert(841, 40);
        s.lag_by_velocity_p.insert(1060, 50);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steady_seed_rerun_is_recorded_as_steady_with_the_tie_fields() {
    let rig = rig().await;
    three_lags(&rig);
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[120.0, 50.0])).await.unwrap();
    sched.prepare_step(&id, step(-1, 1, 2)).await.unwrap();
    let gate = rig.hold_stimulus();
    sched.confirm_step(&id).await.unwrap();
    for n in 1..=24 {
        wait_stimuli(&rig, n).await;
        gate.notify_one();
    }
    wait_stimuli(&rig, 25).await;
    release(&rig, &gate);
    let done = wait_state(&rig, &id, TuneState::AwaitingChoice).await;

    let step = &done.steps[0];
    let a = step.assessment.as_ref().unwrap();
    assert_eq!(a.best, Some(1));
    assert_eq!(a.tie, vec![1]);
    assert_eq!(a.spreads.len(), 2);
    assert!(a.spreads.iter().all(|s| *s == Some(0.0)));
    for v in a.verdicts.iter().filter(|v| v.score.is_some()) {
        assert_eq!(v.deciding_speed, Some(0));
    }
    assert_eq!(a.verdicts[1].gap, Some(0.0));
    assert_eq!(a.verdicts[1].tolerance, None);
    let expected = a.medians[2].p99_by_speed[0].unwrap() - a.medians[1].p99_by_speed[0].unwrap();
    assert!((a.verdicts[2].gap.unwrap() - expected).abs() < 1e-9);
    assert_eq!(a.verdicts[2].tolerance, Some(0.0));
    let p99: Vec<_> = step
        .drift
        .iter()
        .filter(|d| d.metric.starts_with("p99"))
        .collect();
    assert_eq!(p99.len(), 2);
    assert!(p99.iter().all(|d| d.verdict == Some(DriftVerdict::Steady)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seed_rerun_with_a_changed_lag_is_recorded_as_drifted() {
    let rig = rig().await;
    three_lags(&rig);
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[120.0, 50.0])).await.unwrap();
    sched.prepare_step(&id, step(-1, 1, 2)).await.unwrap();
    let gate = rig.hold_stimulus();
    sched.confirm_step(&id).await.unwrap();
    for n in 1..=24 {
        wait_stimuli(&rig, n).await;
        gate.notify_one();
    }
    wait_stimuli(&rig, 25).await;
    rig.a.sim(|s| {
        s.lag_by_velocity_p.insert(841, 90);
    });
    release(&rig, &gate);
    let done = wait_state(&rig, &id, TuneState::AwaitingChoice).await;

    let step = &done.steps[0];
    let p99: Vec<_> = step
        .drift
        .iter()
        .filter(|d| d.metric.starts_with("p99"))
        .collect();
    assert_eq!(p99.len(), 2);
    for d in p99 {
        assert_eq!(d.verdict, Some(DriftVerdict::Drifted));
        assert!(d.rerun.unwrap() > d.first.unwrap());
    }
    for d in step.drift.iter().filter(|d| !d.metric.starts_with("p99")) {
        assert_eq!(d.verdict, None);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn timing_is_recorded_per_run_rung_and_step() {
    let rig = rig().await;
    let sched = scheduler(&rig);

    let tune = run_step(&rig, &sched, step(-1, 0, 1)).await;

    let step = &tune.steps[0];
    let mut last = step.started_unix_ms.unwrap();
    for rung in step.rungs.iter().chain(&step.drift_rung) {
        let ids: Vec<&RunId> = rung.run_times.iter().map(|t| &t.id).collect();
        assert_eq!(ids, rung.run_ids.iter().collect::<Vec<_>>());
        let started = rung.started_unix_ms.unwrap();
        assert!(started >= last);
        last = started;
        for t in &rung.run_times {
            assert!(t.started_unix_ms >= last && t.ended_unix_ms >= t.started_unix_ms);
            last = t.ended_unix_ms;
        }
        let ended = rung.ended_unix_ms.unwrap();
        assert!(ended >= last);
        last = ended;
    }
    assert!(step.ended_unix_ms.unwrap() >= last);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn zero_and_rounds_to_zero_seed_refuse_start() {
    let rig = rig().await;
    rig.a.sim(|s| s.channels[0].gains[0] = 0);
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[50.0])).await.unwrap();

    let live = sched.prepare_step(&id, step(-1, 1, 1)).await;
    let typed = sched
        .prepare_step(
            &id,
            StepRequest {
                seed: Seed::Typed(0.4),
                ..step(-1, 1, 1)
            },
        )
        .await;

    assert_eq!(live.unwrap_err(), TuneError::SeedRefused(SeedRefused::Zero));
    assert_eq!(
        typed.unwrap_err(),
        TuneError::SeedRefused(SeedRefused::RoundsToZero)
    );
    assert_eq!(rig.store.read_tune(&id).unwrap().state, TuneState::Ready);
    assert_eq!(sched.confirm_step(&id).await, Err(TuneError::NotAnnounced));
    assert_eq!(rig.stimuli(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn extension_is_never_queued_without_confirm() {
    let rig = rig().await;
    rig.a.sim(|s| {
        for (raw, lag) in [(668, 60), (841, 40), (1060, 20), (1335, 30)] {
            s.lag_by_velocity_p.insert(raw, lag);
        }
    });
    let sched = scheduler(&rig);
    let tune = run_step(&rig, &sched, step(-1, 1, 1)).await;
    let id = tune.id.clone();
    let drift_before = tune.steps[0].drift_rung.clone();
    assert!(drift_before.is_some());
    let offered = tune.steps[0].recommendation.unwrap();
    assert_eq!(offered.extend, Some(Extension::Above));

    tokio::time::sleep(Duration::from_millis(200)).await;
    let stimuli = rig.stimuli();
    assert_eq!(stimuli, 12);
    assert!(rig.engine.is_idle());
    let below = sched.confirm_extension(&id, Extension::Below).await;
    assert!(matches!(below, Err(TuneError::NoExtension(_))), "{below:?}");
    assert_eq!(rig.stimuli(), stimuli);

    sched
        .confirm_extension(&id, Extension::Above)
        .await
        .unwrap();
    let tune = wait_state(&rig, &id, TuneState::AwaitingChoice).await;

    let step = &tune.steps[0];
    let raws: Vec<i32> = step.rungs.iter().map(|r| r.raw).collect();
    assert_eq!(raws, vec![668, 841, 1060, 1335]);
    assert_eq!(step.rungs[3].repeat_values.len(), 1);
    assert_eq!(step.assessment.as_ref().unwrap().verdicts.len(), 4);
    assert_eq!(rig.stimuli(), stimuli + 3);
    assert_eq!(step.drift_rung, drift_before);
}

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeat_result_reads_per_edge_damping_not_the_ensemble_fit() {
    let rig = rig().await;
    let probe_run = rig.run_once(probe()).await;
    let recorded = probe_run.metrics.as_ref().unwrap().channels[0]
        .probe
        .as_ref()
        .unwrap();
    assert!(
        matches!(
            recorded.damping,
            Some(DampingOutcome::Assessed { fitted: 25, .. })
        ),
        "{recorded:?}"
    );

    let mut incoherent = probe_run.clone();
    let p = incoherent.metrics.as_mut().unwrap().channels[0]
        .probe
        .as_mut()
        .unwrap();
    assert!(matches!(p.velocity, FitOutcome::Fit { .. }));
    p.damping = Some(DampingOutcome::Unassessable {
        reason: "ring frequency spread 7.1 Hz exceeds 3 Hz".into(),
    });

    let result = repeat_result(Some(&incoherent), &[], None);

    assert_eq!(result.zeta, RungOutcome::Unassessable);
}

fn board(rig: &Rig) -> ([i32; 4], [u16; 4]) {
    let sim = rig.a.sim.lock().unwrap();
    (sim.channels[0].gains, sim.channels[0].filters)
}

fn raw_of(snapshot: &(Gains, Filters)) -> ([i32; 4], [u16; 4]) {
    let (g, f) = snapshot;
    (
        [g.velocity_p, g.velocity_i, g.position_p, g.position_i],
        [f.velocity_hz, f.torque_hz, f.position_hz, f.flux_hz],
    )
}

fn release(rig: &Rig, gate: &Arc<Notify>) {
    rig.a.sim(|s| s.gate = None);
    gate.notify_one();
}

async fn wait_disk_state(rig: &Rig, id: &TuneId, state: TuneState) {
    let deadline = Instant::now() + WAIT;
    while rig.store.read_tune(id).unwrap().state != state {
        assert!(Instant::now() < deadline, "tune never reached {state:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// A finished one-rung velocity P step: rung 1060, three runs, no drift re-run.
async fn first_step_done(rig: &Rig, sched: &TuneScheduler<MoonrakerClient>) -> TuneId {
    run_step(rig, sched, step(1, 1, 1)).await.id
}

fn no_stimulus_after_last_gain_write(rig: &Rig) -> i32 {
    let scripts = rig.scripts();
    let last = scripts
        .iter()
        .rposition(|s| s.starts_with("FOCI_SET_GAINS"))
        .unwrap();
    assert!(
        scripts[last..].iter().all(|s| !s.starts_with("G90")),
        "{scripts:?}"
    );
    (float(&scripts[last], "VELOCITY_P") * 256.0).round() as i32
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chosen_value_becomes_next_seed_and_is_applied() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[50.0])).await.unwrap();
    sched.prepare_step(&id, step(1, 1, 1)).await.unwrap();
    let early = sched.choose(&id, Choice::Value(900.0)).await;
    assert!(matches!(early, Err(TuneError::OutOfOrder(_))), "{early:?}");
    sched.confirm_step(&id).await.unwrap();
    wait_state(&rig, &id, TuneState::AwaitingChoice).await;
    let writes = written_velocity_p(&rig).len();

    for refused in [Choice::Value(0.0), Choice::Value(40_000.0), Choice::Rung(1)] {
        let err = sched.choose(&id, refused).await;
        assert!(matches!(err, Err(TuneError::InvalidRequest(_))), "{err:?}");
    }
    assert_eq!(written_velocity_p(&rig).len(), writes);
    sched.choose(&id, Choice::Value(900.0)).await.unwrap();

    assert_eq!(board(&rig).0, [900, 412, 141, 0]);
    assert_eq!(board(&rig).1, raw_of(&BASE).1);
    let tune = rig.store.read_tune(&id).unwrap();
    assert_eq!(tune.state, TuneState::Ready);
    assert_eq!(tune.steps[0].chosen, Some(900));
    assert_eq!(tune.steps[0].chosen_shown, Some(900.0));
    assert_eq!(tune.snapshot, BASE);

    sched.prepare_step(&id, step(0, 0, 1)).await.unwrap();
    let tune = rig.store.read_tune(&id).unwrap();
    assert_eq!(tune.steps[1].base.0.velocity_p, 900);
    assert_eq!(tune.snapshot, BASE);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abandon_after_first_step_restores_all_four_gains_and_filters() {
    let rig = rig().await;
    rig.a.sim(|s| s.channels[0].gains[3] = 77);
    let sched = scheduler(&rig);
    let id = first_step_done(&rig, &sched).await;
    let snapshot = rig.store.read_tune(&id).unwrap().snapshot;
    assert_eq!(raw_of(&snapshot).0, [841, 412, 141, 77]);
    sched.choose(&id, Choice::Rung(0)).await.unwrap();
    rig.a.sim(|s| s.channels[0].filters = [400, 900, 450, 950]);
    sched.prepare_step(&id, step(1, 1, 1)).await.unwrap();
    let gate = rig.hold_stimulus();
    sched.confirm_step(&id).await.unwrap();
    wait_stimuli(&rig, 4).await;
    assert_ne!(board(&rig), raw_of(&snapshot));

    let abandon = tokio::spawn({
        let (sched, id) = (sched.clone(), id.clone());
        async move { sched.abandon(&id).await }
    });
    wait_disk_state(&rig, &id, TuneState::RestorePending).await;
    release(&rig, &gate);
    abandon.await.unwrap().unwrap();

    assert_eq!(board(&rig), raw_of(&snapshot));
    let tune = rig.store.read_tune(&id).unwrap();
    assert_eq!(tune.state, TuneState::Abandoned);
    assert_eq!(rig.engine.lock_state("stepper_x"), None);
    assert_eq!(rig.stimuli(), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abandon_mid_rung_aborts_waits_and_restores() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[50.0])).await.unwrap();
    sched.prepare_step(&id, step(-1, 1, 1)).await.unwrap();
    let gate = rig.hold_stimulus();
    sched.confirm_step(&id).await.unwrap();
    wait_stimuli(&rig, 1).await;

    let abandon = tokio::spawn({
        let (sched, id) = (sched.clone(), id.clone());
        async move { sched.abandon(&id).await }
    });
    wait_disk_state(&rig, &id, TuneState::RestorePending).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !abandon.is_finished(),
        "the restore waits for the run in flight"
    );
    release(&rig, &gate);
    abandon.await.unwrap().unwrap();

    let tune = rig.store.read_tune(&id).unwrap();
    assert_eq!(tune.state, TuneState::Abandoned);
    assert!(
        tune.steps[0].rungs[1..]
            .iter()
            .all(|r| r.run_ids.is_empty())
    );
    assert_eq!(no_stimulus_after_last_gain_write(&rig), BASE.0.velocity_p);
    assert_eq!(board(&rig), raw_of(&BASE));
    assert!(rig.engine.is_idle());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(rig.stimuli(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_an_abandon_keeps_the_stepper_locked_and_still_restores() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[50.0])).await.unwrap();
    sched.prepare_step(&id, step(1, 1, 1)).await.unwrap();
    let gate = rig.hold_stimulus();
    sched.confirm_step(&id).await.unwrap();
    wait_stimuli(&rig, 1).await;

    let abandon = tokio::spawn({
        let (sched, id) = (sched.clone(), id.clone());
        async move { sched.abandon(&id).await }
    });
    wait_disk_state(&rig, &id, TuneState::RestorePending).await;
    abandon.abort();
    assert!(abandon.await.unwrap_err().is_cancelled());

    assert_eq!(
        rig.engine.lock_state("stepper_x").map(|l| l.snapshot),
        Some(BASE)
    );
    let failing = rig.engine.preconditions("stepper_x").await;
    assert!(
        failing
            .iter()
            .any(|p| matches!(p, Precondition::TuneLocked(_))),
        "{failing:?}"
    );
    release(&rig, &gate);
    wait_disk_state(&rig, &id, TuneState::Abandoned).await;
    assert_eq!(rig.engine.lock_state("stepper_x"), None);
    assert_eq!(board(&rig), raw_of(&BASE));
    assert_eq!(rig.stimuli(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abandon_between_two_runs_submits_nothing_more() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[50.0])).await.unwrap();
    sched.prepare_step(&id, step(1, 1, 1)).await.unwrap();
    let mut holder_tune = rig.store.read_tune(&id).unwrap();
    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    sched.confirm_step(&id).await.unwrap();
    wait_stimuli(&rig, 1).await;

    // A second announced record on the same stepper: confirming it holds the stepper's mutex
    // across a held settings read, so the step task's filing of run one and the abandon queue
    // behind it in that order, and the abandon lands before the step task's next submit.
    holder_tune.id = TuneId::generate(std::time::SystemTime::now());
    rig.store.write_tune(&holder_tune).unwrap();
    rig.fake.hold_settings(true);
    rig.engine.refresh_steppers();
    let queries = rig.fake.settings_queries();
    let holder = tokio::spawn({
        let (sched, id) = (sched.clone(), holder_tune.id.clone());
        async move { sched.confirm_step(&id).await }
    });
    while rig.fake.settings_queries() == queries {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    release(&rig, &gate);
    let first = tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(EngineEvent::Finished(run)) = events.recv().await {
                return run.id;
            }
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let abandon = tokio::spawn({
        let (sched, id) = (sched.clone(), id.clone());
        async move { sched.abandon(&id).await }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!holder.is_finished());
    rig.fake.hold_settings(false);

    let held = holder.await.unwrap();
    assert_eq!(held, Err(TuneError::Precondition(Precondition::Busy)));
    let abandoned = tokio::time::timeout(WAIT, abandon).await;
    abandoned.expect("abandon did not finish").unwrap().unwrap();

    let tune = rig.store.read_tune(&id).unwrap();
    assert_eq!(tune.state, TuneState::Abandoned);
    assert_eq!(tune.steps[0].rungs[0].run_ids, vec![first.clone()]);
    assert_eq!(rig.stimuli(), 1);
    while let Ok(event) = events.try_recv() {
        if let EngineEvent::Queue(queued) = event {
            assert!(queued.iter().all(|r| *r == first), "{queued:?}");
        }
    }
    assert_eq!(no_stimulus_after_last_gain_write(&rig), BASE.0.velocity_p);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn confirm_racing_abandon_has_one_outcome() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[50.0])).await.unwrap();
    sched.prepare_step(&id, step(1, 1, 1)).await.unwrap();

    let confirm = tokio::spawn({
        let (sched, id) = (sched.clone(), id.clone());
        async move { sched.confirm_step(&id).await }
    });
    let abandon = tokio::spawn({
        let (sched, id) = (sched.clone(), id.clone());
        async move { sched.abandon(&id).await }
    });
    let (confirmed, abandoned) = (confirm.await.unwrap(), abandon.await.unwrap());

    abandoned.unwrap();
    assert_eq!(
        rig.store.read_tune(&id).unwrap().state,
        TuneState::Abandoned
    );
    match confirmed {
        Ok(()) => assert!(rig.stimuli() <= 1),
        Err(e) => {
            assert_eq!(e, TuneError::NotAnnounced);
            assert_eq!(rig.stimuli(), 0);
        }
    }
    let stimuli = rig.stimuli();
    assert_eq!(no_stimulus_after_last_gain_write(&rig), BASE.0.velocity_p);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(rig.stimuli(), stimuli);
}

async fn failed_abandon(rig: &Rig, sched: &TuneScheduler<MoonrakerClient>) -> TuneId {
    let id = first_step_done(rig, sched).await;
    rig.a.sim(|s| s.confirm_gains = false);
    let err = sched.abandon(&id).await;
    assert!(matches!(err, Err(TuneError::Engine(_))), "{err:?}");
    id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_restore_locks_and_blocks_every_route() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = failed_abandon(&rig, &sched).await;
    let stimuli = rig.stimuli();

    let tune = rig.store.read_tune(&id).unwrap();
    let TuneState::RestoreFailed(reason) = &tune.state else {
        panic!("{:?}", tune.state);
    };
    assert!(reason.contains("unconfirmed"), "{reason}");
    let lock = rig.engine.lock_state("stepper_x").unwrap();
    assert_eq!(lock.snapshot, tune.snapshot);
    let locked = |p: &Precondition| matches!(p, Precondition::TuneLocked(_));
    assert!(locked(
        &rig.engine
            .submit(rig.request(tracking()))
            .await
            .unwrap_err()
    ));
    let ladder = rig
        .engine
        .submit_ladder(rig.ladder(tracking(), "velocity_p", &[841.0, 941.0], 1))
        .await;
    assert!(matches!(ladder, Err(LadderRejected::Precondition(ref p)) if locked(p)));
    let create = sched.create(setup(&[50.0])).await;
    assert!(matches!(create, Err(TuneError::Precondition(ref p)) if locked(p)));
    let prepare = sched.prepare_step(&id, step(0, 0, 1)).await;
    assert!(matches!(prepare, Err(TuneError::OutOfOrder(_))));
    let choose = sched.choose(&id, Choice::Rung(0)).await;
    assert!(matches!(choose, Err(TuneError::OutOfOrder(_))));
    assert_eq!(sched.confirm_step(&id).await, Err(TuneError::NotAnnounced));
    assert_eq!(rig.stimuli(), stimuli);

    rig.a.sim(|s| s.confirm_gains = true);
    sched.restore(&id).await.unwrap();
    assert_eq!(
        rig.store.read_tune(&id).unwrap().state,
        TuneState::Abandoned
    );
    assert_eq!(rig.engine.lock_state("stepper_x"), None);
    assert_eq!(board(&rig), raw_of(&BASE));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lock_cleared_event_updates_the_tune() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = first_step_done(&rig, &sched).await;
    rig.a.sim(|s| s.confirm_filters[1] = false);
    assert!(sched.abandon(&id).await.is_err());
    assert!(matches!(
        rig.store.read_tune(&id).unwrap().state,
        TuneState::RestoreFailed(_)
    ));
    assert_eq!(board(&rig), raw_of(&BASE));

    rig.engine.read_control_state("stepper_x").await.unwrap();

    wait_disk_state(&rig, &id, TuneState::Abandoned).await;
    assert_eq!(rig.engine.lock_state("stepper_x"), None);
}

#[tokio::test(flavor = "current_thread")]
async fn missed_lock_events_do_not_leave_a_cleared_lock_on_its_tune() {
    let rig = rig().await;
    let (tx, rx) = tokio::sync::broadcast::channel(1);
    let sched = TuneScheduler::with_events(rig.engine.clone(), rig.store.clone(), rx);
    let id = first_step_done(&rig, &sched).await;
    rig.a.sim(|s| s.confirm_filters[1] = false);
    assert!(sched.abandon(&id).await.is_err());
    assert!(matches!(
        rig.store.read_tune(&id).unwrap().state,
        TuneState::RestoreFailed(_)
    ));
    rig.engine.read_control_state("stepper_x").await.unwrap();
    assert_eq!(rig.engine.lock_state("stepper_x"), None);

    for _ in 0..3 {
        let _ = tx.send(EngineEvent::LockCleared {
            stepper: "unrelated".to_owned(),
        });
    }

    wait_disk_state(&rig, &id, TuneState::Abandoned).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stale_lock_cleared_event_leaves_a_relocked_tune_awaiting_its_restore() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = failed_abandon(&rig, &sched).await;
    let mut events = rig.engine.events();

    let (restored, ()) = tokio::join!(sched.restore(&id), async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let frame = {
            let mut sim = rig.a.sim.lock().unwrap();
            let c = &mut sim.channels[0];
            c.change_sequence += 1;
            (c.gains, c.filters) = raw_of(&BASE);
            Board::control(&mut sim, 0, 0)
        };
        rig.a.emit(vec![frame]);
    });
    let persisted = rig.store.read_tune(&id).unwrap();
    let cleared = tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(EngineEvent::LockCleared { stepper }) = events.recv().await {
                return stepper;
            }
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(restored.is_err());
    assert_eq!(cleared, "stepper_x");
    assert!(matches!(persisted.state, TuneState::RestoreFailed(_)));
    assert_eq!(rig.store.read_tune(&id).unwrap(), persisted);
    assert_eq!(
        rig.engine.lock_state("stepper_x").map(|l| l.snapshot),
        Some(BASE)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kill_sends_no_gcode_and_restore_is_explicit() {
    let rig = rig().await;
    rig.fake
        .script_emergency_stop(None, Duration::ZERO, Some(Duration::ZERO));
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[50.0])).await.unwrap();
    sched.prepare_step(&id, step(-1, 1, 1)).await.unwrap();
    let _gate = rig.hold_stimulus();
    sched.confirm_step(&id).await.unwrap();
    wait_stimuli(&rig, 1).await;

    rig.engine.kill().await;
    let sent = rig.scripts().len();
    let tune = wait_state(&rig, &id, TuneState::AwaitingChoice).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(tune.steps[0].failed.is_some());
    assert_eq!(rig.scripts().len(), sent, "{:?}", rig.scripts());
    assert_eq!(rig.engine.lock_state("stepper_x"), None);

    let refused = sched.abandon(&id).await;
    assert!(refused.is_err(), "Klipper is shut down after the kill");
    assert_eq!(rig.scripts().len(), sent, "{:?}", rig.scripts());
    assert!(matches!(
        rig.store.read_tune(&id).unwrap().state,
        TuneState::RestoreFailed(_)
    ));
    assert_eq!(
        rig.engine.lock_state("stepper_x").map(|l| l.snapshot),
        Some(BASE)
    );

    rig.fake.push("notify_klippy_disconnected", json!([]));
    rig.fake.push("notify_klippy_ready", json!([]));
    let deadline = Instant::now() + WAIT;
    while sched.restore(&id).await.is_err() {
        assert!(Instant::now() < deadline, "restore never succeeded");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        rig.store.read_tune(&id).unwrap().state,
        TuneState::Abandoned
    );
    assert_eq!(rig.engine.lock_state("stepper_x"), None);
    assert_eq!(no_stimulus_after_last_gain_write(&rig), BASE.0.velocity_p);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn final_step_produces_the_config_block_with_three_accepted_values() {
    let rig = rig().await;
    rig.a.sim(|s| {
        s.lag_by_velocity_p.insert(1060, 40);
    });
    let sched = scheduler(&rig);
    let id = first_step_done(&rig, &sched).await;
    sched.choose(&id, Choice::Rung(0)).await.unwrap();
    assert_eq!(sched.config_block(&id), None);

    sched.prepare_step(&id, step(1, 1, 1)).await.unwrap();
    sched.confirm_step(&id).await.unwrap();
    wait_state(&rig, &id, TuneState::AwaitingChoice).await;
    sched.choose(&id, Choice::Value(60.0)).await.unwrap();
    assert_eq!(sched.config_block(&id), None);

    sched.prepare_step(&id, step(1, 1, 1)).await.unwrap();
    sched.confirm_step(&id).await.unwrap();
    let tune = wait_state(&rig, &id, TuneState::AwaitingChoice).await;
    let recommended = tune.steps[2].recommendation.unwrap();
    assert_eq!(recommended.choice, Recommendation::Measured { rung: 0 });
    sched.choose(&id, Choice::Recommended).await.unwrap();

    let tune = rig.store.read_tune(&id).unwrap();
    assert_eq!(tune.state, TuneState::Done);
    let chosen: Vec<i32> = tune.steps.iter().map(|s| s.chosen.unwrap()).collect();
    assert_eq!(chosen[0], 1060);
    assert_eq!(chosen[2], tune.steps[2].rungs[0].raw);
    let block = sched.config_block(&id).unwrap();
    for (key, value) in [
        ("pid_velocity_p", chosen[0]),
        ("pid_velocity_i", chosen[1]),
        ("pid_position_p", chosen[2]),
        ("pid_position_i", 0),
        ("velocity_filter_hz", 500),
        ("torque_filter_hz", 1000),
    ] {
        assert!(
            block
                .lines()
                .any(|l| l.starts_with(&format!("{key}: {value}"))),
            "{key}: {value} missing from\n{block}"
        );
    }
    let corner = to_raw(LadderAxis::VelocityICornerHz, 60.0, tune.steps[1].pwm_hz).unwrap();
    assert_eq!(chosen[1], corner.raw);
    assert!(block.contains("Hz corner"), "{block}");
    let after = sched.choose(&id, Choice::Rung(0)).await;
    assert!(matches!(after, Err(TuneError::OutOfOrder(_))));
}

async fn finished_tune(rig: &Rig, sched: &TuneScheduler<MoonrakerClient>) -> TuneId {
    rig.a.sim(|s| {
        s.lag_by_velocity_p.insert(1060, 40);
    });
    let id = first_step_done(rig, sched).await;
    sched.choose(&id, Choice::Rung(0)).await.unwrap();
    sched.prepare_step(&id, step(1, 1, 1)).await.unwrap();
    sched.confirm_step(&id).await.unwrap();
    wait_state(rig, &id, TuneState::AwaitingChoice).await;
    sched.choose(&id, Choice::Value(60.0)).await.unwrap();
    sched.prepare_step(&id, step(1, 1, 1)).await.unwrap();
    sched.confirm_step(&id).await.unwrap();
    wait_state(rig, &id, TuneState::AwaitingChoice).await;
    sched.choose(&id, Choice::Recommended).await.unwrap();
    id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn applying_a_finished_tune_writes_all_three_chosen_gains_over_whatever_the_stepper_has() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = finished_tune(&rig, &sched).await;
    let tune = rig.store.read_tune(&id).unwrap();
    let chosen: Vec<i32> = tune.steps.iter().map(|s| s.chosen.unwrap()).collect();
    rig.engine.apply_gains("stepper_x", BASE.0).await.unwrap();
    let before = rig.scripts().len();

    sched.apply_result(&id).await.unwrap();

    let scripts = rig.scripts();
    let line = scripts[before..]
        .iter()
        .rfind(|s| s.starts_with("FOCI_SET_GAINS"))
        .expect("the gains were written");
    assert_eq!(
        (float(line, "VELOCITY_P") * 256.0).round() as i32,
        chosen[0]
    );
    assert_eq!(
        (float(line, "VELOCITY_I") * 4096.0).round() as i32,
        chosen[1]
    );
    assert_eq!(
        (float(line, "POSITION_P") * 256.0).round() as i32,
        chosen[2]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_gains_of_a_tune_that_is_not_finished_cannot_be_applied() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[50.0])).await.unwrap();
    let writes = rig.scripts().len();

    let err = sched.apply_result(&id).await.unwrap_err();

    assert!(
        matches!(&err, TuneError::OutOfOrder(text) if text.contains("all three steps")),
        "{err:?}"
    );
    assert_eq!(rig.scripts().len(), writes, "nothing was written");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_extension_clears_the_assessment_while_its_rung_runs() {
    let rig = rig().await;
    rig.a.sim(|s| {
        for (raw, lag) in [(668, 60), (841, 40), (1060, 20), (1335, 30)] {
            s.lag_by_velocity_p.insert(raw, lag);
        }
    });
    let sched = scheduler(&rig);
    let tune = run_step(&rig, &sched, step(-1, 1, 1)).await;
    let id = tune.id.clone();
    assert!(tune.steps[0].assessment.is_some());
    let stimuli = rig.stimuli();

    let gate = rig.hold_stimulus();
    sched
        .confirm_extension(&id, Extension::Above)
        .await
        .unwrap();
    wait_stimuli(&rig, stimuli + 1).await;

    let running = rig.store.read_tune(&id).unwrap();
    let step = &running.steps[0];
    assert_eq!(step.rungs.len(), 4);
    assert!(step.assessment.is_none());
    assert!(step.recommendation.is_none());

    release(&rig, &gate);
    let done = wait_state(&rig, &id, TuneState::AwaitingChoice).await;
    assert_eq!(done.steps[0].assessment.as_ref().unwrap().verdicts.len(), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_assessment_is_stored_as_each_rung_completes() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[50.0, 120.0])).await.unwrap();
    sched.prepare_step(&id, step(-1, 1, 1)).await.unwrap();
    let gate = rig.hold_stimulus();
    sched.confirm_step(&id).await.unwrap();
    for n in 1..=4 {
        wait_stimuli(&rig, n).await;
        gate.notify_one();
    }
    wait_stimuli(&rig, 5).await;

    let running = rig.store.read_tune(&id).unwrap();
    let step = &running.steps[0];
    assert_eq!(running.state, TuneState::Running);
    assert_eq!(step.rungs[0].repeat_values.len(), 1);
    assert!(step.rungs[1].repeat_values.is_empty());
    assert_eq!(step.assessment.as_ref().unwrap().verdicts.len(), 3);
    assert!(step.recommendation.is_none());

    release(&rig, &gate);
    wait_state(&rig, &id, TuneState::AwaitingChoice).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stored_run_count_is_the_announced_one() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[50.0])).await.unwrap();
    let plan = sched.prepare_step(&id, step(-1, 1, 2)).await.unwrap();
    assert_eq!(
        rig.store.read_tune(&id).unwrap().steps[0].run_count,
        Some(plan.run_count)
    );

    sched.confirm_step(&id).await.unwrap();
    let mut tune = wait_state(&rig, &id, TuneState::AwaitingChoice).await;
    tune.steps[0].chosen = Some(841);
    tune.state = TuneState::Ready;
    rig.store.write_tune(&tune).unwrap();
    rig.a.sim(|s| s.channels[0].gains[1] = 0);

    let plan = sched.prepare_step(&id, step(-1, 1, 2)).await.unwrap();

    assert_eq!(plan.axis, LadderAxis::VelocityICornerHz);
    let stored = rig.store.read_tune(&id).unwrap();
    assert_eq!(stored.steps[1].seed_raw, 0);
    assert!(stored.steps[1].rungs.iter().all(|r| r.raw != 0));
    assert_eq!(stored.steps[1].run_count, Some(plan.run_count));
    assert_eq!(plan.run_count, (3 * 2 + 1) * 3);
}

fn no_edges_at(rig: &Rig, velocity_p: i32) {
    rig.a.sim(|s| {
        let no_edges = Profile {
            probe_edges: 0,
            ..Profile::default()
        };
        s.profile_by_velocity_p.insert(velocity_p, no_edges);
    });
}

/// Gives every rung of a `k` -1..=1 ladder from 841 a tracking lag, so its tracking p99 exists
/// and an assessed rung can be scored.
fn tracking_lag(rig: &Rig) {
    rig.a.sim(|s| {
        for raw in [668, 841, 1060] {
            s.lag_by_velocity_p.insert(raw, 50);
        }
    });
}

fn edges_again_at(rig: &Rig, velocity_p: i32) {
    rig.a.sim(|s| {
        s.profile_by_velocity_p.remove(&velocity_p);
    });
}

/// Seed 841 with `k` from -1 to 1 gives rungs 668, 841 and 1060 at one repeat each; the probe at
/// 841 yields no edges, so the step pauses at rung 1.
async fn paused_tune(rig: &Rig, sched: &TuneScheduler<MoonrakerClient>) -> TuneId {
    no_edges_at(rig, 841);
    let id = sched.create(setup(&[50.0])).await.unwrap();
    sched.prepare_step(&id, step(-1, 1, 1)).await.unwrap();
    sched.confirm_step(&id).await.unwrap();
    wait_state(rig, &id, TuneState::Paused).await;
    id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unassessable_rung_pauses_with_the_last_write_at_that_rung() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = paused_tune(&rig, &sched).await;
    sched.wait_idle(&id).await;

    let tune = rig.store.read_tune(&id).unwrap();
    let step = &tune.steps[0];
    assert_eq!(
        step.paused.as_ref().map(|p| (p.rung, p.reason)),
        Some((1, Pause::DampingUnassessable))
    );
    assert_eq!(
        step.assessment.as_ref().unwrap().pause,
        Some((1, Pause::DampingUnassessable))
    );
    assert!(step.rungs[2].run_ids.is_empty() && step.rungs[2].skipped.is_none());
    assert_eq!(written_velocity_p(&rig).last(), Some(&841));
    assert!(!written_velocity_p(&rig).contains(&1060));
    assert!(step.drift_rung.is_none());
    assert!(step.ended_unix_ms.is_none());
    let stimuli = rig.stimuli();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rig.stimuli(), stimuli, "nothing runs while paused");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paused_tune_survives_a_reload() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = paused_tune(&rig, &sched).await;

    let on_disk = rig.store.read_tune(&id).unwrap();

    assert_eq!(on_disk.state, TuneState::Paused);
    assert!(on_disk.steps[0].paused.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn continue_waives_the_rung_and_runs_the_rest() {
    let rig = rig().await;
    tracking_lag(&rig);
    let sched = scheduler(&rig);
    let id = paused_tune(&rig, &sched).await;

    sched.decide(&id, PauseDecision::Continue).await.unwrap();
    let tune = wait_state(&rig, &id, TuneState::AwaitingChoice).await;

    let step = &tune.steps[0];
    assert!(step.rungs[1].waived.is_some());
    assert_eq!(step.paused, None);
    let a = step.assessment.as_ref().unwrap();
    assert_eq!(a.verdicts[1].unscored, Some(UnscoredReason::DampingWaived));
    assert!(
        a.verdicts[2].score.is_some(),
        "the rung above was run and scored"
    );
    assert!(written_velocity_p(&rig).contains(&1060));
    assert!(
        step.drift_rung.is_some(),
        "a completed ladder still runs its drift check"
    );
    assert!(tune.notes.iter().any(|n| n.text.contains("continued")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn continuing_keeps_the_results_visible_while_the_rung_above_runs() {
    let rig = rig().await;
    tracking_lag(&rig);
    let sched = scheduler(&rig);
    let id = paused_tune(&rig, &sched).await;
    let stimuli = rig.stimuli();
    let gate = rig.hold_stimulus();

    sched.decide(&id, PauseDecision::Continue).await.unwrap();
    wait_stimuli(&rig, stimuli + 1).await;

    let running = rig.store.read_tune(&id).unwrap();
    let a = running.steps[0].assessment.as_ref().unwrap();
    assert_eq!(a.verdicts[1].unscored, Some(UnscoredReason::DampingWaived));
    assert!(a.medians[0].zeta.is_some());

    release(&rig, &gate);
    wait_state(&rig, &id, TuneState::AwaitingChoice).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn continue_on_the_last_rung_completes_the_step() {
    let rig = rig().await;
    no_edges_at(&rig, 1060);
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[50.0])).await.unwrap();
    sched.prepare_step(&id, step(-1, 1, 1)).await.unwrap();
    sched.confirm_step(&id).await.unwrap();
    wait_state(&rig, &id, TuneState::Paused).await;

    sched.decide(&id, PauseDecision::Continue).await.unwrap();
    let tune = wait_state(&rig, &id, TuneState::AwaitingChoice).await;

    assert!(tune.steps[0].drift_rung.is_some());
    assert!(tune.steps[0].recommendation.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rerun_supersedes_the_attempt_and_an_assessed_rerun_continues() {
    let rig = rig().await;
    tracking_lag(&rig);
    let sched = scheduler(&rig);
    let id = paused_tune(&rig, &sched).await;
    let first_ids = rig.store.read_tune(&id).unwrap().steps[0].rungs[1]
        .run_ids
        .clone();
    edges_again_at(&rig, 841);

    sched.decide(&id, PauseDecision::Rerun).await.unwrap();
    let tune = wait_state(&rig, &id, TuneState::AwaitingChoice).await;

    let rung = &tune.steps[0].rungs[1];
    assert_eq!(rung.superseded.len(), 1);
    assert_eq!(rung.superseded[0].run_ids, first_ids);
    assert!(rung.run_ids.iter().all(|r| !first_ids.contains(r)));
    assert_eq!(rung.waived, None);
    assert!(
        tune.steps[0].assessment.as_ref().unwrap().verdicts[1]
            .score
            .is_some()
    );
    assert!(written_velocity_p(&rig).contains(&1060));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unassessable_rerun_pauses_again() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = paused_tune(&rig, &sched).await;
    let first = rig.store.read_tune(&id).unwrap().steps[0]
        .paused
        .clone()
        .unwrap();

    sched.decide(&id, PauseDecision::Rerun).await.unwrap();
    assert_eq!(
        rig.store.read_tune(&id).unwrap().steps[0].rungs[1]
            .superseded
            .len(),
        1
    );
    let tune = wait_state(&rig, &id, TuneState::Paused).await;

    let again = tune.steps[0].paused.clone().unwrap();
    assert_eq!(again.rung, 1);
    assert!(again.unix_ms >= first.unix_ms);
    assert_eq!(
        tune.steps[0].rungs[1].run_ids.len(),
        3,
        "the re-run filed its own three runs"
    );
    assert_eq!(tune.steps[0].rungs[1].superseded.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_below_floor_rerun_halts() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = paused_tune(&rig, &sched).await;
    edges_again_at(&rig, 841);
    rig.a.sim(|s| {
        s.zeta_by_velocity_p.insert(841, 0.03);
    });

    sched.decide(&id, PauseDecision::Rerun).await.unwrap();
    let tune = wait_state(&rig, &id, TuneState::AwaitingChoice).await;

    assert!(matches!(
        tune.steps[0].assessment.as_ref().unwrap().halt,
        Some((1, Halt::BelowFloor { .. }))
    ));
    assert!(tune.steps[0].rungs[2].skipped.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_skips_the_rungs_above_and_recommends_from_the_assessed_ones() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = paused_tune(&rig, &sched).await;

    sched.decide(&id, PauseDecision::Stop).await.unwrap();
    let tune = wait_state(&rig, &id, TuneState::AwaitingChoice).await;

    let step = &tune.steps[0];
    let skipped = step.rungs[2].skipped.as_deref().unwrap();
    assert!(skipped.contains("unassessable"), "{skipped}");
    assert!(step.rungs[2].run_ids.is_empty());
    assert!(step.recommendation.is_some());
    assert!(step.drift_rung.is_none());
    assert!(!written_velocity_p(&rig).contains(&1060));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_records_a_note_naming_the_rung_and_the_reason() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = paused_tune(&rig, &sched).await;

    sched.decide(&id, PauseDecision::Stop).await.unwrap();
    let tune = wait_state(&rig, &id, TuneState::AwaitingChoice).await;

    let note = tune
        .notes
        .iter()
        .find(|n| n.text.contains("stopped"))
        .unwrap();
    assert!(note.text.contains("rung 2"), "{}", note.text);
    assert!(note.text.contains("unassessable"), "{}", note.text);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn continuing_a_paused_extension_of_a_halted_step_runs_no_drift() {
    let rig = rig().await;
    tracking_lag(&rig);
    rig.a.sim(|s| {
        s.zeta_by_velocity_p.insert(841, 0.03);
    });
    let sched = scheduler(&rig);
    let halted = run_step(&rig, &sched, step(-1, 1, 1)).await;
    let id = halted.id.clone();
    let first = &halted.steps[0];
    assert!(matches!(
        first.assessment.as_ref().unwrap().halt,
        Some((1, Halt::BelowFloor { .. }))
    ));
    assert!(first.drift_rung.is_none());
    assert_eq!(
        first.recommendation.and_then(|r| r.extend),
        Some(Extension::Below)
    );
    let below = rung_values(first.axis, first.seed_raw, first.pwm_hz, -2, -2).values[0].raw;
    no_edges_at(&rig, below);

    sched
        .confirm_extension(&id, Extension::Below)
        .await
        .unwrap();
    wait_state(&rig, &id, TuneState::Paused).await;
    let stimuli = rig.stimuli();
    sched.decide(&id, PauseDecision::Continue).await.unwrap();
    let tune = wait_state(&rig, &id, TuneState::AwaitingChoice).await;

    assert!(tune.steps[0].drift_rung.is_none());
    assert_eq!(rig.stimuli(), stimuli, "the seed rung was not run again");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn standstill_baseline_pause_continue_disables_the_gate() {
    let rig = rig_with(Options::default()).await;
    tracking_lag(&rig);
    rig.a.sim(|s| {
        let dropped = Profile {
            drop_flag: true,
            ..Profile::default()
        };
        s.profile_by_script.push(("G4 P200".into(), dropped));
    });
    let sched = scheduler(&rig);
    let id = sched.create(gated_setup()).await.unwrap();
    sched.prepare_step(&id, step(0, 1, 1)).await.unwrap();
    sched.confirm_step(&id).await.unwrap();
    let paused = wait_state(&rig, &id, TuneState::Paused).await;
    assert_eq!(
        paused.steps[0].paused.as_ref().map(|p| p.reason),
        Some(Pause::StandstillBaselineUnassessable)
    );
    sched.decide(&id, PauseDecision::Continue).await.unwrap();
    let tune = wait_state(&rig, &id, TuneState::AwaitingChoice).await;

    let step = &tune.steps[0];
    assert!(step.standstill_gate_off.is_some());
    let note = tune
        .notes
        .iter()
        .find(|n| n.text.contains("continued"))
        .expect("the decision is noted");
    assert!(
        note.text.contains("continued past rung 1:"),
        "the first rung is rung 1, as in the stop note: {}",
        note.text
    );
    assert_eq!(
        step.rungs[0].waived.as_ref().map(|w| w.reason),
        Some(Pause::StandstillBaselineUnassessable)
    );
    assert_eq!(
        rig.store.read_tune(&id).unwrap().steps[0].rungs[0].waived,
        step.rungs[0].waived,
        "the waiver survives a reload"
    );
    let a = step.assessment.as_ref().unwrap();
    assert_eq!(a.pause, None);
    assert_ne!(
        a.verdicts[0].unscored,
        Some(UnscoredReason::DampingWaived),
        "a standstill waiver is not a damping waiver"
    );
    assert!(
        a.verdicts
            .iter()
            .all(|v| v.inadmissible != Some(InadmissibleReason::StandstillHigh))
    );
    assert!(a.verdicts.iter().all(|v| v.score.is_some()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_waived_rung_zero_pauses_for_its_baseline_before_a_rung_above_runs() {
    let rig = rig().await;
    tracking_lag(&rig);
    rig.a.sim(|s| {
        let dropped = Profile {
            probe_edges: 0,
            drop_flag: true,
            ..Profile::default()
        };
        s.profile_by_velocity_p.insert(668, dropped);
    });
    let sched = scheduler(&rig);
    let id = sched.create(gated_setup()).await.unwrap();
    sched.prepare_step(&id, step(-1, 1, 1)).await.unwrap();
    sched.confirm_step(&id).await.unwrap();
    wait_state(&rig, &id, TuneState::Paused).await;
    let stimuli = rig.stimuli();

    sched.decide(&id, PauseDecision::Continue).await.unwrap();

    let again = rig.store.read_tune(&id).unwrap();
    assert_eq!(again.state, TuneState::Paused);
    assert_eq!(
        again.steps[0].paused.as_ref().map(|p| (p.rung, p.reason)),
        Some((0, Pause::StandstillBaselineUnassessable))
    );
    assert_eq!(
        rig.stimuli(),
        stimuli,
        "no rung above ran before the baseline decision"
    );

    sched.decide(&id, PauseDecision::Continue).await.unwrap();
    let tune = wait_state(&rig, &id, TuneState::AwaitingChoice).await;

    let a = tune.steps[0].assessment.as_ref().unwrap();
    assert_eq!(a.verdicts[0].unscored, Some(UnscoredReason::DampingWaived));
    assert!(tune.steps[0].standstill_gate_off.is_some());
    assert!(a.verdicts[1].score.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_decision_needs_a_paused_tune_and_a_paused_tune_refuses_a_choice_and_an_extension() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = sched.create(setup(&[50.0])).await.unwrap();
    assert!(matches!(
        sched.decide(&id, PauseDecision::Stop).await,
        Err(TuneError::OutOfOrder(_))
    ));
    sched.abandon(&id).await.unwrap();

    let id = paused_tune(&rig, &sched).await;
    assert!(matches!(
        sched.choose(&id, Choice::Rung(0)).await,
        Err(TuneError::OutOfOrder(_))
    ));
    assert!(matches!(
        sched.confirm_extension(&id, Extension::Above).await,
        Err(TuneError::OutOfOrder(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abandon_while_paused_restores_and_submits_nothing() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = paused_tune(&rig, &sched).await;
    let stimuli = rig.stimuli();

    sched.abandon(&id).await.unwrap();

    let tune = rig.store.read_tune(&id).unwrap();
    assert_eq!(tune.state, TuneState::Abandoned);
    assert_eq!(rig.stimuli(), stimuli);
    assert_eq!(board(&rig), raw_of(&tune.snapshot));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_rerun_ends_the_step_failed_and_keeps_the_superseded_attempt() {
    let rig = rig().await;
    let sched = scheduler(&rig);
    let id = paused_tune(&rig, &sched).await;
    rig.a.sim(|s| {
        s.failures.push(("G90".into(), "refused".into()));
    });

    sched.decide(&id, PauseDecision::Rerun).await.unwrap();
    let tune = wait_state(&rig, &id, TuneState::AwaitingChoice).await;

    let step = &tune.steps[0];
    assert!(step.failed.is_some());
    assert_eq!(step.rungs[1].superseded.len(), 1);
    assert!(step.rungs[2].skipped.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rerun_drops_the_rungs_damping_waiver() {
    let rig = rig().await;
    rig.a.sim(|s| {
        let dropped = Profile {
            probe_edges: 0,
            drop_flag: true,
            ..Profile::default()
        };
        s.profile_by_velocity_p.insert(668, dropped);
    });
    let sched = scheduler(&rig);
    let id = sched.create(gated_setup()).await.unwrap();
    sched.prepare_step(&id, step(-1, 1, 1)).await.unwrap();
    sched.confirm_step(&id).await.unwrap();
    wait_state(&rig, &id, TuneState::Paused).await;
    sched.decide(&id, PauseDecision::Continue).await.unwrap();

    sched.decide(&id, PauseDecision::Rerun).await.unwrap();
    let tune = wait_state(&rig, &id, TuneState::Paused).await;

    assert_eq!(
        tune.steps[0].paused.as_ref().map(|p| (p.rung, p.reason)),
        Some((0, Pause::DampingUnassessable))
    );
    assert_eq!(tune.steps[0].rungs[0].waived, None);
}
