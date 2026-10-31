mod fake_moonraker;
mod rig;

use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::{Method, Request, StatusCode};
use foci_tune::api::{Api, ApiError, router};
use foci_tune::assets;
use foci_tune::engine::Precondition;
use foci_tune::model::{RunId, SessionId};
use futures_util::{SinkExt, StreamExt};
use rig::*;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

fn app(rig: &Rig) -> Router {
    router(Api {
        engine: rig.engine.clone(),
        moonraker: rig.client.clone(),
        live: rig.live.clone(),
        frames: rig.frames.clone(),
        store: rig.store.clone(),
    })
}

struct Reply {
    status: StatusCode,
    content_type: String,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.text()))
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn error(&self) -> String {
        self.json()["error"].as_str().unwrap_or_default().to_owned()
    }
}

async fn send(app: &Router, method: Method, uri: &str, body: Option<Value>) -> Reply {
    let mut req = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(v) => {
            req = req.header(CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    Reply {
        status,
        content_type,
        body,
    }
}

async fn get(app: &Router, uri: &str) -> Reply {
    send(app, Method::GET, uri, None).await
}

async fn post(app: &Router, uri: &str, body: Value) -> Reply {
    send(app, Method::POST, uri, Some(body)).await
}

fn tracking_body() -> Value {
    json!({
        "stepper": "stepper_x",
        "type": "tracking",
        "pattern": {"start": [100.0, 100.0], "end": [200.0, 200.0]},
        "parameters": {"speeds_mm_s": [50.0, 195.0], "strokes": 1},
        "gains": {"velocity_p": 841, "velocity_i": 412, "position_p": 141},
        "filters": {"velocity_hz": 500, "torque_hz": 1000, "position_hz": 500, "flux_hz": 1000},
    })
}

fn probe_body() -> Value {
    let mut body = tracking_body();
    body["type"] = json!("probe");
    body["parameters"] = json!({
        "speed_mm_s": 195.0,
        "high_gain": 1.3,
        "segments_between_toggles": 3,
        "strokes": 2,
    });
    body
}

fn ladder_body(mut run: Value, axis: &str, values: &[f64]) -> Value {
    run["axis"] = json!(axis);
    run["values"] = json!(values);
    run["repeats"] = json!(1);
    run
}

const TERMINAL: [&str; 7] = [
    "done",
    "not_comparable",
    "failed",
    "aborted",
    "skipped",
    "kill_unconfirmed",
    "killed",
];

async fn finish(rig: &Rig, app: &Router, run: &str) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let reply = get(app, &format!("/api/runs/{run}")).await;
        if reply.status == StatusCode::OK {
            let run = reply.json();
            if TERMINAL.contains(&run["state"].as_str().unwrap()) && rig.engine.is_idle() {
                return run;
            }
        }
        assert!(Instant::now() < deadline, "run {run} did not finish");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn submit(app: &Router, body: Value) -> (String, String) {
    let reply = post(app, "/api/runs", body).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let v = reply.json();
    (
        v["session"].as_str().unwrap().to_owned(),
        v["run"].as_str().unwrap().to_owned(),
    )
}

fn find<'a, V>(list: &'a Value, key: &str, value: V) -> &'a Value
where
    V: Copy + std::fmt::Debug,
    Value: PartialEq<V>,
{
    list.as_array()
        .unwrap()
        .iter()
        .find(|v| v[key] == value)
        .unwrap_or_else(|| panic!("no entry with {key} = {value:?} in {list}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn printer_lists_the_stepper_table_boards_and_preconditions() {
    let rig = rig().await;
    let app = app(&rig);

    let reply = get(&app, "/api/printer").await;

    assert_eq!(reply.status, StatusCode::OK);
    let p = reply.json();
    assert_eq!(p["moonraker"]["klippy"], "ready");
    assert_eq!(p["moonraker"]["homed_axes"], "xyz");
    let x = find(&p["steppers"], "name", "stepper_x");
    assert_eq!(x["channel"], 0);
    assert_eq!(x["serial"], SERIAL_A);
    assert_eq!(x["motor_rotation_distance_mm"], 40.0);
    assert_eq!(x["electrical_model_present"], true);
    assert!(x["unusable"].is_null());
    assert!(x["proposal"]["pattern"]["start"].is_array(), "{x}");
    let pre = find(&p["preconditions"], "stepper", "stepper_x");
    assert_eq!(pre["ok"], true, "{pre}");
    let board = find(&p["boards"], "serial", SERIAL_A);
    assert_eq!(board["open"], true);
    assert_eq!(board["protocol_version"], 25);
    let ch0 = find(&board["channels"], "channel", 0);
    assert_eq!(ch0["control"]["velocity_p"], 841);
    assert_eq!(ch0["control"]["pwm_hz"], 50_000);
    assert!(p["free_bytes"].as_u64().unwrap() > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn printer_names_the_failing_precondition_per_stepper() {
    let rig = rig_with(Options {
        homed: "y",
        ..Options::default()
    })
    .await;
    let app = app(&rig);

    let p = get(&app, "/api/printer").await.json();

    let x = find(&p["preconditions"], "stepper", "stepper_x");
    assert_eq!(x["ok"], false);
    assert_eq!(x["failing"], json!(["axis x is not homed"]));
    assert_eq!(
        find(&p["preconditions"], "stepper", "stepper_y")["failing"],
        json!([])
    );
    assert_eq!(
        find(&p["preconditions"], "stepper", "stepper_y")["ok"],
        true
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_precondition_is_a_409_with_its_reason_and_creates_no_session() {
    let rig = rig_with(Options {
        homed: "",
        ..Options::default()
    })
    .await;
    let app = app(&rig);

    let reply = post(&app, "/api/runs", tracking_body()).await;

    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.error(), "axis x is not homed");
    assert_eq!(
        rig.store.scan().unwrap().len(),
        1,
        "only the rig's own session"
    );
    assert!(rig.scripts().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn preconditions_map_to_409_with_their_display_text() {
    let rig = rig_with(Options {
        initial_hz_a: None,
        ..Options::default()
    })
    .await;
    let app = app(&rig);

    let no_frames = post(&app, "/api/runs", tracking_body()).await;
    assert_eq!(no_frames.status, StatusCode::CONFLICT);
    assert_eq!(no_frames.error(), Precondition::NoTraceFrames.to_string());

    let mut unknown = tracking_body();
    unknown["stepper"] = json!("stepper_q");
    let reply = post(&app, "/api/runs", unknown).await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.error(), "unknown stepper stepper_q");

    let mut settings_without_ppr = rig_settings();
    settings_without_ppr["foci stepper_z"] = json!({"identified_r_count_milli": 700});
    rig.fake.set_settings(settings_without_ppr);
    rig.engine.refresh_steppers();
    let mut z = tracking_body();
    z["stepper"] = json!("stepper_z");
    let reply = post(&app, "/api/runs", z).await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.error(), "encoder_ppr is missing in the foci section");

    let killed = ApiError::from(Precondition::KillPressed);
    assert_eq!(killed.status(), StatusCode::CONFLICT);
    assert_eq!(
        killed.message(),
        "the emergency stop was pressed while the run was being queued"
    );
}

fn rig_settings() -> Value {
    settings(true)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_session_and_a_busy_engine_are_409() {
    let rig = rig().await;
    let app = app(&rig);
    let mut body = tracking_body();
    body["session"] = json!("20990101T000000Z-zzzz");

    let reply = post(&app, "/api/runs", body).await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.error(), "unknown session 20990101T000000Z-zzzz");

    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let (_, first) = submit(&app, tracking_body()).await;
    rig.wait_moving(&mut events, &RunId::parse(&first).unwrap())
        .await;
    let busy = post(&app, "/api/runs", tracking_body()).await;
    assert_eq!(busy.status, StatusCode::CONFLICT);
    assert_eq!(busy.error(), "a run is in flight");
    rig.a.sim(|s| s.gate = None);
    gate.notify_one();
    finish(&rig, &app, &first).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_session_starts_from_the_live_control_state() {
    let rig = rig().await;
    let app = app(&rig);

    let (session, run) = submit(&app, tracking_body()).await;
    let run = finish(&rig, &app, &run).await;

    assert_eq!(run["state"], "done", "{run}");
    let s = get(&app, &format!("/api/sessions/{session}")).await.json();
    assert_eq!(s["session"]["stepper"], "stepper_x");
    assert_eq!(s["session"]["gains"]["velocity_p"], 841);
    assert_eq!(s["session"]["gains"]["velocity_i"], 412);
    assert_eq!(s["session"]["filters"]["torque_hz"], 1000);
    assert_eq!(s["runs"].as_array().unwrap().len(), 1);

    let mut other_setup = tracking_body();
    other_setup["session"] = json!(session);
    other_setup["stepper"] = json!("stepper_y");
    let reply = post(&app, "/api/runs", other_setup).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.json()["field"], "stepper");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_config_block_renders_the_run_values_with_units() {
    let rig = rig().await;
    let app = app(&rig);
    let mut body = tracking_body();
    body["gains"] = json!({"velocity_p": 841, "velocity_i": 618, "position_p": 141});
    body["filters"] = json!({"velocity_hz": 0, "torque_hz": 0, "position_hz": 0, "flux_hz": 0});
    let (_, run) = submit(&app, body).await;
    assert_eq!(finish(&rig, &app, &run).await["state"], "done");

    let reply = get(&app, &format!("/api/runs/{run}/config-block")).await;

    assert_eq!(reply.status, StatusCode::OK);
    assert!(
        reply.content_type.starts_with("text/plain"),
        "{}",
        reply.content_type
    );
    assert_eq!(
        reply.text(),
        "[foci stepper_x]\n\
         pid_velocity_p: 841      # 3.285 Q8.8\n\
         pid_velocity_i: 618      # 75.0 Hz corner at 50 kHz PWM\n\
         pid_position_p: 141      # 0.551 Q8.8\n\
         pid_position_i: 0\n\
         velocity_filter_hz: 0\n\
         torque_filter_hz: 0\n\
         position_filter_hz: 0\n\
         flux_filter_hz: 0\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stopped_ladder_lists_its_remaining_runs_as_skipped_with_the_reason() {
    let rig = rig().await;
    rig.a.sim(|s| {
        s.zeta_by_position_p.insert(141, 0.03);
    });
    let app = app(&rig);

    let reply = post(
        &app,
        "/api/ladders",
        ladder_body(probe_body(), "position_p", &[141.0, 200.0, 240.0]),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let v = reply.json();
    let session = v["session"].as_str().unwrap();
    let ids: Vec<&str> = v["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 3);
    finish(&rig, &app, ids[0]).await;

    let s = get(&app, &format!("/api/sessions/{session}")).await.json();

    assert_eq!(s["session"]["ladder"]["axis"], "position_p");
    let runs = &s["runs"];
    assert_eq!(runs.as_array().unwrap().len(), 3);
    assert_eq!(find(runs, "id", ids[0])["state"], "done");
    for id in &ids[1..] {
        let run = find(runs, "id", *id);
        assert_eq!(run["state"], "skipped", "{run}");
        let reason = run["reason"].as_str().unwrap();
        assert!(
            reason.starts_with("ladder stopped: median zeta") && reason.contains("below the floor"),
            "{reason}"
        );
    }
    assert_eq!(rig.stimuli(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kill_reports_killing_then_killed() {
    let rig = rig().await;
    let app = app(&rig);
    rig.fake.script_emergency_stop(
        None,
        Duration::from_millis(100),
        Some(Duration::from_millis(20)),
    );
    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let (_, run) = submit(&app, tracking_body()).await;
    rig.wait_moving(&mut events, &RunId::parse(&run).unwrap())
        .await;

    let killer = {
        let app = app.clone();
        tokio::spawn(async move { post(&app, "/api/kill", json!({})).await })
    };
    let deadline = Instant::now() + WAIT;
    while get(&app, &format!("/api/runs/{run}")).await.json()["state"] != "killing" {
        assert!(Instant::now() < deadline, "never killing");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let reply = killer.await.unwrap();

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["state"], "killed");
    assert_eq!(reply.json()["run"], run.as_str());
    assert_eq!(rig.fake.emergency_stops(), 1);
    assert_eq!(finish(&rig, &app, &run).await["state"], "killed");
    rig.a.sim(|s| s.gate = None);
    gate.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unconfirmed_kill_is_reported_and_gates_the_next_run() {
    let rig = rig().await;
    let app = app(&rig);
    rig.fake
        .script_emergency_stop(Some(500), Duration::ZERO, None);
    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let (_, run) = submit(&app, tracking_body()).await;
    rig.wait_moving(&mut events, &RunId::parse(&run).unwrap())
        .await;

    let reply = post(&app, "/api/kill", json!({})).await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["state"], "kill_unconfirmed");
    assert_eq!(finish(&rig, &app, &run).await["state"], "kill_unconfirmed");
    let printer = get(&app, "/api/printer").await.json();
    assert_eq!(printer["engine"]["unconfirmed_kill"], run.as_str());
    let next = post(&app, "/api/runs", tracking_body()).await;
    assert_eq!(next.status, StatusCode::CONFLICT);
    assert_eq!(next.error(), Precondition::KillUnconfirmed.to_string());
    rig.a.sim(|s| s.gate = None);
    gate.notify_one();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abort_lets_the_current_run_finish_and_marks_the_queue_aborted() {
    let rig = rig().await;
    let app = app(&rig);
    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let reply = post(
        &app,
        "/api/ladders",
        ladder_body(tracking_body(), "velocity_p", &[841.0, 941.0, 1041.0]),
    )
    .await;
    let ids: Vec<String> = reply.json()["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap().to_owned())
        .collect();
    rig.wait_moving(&mut events, &RunId::parse(&ids[0]).unwrap())
        .await;

    let reply = post(&app, &format!("/api/runs/{}/abort", ids[0]), json!({})).await;

    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.json()["aborted"], json!([ids[1], ids[2]]));
    rig.a.sim(|s| s.gate = None);
    gate.notify_one();
    assert_eq!(finish(&rig, &app, &ids[0]).await["state"], "done");
    for id in &ids[1..] {
        assert_eq!(
            get(&app, &format!("/api/runs/{id}")).await.json()["state"],
            "aborted"
        );
    }
    let again = post(&app, &format!("/api/runs/{}/abort", ids[0]), json!({})).await;
    assert_eq!(again.status, StatusCode::CONFLICT);
    assert_eq!(rig.stimuli(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revert_writes_the_session_starting_values() {
    let rig = rig().await;
    let app = app(&rig);

    let reply = post(
        &app,
        &format!("/api/sessions/{}/revert", rig.session),
        json!({}),
    )
    .await;

    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let scripts = rig.scripts();
    assert!(
        scripts.iter().any(|s| s.starts_with(
            "FOCI_SET_GAINS STEPPER=stepper_x VELOCITY_P=3.51562500 VELOCITY_I=0.009765625000 \
             POSITION_P=0.46875000 POSITION_I=0"
        )),
        "{scripts:?}"
    );
    assert!(
        scripts
            .iter()
            .any(|s| s.contains("VELOCITY_HZ=200 TORQUE_HZ=4000"))
    );
    let missing = post(
        &app,
        "/api/sessions/20990101T000000Z-zzzz/revert",
        json!({}),
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert!(missing.error().contains("20990101T000000Z-zzzz"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_validation_errors_are_400_with_the_field_named() {
    let rig = rig().await;
    let app = app(&rig);
    let cases: Vec<(Value, &str)> = vec![
        (
            {
                let mut b = tracking_body();
                b["gains"]["velocity_p"] = json!(40_000);
                b
            },
            "gains.velocity_p",
        ),
        (
            {
                let mut b = tracking_body();
                b.as_object_mut().unwrap().remove("stepper");
                b
            },
            "stepper",
        ),
        (
            {
                let mut b = tracking_body();
                b["type"] = json!("sprint");
                b
            },
            "type",
        ),
        (
            {
                let mut b = tracking_body();
                b["parameters"] = json!({"strokes": "many"});
                b
            },
            "parameters",
        ),
        (
            {
                let mut b = tracking_body();
                b["pattern"] = json!({"start": [1.0, 1.0], "end": [200.0, 200.0]});
                b
            },
            "pattern",
        ),
        (
            {
                let mut b = tracking_body();
                b["session"] = json!("../etc");
                b
            },
            "session",
        ),
    ];
    for (body, field) in cases {
        let reply = post(&app, "/api/runs", body).await;
        assert_eq!(
            reply.status,
            StatusCode::BAD_REQUEST,
            "{field}: {}",
            reply.text()
        );
        assert_eq!(reply.json()["field"], field, "{}", reply.text());
        assert!(!reply.error().is_empty());
    }

    let axis = post(
        &app,
        "/api/ladders",
        ladder_body(tracking_body(), "torque_p", &[1.0]),
    )
    .await;
    assert_eq!(axis.status, StatusCode::BAD_REQUEST);
    assert_eq!(axis.json()["field"], "axis");
    let range = post(
        &app,
        "/api/ladders",
        ladder_body(tracking_body(), "velocity_p", &[841.0, 99_999.0]),
    )
    .await;
    assert_eq!(range.status, StatusCode::BAD_REQUEST);
    assert_eq!(range.json()["field"], "values");
    assert!(range.error().contains("99999"), "{}", range.error());

    let not_json = send(&app, Method::POST, "/api/runs", None).await;
    assert_eq!(not_json.status, StatusCode::BAD_REQUEST);
    assert!(!not_json.error().is_empty());
    assert!(rig.scripts().is_empty());
    assert_eq!(rig.store.scan().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_kinematics_is_refused_before_anything_is_queued() {
    let rig = rig().await;
    let mut delta = rig_settings();
    delta["printer"]["kinematics"] = json!("delta");
    rig.fake.set_settings(delta);
    rig.engine.refresh_steppers();
    let app = app(&rig);

    let reply = post(&app, "/api/runs", tracking_body()).await;

    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(
        reply.error(),
        "kinematics delta is not supported by the stimulus composer"
    );
    assert!(rig.scripts().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ids_in_urls_are_validated_before_the_store_is_touched() {
    let rig = rig().await;
    let app = app(&rig);

    for uri in [
        "/api/runs/bad.id",
        "/api/runs/bad.id/series",
        "/api/runs/bad.id/capture",
        "/api/runs/bad.id/config-block",
        "/api/sessions/bad.id",
    ] {
        let reply = get(&app, uri).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(reply.error(), "invalid id `bad.id`", "{uri}");
    }
    let revert = post(&app, "/api/sessions/bad.id/revert", json!({})).await;
    assert_eq!(revert.status, StatusCode::BAD_REQUEST);
    let missing = get(&app, "/api/runs/20990101T000000Z-zzzz").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert!(missing.error().contains("20990101T000000Z-zzzz"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runs_series_and_capture_are_served_for_a_finished_run() {
    let rig = rig().await;
    let app = app(&rig);
    let (session, run) = submit(&app, probe_body()).await;
    assert_eq!(finish(&rig, &app, &run).await["state"], "done");

    let series = get(&app, &format!("/api/runs/{run}/series")).await;
    let capture = get(&app, &format!("/api/runs/{run}/capture")).await;
    let sessions = get(&app, "/api/sessions").await.json();

    assert_eq!(series.status, StatusCode::OK);
    let s = series.json();
    assert_eq!(s["format_version"], 2);
    let ch = find(&s["channels"], "channel", 0);
    assert!(ch["probe"]["velocity"]["edges"].as_u64().unwrap() >= 20);
    assert_eq!(capture.status, StatusCode::OK);
    assert_eq!(capture.content_type, "application/zstd");
    let id = SessionId::parse(&session).unwrap();
    let on_disk = std::fs::read(rig.store.capture_path(&id, &RunId::parse(&run).unwrap())).unwrap();
    assert_eq!(capture.body, on_disk);
    assert_eq!(sessions.as_array().unwrap().len(), 2);
    let listed = find(&sessions, "id", session.as_str());
    assert_eq!(listed["runs"][0]["run"]["id"], run.as_str());
    assert_eq!(listed["runs"][0]["state"], "done");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ui_is_served_with_spa_fallback_and_unknown_api_paths_are_json_404() {
    let rig = rig().await;
    let app = app(&rig);
    let index = assets::index_html().expect("embedded index.html");

    let root = get(&app, "/").await;
    let deep = get(&app, "/sessions/20261030T174512Z-k3q9").await;
    let api = get(&app, "/api/nope").await;

    assert_eq!(root.status, StatusCode::OK);
    assert!(root.content_type.starts_with("text/html"));
    assert_eq!(root.body, index.data.as_ref());
    assert_eq!(deep.status, StatusCode::OK);
    assert_eq!(deep.body, index.data.as_ref());
    assert_eq!(api.status, StatusCode::NOT_FOUND);
    assert!(api.content_type.starts_with("application/json"));
    assert!(api.error().contains("/api/nope"), "{}", api.text());
    if let Some(name) = assets::list().into_iter().find(|n| n.ends_with(".js")) {
        let js = get(&app, &format!("/{name}")).await;
        assert_eq!(js.status, StatusCode::OK);
        assert!(
            js.content_type.contains("javascript"),
            "{}",
            js.content_type
        );
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_message(ws: &mut Socket, within: Duration) -> Option<Value> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Err(_) => return None,
            Ok(Some(Ok(Message::Text(t)))) => {
                return Some(serde_json::from_str(t.as_str()).unwrap());
            }
            Ok(Some(Ok(_))) => {}
            Ok(other) => panic!("socket ended: {other:?}"),
        }
    }
}

/// Reads messages until the `run_result` for `run`, then for `linger` more; returns them all.
async fn collect_run(ws: &mut Socket, run: &str, linger: Duration) -> Vec<Value> {
    let mut seen = Vec::new();
    let deadline = Instant::now() + WAIT;
    loop {
        let m = next_message(ws, WAIT).await.expect("run_result");
        let done = m["type"] == "run_result" && m["run"]["id"] == run;
        seen.push(m);
        if done {
            break;
        }
        assert!(Instant::now() < deadline, "no run_result for {run}");
    }
    while let Some(m) = next_message(ws, linger).await {
        seen.push(m);
    }
    seen
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_event_stream_sends_printer_results_and_live_only_when_subscribed() {
    let rig = rig().await;
    let app = app(&rig);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = {
        let app = app.clone();
        tokio::spawn(async move { axum::serve(listener, app).await })
    };
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/events"))
        .await
        .unwrap();

    let first = next_message(&mut ws, WAIT)
        .await
        .expect("a message on connect");
    assert_eq!(first["type"], "printer");
    assert_eq!(
        find(&first["steppers"], "name", "stepper_x")["serial"],
        SERIAL_A
    );

    let (session, run) = submit(&app, tracking_body()).await;
    let quiet = collect_run(&mut ws, &run, Duration::from_millis(200)).await;
    let result = quiet.iter().find(|m| m["type"] == "run_result").unwrap();
    assert_eq!(result["run"]["state"], "done");
    assert_eq!(result["session"], session.as_str());
    assert!(
        quiet.iter().any(|m| m["type"] == "run_state"
            && m["run"] == run.as_str()
            && m["state"] == "moving")
    );
    assert!(
        quiet.iter().all(|m| m["type"] != "live"),
        "live before subscribing"
    );

    ws.send(Message::text(json!({"subscribe": "live"}).to_string()))
        .await
        .unwrap();
    loop {
        let m = next_message(&mut ws, WAIT)
            .await
            .expect("subscription acknowledged");
        if m["type"] == "subscribed" {
            assert_eq!(m["stream"], "live");
            break;
        }
    }
    let mut body = tracking_body();
    body["session"] = json!(session);
    let (_, second) = submit(&app, body).await;
    let watched = collect_run(&mut ws, &second, Duration::from_millis(300)).await;

    let live: Vec<&Value> = watched.iter().filter(|m| m["type"] == "live").collect();
    assert!(!live.is_empty(), "no live batch while subscribed");
    let batch = live.iter().find(|m| m["serial"] == SERIAL_A).unwrap();
    let ch0 = find(&batch["channels"], "channel", 0);
    let n = ch0["ticks"].as_array().unwrap().len();
    assert!(n > 0);
    assert_eq!(ch0["position_error"].as_array().unwrap().len(), n);
    assert_eq!(ch0["torque"].as_array().unwrap().len(), n);
    server.abort();
}

async fn serve_app(app: &Router) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = app.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, task)
}

async fn connect(addr: std::net::SocketAddr) -> Socket {
    tokio_tungstenite::connect_async(format!("ws://{addr}/api/events"))
        .await
        .unwrap()
        .0
}

async fn next_of_type(ws: &mut Socket, kind: &str, within: Duration) -> Option<Value> {
    let deadline = Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let m = next_message(ws, left).await?;
        if m["type"] == kind {
            return Some(m);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn printer_snapshots_are_shared_and_the_stepper_table_is_read_once() {
    let rig = rig().await;
    let app = app(&rig);
    let (addr, server) = serve_app(&app).await;
    let mut clients = Vec::new();
    for _ in 0..3 {
        clients.push(connect(addr).await);
    }

    for ws in &mut clients {
        let printer = next_of_type(ws, "printer", WAIT)
            .await
            .expect("printer on connect");
        assert_eq!(
            find(&printer["steppers"], "name", "stepper_x")["serial"],
            SERIAL_A
        );
    }
    tokio::time::sleep(Duration::from_millis(2500)).await;

    let queries = rig.fake.settings_queries();
    assert!(
        (1..=2).contains(&queries),
        "{queries} settings queries for 3 clients"
    );
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hung_settings_query_does_not_delay_run_events() {
    let rig = rig().await;
    let app = app(&rig);
    let (addr, server) = serve_app(&app).await;
    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let (_, run) = submit(&app, tracking_body()).await;
    rig.wait_moving(&mut events, &RunId::parse(&run).unwrap())
        .await;
    let mut ws = connect(addr).await;
    next_of_type(&mut ws, "printer", WAIT)
        .await
        .expect("printer on connect");
    rig.fake.delay_settings(Duration::from_secs(30));
    rig.engine.refresh_steppers();
    tokio::time::sleep(Duration::from_millis(1200)).await;

    let killer = {
        let engine = rig.engine.clone();
        tokio::spawn(async move { engine.kill().await })
    };
    let killing = loop {
        let m = next_of_type(&mut ws, "run_state", Duration::from_secs(1))
            .await
            .expect("run_state within a second while settings hang");
        if m["run"] == run.as_str() && m["state"] == "killing" {
            break m;
        }
    };

    assert_eq!(killing["state"], "killing");
    killer.await.unwrap();
    rig.a.sim(|s| s.gate = None);
    gate.notify_one();
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_requests_are_400_naming_the_parameter() {
    let rig = rig().await;
    let app = app(&rig);
    let mut strokes = tracking_body();
    strokes["parameters"]["strokes"] = json!(1_000_000_000u64);
    let mut speeds = tracking_body();
    speeds["parameters"]["speeds_mm_s"] = json!(vec![50.0; 11]);
    let values: Vec<f64> = (0..30).map(|k| 800.0 + f64::from(k)).collect();

    let strokes = post(&app, "/api/runs", strokes).await;
    let speeds = post(&app, "/api/runs", speeds).await;
    let ladder = post(
        &app,
        "/api/ladders",
        ladder_body(tracking_body(), "velocity_p", &values),
    )
    .await;

    assert_eq!(strokes.status, StatusCode::BAD_REQUEST);
    assert_eq!(strokes.json()["field"], "parameters.strokes");
    assert!(
        strokes.error().contains("at most 50"),
        "{}",
        strokes.error()
    );
    assert_eq!(speeds.status, StatusCode::BAD_REQUEST);
    assert_eq!(speeds.json()["field"], "parameters.speeds_mm_s");
    assert_eq!(ladder.status, StatusCode::BAD_REQUEST);
    assert_eq!(ladder.json()["field"], "values");
    assert!(rig.scripts().is_empty());
    assert_eq!(rig.store.scan().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn printer_names_every_failing_precondition_of_a_stepper() {
    let rig = rig_with(Options {
        homed: "y",
        electrical_model: false,
        ..Options::default()
    })
    .await;
    let app = app(&rig);

    let p = get(&app, "/api/printer").await.json();

    let x = find(&p["preconditions"], "stepper", "stepper_x");
    assert_eq!(x["ok"], false);
    assert_eq!(
        x["failing"],
        json!([
            "axis x is not homed",
            "the stepper has no identified electrical model"
        ])
    );
}
