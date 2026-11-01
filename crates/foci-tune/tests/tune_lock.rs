mod fake_moonraker;
mod rig;

use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use foci_tune::engine::{CommandError, Engine, EngineEvent, LadderRejected, Lock, Precondition};
use foci_tune::model::{Filters, Gains, RunState};
use foci_tune_analysis::ladder::PID_GAIN_MAX_RAW;
use rig::*;
use tokio::sync::broadcast;

const REASON: &str = "abandoned, restore failed: gain write unconfirmed";

fn snapshot() -> (Gains, Filters) {
    (
        Gains {
            velocity_p: 900,
            velocity_i: 40,
            position_p: 120,
            position_i: 0,
        },
        Filters {
            velocity_hz: 200,
            torque_hz: 4000,
            position_hz: 500,
            flux_hz: 1000,
        },
    )
}

fn locks_dir(rig: &Rig) -> PathBuf {
    rig._dir.path().join("locks")
}

fn set_mode(path: &PathBuf, mode: u32) {
    fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
}

async fn submitted(rig: &Rig) -> Result<(), Precondition> {
    rig.engine.submit(rig.request(tracking())).await.map(|_| ())
}

fn locked() -> Precondition {
    Precondition::TuneLocked(REASON.to_owned())
}

/// Emits a control-state frame of channel 0 carrying `values`.
fn emit_control(rig: &Rig, values: (Gains, Filters)) {
    let (g, f) = values;
    let frame = {
        let mut sim = rig.a.sim.lock().unwrap();
        let c = &mut sim.channels[0];
        c.change_sequence += 1;
        c.gains = [g.velocity_p, g.velocity_i, g.position_p, g.position_i];
        c.filters = [f.velocity_hz, f.torque_hz, f.position_hz, f.flux_hz];
        Board::control(&mut sim, 0, 0)
    };
    rig.a.emit(vec![frame]);
}

