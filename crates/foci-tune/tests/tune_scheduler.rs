mod fake_moonraker;
mod rig;

use std::time::{Duration, Instant};

use foci_tune::engine::{EngineEvent, Precondition};
use foci_tune::model::{Filters, Gains, Run, RunId, RunState};
use foci_tune::moonraker::MoonrakerClient;
use foci_tune::stimulus::RunKind;
use foci_tune::tune::{
    ProbeSettings, Seed, StandstillSettings, StepRequest, Tune, TuneError, TuneId, TuneScheduler,
    TuneSetup, TuneState, repeat_result,
};
use foci_tune_analysis::ladder::{LadderAxis, RungOutcome};
use foci_tune_analysis::recommend::Extension;
use foci_tune_analysis::rungs::SeedRefused;
use foci_tune_analysis::verdict::{Gates, Halt, assess_ladder};
use rig::*;

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

fn scheduler(rig: &Rig) -> TuneScheduler<MoonrakerClient> {
    TuneScheduler::new(rig.engine.clone(), rig.store.clone())
}

fn step(lower_k: i32, upper_k: i32, repeats: u32) -> StepRequest {
    StepRequest {
        seed: Seed::Live,
        repeats,
        lower_k,
        upper_k,
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

    let tune = run_step(&rig, &sched, crate::step(0, 1, 1)).await;

    let step = &tune.steps[0];
    assert!(matches!(
        step.assessment.as_ref().unwrap().halt,
        Some((0, Halt::StandstillBaselineUnassessable))
    ));
    assert!(step.rungs[1].skipped.is_some());
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
    let main: Vec<_> = step.rungs.iter().map(|r| r.repeat_values.clone()).collect();
    assert_eq!(
        step.assessment.as_ref().unwrap(),
        &assess_ladder(&main, 1, &Gates::default(), false)
    );
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
