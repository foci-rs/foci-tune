//! A simulated printer for exercising the UI without hardware: an in-process Moonraker
//! stand-in and one two-channel FOCI board whose trace frames answer the tool's G-code.
//!
//! Debug builds only. The responses are shaped so every run type completes and analyses, not
//! to model a real plant.

use std::collections::VecDeque;
use std::f64::consts::TAU;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::server::{ServeConfig, ServeError, serve_with};
use crate::trace::{FrameSource, pump_source};

const SERIAL: &str = "2600FAKE";
const CLOCK: u32 = 260_000_000;
const TICKS_PER_MS: u64 = 260_000;
const COUNTS_PER_REV: u32 = 4096;
const PWM_HZ: u32 = 50_000;
const ROTATION_DISTANCE_MM: f64 = 40.0;
const UNITS_PER_MM: f64 = 65536.0 / ROTATION_DISTANCE_MM;
const PROTOCOL: u8 = 25;
const BATCH_MS: usize = 50;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs the service against the simulated printer until SIGINT or SIGTERM.
///
/// # Errors
/// As [`crate::server::serve`].
pub async fn serve_fake(listen: String) -> Result<(), ServeError> {
    let root = std::env::temp_dir().join(format!("foci-tune-fake-{}", std::process::id()));
    std::fs::create_dir_all(&root).map_err(|source| ServeError::DataDir {
        path: root.clone(),
        source,
    })?;
    let board = Arc::new(Board::new());
    let moonraker = FakeMoonraker::start(root, board.clone()).await?;
    tracing::info!(url = %moonraker, "simulated moonraker listening");
    let config = ServeConfig {
        moonraker_url: moonraker,
        api_key: None,
        listen,
    };
    serve_with(config, move |live, frames, stop| {
        let (tx, rx) = mpsc::channel();
        *lock(&board.tx) = Some(tx);
        board.announce(0);
        tokio::task::spawn_blocking(move || {
            let mut source = Source {
                rx,
                pending: VecDeque::new(),
            };
            pump_source(&mut source, &live, &frames, &stop);
        })
    })
    .await
}

struct Source {
    rx: mpsc::Receiver<Vec<u8>>,
    pending: VecDeque<u8>,
}

impl FrameSource for Source {
    fn serial(&self) -> &str {
        SERIAL
    }

    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        if self.pending.is_empty() {
            match self.rx.recv_timeout(Duration::from_millis(5)) {
                Ok(bytes) => self.pending.extend(bytes),
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(None),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other("simulated board gone"));
                }
            }
        }
        let n = buf.len().min(self.pending.len());
        for (slot, byte) in buf.iter_mut().zip(self.pending.drain(..n)) {
            *slot = byte;
        }
        Ok(Some(n))
    }
}

#[derive(Clone)]
struct Channel {
    change_sequence: u32,
    gains: [i32; 4],
    filters: [u16; 4],
}

struct Sim {
    sequence: u32,
    ticks: u64,
    channels: [Channel; 2],
}

struct Board {
    tx: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    sim: Mutex<Sim>,
    started: Instant,
}

