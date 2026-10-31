mod fake_moonraker;

use std::collections::{HashMap, VecDeque};
use std::f64::consts::TAU;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use fake_moonraker::{FakeMoonraker, ScriptReply};
use foci_tune::engine::{
    Engine, EngineConfig, EngineEvent, LadderRejected, LadderRequest, Precondition,
};
use foci_tune::live::LiveState;
use foci_tune::model::{
    FORMAT_VERSION, Filters, Gains, LadderDef, Run, RunId, RunRequest, RunState, Session, SessionId,
};
use foci_tune::moonraker::{ClientConfig, KlippyState, MoonrakerApi, MoonrakerClient};
use foci_tune::pattern::Pattern;
use foci_tune::stimulus::RunKind;
use foci_tune::store::Store;
use foci_tune::trace::{FrameSource, pump_source};
use foci_tune_analysis::ladder::LadderError;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::{Notify, broadcast};

const SERIAL_A: &str = "2600AAAA";
const SERIAL_B: &str = "3300BBBB";
const CLOCK: u32 = 260_000_000;
const TICKS_PER_MS: u64 = 260_000;
const COUNTS_PER_REV: u32 = 4096;
const Q: i32 = 16;
const PWM_HZ: u32 = 50_000;
const UNITS_PER_MM: f64 = 65536.0 / 40.0;
const WAIT: Duration = Duration::from_secs(20);

const GAINS_LINE: &str = "FOCI_SET_GAINS STEPPER=stepper_x VELOCITY_P=3.28515625 \
    VELOCITY_I=0.100585937500 POSITION_P=0.55078125 POSITION_I=0";
const FILTERS_LINE: &str = "FOCI_SET_FILTERS STEPPER=stepper_x VELOCITY_HZ=500 TORQUE_HZ=1000 POSITION_HZ=500 FLUX_HZ=1000";
const CRITICAL_LINE: &str = "FOCI_TRACE_MODE STEPPER=stepper_x MODE=critical";
const DORMANT_LINE: &str = "FOCI_TRACE_MODE STEPPER=stepper_x MODE=dormant";
const RESET_FF_LINE: &str = "FOCI_SET_VELOCITY_FEEDFORWARD STEPPER=stepper_x ENABLE=1 GAIN=1.0";

struct ScriptedSource {
    serial: String,
    rx: mpsc::Receiver<Vec<u8>>,
    pending: VecDeque<u8>,
}

impl FrameSource for ScriptedSource {
    fn serial(&self) -> &str {
        &self.serial
    }

    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        if self.pending.is_empty() {
            match self.rx.recv_timeout(Duration::from_millis(5)) {
                Ok(bytes) => self.pending.extend(bytes),
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(None),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other("unplugged"));
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

#[derive(Clone, Default)]
struct Channel {
    change_sequence: u32,
    gains: [i32; 4],
    filters: [u16; 4],
}

#[derive(Clone)]
struct Profile {
    declared_range: i32,
    other_range: i32,
    probe_edges: usize,
    drop_flag: bool,
    sequence_jump: bool,
    burst: usize,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            declared_range: 50_000,
            other_range: 0,
            probe_edges: 25,
            drop_flag: false,
            sequence_jump: false,
            burst: 0,
        }
    }
}

struct Sim {
    sequence: u32,
    ticks: u64,
    channels: [Channel; 2],
    confirm_gains: bool,
    confirm_filters: [bool; 4],
    cutoff_offset: [u16; 4],
    baseline: bool,
    failures: Vec<(String, String)>,
    gate: Option<Arc<Notify>>,
    profile: Profile,
    zeta_by_position_p: HashMap<i32, f64>,
    edges_by_position_p: HashMap<i32, usize>,
}

impl Default for Sim {
    fn default() -> Self {
        let channel = Channel {
            change_sequence: 5,
            gains: [841, 412, 141, 0],
            filters: [500, 1000, 500, 1000],
        };
        Self {
            sequence: 0,
            ticks: 0,
            channels: [channel.clone(), channel],
            confirm_gains: true,
            confirm_filters: [true; 4],
            cutoff_offset: [0; 4],
            baseline: true,
            failures: Vec::new(),
            gate: None,
            profile: Profile::default(),
            zeta_by_position_p: HashMap::new(),
            edges_by_position_p: HashMap::new(),
        }
    }
}

struct Board {
    serial: &'static str,
    tx: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    sim: Mutex<Sim>,
    live: Arc<Mutex<LiveState>>,
    frames: broadcast::Sender<(String, Bytes)>,
}

fn frame_bytes(
    version: u8,
    frame_type: u8,
    flags: u8,
    source: (u8, u8),
    seq: u32,
    ticks: u64,
    payload: &[u8],
) -> Vec<u8> {
    let mut v = Vec::with_capacity(32 + payload.len());
    v.extend_from_slice(&0x4352_5446u32.to_le_bytes());
    v.extend_from_slice(&[version, 32, frame_type, flags]);
    v.extend_from_slice(&(payload.len() as u16).to_le_bytes());
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

fn metadata_payload(serial: &str, critical_hz: u32) -> Vec<u8> {
    let mut p = vec![1, 0, 2, 0];
    for v in [CLOCK, critical_hz, 100, 10, 0, 0, 0, 0] {
        p.extend_from_slice(&v.to_le_bytes());
    }
    for s in ["fw", "FOCI Ouroboros", serial] {
        p.push(s.len() as u8);
        p.extend_from_slice(s.as_bytes());
    }
    for (index, label) in [(0u8, "channel0"), (1, "channel1")] {
        p.extend_from_slice(&[1, index, label.len() as u8]);
        p.extend_from_slice(label.as_bytes());
    }
    p
}

fn critical_payload(step: i32, tmc: i32, ff: i32) -> Vec<u8> {
    let mut p = vec![0u8; 88];
    p[0..4].copy_from_slice(&step.to_le_bytes());
    p[4..8].copy_from_slice(&tmc.to_le_bytes());
    p[20..24].copy_from_slice(&ff.to_le_bytes());
    p
}

fn param<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
}

