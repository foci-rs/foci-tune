mod fake_moonraker;
mod rig;

use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::{Method, Request, StatusCode};
use foci_tune::api::{Api, router};
use foci_tune::model::{Filters, Gains, SessionId};
use foci_tune::tune::{TuneScheduler, TuneSetup, TuneState};
use futures_util::StreamExt;
use rig::*;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

fn snapshot() -> (Gains, Filters) {
    (
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
    )
}

const REASON: &str = "abandoned, restore failed: gain write unconfirmed";

fn app(rig: &Rig) -> (Router, TuneScheduler<foci_tune::moonraker::MoonrakerClient>) {
    let tunes = TuneScheduler::new(rig.engine.clone(), rig.store.clone());
    let router = router(Api {
        engine: rig.engine.clone(),
        moonraker: rig.client.clone(),
        live: rig.live.clone(),
        frames: rig.frames.clone(),
        store: rig.store.clone(),
        tunes: tunes.clone(),
    });
    (router, tunes)
}

struct Reply {
    status: StatusCode,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&self.body)))
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
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    Reply { status, body }
}

async fn get(app: &Router, uri: &str) -> Reply {
    send(app, Method::GET, uri, None).await
}

async fn post(app: &Router, uri: &str, body: Value) -> Reply {
    send(app, Method::POST, uri, Some(body)).await
}

async fn put(app: &Router, uri: &str, body: Value) -> Reply {
    send(app, Method::PUT, uri, Some(body)).await
}

async fn post_empty(app: &Router, uri: &str) -> Reply {
    send(app, Method::POST, uri, None).await
}

fn setup_body(speeds: &[f64]) -> Value {
    json!({
        "stepper": "stepper_x",
        "speeds_mm_s": speeds,
        "pattern": {"start": [100.0, 100.0], "end": [200.0, 200.0]},
        "probe": {
            "speed_mm_s": 195.0,
            "high_gain": 1.3,
            "segments_between_toggles": 3,
            "strokes": 2,
        },
        "tracking_strokes": 1,
        "standstill": {"rests": 2, "dwell_s": 0.2},
        "gates": {"damping_floor": 0.05, "marginal": 0.08},
        "include_marginal": false,
    })
}

fn step_body(lower_k: i32, upper_k: i32) -> Value {
    json!({"seed": "live", "repeats": 1, "lower_k": lower_k, "upper_k": upper_k})
}

async fn create(app: &Router) -> String {
    let reply = post(app, "/api/tunes", setup_body(&[50.0])).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.error());
    reply.json()["id"].as_str().unwrap().to_owned()
}

async fn wait_state(app: &Router, id: &str, state: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let tune = get(app, &format!("/api/tunes/{id}")).await.json();
        if tune["state"] == json!(state) {
            return tune;
        }
        assert!(Instant::now() < deadline, "tune stuck: {}", tune["state"]);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tune_needs_one_to_three_speeds() {
    let rig = rig().await;
    let (app, _) = app(&rig);

    for speeds in [&[][..], &[50.0, 100.0, 150.0, 195.0][..]] {
        let reply = post(&app, "/api/tunes", setup_body(speeds)).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{speeds:?}");
        assert_eq!(reply.json()["field"], "speeds_mm_s");
    }
    for speeds in [&[50.0][..], &[50.0, 100.0, 150.0][..]] {
        let reply = post(&app, "/api/tunes", setup_body(speeds)).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.error());
        let id = reply.json()["id"].as_str().unwrap().to_owned();
        post_empty(&app, &format!("/api/tunes/{id}/abandon")).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_probe_speed_must_be_explicit() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let mut body = setup_body(&[50.0]);
    body["probe"].as_object_mut().unwrap().remove("speed_mm_s");

    let reply = post(&app, "/api/tunes", body).await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.json()["field"], "probe.speed_mm_s");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gates_that_would_disable_a_check_are_rejected() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let cases = [
        ("damping_floor", json!(-1.0)),
        ("marginal", json!(0.05)),
        ("standstill_factor", json!(0.5)),
    ];

    for (field, bad) in &cases {
        let mut body = setup_body(&[50.0]);
        body["gates"][field] = bad.clone();

        let reply = post(&app, "/api/tunes", body).await;

        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{field}");
        assert_eq!(reply.json()["field"], format!("gates.{field}"));
    }
    assert_eq!(get(&app, "/api/tunes").await.json(), json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tune_without_a_standstill_factor_stores_none_and_one_with_it_keeps_it() {
    let rig1 = self::rig().await;
    let (app1, _) = self::app(&rig1);

    let reply = post(&app1, "/api/tunes", setup_body(&[50.0])).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.error());
    assert_eq!(
        reply.json()["setup"]["gates"]["standstill_factor"],
        Value::Null
    );

    let rig2 = self::rig().await;
    let (app2, _) = self::app(&rig2);
    let mut body = setup_body(&[50.0]);
    body["gates"]["standstill_factor"] = json!(2.0);
    let reply = post(&app2, "/api/tunes", body).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.error());
    assert_eq!(reply.json()["setup"]["gates"]["standstill_factor"], 2.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_non_numeric_standstill_factor_is_malformed_input() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let mut body = setup_body(&[50.0]);
    body["gates"]["standstill_factor"] = json!("two");

    let reply = post(&app, "/api/tunes", body).await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.json().get("field").is_none_or(Value::is_null));
    assert_eq!(get(&app, "/api/tunes").await.json(), json!([]));
}