fn frame(
    frame_type: u8,
    flags: u8,
    source: (u8, u8),
    seq: u32,
    ticks: u64,
    payload: &[u8],
) -> Vec<u8> {
    let mut v = Vec::with_capacity(32 + payload.len());
    v.extend_from_slice(&0x4352_5446u32.to_le_bytes());
    v.extend_from_slice(&[PROTOCOL, 32, frame_type, flags]);
    v.extend_from_slice(
        &u16::try_from(payload.len())
            .unwrap_or(u16::MAX)
            .to_le_bytes(),
    );
    v.extend_from_slice(&[source.0, source.1]);
    v.extend_from_slice(&seq.to_le_bytes());
    v.extend_from_slice(&ticks.to_le_bytes());
    v.extend_from_slice(&7u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(payload);
    v
}

fn control_payload(c: &Channel, applied_request: u8) -> Vec<u8> {
    let mut p = vec![0u8; 88];
    p[0..4].copy_from_slice(&c.change_sequence.to_le_bytes());
    p[12] = applied_request;
    p[16..20].copy_from_slice(&COUNTS_PER_REV.to_le_bytes());
    p[20..24].copy_from_slice(&PWM_HZ.to_le_bytes());
    for (k, hz) in c.filters.iter().enumerate() {
        p[28 + 2 * k..30 + 2 * k].copy_from_slice(&hz.to_le_bytes());
    }
    for (k, g) in c.gains.iter().enumerate() {
        p[36 + 4 * k..40 + 4 * k].copy_from_slice(&g.to_le_bytes());
    }
    p
}

fn metadata_payload(critical_hz: u32) -> Vec<u8> {
    let mut p = vec![1, 0, 2, 0];
    for v in [CLOCK, critical_hz, 100, 10, 0, 0, 0, 0] {
        p.extend_from_slice(&v.to_le_bytes());
    }
    for s in ["devfake", "FOCI Ouroboros (simulated)", SERIAL] {
        p.push(u8::try_from(s.len()).unwrap_or(u8::MAX));
        p.extend_from_slice(s.as_bytes());
    }
    for (index, label) in [(0u8, "channel0"), (1, "channel1")] {
        p.extend_from_slice(&[1, index, u8::try_from(label.len()).unwrap_or(u8::MAX)]);
        p.extend_from_slice(label.as_bytes());
    }
    p
}

struct Sample {
    step: i32,
    tmc: i32,
    error: i32,
    ff: i32,
    torque: i32,
}

fn critical_payload(s: &Sample) -> Vec<u8> {
    let mut p = vec![0u8; 88];
    p[0..4].copy_from_slice(&s.step.to_le_bytes());
    p[4..8].copy_from_slice(&s.tmc.to_le_bytes());
    p[8..12].copy_from_slice(&s.error.to_le_bytes());
    p[20..24].copy_from_slice(&s.ff.to_le_bytes());
    p[60..64].copy_from_slice(&s.torque.to_le_bytes());
    p
}

fn param<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
}

fn float(line: &str, key: &str) -> f64 {
    param(line, key).and_then(|v| v.parse().ok()).unwrap_or(0.0)
}

/// A small deterministic jitter in -1..1, so traces are not perfectly smooth.
fn jitter(n: usize) -> f64 {
    let x = (n as u64)
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    ((x >> 33) as f64 / f64::from(u32::MAX)) * 2.0 - 1.0
}

impl Board {
    fn new() -> Self {
        let channel = Channel {
            change_sequence: 5,
            gains: [841, 412, 141, 0],
            filters: [500, 1000, 500, 1000],
        };
        Self {
            tx: Mutex::new(None),
            sim: Mutex::new(Sim {
                sequence: 0,
                ticks: 0,
                channels: [channel.clone(), channel],
            }),
            started: Instant::now(),
        }
    }

    fn send(&self, frames: Vec<Vec<u8>>) {
        if let Some(tx) = lock(&self.tx).as_ref() {
            let _ = tx.send(frames.concat());
        }
    }

    fn next(&self, sim: &mut Sim) -> (u32, u64) {
        let now = self.started.elapsed().as_millis() as u64 * TICKS_PER_MS;
        sim.ticks = sim.ticks.max(now);
        sim.sequence = sim.sequence.wrapping_add(1);
        (sim.sequence, sim.ticks)
    }

    fn control(&self, sim: &mut Sim, channel: u8, request: u8) -> Vec<u8> {
        let payload = control_payload(&sim.channels[usize::from(channel)], request);
        let (seq, ticks) = self.next(sim);
        frame(10, 0, (1, channel), seq, ticks, &payload)
    }

    fn announce(&self, critical_hz: u32) {
        let mut sim = lock(&self.sim);
        let (seq, ticks) = self.next(&mut sim);
        let mut out = vec![frame(
            5,
            0,
            (0, 0),
            seq,
            ticks,
            &metadata_payload(critical_hz),
        )];
        out.push(self.control(&mut sim, 0, 0));
        out.push(self.control(&mut sim, 1, 0));
        drop(sim);
        self.send(out);
    }

