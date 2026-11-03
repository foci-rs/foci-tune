mod fake_moonraker;

use std::path::PathBuf;
use std::time::Duration;

use fake_moonraker::FakeMoonraker;
use foci_tune::moonraker::{ClientConfig, KlippyState, MoonrakerApi, MoonrakerClient};
use serde_json::json;
use tokio::sync::watch;

use foci_tune::moonraker::PrinterState;

const WAIT: Duration = Duration::from_secs(5);

fn client_config(url: String) -> ClientConfig {
    ClientConfig {
        url,
        api_key: None,
        backoff_initial: Duration::from_millis(10),
        backoff_max: Duration::from_millis(40),
    }
}

async fn wait_for(
    rx: &mut watch::Receiver<PrinterState>,
    pred: impl FnMut(&PrinterState) -> bool,
) -> PrinterState {
    tokio::time::timeout(WAIT, rx.wait_for(pred))
        .await
        .expect("state condition not reached in time")
        .expect("state sender dropped")
        .clone()
}

#[tokio::test]
async fn config_root_and_settings_come_from_http() {
    let fake = FakeMoonraker::start().await;
    fake.set_settings(json!({"printer": {"kinematics": "corexy"}}));
    let client = MoonrakerClient::connect(client_config(fake.url()));

    let root = client.config_root().await.unwrap();
    let settings = client.settings().await.unwrap();

    assert_eq!(root, PathBuf::from("/home/pi/printer_data/config"));
    assert_eq!(settings["printer"]["kinematics"], "corexy");
}

#[tokio::test]
async fn mcu_versions_come_from_the_mcu_objects() {
    let fake = FakeMoonraker::start().await;
    fake.set_status(json!({
        "mcu": {"mcu_version": "v0.13.0-688-gb7c0329f", "mcu_build_versions": "gcc: 14"},
        "mcu ouroboros": {"mcu_version": "v0.4.1-30-g4f2b042b", "mcu_build_versions": "fw=68e16122"},
    }));
    let client = MoonrakerClient::connect(client_config(fake.url()));

    let versions = client
        .mcu_versions(&[
            "mcu".to_owned(),
            "ouroboros".to_owned(),
            "absent".to_owned(),
        ])
        .await
        .unwrap();

    assert_eq!(versions.len(), 2);
    assert_eq!(versions["mcu"].version, "v0.13.0-688-gb7c0329f");
    assert_eq!(versions["ouroboros"].version, "v0.4.1-30-g4f2b042b");
    assert_eq!(versions["ouroboros"].build, "fw=68e16122");
}

#[tokio::test]
async fn script_returns_err_with_the_moonraker_message_on_a_bang_response() {
    let fake = FakeMoonraker::start().await;
    fake.fail_scripts_with("!! Unknown command: \"FOCI_NOPE\"");
    let client = MoonrakerClient::connect(client_config(fake.url()));

    let err = client.script("FOCI_NOPE").await.unwrap_err();

    assert!(
        err.to_string()
            .contains("!! Unknown command: \"FOCI_NOPE\""),
        "got: {err}"
    );
    assert_eq!(fake.scripts(), vec!["FOCI_NOPE".to_owned()]);
}

#[tokio::test]
async fn script_succeeds_and_actions_reach_the_server() {
    let fake = FakeMoonraker::start().await;
    let client = MoonrakerClient::connect(client_config(fake.url()));

    client.script("M400").await.unwrap();
    client.emergency_stop().await.unwrap();
    client.firmware_restart().await.unwrap();

    assert_eq!(fake.scripts(), vec!["M400".to_owned()]);
    assert_eq!(fake.emergency_stops(), 1);
    assert_eq!(fake.firmware_restarts(), 1);
}

#[tokio::test]
async fn state_follows_the_subscription_and_status_updates() {
    let fake = FakeMoonraker::start().await;
    let client = MoonrakerClient::connect(client_config(fake.url()));
    let mut state = client.state();

    let initial = wait_for(&mut state, |s| s.klippy == KlippyState::Ready).await;
    assert_eq!(initial.max_velocity, 300.0);
    assert_eq!(initial.print_state, "standby");

    fake.push(
        "notify_status_update",
        json!([{"toolhead": {"homed_axes": "xy"}, "print_stats": {"state": "printing"}}, 2.0]),
    );
    let updated = wait_for(&mut state, |s| s.homed_axes == "xy").await;

    assert_eq!(updated.print_state, "printing");
    assert_eq!(updated.max_accel, 3000.0);
}

#[tokio::test]
async fn state_flips_to_shutdown_on_klippy_shutdown() {
    let fake = FakeMoonraker::start().await;
    let client = MoonrakerClient::connect(client_config(fake.url()));
    let mut state = client.state();
    wait_for(&mut state, |s| s.klippy == KlippyState::Ready).await;

    fake.push("notify_klippy_shutdown", json!([]));
    wait_for(&mut state, |s| s.klippy == KlippyState::Shutdown).await;

    fake.push("notify_klippy_ready", json!([]));
    wait_for(&mut state, |s| s.klippy == KlippyState::Ready).await;

    fake.push("notify_klippy_disconnected", json!([]));
    wait_for(&mut state, |s| s.klippy == KlippyState::Disconnected).await;
}

#[tokio::test]
async fn gcode_responses_are_broadcast() {
    let fake = FakeMoonraker::start().await;
    let client = MoonrakerClient::connect(client_config(fake.url()));
    let mut state = client.state();
    let mut responses = client.gcode_responses();
    wait_for(&mut state, |s| s.klippy == KlippyState::Ready).await;

    fake.push("notify_gcode_response", json!(["// foci: gains applied"]));

    let line = tokio::time::timeout(WAIT, responses.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(line, "// foci: gains applied");
}

#[tokio::test]
async fn dropped_websocket_reconnects_with_backoff_and_reads_disconnected_meanwhile() {
    let fake = FakeMoonraker::start().await;
    let client = MoonrakerClient::connect(client_config(fake.url()));
    let mut state = client.state();
    wait_for(&mut state, |s| s.klippy == KlippyState::Ready).await;

    fake.refuse_websockets(true);
    fake.drop_websockets();
    wait_for(&mut state, |s| s.klippy == KlippyState::Disconnected).await;

    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(state.borrow().klippy, KlippyState::Disconnected);

    fake.refuse_websockets(false);
    wait_for(&mut state, |s| s.klippy == KlippyState::Ready).await;
}

#[tokio::test]
async fn only_moonrakers_own_disconnect_and_ready_count_as_a_klipper_restart() {
    let fake = FakeMoonraker::start().await;
    let client = MoonrakerClient::connect(client_config(fake.url()));
    let mut state = client.state();
    wait_for(&mut state, |s| s.klippy == KlippyState::Ready).await;
    let restarts = client.klippy_restarts();

    fake.drop_websockets();
    wait_for(&mut state, |s| s.klippy == KlippyState::Disconnected).await;
    wait_for(&mut state, |s| s.klippy == KlippyState::Ready).await;
    let after_reconnect = *restarts.borrow();
    fake.push("notify_klippy_disconnected", json!([]));
    fake.push("notify_klippy_ready", json!([]));
    let mut counted = restarts.clone();
    tokio::time::timeout(WAIT, counted.wait_for(|n| *n == 1))
        .await
        .expect("restart not counted")
        .unwrap();

    assert_eq!(after_reconnect, 0);
}