#[test]
fn a_stored_setup_keeps_its_numeric_factor_and_reads_a_missing_one_as_off() {
    let without: TuneSetup = serde_json::from_value(setup_body(&[195.0])).unwrap();
    assert_eq!(without.gates.standstill_factor, None);
    let mut body = setup_body(&[195.0]);
    body["gates"]["standstill_factor"] = json!(2.0);
    let with: TuneSetup = serde_json::from_value(body).unwrap();
    assert_eq!(with.gates.standstill_factor, Some(2.0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closely_spaced_probe_toggles_are_rejected() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let mut body = setup_body(&[50.0]);
    body["probe"]["segments_between_toggles"] = json!(1);

    let reply = post(&app, "/api/tunes", body).await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.error());
    assert_eq!(reply.json()["field"], "probe.segments_between_toggles");
    assert!(
        reply.error().contains("toggle spacing"),
        "{}",
        reply.error()
    );
    assert_eq!(get(&app, "/api/tunes").await.json(), json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_locked_stepper_refuses_a_step_with_the_lock_reason() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;
    rig.engine
        .lock_stepper("stepper_x", REASON.to_owned(), snapshot())
        .unwrap();

    let prepare = post(&app, &format!("/api/tunes/{id}/steps"), step_body(0, 0)).await;

    assert_eq!(prepare.status, StatusCode::CONFLICT);
    assert!(prepare.error().contains(REASON), "{}", prepare.error());
    let detail = get(&app, &format!("/api/tunes/{id}")).await.json();
    assert_eq!(detail["lock"], REASON);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_step_out_of_order_is_a_conflict() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;
    post(&app, &format!("/api/tunes/{id}/steps"), step_body(0, 0)).await;

    let again = post(&app, &format!("/api/tunes/{id}/steps"), step_body(0, 0)).await;

    assert_eq!(again.status, StatusCode::CONFLICT);
    assert!(!again.error().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_tune_is_not_found() {
    let rig = rig().await;
    let (app, _) = app(&rig);

    let reply = get(&app, "/api/tunes/20260101T000000Z-aaaaaa").await;

    assert_eq!(reply.status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_prepared_step_submits_nothing_and_a_choice_waits_for_the_step() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;

    let plan = post(&app, &format!("/api/tunes/{id}/steps"), step_body(0, 0)).await;

    assert_eq!(plan.status, StatusCode::OK, "{}", plan.error());
    let plan = plan.json();
    assert!(plan["announcement"].as_str().unwrap().contains("stepper_x"));
    assert!(plan["run_count"].as_u64().unwrap() > 0);
    assert_eq!(rig.stimuli(), 0);
    let early = post(
        &app,
        &format!("/api/tunes/{id}/choice"),
        json!({"choice": "recommended"}),
    )
    .await;
    assert_eq!(early.status, StatusCode::CONFLICT);
    assert_eq!(rig.stimuli(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_listed_session_names_the_tune_and_step_it_belongs_to_or_none() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;
    let plan = post(&app, &format!("/api/tunes/{id}/steps"), step_body(0, 0)).await;
    assert_eq!(plan.status, StatusCode::OK, "{}", plan.error());
    let tune = get(&app, &format!("/api/tunes/{id}")).await.json();
    let guided: Vec<String> = ["probe", "tracking", "standstill"]
        .iter()
        .map(|kind| {
            tune["steps"][0]["sessions"][kind]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    let template = rig
        .store
        .read_session(&SessionId::parse(&guided[0]).unwrap())
        .unwrap();
    let manual = rig.store.new_session(&template).unwrap();

    let listed = get(&app, "/api/sessions").await.json();
    let single = get(&app, &format!("/api/sessions/{}", guided[1]))
        .await
        .json();

    for session in &guided {
        let entry = listed
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == session.as_str())
            .unwrap();
        assert_eq!(entry["tune"], json!({"id": id, "step": 1}), "{session}");
    }
    let entry = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == manual.as_str())
        .unwrap();
    assert_eq!(entry["tune"], Value::Null);
    assert_eq!(single["tune"], json!({"id": id, "step": 1}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn omitted_rungs_survive_a_reload_and_the_block_waits_for_all_steps() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;

    let plan = post(&app, &format!("/api/tunes/{id}/steps"), step_body(-6, 6)).await;
    assert_eq!(plan.status, StatusCode::OK, "{}", plan.error());

    let detail = get(&app, &format!("/api/tunes/{id}")).await.json();
    let step = &detail["steps"][0];
    let shown = step["rungs"].as_array().unwrap().len();
    let notes = step["notes"].as_array().unwrap().len();
    assert_eq!(shown + notes, 13, "{step}");
    assert_eq!(detail["config_block"], Value::Null);

    post_empty(&app, &format!("/api/tunes/{id}/steps/confirm")).await;
    wait_state(&app, &id, "AwaitingChoice").await;
    let chosen = post(
        &app,
        &format!("/api/tunes/{id}/choice"),
        json!({"choice": {"rung": 0}}),
    )
    .await;
    assert_eq!(chosen.status, StatusCode::OK, "{}", chosen.error());
    let detail = get(&app, &format!("/api/tunes/{id}")).await.json();
    assert_eq!(detail["state"], json!("Ready"));
    assert_eq!(detail["config_block"], Value::Null);
    let listed = get(&app, "/api/tunes").await.json();
    assert_eq!(listed[0]["id"], json!(id));
    assert_eq!(listed[0]["stepper"], "stepper_x");
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_of_type(ws: &mut Socket, kind: &str, within: Duration) -> Option<Value> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Err(_) => return None,
            Ok(Some(Ok(Message::Text(t)))) => {
                let m: Value = serde_json::from_str(t.as_str()).unwrap();
                if m["type"] == kind {
                    return Some(m);
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(other) => panic!("socket ended: {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_event_stream_broadcasts_a_tune_when_a_rung_changes() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = {
        let app = app.clone();
        tokio::spawn(async move { axum::serve(listener, app).await })
    };
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/events"))
        .await
        .unwrap();
    next_of_type(&mut ws, "printer", WAIT).await.unwrap();
    let id = create(&app).await;
    post(&app, &format!("/api/tunes/{id}/steps"), step_body(0, 1)).await;
    post_empty(&app, &format!("/api/tunes/{id}/steps/confirm")).await;

    let mut finished_rungs = 0;
    let deadline = Instant::now() + Duration::from_secs(60);
    while finished_rungs == 0 {
        let m = next_of_type(&mut ws, "tune", WAIT)
            .await
            .expect("tune event");
        assert_eq!(m["tune"]["id"], json!(id));
        finished_rungs = m["tune"]["steps"][0]["rungs"]
            .as_array()
            .map_or(0, |rungs| {
                rungs
                    .iter()
                    .filter(|r| r["ended_unix_ms"].is_number())
                    .count()
            });
        assert!(Instant::now() < deadline);
    }
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_decision_waits_for_a_paused_step() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;
    let early = post(
        &app,
        &format!("/api/tunes/{id}/decision"),
        json!({"decision": "continue"}),
    )
    .await;
    assert_eq!(early.status, StatusCode::CONFLICT);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paused_tune_accepts_stop_and_awaits_a_choice() {
    let rig = rig().await;
    rig.a.sim(|s| {
        let no_edges = Profile {
            probe_edges: 0,
            ..Profile::default()
        };
        s.profile_by_velocity_p.insert(841, no_edges);
    });
    let (app, _) = app(&rig);
    let id = create(&app).await;
    post(&app, &format!("/api/tunes/{id}/steps"), step_body(0, 1)).await;
    post_empty(&app, &format!("/api/tunes/{id}/steps/confirm")).await;
    wait_state(&app, &id, "Paused").await;

    let bad = post(
        &app,
        &format!("/api/tunes/{id}/decision"),
        json!({"decision": "skip"}),
    )
    .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    let stopped = post(
        &app,
        &format!("/api/tunes/{id}/decision"),
        json!({"decision": "stop"}),
    )
    .await;
    assert_eq!(stopped.status, StatusCode::OK, "{}", stopped.error());
    let detail = wait_state(&app, &id, "AwaitingChoice").await;
    assert_eq!(detail["steps"][0]["paused"], Value::Null);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tune_without_steps_can_have_its_setup_replaced() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;
    let before = get(&app, &format!("/api/tunes/{id}")).await.json();
    let mut body = setup_body(&[100.0, 200.0, 400.0]);
    body["include_marginal"] = json!(true);

    let reply = put(&app, &format!("/api/tunes/{id}/setup"), body).await;

    assert_eq!(reply.status, StatusCode::OK, "{}", reply.error());
    let after = get(&app, &format!("/api/tunes/{id}")).await.json();
    assert_eq!(after["setup"]["speeds_mm_s"], json!([100.0, 200.0, 400.0]));
    assert_eq!(after["setup"]["include_marginal"], true);
    assert_eq!(after["id"], before["id"]);
    assert_eq!(after["snapshot"], before["snapshot"]);
    assert_eq!(after["created_unix_s"], before["created_unix_s"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_setup_is_frozen_once_a_step_is_prepared() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;
    let plan = post(&app, &format!("/api/tunes/{id}/steps"), step_body(0, 0)).await;
    assert_eq!(plan.status, StatusCode::OK, "{}", plan.error());

    let reply = put(
        &app,
        &format!("/api/tunes/{id}/setup"),
        setup_body(&[100.0]),
    )
    .await;

    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert!(!reply.error().is_empty());
    let after = get(&app, &format!("/api/tunes/{id}")).await.json();
    assert_eq!(after["setup"]["speeds_mm_s"], json!([50.0]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stepper_of_a_tune_cannot_change() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;
    let mut body = setup_body(&[50.0]);
    body["stepper"] = json!("stepper_y");

    let reply = put(&app, &format!("/api/tunes/{id}/setup"), body).await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.error());
    let after = get(&app, &format!("/api/tunes/{id}")).await.json();
    assert_eq!(after["setup"]["stepper"], "stepper_x");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replacement_setup_is_validated_like_a_new_one() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;
    let mut closely_spaced = setup_body(&[50.0]);
    closely_spaced["probe"]["segments_between_toggles"] = json!(1);
    let mut bad_gate = setup_body(&[50.0]);
    bad_gate["gates"]["marginal"] = json!(0.05);

    for (body, field) in [
        (setup_body(&[]), "speeds_mm_s"),
        (closely_spaced, "probe.segments_between_toggles"),
        (bad_gate, "gates.marginal"),
    ] {
        let reply = put(&app, &format!("/api/tunes/{id}/setup"), body).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{field}");
        assert_eq!(reply.json()["field"], field);
    }
    let after = get(&app, &format!("/api/tunes/{id}")).await.json();
    assert_eq!(after["setup"]["speeds_mm_s"], json!([50.0]));
}

#[test]
fn every_state_that_freezes_the_setup_says_why_and_what_to_do() {
    let cases = [
        (
            TuneState::Abandoned,
            false,
            "was abandoned",
            "Start a new tune",
        ),
        (TuneState::Done, true, "is finished", "Start a new tune"),
        (
            TuneState::RestorePending,
            false,
            "restoring the starting gains",
            "right now",
        ),
        (
            TuneState::RestoreFailed("gain write unconfirmed".into()),
            false,
            "restoring the starting gains",
            "right now",
        ),
        (
            TuneState::Announced,
            true,
            "already started its first step",
            "abandon it",
        ),
        (
            TuneState::Running,
            true,
            "already started its first step",
            "abandon it",
        ),
        (
            TuneState::AwaitingChoice,
            true,
            "already started its first step",
            "abandon it",
        ),
        (
            TuneState::Paused,
            true,
            "already started its first step",
            "abandon it",
        ),
        (
            TuneState::Ready,
            true,
            "already started its first step",
            "abandon it",
        ),
    ];
    for (state, has_steps, why, advice) in cases {
        let text = state.setup_refusal(has_steps).expect("setup is frozen");
        assert!(text.contains(why), "{state:?}: {text}");
        assert!(text.contains(advice), "{state:?}: {text}");
    }
    assert_eq!(TuneState::Ready.setup_refusal(false), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_setup_change_on_an_abandoned_tune_is_refused_in_plain_words() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;
    post_empty(&app, &format!("/api/tunes/{id}/abandon")).await;

    let reply = put(
        &app,
        &format!("/api/tunes/{id}/setup"),
        setup_body(&[100.0]),
    )
    .await;

    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert!(reply.error().contains("was abandoned"), "{}", reply.error());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abandoning_a_tune_that_is_over_says_there_is_nothing_to_abandon() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;
    post_empty(&app, &format!("/api/tunes/{id}/abandon")).await;

    let again = post_empty(&app, &format!("/api/tunes/{id}/abandon")).await;

    assert_eq!(again.status, StatusCode::CONFLICT);
    assert!(
        again.error().contains("nothing to abandon"),
        "{}",
        again.error()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_zero_seed_is_refused_with_a_sentence_that_says_what_to_use() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;
    let body = json!({"seed": {"typed": 0.0}, "repeats": 1, "lower_k": 0, "upper_k": 0});

    let reply = post(&app, &format!("/api/tunes/{id}/steps"), body).await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.error());
    assert_eq!(reply.json()["field"], "seed");
    assert!(reply.error().contains("can't be zero"), "{}", reply.error());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_tune_on_a_busy_stepper_is_refused_without_naming_a_record() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let first = create(&app).await;

    let second = post(&app, "/api/tunes", setup_body(&[50.0])).await;

    assert_eq!(second.status, StatusCode::CONFLICT);
    assert!(
        second.error().contains("already has an unfinished tune"),
        "{}",
        second.error()
    );
    assert!(!second.error().contains(&first), "{}", second.error());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn starting_without_a_prepared_step_says_to_prepare_one() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;

    let reply = post_empty(&app, &format!("/api/tunes/{id}/steps/confirm")).await;

    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert!(
        reply.error().contains("Prepare a step first"),
        "{}",
        reply.error()
    );
}

fn rung_raws(tune: &Value) -> Vec<i64> {
    tune["steps"][0]["rungs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["raw"].as_i64().unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_step_without_a_step_size_uses_the_third_octave() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;
    let body = json!({"seed": {"typed": 1000.0}, "repeats": 1, "lower_k": -3, "upper_k": 3});

    let plan = post(&app, &format!("/api/tunes/{id}/steps"), body).await;

    assert_eq!(plan.status, StatusCode::OK, "{}", plan.error());
    let tune = get(&app, &format!("/api/tunes/{id}")).await.json();
    assert_eq!(tune["steps"][0]["octave_divisions"], 3);
    assert_eq!(rung_raws(&tune), [500, 630, 794, 1000, 1260, 1587, 2000]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_finer_step_size_spaces_the_same_steps_closer_and_is_recorded() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;
    let body = json!({
        "seed": {"typed": 1000.0}, "repeats": 1, "lower_k": -3, "upper_k": 3, "octave_divisions": 6,
    });

    let plan = post(&app, &format!("/api/tunes/{id}/steps"), body).await;

    assert_eq!(plan.status, StatusCode::OK, "{}", plan.error());
    let tune = get(&app, &format!("/api/tunes/{id}")).await.json();
    assert_eq!(tune["steps"][0]["octave_divisions"], 6);
    assert_eq!(rung_raws(&tune), [707, 794, 891, 1000, 1122, 1260, 1414]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_the_offered_step_sizes_are_accepted() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;
    let body = json!({
        "seed": "live", "repeats": 1, "lower_k": 0, "upper_k": 0, "octave_divisions": 5,
    });

    let reply = post(&app, &format!("/api/tunes/{id}/steps"), body).await;

    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.error());
    assert!(reply.error().contains("step size"), "{}", reply.error());
    let tune = get(&app, &format!("/api/tunes/{id}")).await.json();
    assert_eq!(tune["steps"], json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_prepared_step_can_be_discarded_and_prepared_again() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;
    let plan = post(&app, &format!("/api/tunes/{id}/steps"), step_body(-3, 3)).await;
    assert_eq!(plan.status, StatusCode::OK, "{}", plan.error());
    let prepared = get(&app, &format!("/api/tunes/{id}")).await.json();
    let sessions: Vec<String> = ["probe", "tracking", "standstill"]
        .iter()
        .map(|k| {
            prepared["steps"][0]["sessions"][k]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();

    let reply = post_empty(&app, &format!("/api/tunes/{id}/steps/cancel")).await;

    assert_eq!(reply.status, StatusCode::OK, "{}", reply.error());
    let after = get(&app, &format!("/api/tunes/{id}")).await.json();
    assert_eq!(after["state"], "Ready");
    assert_eq!(after["steps"], json!([]));
    let listed = get(&app, "/api/sessions").await.json();
    for session in &sessions {
        assert!(
            !listed.to_string().contains(session),
            "{session} is still listed"
        );
    }
    let again = post(&app, &format!("/api/tunes/{id}/steps"), step_body(-1, 1)).await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.error());
    let replaced = get(&app, &format!("/api/tunes/{id}")).await.json();
    assert_eq!(replaced["steps"].as_array().unwrap().len(), 1);
    assert_eq!(rung_raws(&replaced).len(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_setup_can_change_again_once_the_prepared_step_is_discarded() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;
    post(&app, &format!("/api/tunes/{id}/steps"), step_body(0, 0)).await;
    post_empty(&app, &format!("/api/tunes/{id}/steps/cancel")).await;

    let reply = put(
        &app,
        &format!("/api/tunes/{id}/setup"),
        setup_body(&[100.0]),
    )
    .await;

    assert_eq!(reply.status, StatusCode::OK, "{}", reply.error());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn there_is_nothing_to_discard_without_a_prepared_step() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;

    let reply = post_empty(&app, &format!("/api/tunes/{id}/steps/cancel")).await;

    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert!(
        reply.error().contains("no prepared step to adjust"),
        "{}",
        reply.error()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_step_that_has_started_cannot_be_discarded() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;
    post(&app, &format!("/api/tunes/{id}/steps"), step_body(0, 0)).await;
    let started = post_empty(&app, &format!("/api/tunes/{id}/steps/confirm")).await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.error());

    let reply = post_empty(&app, &format!("/api/tunes/{id}/steps/cancel")).await;

    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert!(
        reply.error().contains("already started"),
        "{}",
        reply.error()
    );
    wait_state(&app, &id, "AwaitingChoice").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discarding_a_step_of_an_abandoned_tune_says_the_tune_was_abandoned() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;
    post(&app, &format!("/api/tunes/{id}/steps"), step_body(0, 0)).await;
    post_empty(&app, &format!("/api/tunes/{id}/abandon")).await;

    let reply = post_empty(&app, &format!("/api/tunes/{id}/steps/cancel")).await;

    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert!(reply.error().contains("was abandoned"), "{}", reply.error());
    assert!(
        !reply.error().contains("already started"),
        "{}",
        reply.error()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn applying_the_gains_of_a_tune_that_is_not_finished_is_refused_with_a_sentence() {
    let rig = rig().await;
    let (app, _) = app(&rig);
    let id = create(&app).await;

    let reply = post_empty(&app, &format!("/api/tunes/{id}/apply")).await;

    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert!(
        reply.error().contains("all three steps"),
        "{}",
        reply.error()
    );
}