    fn react(&self, script: &str) -> Result<(), String> {
        let channel: u8 = if param(script, "STEPPER") == Some("stepper_y") {
            1
        } else {
            0
        };
        let ch = usize::from(channel);
        if script.starts_with("FOCI_SET_GAINS") {
            let mut sim = lock(&self.sim);
            let c = &mut sim.channels[ch];
            c.change_sequence += 1;
            c.gains = [
                (float(script, "VELOCITY_P") * 256.0).round() as i32,
                (float(script, "VELOCITY_I") * 4096.0).round() as i32,
                (float(script, "POSITION_P") * 256.0).round() as i32,
                (float(script, "POSITION_I") * 4096.0).round() as i32,
            ];
            let f = self.control(&mut sim, channel, 1);
            drop(sim);
            self.send(vec![f]);
        } else if script.starts_with("FOCI_SET_FILTERS") {
            let mut sim = lock(&self.sim);
            let mut out = Vec::new();
            for (k, key) in ["VELOCITY_HZ", "TORQUE_HZ", "POSITION_HZ", "FLUX_HZ"]
                .iter()
                .enumerate()
            {
                let c = &mut sim.channels[ch];
                c.change_sequence += 1;
                c.filters[k] = float(script, key) as u16;
                out.push(self.control(&mut sim, channel, 3 + k as u8));
            }
            drop(sim);
            self.send(out);
        } else if script.starts_with("FOCI_TRACE_MODE") {
            self.announce(if param(script, "MODE") == Some("dormant") {
                0
            } else {
                1000
            });
        } else if script.starts_with("G90") {
            let gains = lock(&self.sim).channels[ch].gains;
            let rests: Vec<usize> = script
                .lines()
                .filter_map(|l| l.strip_prefix("G4 P")?.trim().parse().ok())
                .filter(|&ms: &usize| ms > 500)
                .collect();
            let samples = if script.matches("FOCI_SET_VELOCITY_FEEDFORWARD").count() > 3 {
                probe_samples(gains)
            } else if !rests.is_empty() {
                rest_samples(gains, &rests)
            } else {
                stroke_samples(gains, &block_speeds(script))
            };
            self.stimulus(channel, &samples);
        }
        Ok(())
    }

    /// Streams the stimulus at 1 kHz in real time, so live panels see it while it moves.
    fn stimulus(&self, declared: u8, samples: &[Sample]) {
        let probe = samples.iter().any(|s| s.ff != 0);
        let middle = samples.len() / 2;
        let other = 1 - declared;
        for (start, chunk) in samples.chunks(BATCH_MS).enumerate() {
            let begun = Instant::now();
            let mut sim = lock(&self.sim);
            let mut out = Vec::new();
            for (k, s) in chunk.iter().enumerate() {
                let n = start * BATCH_MS + k;
                sim.ticks += TICKS_PER_MS;
                sim.sequence = sim.sequence.wrapping_add(1);
                out.push(frame(
                    1,
                    0,
                    (1, declared),
                    sim.sequence,
                    sim.ticks,
                    &critical_payload(s),
                ));
                sim.sequence = sim.sequence.wrapping_add(1);
                let still = Sample {
                    step: 500,
                    tmc: 500,
                    error: 0,
                    ff: 0,
                    torque: (jitter(n) * 20.0) as i32,
                };
                out.push(frame(
                    1,
                    0,
                    (1, other),
                    sim.sequence,
                    sim.ticks,
                    &critical_payload(&still),
                ));
                if probe && n == middle {
                    let c = &mut sim.channels[usize::from(declared)];
                    c.change_sequence += 1;
                    let payload = control_payload(c, 7);
                    sim.sequence = sim.sequence.wrapping_add(1);
                    out.push(frame(
                        10,
                        0,
                        (1, declared),
                        sim.sequence,
                        sim.ticks,
                        &payload,
                    ));
                }
            }
            drop(sim);
            self.send(out);
            std::thread::sleep(
                Duration::from_millis(BATCH_MS as u64).saturating_sub(begun.elapsed()),
            );
        }
    }
}

/// Feeds of the script's speed blocks in mm/s, in order: the feed of each run of `G1` lines
/// that follows a 500 ms dwell.
fn block_speeds(script: &str) -> Vec<f64> {
    let mut speeds = Vec::new();
    let mut after_dwell = false;
    for line in script.lines() {
        if line.trim() == "G4 P500" {
            after_dwell = true;
        } else if after_dwell && line.starts_with("G1") {
            let feed = line
                .split_whitespace()
                .find_map(|w| w.strip_prefix('F')?.parse::<f64>().ok());
            if let Some(feed) = feed {
                speeds.push(feed / 60.0);
            }
            after_dwell = false;
        }
    }
    speeds
}