fn float(line: &str, key: &str) -> f64 {
    param(line, key).unwrap().parse().unwrap()
}

/// Velocity of a probe run in mm/s: a cruise plus a damped response to every feedforward
/// edge, scaled by the edge's step.
fn probe_velocity(n: usize, edges: &[(usize, f64)], zeta: f64) -> f64 {
    let w = TAU * 150.0;
    let wd = w * (1.0 - zeta * zeta).sqrt();
    100.0
        + edges
            .iter()
            .filter(|(at, _)| *at <= n)
            .map(|(at, step)| {
                let t = (n - at) as f64 * 1e-3;
                step * 0.1 * (-zeta * w * t).exp() * (wd * t).cos()
            })
            .sum::<f64>()
}

impl Board {
    fn new(
        serial: &'static str,
        live: &Arc<Mutex<LiveState>>,
        frames: &broadcast::Sender<(String, Bytes)>,
        stop: &Arc<AtomicBool>,
        open: bool,
    ) -> Arc<Self> {
        let (tx, rx) = mpsc::channel();
        if open {
            let (live, frames, stop) = (live.clone(), frames.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut source = ScriptedSource {
                    serial: serial.to_owned(),
                    rx,
                    pending: VecDeque::new(),
                };
                pump_source(&mut source, &live, &frames, &stop);
            });
        }
        Arc::new(Self {
            serial,
            tx: Mutex::new(open.then_some(tx)),
            sim: Mutex::new(Sim::default()),
            live: live.clone(),
            frames: frames.clone(),
        })
    }

    fn sim(&self, f: impl FnOnce(&mut Sim)) {
        f(&mut self.sim.lock().unwrap());
    }

    fn unplug(&self) {
        self.tx.lock().unwrap().take();
    }

    fn next(sim: &mut Sim) -> (u32, u64) {
        sim.sequence += 1;
        (sim.sequence, sim.ticks)
    }

    /// Sends frames to the reader and waits until it has broadcast the last of them.
    fn emit(&self, frames: Vec<Vec<u8>>) {
        let Some(last) = frames
            .last()
            .map(|f| u32::from_le_bytes(f[12..16].try_into().unwrap()))
        else {
            return;
        };
        let bytes = frames.concat();
        if let Some(tx) = self.tx.lock().unwrap().as_ref() {
            let _ = tx.send(bytes);
        } else {
            return;
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let seen = self
                .live
                .lock()
                .unwrap()
                .boards
                .get(self.serial)
                .and_then(|b| b.last_sequence);
            if seen == Some(last) {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("reader did not consume frame {last} of {}", self.serial);
    }

    fn control(sim: &mut Sim, channel: u8, request: u8) -> Vec<u8> {
        let payload = control_payload(&sim.channels[usize::from(channel)], request);
        let (seq, ticks) = Self::next(sim);
        frame_bytes(25, 10, 0, (1, channel), seq, ticks, &payload)
    }

    fn mode_frames(&self, critical_hz: u32, baseline: bool) -> Vec<Vec<u8>> {
        let mut sim = self.sim.lock().unwrap();
        let (seq, ticks) = Self::next(&mut sim);
        let mut out = vec![frame_bytes(
            25,
            5,
            0,
            (0, 0),
            seq,
            ticks,
            &metadata_payload(self.serial, critical_hz),
        )];
        if baseline {
            out.push(Self::control(&mut sim, 0, 0));
            out.push(Self::control(&mut sim, 1, 0));
        }
        out
    }

    fn announce(&self, critical_hz: u32) {
        let frames = self.mode_frames(critical_hz, true);
        self.emit(frames);
    }

    fn react(&self, script: &str, channel: u8) -> ScriptReply {
        let reply =
            |result: Result<(), String>, gate: Option<Arc<Notify>>| ScriptReply { result, gate };
        let failure = {
            let sim = self.sim.lock().unwrap();
            sim.failures
                .iter()
                .find(|(prefix, _)| script.starts_with(prefix.as_str()))
                .map(|(_, message)| message.clone())
        };
        if let Some(message) = failure {
            return reply(Err(message), None);
        }
        let ch = usize::from(channel);
        if script.starts_with("FOCI_SET_GAINS") {
            let mut sim = self.sim.lock().unwrap();
            if sim.confirm_gains {
                let c = &mut sim.channels[ch];
                c.change_sequence += 1;
                c.gains = [
                    (float(script, "VELOCITY_P") * 256.0).round() as i32,
                    (float(script, "VELOCITY_I") * 4096.0).round() as i32,
                    (float(script, "POSITION_P") * 256.0).round() as i32,
                    (float(script, "POSITION_I") * 4096.0).round() as i32,
                ];
                let frame = Self::control(&mut sim, channel, 1);
                drop(sim);
                self.emit(vec![frame]);
            }
        } else if script.starts_with("FOCI_SET_FILTERS") {
            let mut sim = self.sim.lock().unwrap();
            let mut frames = Vec::new();
            for (k, key) in ["VELOCITY_HZ", "TORQUE_HZ", "POSITION_HZ", "FLUX_HZ"]
                .iter()
                .enumerate()
            {
                if sim.confirm_filters[k] {
                    let hz = float(script, key) as u16 + sim.cutoff_offset[k];
                    let c = &mut sim.channels[ch];
                    c.change_sequence += 1;
                    c.filters[k] = hz;
                    frames.push(Self::control(&mut sim, channel, 3 + k as u8));
                }
            }
            drop(sim);
            self.emit(frames);
        } else if script.starts_with("FOCI_TRACE_MODE") {
            let hz = if param(script, "MODE") == Some("critical") {
                1000
            } else {
                0
            };
            let baseline = self.sim.lock().unwrap().baseline;
            let frames = self.mode_frames(hz, baseline);
            self.emit(frames);
        } else if script.starts_with("G90") {
            let probe = script.matches("FOCI_SET_VELOCITY_FEEDFORWARD").count() > 3;
            let frames = self.stimulus(channel, probe);
            self.emit(frames);
            let (burst, gate) = {
                let sim = self.sim.lock().unwrap();
                (sim.profile.burst, sim.gate.clone())
            };
            if burst > 0 {
                let frame = Bytes::from(frame_bytes(
                    25,
                    1,
                    0,
                    (1, channel),
                    1,
                    1,
                    &critical_payload(0, 0, 0),
                ));
                for _ in 0..burst {
                    let _ = self.frames.send((self.serial.to_owned(), frame.clone()));
                }
            }
            return reply(Ok(()), gate);
        }
        reply(Ok(()), None)
    }

    fn stimulus(&self, declared: u8, probe: bool) -> Vec<Vec<u8>> {
        let mut sim = self.sim.lock().unwrap();
        let profile = sim.profile.clone();
        let other = 1 - declared;
        let position_p = sim.channels[usize::from(declared)].gains[2];
        let mut samples: Vec<(i32, i32, i32)> = Vec::new();
        if probe {
            let edge_count = *sim
                .edges_by_position_p
                .get(&position_p)
                .unwrap_or(&profile.probe_edges);
            let zeta = *sim.zeta_by_position_p.get(&position_p).unwrap_or(&0.2);
            let edges: Vec<(usize, f64)> = (0..edge_count)
                .map(|k| (100 + 150 * k, if k % 2 == 0 { 300.0 } else { -300.0 }))
                .collect();
            let len = 100 + 150 * edge_count + 150;
            let mut position = 0.0;
            for n in 0..len {
                position += probe_velocity(n, &edges, zeta) * 1e-3 * UNITS_PER_MM;
                let level = edges.iter().filter(|(at, _)| *at <= n).count();
                let ff = if level % 2 == 0 { 1000 } else { 1300 };
                samples.push((position.round() as i32, position.round() as i32, ff));
            }
        } else {
            let r = profile.declared_range;
            let ramp = |k: i32| (i64::from(r) * i64::from(k) / 150) as i32;
            samples.extend((0..=150).map(|k| (ramp(k), ramp(k), 0)));
            samples.extend((0..400).map(|_| (r, r, 0)));
            samples.extend((0..=150).map(|k| (ramp(150 - k), ramp(150 - k), 0)));
        }
        let mut out = Vec::new();
        let middle = samples.len() / 2;
        for (n, (step, tmc, ff)) in samples.into_iter().enumerate() {
            sim.ticks += TICKS_PER_MS;
            let flags = u8::from(profile.drop_flag && n == middle);
            let (seq, ticks) = Self::next(&mut sim);
            out.push(frame_bytes(
                25,
                1,
                flags,
                (1, declared),
                seq,
                ticks,
                &critical_payload(step + 1000, tmc + 1000, ff),
            ));
            let still = 500 + if n == middle { profile.other_range } else { 0 };
            let (seq, ticks) = Self::next(&mut sim);
            out.push(frame_bytes(
                25,
                1,
                0,
                (1, other),
                seq,
                ticks,
                &critical_payload(500, still, 0),
            ));
            if n == middle && (probe || profile.sequence_jump) {
                let c = &mut sim.channels[usize::from(declared)];
                c.change_sequence += if profile.sequence_jump { 2 } else { 1 };
                out.push(Self::control(&mut sim, declared, 7));
            }
        }
        out
    }
}

fn settings(electrical_model: bool) -> Value {
    let mut foci_x = json!({"encoder_ppr": 1000});
    if electrical_model {
        foci_x["identified_r_count_milli"] = json!(812);
    }
    json!({
        "printer": {"kinematics": "corexy"},
        "mcu": {"serial": "/dev/serial/by-id/usb-Klipper_stm32f446xx_host-if00"},
        "mcu boarda": {"serial": format!("/dev/serial/by-id/usb-foci_FOCI_Ouroboros_{SERIAL_A}-if00")},
        "mcu boardb": {"serial": format!("/dev/serial/by-id/usb-foci_FOCI_Ouroboros_{SERIAL_B}-if00")},
        "stepper_x": {"step_pin": "boarda:STEP0", "rotation_distance": 40.0, "position_max": 300.0},
        "stepper_y": {"step_pin": "boarda:STEP1", "rotation_distance": 40.0, "position_max": 300.0},
        "stepper_z": {"step_pin": "boardb:STEP0", "rotation_distance": 8.0, "position_max": 250.0},
        "foci stepper_x": foci_x,
        "foci stepper_y": {"encoder_ppr": 1000, "identified_r_count_milli": 790},
        "foci stepper_z": {"encoder_ppr": 1000, "identified_r_count_milli": 700},
    })
}

fn fast_config() -> EngineConfig {
    EngineConfig {
        confirm_timeout: Duration::from_millis(300),
        baseline_timeout: Duration::from_millis(300),
        drain: Duration::from_millis(50),
        kill_timeout: Duration::from_millis(300),
        min_free_bytes: 0,
        loss_poll: Duration::from_millis(10),
    }
}

struct Options {
    config: EngineConfig,
    electrical_model: bool,
    open_a: bool,
    initial_hz_a: Option<u32>,
    frames_capacity: usize,
    homed: &'static str,
    b_echoes: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            config: fast_config(),
            electrical_model: true,
            open_a: true,
            initial_hz_a: Some(0),
            frames_capacity: 1 << 16,
            homed: "xyz",
            b_echoes: false,
        }
    }
}

