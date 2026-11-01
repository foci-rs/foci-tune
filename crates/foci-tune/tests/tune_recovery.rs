mod fake_moonraker;
mod rig;

use std::sync::Arc;
use std::time::{Duration, Instant};

use foci_tune::engine::{Engine, EngineEvent, Precondition};
use foci_tune::model::RunState;
use foci_tune::moonraker::MoonrakerClient;
use foci_tune::tune::{
    ProbeSettings, Seed, StandstillSettings, StepRequest, TuneScheduler, TuneSetup, TuneState,
};
use foci_tune_analysis::verdict::Gates;
use rig::*;

fn setup() -> TuneSetup {
    TuneSetup {
        stepper: "stepper_x".into(),
        speeds_mm_s: vec![50.0],
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

const STEP: StepRequest = StepRequest {
    seed: Seed::Live,
    repeats: 1,
    lower_k: 1,
    upper_k: 1,
};

fn restart_note(notes: &[foci_tune::tune::TuneNote]) -> bool {
    notes.iter().any(|n| n.text.contains("restart"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_during_rung_marks_restore_pending_and_does_not_resume() {
    let rig = rig().await;
    let before = TuneScheduler::new(rig.engine.clone(), rig.store.clone());
    let id = before.create(setup()).await.unwrap();
    before.prepare_step(&id, STEP).await.unwrap();
    let _gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    before.confirm_step(&id).await.unwrap();
    // The old engine stays alive behind the gate and writes run records through the same
    // store; its Moving event follows the Moving write, after which it writes nothing more
    // until the stimulus returns, so the new engine's reconcile cannot race it.
    tokio::time::timeout(WAIT, async {
        while !matches!(
            events.recv().await,
            Ok(EngineEvent::State {
                state: RunState::Moving,
                ..
            })
        ) {}
    })
    .await
    .expect("run never moved");
    let deadline = Instant::now() + WAIT;
    while rig.stimuli() == 0 {
        assert!(Instant::now() < deadline, "stimulus not sent");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let in_flight = rig.store.read_tune(&id).unwrap().steps[0].rungs[0]
        .run_ids
        .clone();
    assert!(
        in_flight.is_empty(),
        "the run in flight is filed when it ends"
    );

    let engine = Arc::new(
        Engine::start(
            rig.client.clone(),
            rig.live.clone(),
            rig.frames.clone(),
            rig.store.clone(),
            fast_config(),
        )
        .unwrap(),
    );
    let after: TuneScheduler<MoonrakerClient> =
        TuneScheduler::new(engine.clone(), rig.store.clone());
    let recovered = after.recover().await.unwrap();

    assert_eq!(recovered, vec![id.clone()]);
    let tune = rig.store.read_tune(&id).unwrap();
    assert_eq!(tune.state, TuneState::RestorePending);
    assert!(restart_note(&tune.notes), "{:?}", tune.notes);
    let lock = engine.lock_state("stepper_x").unwrap();
    assert_eq!(lock.snapshot, tune.snapshot);
    assert_eq!(lock.reason, "restart recovery");
    assert_eq!(rig.store.read_lock("stepper_x").unwrap(), Some(lock));
    assert!(matches!(
        engine.submit(rig.request(tracking())).await,
        Err(Precondition::TuneLocked(_))
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rig.stimuli(), 1);

    after.restore(&id).await.unwrap();

    assert_eq!(
        rig.store.read_tune(&id).unwrap().state,
        TuneState::Abandoned
    );
    assert_eq!(engine.lock_state("stepper_x"), None);
    assert_eq!(rig.stimuli(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_in_every_other_nonterminal_state_locks_and_never_resumes() {
    let rig = rig().await;
    for state in [
        TuneState::Ready,
        TuneState::Announced,
        TuneState::AwaitingChoice,
        TuneState::RestorePending,
        TuneState::RestoreFailed("gain write unconfirmed".into()),
    ] {
        let sched = TuneScheduler::new(rig.engine.clone(), rig.store.clone());
        let id = sched.create(setup()).await.unwrap();
        let mut tune = rig.store.read_tune(&id).unwrap();
        tune.state = state.clone();
        rig.store.write_tune(&tune).unwrap();
        let earlier = matches!(state, TuneState::RestoreFailed(_)).then(|| {
            rig.engine
                .lock_stepper("stepper_x", "restore failed: earlier".into(), tune.snapshot)
                .unwrap();
            "restore failed: earlier"
        });
        drop(sched);

        let restarted = TuneScheduler::new(rig.engine.clone(), rig.store.clone());
        let recovered = restarted.recover().await.unwrap();

        assert_eq!(recovered, vec![id.clone()], "{state:?}");
        let tune = rig.store.read_tune(&id).unwrap();
        assert_eq!(tune.state, TuneState::RestorePending, "{state:?}");
        assert!(restart_note(&tune.notes), "{state:?}");
        let lock = rig.engine.lock_state("stepper_x").unwrap();
        assert_eq!(lock.snapshot, tune.snapshot);
        assert_eq!(
            lock.reason,
            earlier.unwrap_or("restart recovery"),
            "{state:?}"
        );
        assert_eq!(rig.stimuli(), 0);

        restarted.restore(&id).await.unwrap();
        assert_eq!(
            rig.store.read_tune(&id).unwrap().state,
            TuneState::Abandoned
        );
    }
    let restarted = TuneScheduler::new(rig.engine.clone(), rig.store.clone());
    assert!(restarted.recover().await.unwrap().is_empty());
    assert_eq!(rig.engine.lock_state("stepper_x"), None);
}