/// An approach, then two 100 mm strokes out and back per speed with a still gap between
/// speeds; the tracking error grows with speed and as position P falls.
fn stroke_samples(gains: [i32; 4], speeds: &[f64]) -> Vec<Sample> {
    let error_scale = 400.0 / f64::from(gains[2].max(10));
    let mut out = Vec::new();
    let mut position = 0.0_f64;
    let mut n = 0;
    let mut push = |position: f64, velocity: f64, n: &mut usize| {
        let error = if velocity == 0.0 {
            0
        } else {
            (error_scale * velocity.abs() * ((*n as f64) * TAU / 37.0).sin() / 10.0
                + jitter(*n) * 3.0) as i32
        };
        let step = position.round() as i32 + 1000;
        out.push(Sample {
            step,
            tmc: step - error,
            error,
            ff: 0,
            torque: (f64::from(error) * 8.0 + jitter(*n) * 40.0) as i32,
        });
        *n += 1;
    };
    for (k, &speed) in std::iter::once(&50.0).chain(speeds).enumerate() {
        let strokes = if k == 0 { 1 } else { 4 };
        let per_ms = speed * UNITS_PER_MM / 1000.0;
        let length = 100.0 * UNITS_PER_MM;
        for stroke in 0..strokes {
            let dir = if stroke % 2 == 0 { 1.0 } else { -1.0 };
            let steps = (length / per_ms).ceil() as usize;
            for _ in 0..steps {
                position += dir * per_ms;
                push(position, dir * speed, &mut n);
            }
        }
        for _ in 0..400 {
            push(position, 0.0, &mut n);
        }
    }
    out
}

/// Holds at sub-step offsets, each with dither whose spread falls as velocity P rises.
fn rest_samples(gains: [i32; 4], dwells_ms: &[usize]) -> Vec<Sample> {
    let spread = 20_000.0 / f64::from(gains[0].max(50));
    let mut out = Vec::new();
    let mut n = 0;
    for (k, &dwell) in dwells_ms.iter().enumerate() {
        let at = 1000 + (k as i32) * 40;
        for _ in 0..dwell + 200 {
            let error = (jitter(n) * spread / 20.0) as i32;
            out.push(Sample {
                step: at,
                tmc: at - error,
                error,
                ff: 0,
                torque: (jitter(n + 7919) * spread) as i32,
            });
            n += 1;
        }
    }
    out
}

/// A cruise with the feedforward toggled every 150 samples and a damped response to each
/// edge; damping rises with velocity P.
fn probe_samples(gains: [i32; 4]) -> Vec<Sample> {
    let zeta = (0.02 + f64::from(gains[0]) / 6000.0).clamp(0.02, 0.6);
    let edges: Vec<(usize, f64)> = (0..30)
        .map(|k| (100 + 150 * k, if k % 2 == 0 { 300.0 } else { -300.0 }))
        .collect();
    let w = TAU * 150.0;
    let wd = w * (1.0 - zeta * zeta).sqrt();
    let response = |n: usize| -> f64 {
        edges
            .iter()
            .filter(|(at, _)| *at <= n)
            .map(|(at, step)| {
                let t = (n - at) as f64 * 1e-3;
                step * 0.1 * (-zeta * w * t).exp() * (wd * t).cos()
            })
            .sum()
    };
    let len = 100 + 150 * edges.len() + 150;
    let mut position = 0.0;
    (0..len)
        .map(|n| {
            let r = response(n);
            position += (100.0 + r) * 1e-3 * UNITS_PER_MM;
            let level = edges.iter().filter(|(at, _)| *at <= n).count();
            let p = position.round() as i32;
            Sample {
                step: p + 1000,
                tmc: p + 1000,
                error: (r * 4.0) as i32,
                ff: if level % 2 == 0 { 1000 } else { 1300 },
                torque: (r * 50.0 + jitter(n) * 30.0) as i32,
            }
        })
        .collect()
}

struct Shared {
    board: Arc<Board>,
    status: Mutex<Value>,
    settings: Value,
    config_root: PathBuf,
    pushes: broadcast::Sender<String>,
}

impl Shared {
    fn push(&self, method: &str, params: Value) {
        let text = json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string();
        let _ = self.pushes.send(text);
    }

    fn set_klippy(&self, state: &str) {
        lock(&self.status)["webhooks"]["state"] = json!(state);
        self.push(&format!("notify_klippy_{state}"), json!([]));
    }

    fn call(&self, method: &str, params: &Value) -> Result<Value, String> {
        match method {
            "server.files.roots" => Ok(json!([
                {"name": "config", "path": self.config_root, "permissions": "rw"},
            ])),
            "printer.objects.query" => {
                let status = if params.get("configfile").is_some() {
                    json!({"configfile": {"settings": self.settings.clone()}})
                } else {
                    lock(&self.status).clone()
                };
                Ok(json!({"eventtime": 1.0, "status": status}))
            }
            "printer.objects.subscribe" => {
                Ok(json!({"eventtime": 1.0, "status": lock(&self.status).clone()}))
            }
            other => Err(format!("Method not found: {other}")),
        }
    }
}

fn reply(result: Result<Value, String>) -> Response {
    match result {
        Ok(value) => Json(json!({"result": value})).into_response(),
        Err(message) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": {"code": 400, "message": message}})),
        )
            .into_response(),
    }
}

