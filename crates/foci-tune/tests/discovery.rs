mod fake_moonraker;
mod rig;

use std::time::{Duration, Instant};

use foci_tune::discovery::DiscoveryConfig;
use foci_tune::live::TraceMode;
use foci_tune::model::RunState;
use foci_tune::moonraker::{KlippyState, MoonrakerApi};
use rig::*;
use serde_json::json;

fn dormant_line(stepper: &str) -> String {
    format!("FOCI_TRACE_MODE STEPPER={stepper} MODE=dormant")
}

fn sent(rig: &Rig, stepper: &str) -> usize {
    let line = dormant_line(stepper);
    rig.scripts().iter().filter(|s| **s == line).count()
}

fn mode_scripts(rig: &Rig) -> usize {
    rig.scripts()
        .iter()
        .filter(|s| s.starts_with("FOCI_TRACE_MODE"))
        .count()
}

async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn mode_of(rig: &Rig, serial: &str) -> TraceMode {
    rig.live.lock().unwrap().boards[serial].trace_mode
}

fn discovering(o: Options) -> Options {
    Options {
        discovery: Some(fast_discovery()),
        ..o
    }
}

/// Past the grace period plus several retry periods of [`fast_discovery`].
const SETTLE: Duration = Duration::from_millis(700);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_silent_open_board_is_set_dormant_once() {
    let rig = rig_with(discovering(Options {
        initial_hz_a: None,
        ..Options::default()
    }))
    .await;

    wait_until("the dormant command", || sent(&rig, "stepper_x") == 1).await;
    wait_until("the board's state", || {
        let live = rig.live.lock().unwrap();
        let a = &live.boards[SERIAL_A];
        a.protocol_version == Some(25)
            && a.trace_mode == TraceMode::Dormant
            && a.channels.len() == 2
    })
    .await;
    tokio::time::sleep(SETTLE).await;

    assert_eq!(rig.scripts(), vec![DORMANT_LINE.to_owned()]);
    assert_eq!(rig.engine.check("stepper_x").await, Ok(()));
    let run = rig.run_once(tracking()).await;
    assert_eq!(run.state, RunState::Done, "{:?}", run.verification);
    assert_eq!(rig.scripts().last().map(String::as_str), Some(DORMANT_LINE));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_board_already_streaming_is_left_alone() {
    let rig = rig_with(discovering(Options {
        initial_hz_a: Some(1000),
        ..Options::default()
    }))
    .await;

    tokio::time::sleep(SETTLE).await;

    assert_eq!(mode_scripts(&rig), 0, "{:?}", rig.scripts());
    assert_eq!(mode_of(&rig, SERIAL_A), TraceMode::Streaming);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_frame_within_the_grace_period_cancels_the_wake() {
    let rig = rig_with(Options {
        initial_hz_a: None,
        discovery: Some(DiscoveryConfig {
            grace: Duration::from_millis(1500),
            ..fast_discovery()
        }),
        ..Options::default()
    })
    .await;

    rig.a.poke();
    tokio::time::sleep(Duration::from_millis(2000)).await;

    assert_eq!(mode_scripts(&rig), 0, "{:?}", rig.scripts());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_wake_waits_for_a_run_in_flight() {
    let rig = rig_with(discovering(Options {
        announce_b: false,
        ..Options::default()
    }))
    .await;
    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let id = rig.engine.submit(rig.request(tracking())).await.unwrap();
    rig.wait_moving(&mut events, &id).await;

    tokio::time::sleep(SETTLE).await;
    assert_eq!(sent(&rig, "stepper_z"), 0, "{:?}", rig.scripts());
    rig.a.sim(|s| s.gate = None);
    gate.notify_one();
    assert_eq!(rig.finish(&id).await.state, RunState::Done);

    wait_until("the dormant command", || sent(&rig, "stepper_z") == 1).await;
    wait_until("board b dormant", || {
        mode_of(&rig, SERIAL_B) == TraceMode::Dormant
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_wake_waits_for_an_unconfirmed_kill_to_clear() {
    let rig = rig_with(discovering(Options {
        announce_b: false,
        ..Options::default()
    }))
    .await;
    rig.unconfirmed_kill().await;
    let after_kill = rig.scripts().len();

    tokio::time::sleep(SETTLE).await;
    assert_eq!(rig.scripts().len(), after_kill, "{:?}", rig.scripts());
    rig.fake.push("notify_klippy_shutdown", json!([]));
    let mut state = rig.client.state();
    state
        .wait_for(|s| s.klippy == KlippyState::Shutdown)
        .await
        .unwrap();
    tokio::time::sleep(SETTLE).await;
    assert_eq!(rig.scripts().len(), after_kill, "{:?}", rig.scripts());
    rig.fake.push("notify_klippy_ready", json!([]));

    wait_until("the dormant command", || sent(&rig, "stepper_z") == 1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_wake_waits_for_klipper_to_be_ready() {
    let rig = rig_with(Options {
        announce_b: false,
        discovery: Some(DiscoveryConfig {
            grace: Duration::from_millis(800),
            ..fast_discovery()
        }),
        ..Options::default()
    })
    .await;
    rig.fake.push("notify_klippy_shutdown", json!([]));
    let mut state = rig.client.state();
    state
        .wait_for(|s| s.klippy == KlippyState::Shutdown)
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(mode_scripts(&rig), 0, "{:?}", rig.scripts());
    rig.fake.push("notify_klippy_ready", json!([]));

    wait_until("the dormant command", || sent(&rig, "stepper_z") == 1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_wake_is_retried_with_backoff_until_frames_arrive() {
    let rig = rig_with(discovering(Options {
        announce_b: false,
        ..Options::default()
    }))
    .await;
    rig.b.sim(|s| {
        s.failures
            .push(("FOCI_TRACE_MODE".into(), "Unknown command".into()));
    });

    wait_until("the first attempt", || sent(&rig, "stepper_z") >= 1).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let attempts = sent(&rig, "stepper_z");
    assert!(
        (3..=10).contains(&attempts),
        "{attempts} attempts in about one second"
    );
    rig.b.announce(0);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let after_frames = sent(&rig, "stepper_z");
    tokio::time::sleep(SETTLE).await;

    assert_eq!(sent(&rig, "stepper_z"), after_frames);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_board_without_a_stepper_is_left_alone_with_a_reason() {
    let rig = rig_with(discovering(Options {
        announce_b: false,
        ..Options::default()
    }))
    .await;
    let mut unmapped = settings(true);
    let sections = unmapped.as_object_mut().unwrap();
    sections.remove("stepper_z");
    sections.remove("foci stepper_z");
    rig.fake.set_settings(unmapped);
    rig.engine.refresh_steppers();

    wait_until("the reason", || {
        rig.live.lock().unwrap().boards[SERIAL_B]
            .unusable_reason()
            .is_some()
    })
    .await;
    tokio::time::sleep(SETTLE).await;

    assert_eq!(mode_scripts(&rig), 0, "{:?}", rig.scripts());
    let reason = rig.live.lock().unwrap().boards[SERIAL_B]
        .unusable_reason()
        .map(str::to_owned)
        .unwrap();
    assert!(reason.contains("no FOCI stepper"), "{reason}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_silent_board_gets_its_own_command() {
    let rig = rig_with(discovering(Options {
        initial_hz_a: None,
        announce_b: false,
        ..Options::default()
    }))
    .await;

    wait_until("both commands", || {
        sent(&rig, "stepper_x") == 1 && sent(&rig, "stepper_z") == 1
    })
    .await;
    wait_until("both boards dormant", || {
        mode_of(&rig, SERIAL_A) == TraceMode::Dormant
            && mode_of(&rig, SERIAL_B) == TraceMode::Dormant
    })
    .await;
    tokio::time::sleep(SETTLE).await;

    assert_eq!(sent(&rig, "stepper_x"), 1);
    assert_eq!(sent(&rig, "stepper_z"), 1);
    assert_eq!(mode_scripts(&rig), 2, "{:?}", rig.scripts());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn frames_from_another_board_do_not_cancel_the_wake() {
    let rig = rig_with(discovering(Options {
        initial_hz_a: None,
        ..Options::default()
    }))
    .await;

    rig.b.poke();
    wait_until("the dormant command", || sent(&rig, "stepper_x") == 1).await;
    tokio::time::sleep(SETTLE).await;

    assert_eq!(mode_scripts(&rig), 1, "{:?}", rig.scripts());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reopened_silent_board_is_woken_again() {
    let rig = rig_with(discovering(Options {
        initial_hz_a: None,
        ..Options::default()
    }))
    .await;
    wait_until("the first command", || sent(&rig, "stepper_x") == 1).await;
    wait_until("board a dormant", || {
        mode_of(&rig, SERIAL_A) == TraceMode::Dormant
    })
    .await;

    rig.a.unplug();
    wait_until("board a closed", || {
        !rig.live.lock().unwrap().boards[SERIAL_A].open
    })
    .await;
    rig.a.replug(&rig.stop);
    wait_until("the second command", || sent(&rig, "stepper_x") == 2).await;
    tokio::time::sleep(SETTLE).await;

    assert_eq!(sent(&rig, "stepper_x"), 2);
}