struct Rig {
    fake: FakeMoonraker,
    client: Arc<MoonrakerClient>,
    engine: Engine<MoonrakerClient>,
    store: Store,
    a: Arc<Board>,
    b: Arc<Board>,
    session: SessionId,
    stop: Arc<AtomicBool>,
    _dir: TempDir,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn session_record() -> Session {
    Session {
        format_version: FORMAT_VERSION,
        name: "engine".into(),
        created_unix_s: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        stepper: "stepper_x".into(),
        kind: tracking(),
        pattern: pattern(),
        gains: Gains {
            velocity_p: 900,
            velocity_i: 40,
            position_p: 120,
            position_i: 0,
        },
        filters: Filters {
            velocity_hz: 200,
            torque_hz: 4000,
            position_hz: 500,
            flux_hz: 1000,
        },
        ladder: None,
    }
}

async fn rig_with(o: Options) -> Rig {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path().to_owned()).unwrap();
    let live = Arc::new(Mutex::new(LiveState::default()));
    let (frames, _) = broadcast::channel(o.frames_capacity);
    let stop = Arc::new(AtomicBool::new(false));
    let a = Board::new(SERIAL_A, &live, &frames, &stop, o.open_a);
    let b = Board::new(SERIAL_B, &live, &frames, &stop, true);
    if let Some(hz) = o.initial_hz_a {
        a.announce(hz);
    }
    b.announce(0);