fn settings() -> Value {
    let board = format!("/dev/serial/by-id/usb-foci_FOCI_Ouroboros_{SERIAL}-if00");
    json!({
        "printer": {"kinematics": "corexy"},
        "mcu": {"serial": "/dev/serial/by-id/usb-Klipper_stm32f446xx_host-if00"},
        "mcu ouroboros": {"serial": board},
        "stepper_x": {"step_pin": "ouroboros:STEP0", "rotation_distance": ROTATION_DISTANCE_MM, "position_max": 300.0},
        "stepper_y": {"step_pin": "ouroboros:STEP1", "rotation_distance": ROTATION_DISTANCE_MM, "position_max": 300.0},
        "foci stepper_x": {"encoder_ppr": 1024, "identified_r_count_milli": 812},
        "foci stepper_y": {"encoder_ppr": 1024, "identified_r_count_milli": 790},
    })
}

struct FakeMoonraker;

impl FakeMoonraker {
    async fn start(config_root: PathBuf, board: Arc<Board>) -> Result<String, ServeError> {
        let shared = Arc::new(Shared {
            board,
            status: Mutex::new(json!({
                "webhooks": {"state": "ready"},
                "toolhead": {"homed_axes": "xyz", "max_velocity": 300.0, "max_accel": 3000.0},
                "idle_timeout": {"state": "Idle"},
                "print_stats": {"state": "standby"},
            })),
            settings: settings(),
            config_root,
            pushes: broadcast::channel(64).0,
        });
        let app = Router::new()
            .route(
                "/server/files/roots",
                get(|State(s): State<Arc<Shared>>| async move {
                    reply(s.call("server.files.roots", &Value::Null))
                }),
            )
            .route(
                "/printer/objects/query",
                get(
                    |State(s): State<Arc<Shared>>,
                     axum::extract::Query(q): axum::extract::Query<
                        serde_json::Map<String, Value>,
                    >| async move {
                        reply(s.call("printer.objects.query", &Value::Object(q)))
                    },
                ),
            )
            .route("/printer/gcode/script", post(script))
            .route("/printer/emergency_stop", post(emergency_stop))
            .route("/printer/firmware_restart", post(firmware_restart))
            .route("/websocket", get(websocket))
            .with_state(shared);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|source| ServeError::Listen {
                addr: "127.0.0.1:0".to_owned(),
                source,
            })?;
        let addr = listener.local_addr().map_err(|source| ServeError::Listen {
            addr: "127.0.0.1:0".to_owned(),
            source,
        })?;
        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::warn!(error = %e, "simulated moonraker stopped");
            }
        });
        Ok(format!("http://{addr}"))
    }
}

async fn script(State(s): State<Arc<Shared>>, Json(body): Json<Value>) -> Response {
    if lock(&s.status)["webhooks"]["state"] != "ready" {
        return reply(Err("Klipper is not ready".to_owned()));
    }
    let text = body["script"].as_str().unwrap_or_default().to_owned();
    let board = s.board.clone();
    match tokio::task::spawn_blocking(move || board.react(&text)).await {
        Ok(result) => reply(result.map(|()| json!("ok"))),
        Err(e) => reply(Err(e.to_string())),
    }
}

async fn emergency_stop(State(s): State<Arc<Shared>>) -> Response {
    s.set_klippy("shutdown");
    reply(Ok(json!("ok")))
}

async fn firmware_restart(State(s): State<Arc<Shared>>) -> Response {
    let shared = s.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        shared.set_klippy("ready");
        shared.board.announce(0);
    });
    reply(Ok(json!("ok")))
}

async fn websocket(State(s): State<Arc<Shared>>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| serve_socket(s, socket))
}

async fn serve_socket(s: Arc<Shared>, mut socket: WebSocket) {
    let mut pushes = s.pushes.subscribe();
    loop {
        tokio::select! {
            push = pushes.recv() => match push {
                Ok(text) => {
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
            message = socket.recv() => {
                let Some(Ok(message)) = message else { return };
                let Message::Text(text) = message else { continue };
                let Ok(request) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                let method = request["method"].as_str().unwrap_or_default();
                let answer = match s.call(method, &request["params"]) {
                    Ok(result) => json!({"jsonrpc": "2.0", "result": result, "id": request["id"]}),
                    Err(message) => json!({
                        "jsonrpc": "2.0",
                        "error": {"code": -32601, "message": message},
                        "id": request["id"],
                    }),
                };
                if socket.send(Message::Text(answer.to_string().into())).await.is_err() {
                    return;
                }
            }
        }
    }
}