async fn wait_cleared(events: &mut broadcast::Receiver<EngineEvent>) {
    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(EngineEvent::LockCleared { stepper }) = events.recv().await
                && stepper == "stepper_x"
            {
                return;
            }
        }
    })
    .await
    .expect("lock not cleared");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_control_state_requests_a_fresh_frame() {
    let rig = rig().await;

    let c = rig.engine.read_control_state("stepper_x").await.unwrap();

    assert_eq!(
        (c.velocity_p, c.velocity_i, c.position_p, c.position_i),
        (841, 412, 141, 0)
    );
    assert_eq!(
        (
            c.velocity_filter_hz,
            c.torque_filter_hz,
            c.position_filter_hz,
            c.flux_filter_hz
        ),
        (500, 1000, 500, 1000)
    );
    assert_eq!(
        rig.scripts(),
        vec![CRITICAL_LINE.to_owned(), DORMANT_LINE.to_owned()]
    );

    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let ids = rig
        .engine
        .submit_ladder(rig.ladder(tracking(), "velocity_p", &[841.0, 941.0], 1))
        .await
        .unwrap();
    rig.wait_moving(&mut events, &ids[0]).await;
    let sent = rig.scripts().len();

    let refused = rig.engine.read_control_state("stepper_x").await;

    assert_eq!(refused, Err(CommandError::Precondition(Precondition::Busy)));
    assert_eq!(rig.scripts().len(), sent);
    rig.engine.abort().await;
    gate.notify_one();
    rig.finish(&ids[0]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apply_gains_writes_and_confirms() {
    let rig = rig().await;
    let (gains, _) = snapshot();

    rig.engine.apply_gains("stepper_x", gains).await.unwrap();

    assert_eq!(
        rig.a.sim.lock().unwrap().channels[0].gains,
        [900, 40, 120, 0]
    );
    let scripts = rig.scripts();
    assert_eq!(scripts.len(), 2, "{scripts:?}");
    assert_eq!(scripts[0], "M400");
    assert!(scripts[1].starts_with("FOCI_SET_GAINS STEPPER=stepper_x "));

    rig.a.sim(|s| s.confirm_gains = false);
    let unconfirmed = Gains {
        velocity_p: 700,
        ..gains
    };
    assert_eq!(
        rig.engine.apply_gains("stepper_x", unconfirmed).await,
        Err("gain write unconfirmed".to_owned())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apply_gains_refused_while_run_in_flight() {
    let rig = rig().await;
    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let id = rig.engine.submit(rig.request(tracking())).await.unwrap();
    rig.wait_moving(&mut events, &id).await;
    let sent = rig.scripts().len();

    let refused = rig.engine.apply_gains("stepper_x", snapshot().0).await;

    assert_eq!(refused, Err(Precondition::Busy.to_string()));
    assert_eq!(rig.scripts().len(), sent);
    gate.notify_one();
    rig.finish(&id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apply_gains_rejects_out_of_range_raw() {
    let rig = rig().await;
    let (gains, filters) = snapshot();

    for bad in [
        Gains {
            velocity_p: PID_GAIN_MAX_RAW + 1,
            ..gains
        },
        Gains {
            position_i: -1,
            ..gains
        },
    ] {
        assert!(rig.engine.apply_gains("stepper_x", bad).await.is_err());
        assert!(
            rig.engine
                .restore_snapshot("stepper_x", bad, filters)
                .await
                .is_err()
        );
    }

    assert_eq!(rig.scripts(), Vec::<String>::new());
    assert_eq!(rig.engine.lock_state("stepper_x"), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn locked_stepper_refuses_single_run_and_core_ladder() {
    let rig = rig().await;

    rig.engine
        .lock_stepper("stepper_x", REASON.to_owned(), snapshot())
        .unwrap();

    assert_eq!(submitted(&rig).await, Err(locked()));
    assert_eq!(
        rig.engine
            .submit_ladder(rig.ladder(tracking(), "velocity_p", &[841.0, 941.0], 1))
            .await,
        Err(LadderRejected::Precondition(locked()))
    );
    assert_eq!(rig.engine.check("stepper_x").await, Err(locked()));
    assert_eq!(rig.scripts(), Vec::<String>::new());
    assert_eq!(
        rig.engine.lock_state("stepper_x"),
        Some(Lock {
            reason: REASON.to_owned(),
            snapshot: snapshot(),
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lock_write_failure_fails_closed() {
    let rig = rig().await;
    let dir = locks_dir(&rig);
    set_mode(&dir, 0o500);

    let written = rig
        .engine
        .lock_stepper("stepper_x", REASON.to_owned(), snapshot());

    set_mode(&dir, 0o700);
    assert!(written.is_err());
    assert_eq!(submitted(&rig).await, Err(locked()));
    assert!(rig.engine.lock_state("stepper_x").is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lock_remove_failure_keeps_lock() {
    let rig = rig().await;
    let mut events = rig.engine.events();
    rig.engine
        .lock_stepper("stepper_x", REASON.to_owned(), snapshot())
        .unwrap();
    let dir = locks_dir(&rig);
    set_mode(&dir, 0o500);

    let (gains, filters) = snapshot();
    let restored = rig
        .engine
        .restore_snapshot("stepper_x", gains, filters)
        .await;
    emit_control(&rig, snapshot());
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(restored, Ok(false));
    assert_eq!(
        rig.a.sim.lock().unwrap().channels[0].gains,
        [900, 40, 120, 0]
    );
    assert!(rig.engine.lock_state("stepper_x").is_some());
    assert_eq!(submitted(&rig).await, Err(locked()));

    set_mode(&dir, 0o700);
    emit_control(&rig, snapshot());
    wait_cleared(&mut events).await;
    assert_eq!(rig.engine.lock_state("stepper_x"), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lock_survives_restart() {
    let rig = rig().await;
    rig.engine
        .lock_stepper("stepper_x", REASON.to_owned(), snapshot())
        .unwrap();

    let restarted = Engine::start(
        rig.client.clone(),
        rig.live.clone(),
        rig.frames.clone(),
        rig.store.clone(),
        fast_config(),
    )
    .unwrap();

    assert_eq!(
        restarted.submit(rig.request(tracking())).await,
        Err(locked())
    );
    assert_eq!(
        restarted.lock_state("stepper_x"),
        Some(Lock {
            reason: REASON.to_owned(),
            snapshot: snapshot(),
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lock_clears_only_on_confirmed_restore() {
    let rig = rig().await;
    rig.engine
        .lock_stepper("stepper_x", REASON.to_owned(), snapshot())
        .unwrap();
    let (gains, filters) = snapshot();

    rig.a.sim(|s| s.confirm_gains = false);
    let failed = rig
        .engine
        .restore_snapshot("stepper_x", gains, filters)
        .await;
    assert_eq!(failed, Err("gain write unconfirmed".to_owned()));
    assert!(rig.engine.lock_state("stepper_x").is_some());
    assert_eq!(
        submitted(&rig).await,
        Err(Precondition::TuneLocked(
            "restore failed: gain write unconfirmed".to_owned()
        ))
    );

    rig.a.sim(|s| s.confirm_gains = true);
    let other = Gains {
        velocity_p: 700,
        ..gains
    };
    assert_eq!(
        rig.engine
            .restore_snapshot("stepper_x", other, filters)
            .await,
        Ok(false)
    );
    assert!(rig.engine.lock_state("stepper_x").is_some());

    assert_eq!(
        rig.engine
            .restore_snapshot("stepper_x", gains, filters)
            .await,
        Ok(true)
    );

    assert_eq!(rig.engine.lock_state("stepper_x"), None);
    assert_eq!(rig.store.read_lock("stepper_x").unwrap(), None);
    assert!(submitted(&rig).await.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matching_control_state_clears_lock() {
    let rig = rig().await;
    let mut events = rig.engine.events();
    rig.engine
        .lock_stepper("stepper_x", REASON.to_owned(), snapshot())
        .unwrap();
    assert!(rig.store.read_lock("stepper_x").unwrap().is_some());

    let (gains, filters) = snapshot();
    emit_control(
        &rig,
        (
            gains,
            Filters {
                flux_hz: 900,
                ..filters
            },
        ),
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rig.engine.lock_state("stepper_x").is_some());

    emit_control(&rig, snapshot());
    wait_cleared(&mut events).await;

    assert_eq!(rig.engine.lock_state("stepper_x"), None);
    assert_eq!(rig.store.read_lock("stepper_x").unwrap(), None);
    assert!(rig.engine.check("stepper_x").await.is_ok());
    assert!(rig.scripts().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_restore_locks_the_stepper_itself() {
    let rig = rig().await;
    let (gains, filters) = snapshot();
    rig.a.sim(|s| s.confirm_filters = [true, false, true, true]);

    let failed = rig
        .engine
        .restore_snapshot("stepper_x", gains, filters)
        .await;

    assert_eq!(failed, Err("filter write unconfirmed: torque".to_owned()));
    let reason = "restore failed: filter write unconfirmed: torque".to_owned();
    assert_eq!(
        submitted(&rig).await,
        Err(Precondition::TuneLocked(reason.clone()))
    );
    let held = Lock {
        reason,
        snapshot: snapshot(),
    };
    assert_eq!(rig.engine.lock_state("stepper_x"), Some(held.clone()));
    assert_eq!(rig.store.read_lock("stepper_x").unwrap(), Some(held));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queued_run_on_a_stepper_locked_mid_ladder_fails_without_motion() {
    let rig = rig().await;
    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let ids = rig
        .engine
        .submit_ladder(rig.ladder(tracking(), "velocity_p", &[841.0, 941.0], 1))
        .await
        .unwrap();
    rig.wait_moving(&mut events, &ids[0]).await;

    rig.engine
        .lock_stepper("stepper_x", REASON.to_owned(), snapshot())
        .unwrap();
    gate.notify_one();

    assert_eq!(rig.finish(&ids[0]).await.state, RunState::Done);
    assert_eq!(
        rig.finish(&ids[1]).await.state,
        RunState::Failed(locked().to_string())
    );
    assert_eq!(rig.stimuli(), 1);
}