    let fake = FakeMoonraker::start().await;
    fake.set_settings(settings(o.electrical_model));
    fake.set_status(json!({"toolhead": {"homed_axes": o.homed}}));
    let (ha, hb, echo) = (a.clone(), b.clone(), o.b_echoes);
    fake.on_script(move |script| {
        let stepper = script
            .split_whitespace()
            .find_map(|w| w.strip_prefix("STEPPER="))
            .unwrap_or("stepper_x");
        let (board, channel) = match stepper {
            "stepper_y" => (&ha, 1),
            "stepper_z" => (&hb, 0),
            _ => (&ha, 0),
        };
        if echo && stepper == "stepper_x" && !script.starts_with("G90") {
            hb.react(&script.replace("stepper_x", "stepper_z"), 0);
        }
        board.react(script, channel)
    });

    let client = Arc::new(MoonrakerClient::connect(ClientConfig {
        url: fake.url(),
        api_key: None,
        backoff_initial: Duration::from_millis(10),
        backoff_max: Duration::from_millis(40),
    }));
    let mut state = client.state();
    tokio::time::timeout(
        WAIT,
        state.wait_for(|s| s.klippy == KlippyState::Ready && s.homed_axes == o.homed),
    )
    .await
    .expect("client ready")
    .unwrap();

    let session = store.new_session(&session_record()).unwrap();
    let engine = Engine::start(client.clone(), live, frames, store.clone(), o.config).unwrap();
    Rig {
        fake,
        client,
        engine,
        store,
        a,
        b,
        session,
        stop,
        _dir: dir,
    }
}

async fn rig() -> Rig {
    rig_with(Options::default()).await
}

fn tracking() -> RunKind {
    RunKind::Tracking {
        speeds_mm_s: vec![50.0, 195.0],
        strokes: 1,
    }
}

fn probe() -> RunKind {
    RunKind::Probe {
        speed_mm_s: 195.0,
        high_gain: 1.3,
        segments_between_toggles: 3,
        strokes: 2,
    }
}

fn pattern() -> Pattern {
    Pattern {
        start: (100.0, 100.0),
        end: (200.0, 200.0),
    }
}

impl Rig {
    fn request(&self, kind: RunKind) -> RunRequest {
        RunRequest {
            session: self.session.clone(),
            stepper: "stepper_x".into(),
            kind,
            pattern: pattern(),
            gains: Gains {
                velocity_p: 841,
                velocity_i: 412,
                position_p: 141,
                position_i: 77,
            },
            filters: Filters {
                velocity_hz: 500,
                torque_hz: 1000,
                position_hz: 500,
                flux_hz: 1000,
            },
            ladder_value: None,
        }
    }

    fn ladder(&self, kind: RunKind, axis: &str, values: &[f64], repeats: u32) -> LadderRequest {
        LadderRequest {
            run: self.request(kind),
            ladder: LadderDef {
                axis: axis.into(),
                values: values.to_vec(),
                repeats,
                floor: 0.05,
                marginal: 0.08,
            },
        }
    }

    fn scripts(&self) -> Vec<String> {
        self.fake.scripts()
    }

    fn stimuli(&self) -> usize {
        self.scripts()
            .iter()
            .filter(|s| s.starts_with("G90"))
            .count()
    }

    fn run(&self, id: &RunId) -> Run {
        self.store.read_run(&self.session, id).unwrap()
    }

    async fn finish(&self, id: &RunId) -> Run {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Ok(run) = self.store.read_run(&self.session, id)
                && run.state.is_terminal()
                && self.engine.is_idle()
            {
                return run;
            }
            assert!(Instant::now() < deadline, "run {id} did not finish");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn run_once(&self, kind: RunKind) -> Run {
        let id = self.engine.submit(self.request(kind)).await.unwrap();
        self.finish(&id).await
    }

    /// Waits until `run` is moving and its stimulus has reached Moonraker.
    async fn wait_moving(&self, events: &mut broadcast::Receiver<EngineEvent>, run: &RunId) {
        wait_for_state(events, run, &RunState::Moving).await;
        let deadline = Instant::now() + WAIT;
        while self.stimuli() == 0 {
            assert!(Instant::now() < deadline, "stimulus not sent");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn hold_stimulus(&self) -> Arc<Notify> {
        let gate = Arc::new(Notify::new());
        self.a.sim(|s| s.gate = Some(gate.clone()));
        gate
    }
}

async fn wait_for_state(
    events: &mut broadcast::Receiver<EngineEvent>,
    run: &RunId,
    state: &RunState,
) {
    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(EngineEvent::State {
                run: id, state: s, ..
            }) = events.recv().await
                && &id == run
                && &s == state
            {
                return;
            }
        }
    })
    .await
    .expect("state not reached");
}

fn failed(reason: &str) -> RunState {
    RunState::Failed(reason.into())
}

fn not_comparable(reason: &str) -> RunState {
    RunState::NotComparable(reason.into())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_writes_exact_gains_records_and_reaches_done() {
    let rig = rig().await;

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, RunState::Done, "{:?}", run.verification);
    let scripts = rig.scripts();
    assert_eq!(scripts[0], "M400");
    assert_eq!(scripts[1], GAINS_LINE);
    assert_eq!(scripts[2], FILTERS_LINE);
    assert_eq!(scripts[3], CRITICAL_LINE);
    assert!(scripts[4].starts_with("G90"));
    assert_eq!(scripts[5], DORMANT_LINE);
    assert_eq!(scripts.len(), 6);
    assert_eq!(run.request.gains.position_i, 0);
    assert_eq!(run.baselines.len(), 2);
    assert_eq!(run.baselines[0].channel, 0);
    assert_eq!(run.baselines[0].control.velocity_p, 841);
    assert_eq!(run.baselines[0].control.velocity_i, 412);
    assert_eq!(run.baselines[0].control.velocity_filter_hz, 500);
    assert_eq!(run.baselines[1].channel, 1);
    let steps: Vec<&str> = run.steps.iter().map(|s| s.step.as_str()).collect();
    for step in [
        "preparing",
        "writing_gains",
        "baseline",
        "recording",
        "moving",
        "draining",
        "verifying",
        "analyzing",
        "done",
    ] {
        assert!(steps.contains(&step), "{step} missing from {steps:?}");
    }
    assert!(run.steps.windows(2).all(|w| w[0].unix_ms <= w[1].unix_ms));
    assert_eq!(run.applied_speeds, vec![50.0, 195.0]);
    assert_eq!(run.stream.as_ref().unwrap().protocol_version, 25);
    assert_eq!(
        run.regime.as_ref().unwrap().encoder_counts_per_rev,
        COUNTS_PER_REV
    );
    assert!(run.verification.iter().all(|v| v.passed));
    let metrics = run.metrics.as_ref().unwrap();
    assert_eq!(metrics.channels[0].channel, 0);
    assert!(metrics.channels[0].declared);
    assert!(run.capture.as_ref().unwrap().frames > 1000);
    assert!(rig.store.capture_path(&rig.session, &run.id).exists());
    let series = rig.store.series(&rig.session, &run.id).unwrap();
    assert_eq!(series.channels.len(), 2);
    assert!(run.notes.is_empty(), "{:?}", run.notes);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_speed_above_max_velocity_is_clamped_in_run_json() {
    let rig = rig().await;

    let run = rig
        .run_once(RunKind::Tracking {
            speeds_mm_s: vec![50.0, 1000.0],
            strokes: 1,
        })
        .await;

    let limit = 300.0 * std::f64::consts::SQRT_2;
    assert_eq!(run.clamped_speeds.len(), 1);
    assert_eq!(run.clamped_speeds[0].0, 1000.0);
    assert!((run.clamped_speeds[0].1 - limit).abs() < 1e-9);
    assert_eq!(run.applied_speeds.len(), 2);
}

async fn refused(rig: &Rig) -> Precondition {
    let err = rig
        .engine
        .submit(rig.request(tracking()))
        .await
        .unwrap_err();
    assert!(rig.scripts().is_empty(), "{:?}", rig.scripts());
    err
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn klipper_not_ready_is_refused() {
    let rig = rig().await;
    rig.fake.push("notify_klippy_shutdown", json!([]));
    let mut state = rig.client.state();
    state
        .wait_for(|s| s.klippy == KlippyState::Shutdown)
        .await
        .unwrap();

    assert_eq!(refused(&rig).await, Precondition::KlippyNotReady);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn printing_is_refused() {
    let rig = rig().await;
    rig.fake
        .update_status(json!({"print_stats": {"state": "printing"}}));
    let mut state = rig.client.state();
    state
        .wait_for(|s| s.print_state == "printing")
        .await
        .unwrap();

    assert_eq!(refused(&rig).await, Precondition::Printing);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unhomed_axis_is_refused() {
    let rig = rig_with(Options {
        homed: "yz",
        ..Options::default()
    })
    .await;

    assert_eq!(refused(&rig).await, Precondition::NotHomed('x'));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_electrical_model_is_refused() {
    let rig = rig_with(Options {
        electrical_model: false,
        ..Options::default()
    })
    .await;

    assert_eq!(refused(&rig).await, Precondition::NoElectricalModel);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_closed_trace_interface_is_refused() {
    let rig = rig_with(Options {
        open_a: false,
        initial_hz_a: None,
        ..Options::default()
    })
    .await;

    assert_eq!(refused(&rig).await, Precondition::TraceNotOpen);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_board_on_another_protocol_is_refused() {
    let rig = rig_with(Options {
        initial_hz_a: None,
        ..Options::default()
    })
    .await;
    let frame = frame_bytes(24, 5, 0, (0, 0), 1, 1, &metadata_payload(SERIAL_A, 0));
    rig.a
        .tx
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .send(frame)
        .unwrap();
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        let seen = rig.a.live.lock().unwrap().boards[SERIAL_A].protocol_version;
        if seen == Some(24) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    assert_eq!(refused(&rig).await, Precondition::ProtocolVersion(24));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn low_disk_is_refused() {
    let rig = rig_with(Options {
        config: EngineConfig {
            min_free_bytes: u64::MAX,
            ..fast_config()
        },
        ..Options::default()
    })
    .await;

    assert!(matches!(refused(&rig).await, Precondition::LowDisk(free) if free < u64::MAX));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_run_while_one_is_in_flight_is_busy() {
    let rig = rig().await;
    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let first = rig.engine.submit(rig.request(tracking())).await.unwrap();
    rig.wait_moving(&mut events, &first).await;

    let second = rig.engine.submit(rig.request(tracking())).await;

    assert_eq!(second, Err(Precondition::Busy));
    gate.notify_one();
    assert_eq!(rig.finish(&first).await.state, RunState::Done);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unconfirmed_gain_write_fails_before_motion() {
    let rig = rig().await;
    rig.a.sim(|s| s.confirm_gains = false);

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, failed("gain write unconfirmed"));
    assert_eq!(
        rig.scripts(),
        vec!["M400".to_owned(), GAINS_LINE.to_owned()]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_filter_error_response_fails_with_its_message() {
    let rig = rig().await;
    let message = "!! FLUX_HZ must be 0 (disabled) or 20..4000";
    rig.a
        .sim(|s| s.failures.push(("FOCI_SET_FILTERS".into(), message.into())));

    let run = rig.run_once(tracking()).await;

    let RunState::Failed(reason) = &run.state else {
        panic!("{:?}", run.state);
    };
    assert!(reason.contains(message), "{reason}");
    assert_eq!(rig.stimuli(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_filter_frame_fails_before_the_capture() {
    let rig = rig().await;
    rig.a.sim(|s| s.confirm_filters = [true, true, true, false]);

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, failed("filter write unconfirmed: flux"));
    assert!(!rig.store.capture_path(&rig.session, &run.id).exists());
    assert_eq!(rig.scripts().len(), 3, "{:?}", rig.scripts());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wrong_cutoff_is_unconfirmed() {
    let rig = rig().await;
    rig.a.sim(|s| s.cutoff_offset = [0, 0, 0, 10]);

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, failed("filter write unconfirmed: flux"));
    assert_eq!(rig.stimuli(), 0);
    assert!(!rig.store.capture_path(&rig.session, &run.id).exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metadata_without_control_state_is_a_missing_baseline() {
    let rig = rig().await;
    rig.a.sim(|s| s.baseline = false);

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, failed("baseline missing"));
    assert_eq!(rig.stimuli(), 0);
    assert_eq!(rig.scripts().last().unwrap(), DORMANT_LINE);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_channel_moved_thresholds_sit_at_one_and_sixteen_counts() {
    let rig = rig().await;
    let cases = [
        ((Q, 0), not_comparable("channel 0 did not move")),
        ((Q + 1, 0), RunState::Done),
        ((50_000, 16 * Q), not_comparable("channel 1 moved")),
        ((50_000, 16 * Q - 1), RunState::Done),
    ];

    for ((declared, other), expected) in cases {
        rig.a.sim(|s| {
            s.profile.declared_range = declared;
            s.profile.other_range = other;
        });
        let run = rig.run_once(tracking()).await;
        assert_eq!(run.state, expected, "declared {declared} other {other}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sequence_jump_a_drop_flag_and_few_edges_are_not_comparable() {
    let rig = rig().await;

    rig.a.sim(|s| s.profile.sequence_jump = true);
    let jump = rig.run_once(tracking()).await;
    rig.a.sim(|s| {
        s.profile = Profile {
            drop_flag: true,
            ..Profile::default()
        }
    });
    let drop = rig.run_once(tracking()).await;
    rig.a.sim(|s| {
        s.profile = Profile {
            probe_edges: 12,
            ..Profile::default()
        }
    });
    let edges = rig.run_once(probe()).await;

    assert_eq!(
        jump.state,
        not_comparable("change_sequence on channel 0 advanced by 2")
    );
    assert!(
        matches!(&drop.state, RunState::NotComparable(r) if r.starts_with("drop gap")),
        "{:?}",
        drop.state
    );
    assert_eq!(edges.state, not_comparable("12 edges"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_probe_run_with_enough_edges_fits_the_damping() {
    let rig = rig().await;
    rig.a.sim(|s| {
        s.zeta_by_position_p.insert(141, 0.03);
    });

    let run = rig.run_once(probe()).await;

    assert_eq!(run.state, RunState::Done, "{:?}", run.verification);
    let probe = run.metrics.unwrap().channels[0].probe.clone().unwrap();
    assert_eq!(probe.edges, 25);
    let foci_tune::model::FitOutcome::Fit { zeta, .. } = probe.velocity else {
        panic!("{:?}", probe.velocity);
    };
    assert!((zeta - 0.03).abs() < 0.01, "{zeta}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kill_during_motion_is_killed_once_shutdown_is_reported() {
    let rig = rig().await;
    rig.fake.script_emergency_stop(None, Duration::ZERO, true);
    let _gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let ids = rig
        .engine
        .submit_ladder(rig.ladder(tracking(), "velocity_p", &[841.0, 941.0, 1041.0], 1))
        .await
        .unwrap();
    rig.wait_moving(&mut events, &ids[0]).await;
    let sent_before_kill = rig.scripts().len();

    rig.engine.kill().await;

    assert_eq!(rig.finish(&ids[0]).await.state, RunState::Killed);
    assert_eq!(rig.run(&ids[1]).state, RunState::Aborted);
    assert_eq!(rig.run(&ids[2]).state, RunState::Aborted);
    assert!(rig.engine.is_idle());
    assert_eq!(rig.fake.emergency_stops(), 1);
    assert_eq!(rig.scripts().len(), sent_before_kill, "{:?}", rig.scripts());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_emergency_stop_is_kill_unconfirmed_and_stays_so() {
    let rig = rig().await;
    rig.fake
        .script_emergency_stop(Some(500), Duration::ZERO, false);
    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let ids = rig
        .engine
        .submit_ladder(rig.ladder(tracking(), "velocity_p", &[841.0, 941.0], 1))
        .await
        .unwrap();
    rig.wait_moving(&mut events, &ids[0]).await;

    rig.engine.kill().await;
    let unconfirmed = rig.finish(&ids[0]).await;
    rig.fake.refuse_websockets(true);
    rig.fake.drop_websockets();
    let mut state = rig.client.state();
    state
        .wait_for(|s| s.klippy == KlippyState::Disconnected)
        .await
        .unwrap();
    gate.notify_one();
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(unconfirmed.state, RunState::KillUnconfirmed);
    assert_eq!(rig.run(&ids[0]).state, RunState::KillUnconfirmed);
    assert_eq!(rig.run(&ids[1]).state, RunState::Aborted);
    assert!(rig.engine.is_idle());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_emergency_stop_that_times_out_is_kill_unconfirmed() {
    let rig = rig().await;
    rig.fake
        .script_emergency_stop(None, Duration::from_secs(2), true);
    let _gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let id = rig.engine.submit(rig.request(tracking())).await.unwrap();
    rig.wait_moving(&mut events, &id).await;

    rig.engine.kill().await;

    assert_eq!(rig.run(&id).state, RunState::KillUnconfirmed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abort_during_the_first_of_thirty_runs_finishes_it_and_aborts_the_rest() {
    let rig = rig().await;
    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let values: Vec<f64> = (0..10).map(|k| 841.0 + 100.0 * f64::from(k)).collect();
    let ids = rig
        .engine
        .submit_ladder(rig.ladder(tracking(), "velocity_p", &values, 3))
        .await
        .unwrap();
    assert_eq!(ids.len(), 30);
    rig.wait_moving(&mut events, &ids[0]).await;

    rig.engine.abort().await;
    gate.notify_one();

    assert_eq!(rig.finish(&ids[0]).await.state, RunState::Done);
    for id in &ids[1..] {
        assert_eq!(rig.run(id).state, RunState::Aborted);
    }
    assert_eq!(rig.stimuli(), 1);
    let listed = rig.store.scan().unwrap();
    assert_eq!(listed[0].runs.len(), 30);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn losing_moonraker_mid_run_fails_it_and_empties_the_queue() {
    let rig = rig().await;
    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let ids = rig
        .engine
        .submit_ladder(rig.ladder(tracking(), "velocity_p", &[841.0, 941.0], 1))
        .await
        .unwrap();
    rig.wait_moving(&mut events, &ids[0]).await;

    rig.fake.refuse_websockets(true);
    rig.fake.drop_websockets();
    let first = rig.finish(&ids[0]).await;
    gate.notify_one();

    assert_eq!(first.state, failed("moonraker connection lost"));
    assert!(matches!(rig.run(&ids[1]).state, RunState::Skipped(_)));
    assert_eq!(rig.stimuli(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn losing_the_trace_interface_mid_run_fails_it() {
    let rig = rig().await;
    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let id = rig.engine.submit(rig.request(tracking())).await.unwrap();
    rig.wait_moving(&mut events, &id).await;

    rig.a.unplug();
    let run = rig.finish(&id).await;
    gate.notify_one();

    assert_eq!(run.state, failed("trace interface lost"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lagging_capture_receiver_fails_the_run() {
    let rig = rig_with(Options {
        frames_capacity: 1024,
        ..Options::default()
    })
    .await;
    rig.a.sim(|s| s.profile.burst = 200_000);

    let run = rig.run_once(tracking()).await;

    assert!(
        matches!(&run.state, RunState::Failed(r) if r.contains("lost")),
        "{:?}",
        run.state
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_stimulus_restores_the_feedforward_gain() {
    let rig = rig().await;
    rig.a.sim(|s| {
        s.failures
            .push(("G90".into(), "!! Must home axis first".into()))
    });

    let run = rig.run_once(probe()).await;

    let RunState::Failed(reason) = &run.state else {
        panic!("{:?}", run.state);
    };
    assert!(reason.contains("Must home axis first"), "{reason}");
    let scripts = rig.scripts();
    let stimulus = scripts.iter().position(|s| s.starts_with("G90")).unwrap();
    assert_eq!(
        scripts[stimulus + 1..].to_vec(),
        vec![RESET_FF_LINE.to_owned(), DORMANT_LINE.to_owned()]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_streaming_board_is_left_in_critical_with_a_note() {
    let rig = rig_with(Options {
        initial_hz_a: Some(1000),
        ..Options::default()
    })
    .await;

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, RunState::Done);
    assert!(!rig.scripts().iter().any(|s| s.contains("MODE=dormant")));
    assert!(
        run.notes.iter().any(|n| n.contains("streaming")),
        "{:?}",
        run.notes
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_board_of_unknown_mode_is_left_in_critical_with_a_note() {
    let rig = rig_with(Options {
        initial_hz_a: None,
        ..Options::default()
    })
    .await;

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, RunState::Done);
    assert!(!rig.scripts().iter().any(|s| s.contains("MODE=dormant")));
    assert!(
        run.notes.iter().any(|n| n.contains("unknown")),
        "{:?}",
        run.notes
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_on_board_a_never_touches_board_b() {
    let rig = rig_with(Options {
        b_echoes: true,
        ..Options::default()
    })
    .await;
    rig.b.sim(|s| s.cutoff_offset = [1; 4]);

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, RunState::Done);
    let modes: Vec<String> = rig
        .scripts()
        .into_iter()
        .filter(|s| s.starts_with("FOCI_TRACE_MODE"))
        .collect();
    assert_eq!(
        modes,
        vec![CRITICAL_LINE.to_owned(), DORMANT_LINE.to_owned()]
    );
    assert_eq!(run.baselines.len(), 2);
    assert!(
        run.baselines
            .iter()
            .all(|b| b.control.velocity_filter_hz == 500)
    );
    assert_eq!(run.stream.unwrap().serial, SERIAL_A);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_boards_matching_frame_does_not_confirm_a_write() {
    let rig = rig_with(Options {
        b_echoes: true,
        ..Options::default()
    })
    .await;
    rig.a.sim(|s| s.confirm_gains = false);

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, failed("gain write unconfirmed"));
}

fn by_value(rig: &Rig, ids: &[RunId]) -> Vec<(f64, RunState)> {
    ids.iter()
        .map(|id| {
            let run = rig.run(id);
            (run.request.ladder_value.unwrap(), run.state)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_probe_ladder_stops_after_a_value_below_the_floor() {
    let rig = rig().await;
    rig.a.sim(|s| {
        s.zeta_by_position_p.insert(200, 0.03);
    });

    let ids = rig
        .engine
        .submit_ladder(rig.ladder(probe(), "position_p", &[141.0, 200.0, 240.0], 1))
        .await
        .unwrap();
    rig.finish(&ids[1]).await;

    let runs = by_value(&rig, &ids);
    assert_eq!(runs[0].1, RunState::Done);
    assert_eq!(runs[1].1, RunState::Done);
    assert!(
        matches!(&runs[2].1, RunState::Skipped(r) if r.contains("below the floor")),
        "{:?}",
        runs[2]
    );
    assert_eq!(rig.stimuli(), 2);
    assert!(
        rig.scripts()
            .iter()
            .any(|s| s.contains("POSITION_P=0.78125000"))
    );
    let listed = rig.store.scan().unwrap();
    assert_eq!(listed[0].runs.len(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_probe_ladder_stops_on_an_unassessable_value() {
    let rig = rig().await;
    rig.a.sim(|s| {
        s.edges_by_position_p.insert(200, 12);
    });

    let ids = rig
        .engine
        .submit_ladder(rig.ladder(probe(), "position_p", &[141.0, 200.0, 240.0], 1))
        .await
        .unwrap();
    rig.finish(&ids[1]).await;

    let runs = by_value(&rig, &ids);
    assert_eq!(runs[0].1, RunState::Done);
    assert_eq!(runs[1].1, not_comparable("12 edges"));
    assert!(
        matches!(&runs[2].1, RunState::Skipped(r) if r.contains("damping unassessable")),
        "{:?}",
        runs[2]
    );
    assert_eq!(rig.stimuli(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stop_rule_sees_one_values_repeats_in_an_interleaved_queue() {
    let rig = rig().await;
    rig.a.sim(|s| {
        s.zeta_by_position_p.insert(200, 0.03);
    });

    let ids = rig
        .engine
        .submit_ladder(rig.ladder(probe(), "position_p", &[141.0, 200.0, 240.0], 2))
        .await
        .unwrap();
    rig.finish(&ids[4]).await;

    let runs = by_value(&rig, &ids);
    let values: Vec<f64> = runs.iter().map(|r| r.0).collect();
    assert_eq!(values, vec![141.0, 200.0, 240.0, 141.0, 200.0, 240.0]);
    assert!(runs[..5].iter().all(|r| r.1 == RunState::Done), "{runs:?}");
    assert!(matches!(&runs[5].1, RunState::Skipped(r) if r.contains("below the floor")));
    assert_eq!(rig.stimuli(), 5);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_typed_value_outside_the_gain_range_rejects_the_ladder() {
    let rig = rig().await;

    let err = rig
        .engine
        .submit_ladder(rig.ladder(probe(), "position_p", &[141.0, 40_000.0], 1))
        .await
        .unwrap_err();

    assert_eq!(
        err,
        LadderRejected::OutOfRange(LadderError::OutOfRange {
            shown: 40_000.0,
            raw: 40_000
        })
    );
    assert!(rig.store.scan().unwrap()[0].runs.is_empty());
    assert!(rig.scripts().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_corner_ladder_converts_with_the_baseline_pwm() {
    let rig = rig().await;

    let ids = rig
        .engine
        .submit_ladder(rig.ladder(tracking(), "velocity_i_corner_hz", &[75.0], 1))
        .await
        .unwrap();
    let run = rig.finish(&ids[0]).await;

    let raw = (75.0 * TAU * 65536.0 / f64::from(PWM_HZ)).round() as i32;
    assert_eq!(run.request.gains.velocity_i, raw);
    assert_eq!(run.request.ladder_value, Some(75.0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revert_writes_the_sessions_starting_gains_through_the_confirmed_path() {
    let rig = rig().await;

    rig.engine.revert(&rig.session).await.unwrap();

    assert_eq!(
        rig.scripts(),
        vec![
            "M400".to_owned(),
            "FOCI_SET_GAINS STEPPER=stepper_x VELOCITY_P=3.51562500 \
             VELOCITY_I=0.009765625000 POSITION_P=0.46875000 POSITION_I=0"
                .to_owned(),
            "FOCI_SET_FILTERS STEPPER=stepper_x VELOCITY_HZ=200 TORQUE_HZ=4000 POSITION_HZ=500 \
             FLUX_HZ=1000"
                .to_owned(),
        ]
    );

    rig.a.sim(|s| s.confirm_filters = [true, false, true, true]);
    assert_eq!(
        rig.engine.revert(&rig.session).await,
        Err("filter write unconfirmed: torque".to_owned())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runs_left_in_flight_by_a_crash_are_reconciled_to_failed() {
    let rig = rig().await;
    let id = rig.engine.submit(rig.request(tracking())).await.unwrap();
    let mut done = rig.finish(&id).await;
    let mut crashed = done.clone();
    crashed.id = RunId::parse("20261030T120000Z-crsh").unwrap();
    crashed.state = RunState::Moving;
    rig.store.write_run(&rig.session, &crashed).unwrap();
    done.notes.push("kept".into());
    rig.store.write_run(&rig.session, &done).unwrap();

    let restarted = foci_tune::engine::reconcile_interrupted(&rig.store).unwrap();

    assert_eq!(restarted, vec![crashed.id.clone()]);
    assert_eq!(rig.run(&crashed.id).state, failed("interrupted"));
    assert_eq!(rig.run(&id), done);
}
