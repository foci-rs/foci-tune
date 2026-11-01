//! The simulated printer the engine and API tests drive: a scripted Moonraker, two FOCI
//! boards streaming synthetic trace frames, and a store in a temporary directory.

#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::f64::consts::TAU;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use foci_tune::discovery::{DiscoveryConfig, run_trace_discovery};
use foci_tune::engine::{Engine, EngineConfig, EngineEvent, LadderRequest};
use foci_tune::live::LiveState;
use foci_tune::model::{
    FORMAT_VERSION, Filters, Gains, LadderDef, Run, RunId, RunRequest, RunState, Session, SessionId,
};
use foci_tune::moonraker::{ClientConfig, KlippyState, MoonrakerApi, MoonrakerClient};
use foci_tune::pattern::Pattern;
use foci_tune::stimulus::RunKind;
use foci_tune::store::Store;
use foci_tune::trace::{FrameSource, pump_source};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::{Notify, broadcast};
use tokio::task::JoinHandle;

use crate::fake_moonraker::{FakeMoonraker, ScriptReply};

pub const SERIAL_A: &str = "2600AAAA";
pub const SERIAL_B: &str = "3300BBBB";
pub const CLOCK: u32 = 260_000_000;
pub const TICKS_PER_MS: u64 = 260_000;
pub const COUNTS_PER_REV: u32 = 4096;
pub const Q: i32 = 16;
pub const PWM_HZ: u32 = 50_000;
pub const UNITS_PER_MM: f64 = 65536.0 / 40.0;
pub const WAIT: Duration = Duration::from_secs(20);

pub const GAINS_LINE: &str = "FOCI_SET_GAINS STEPPER=stepper_x VELOCITY_P=3.28515625 \
    VELOCITY_I=0.100585937500 POSITION_P=0.55078125 POSITION_I=0";
pub const FILTERS_LINE: &str = "FOCI_SET_FILTERS STEPPER=stepper_x VELOCITY_HZ=500 TORQUE_HZ=1000 POSITION_HZ=500 FLUX_HZ=1000";
pub const CRITICAL_LINE: &str = "FOCI_TRACE_MODE STEPPER=stepper_x MODE=critical";
pub const DORMANT_LINE: &str = "FOCI_TRACE_MODE STEPPER=stepper_x MODE=dormant";
pub const RESET_FF_LINE: &str = "FOCI_SET_VELOCITY_FEEDFORWARD STEPPER=stepper_x ENABLE=1 GAIN=1.0";

pub struct ScriptedSource {
    pub serial: String,
    pub rx: mpsc::Receiver<Vec<u8>>,
    pub pending: VecDeque<u8>,
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
pub struct Channel {
    pub change_sequence: u32,
    pub gains: [i32; 4],
    pub filters: [u16; 4],
}

#[derive(Clone)]
pub struct Profile {
    pub declared_range: i32,
    pub other_range: i32,
    pub probe_edges: usize,
    pub drop_flag: bool,
    pub sequence_jump: bool,
    pub burst: usize,
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

pub type Drift = Box<dyn FnOnce(&mut Channel) + Send>;

pub struct Sim {
    pub sequence: u32,
    pub ticks: u64,
    pub channels: [Channel; 2],
    pub confirm_gains: bool,
    pub confirm_filters: [bool; 4],
    pub cutoff_offset: [u16; 4],
    pub baseline: bool,
    pub failures: Vec<(String, String)>,
    pub gate: Option<Arc<Notify>>,
    pub profile: Profile,
    pub zeta_by_position_p: HashMap<i32, f64>,
    pub edges_by_position_p: HashMap<i32, usize>,
    pub profile_by_velocity_p: HashMap<i32, Profile>,
    /// Profile for a stimulus whose script contains the text; takes precedence over the others.
    pub profile_by_script: Vec<(String, Profile)>,
    /// Probe damping by velocity P; takes precedence over `zeta_by_position_p`.
    pub zeta_by_velocity_p: HashMap<i32, f64>,
    /// Counts the measured position trails the commanded one during tracking ramps, by
    /// velocity P.
    pub lag_by_velocity_p: HashMap<i32, i32>,
    pub unconfirmed_velocity_p: Vec<i32>,
    /// Applied to the channel when the critical trace mode is requested, as another client's
    /// write landing between the confirmed writes and the baseline.
    pub drift: Option<Drift>,
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
            profile_by_velocity_p: HashMap::new(),
            profile_by_script: Vec::new(),
            zeta_by_velocity_p: HashMap::new(),
            lag_by_velocity_p: HashMap::new(),
            unconfirmed_velocity_p: Vec::new(),
            drift: None,
        }
    }
}

pub struct Board {
    pub serial: &'static str,
    pub tx: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    pub sim: Mutex<Sim>,
    pub live: Arc<Mutex<LiveState>>,
    pub frames: broadcast::Sender<(String, Bytes)>,
}

pub fn frame_bytes(
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

pub fn control_payload(c: &Channel, applied_request: u8) -> Vec<u8> {
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

pub fn metadata_payload(serial: &str, critical_hz: u32) -> Vec<u8> {
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

pub fn critical_payload(step: i32, tmc: i32, ff: i32) -> Vec<u8> {
    let mut p = vec![0u8; 88];
    p[0..4].copy_from_slice(&step.to_le_bytes());
    p[4..8].copy_from_slice(&tmc.to_le_bytes());
    p[20..24].copy_from_slice(&ff.to_le_bytes());
    p
}

pub fn param<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
}

pub fn float(line: &str, key: &str) -> f64 {
    param(line, key).unwrap().parse().unwrap()
}

/// Velocity of a probe run in mm/s: a cruise plus a damped response to every feedforward
/// edge, scaled by the edge's step.
pub fn probe_velocity(n: usize, edges: &[(usize, f64)], zeta: f64) -> f64 {
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

fn spawn_pump(
    serial: &'static str,
    rx: mpsc::Receiver<Vec<u8>>,
    live: &Arc<Mutex<LiveState>>,
    frames: &broadcast::Sender<(String, Bytes)>,
    stop: &Arc<AtomicBool>,
) {
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

impl Board {
    pub fn new(
        serial: &'static str,
        live: &Arc<Mutex<LiveState>>,
        frames: &broadcast::Sender<(String, Bytes)>,
        stop: &Arc<AtomicBool>,
        open: bool,
    ) -> Arc<Self> {
        let (tx, rx) = mpsc::channel();
        if open {
            spawn_pump(serial, rx, live, frames, stop);
        }
        Arc::new(Self {
            serial,
            tx: Mutex::new(open.then_some(tx)),
            sim: Mutex::new(Sim::default()),
            live: live.clone(),
            frames: frames.clone(),
        })
    }

    pub fn sim(&self, f: impl FnOnce(&mut Sim)) {
        f(&mut self.sim.lock().unwrap());
    }

    pub fn unplug(&self) {
        self.tx.lock().unwrap().take();
    }

    /// Opens the interface again with a new reader, as after a USB re-enumeration.
    pub fn replug(&self, stop: &Arc<AtomicBool>) {
        let (tx, rx) = mpsc::channel();
        *self.tx.lock().unwrap() = Some(tx);
        spawn_pump(self.serial, rx, &self.live, &self.frames, stop);
    }

    pub fn next(sim: &mut Sim) -> (u32, u64) {
        sim.sequence += 1;
        (sim.sequence, sim.ticks)
    }

    /// Sends frames to the reader and waits until it has broadcast the last of them.
    pub fn emit(&self, frames: Vec<Vec<u8>>) {
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

    pub fn control(sim: &mut Sim, channel: u8, request: u8) -> Vec<u8> {
        let payload = control_payload(&sim.channels[usize::from(channel)], request);
        let (seq, ticks) = Self::next(sim);
        frame_bytes(25, 10, 0, (1, channel), seq, ticks, &payload)
    }

    pub fn mode_frames(&self, critical_hz: u32, baseline: bool) -> Vec<Vec<u8>> {
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

    /// One critical frame: the board's protocol is known, its trace mode is not.
    pub fn poke(&self) {
        let mut sim = self.sim.lock().unwrap();
        let (seq, ticks) = Self::next(&mut sim);
        let frame = frame_bytes(25, 1, 0, (1, 0), seq, ticks, &critical_payload(0, 0, 0));
        drop(sim);
        self.emit(vec![frame]);
    }

    pub fn announce(&self, critical_hz: u32) {
        let frames = self.mode_frames(critical_hz, true);
        self.emit(frames);
    }

    pub fn react(&self, script: &str, channel: u8) -> ScriptReply {
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
            let velocity_p = (float(script, "VELOCITY_P") * 256.0).round() as i32;
            if sim.confirm_gains && !sim.unconfirmed_velocity_p.contains(&velocity_p) {
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
            let baseline = {
                let mut sim = self.sim.lock().unwrap();
                if hz > 0
                    && let Some(drift) = sim.drift.take()
                {
                    drift(&mut sim.channels[ch]);
                }
                sim.baseline
            };
            let frames = self.mode_frames(hz, baseline);
            self.emit(frames);
        } else if script.starts_with("G90") {
            let probe = script.matches("FOCI_SET_VELOCITY_FEEDFORWARD").count() > 3;
            let frames = self.stimulus(channel, probe, script);
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

    pub fn stimulus(&self, declared: u8, probe: bool, script: &str) -> Vec<Vec<u8>> {
        let mut sim = self.sim.lock().unwrap();
        let velocity_p = sim.channels[usize::from(declared)].gains[0];
        let profile = sim
            .profile_by_script
            .iter()
            .find(|(text, _)| script.contains(text.as_str()))
            .map(|(_, p)| p)
            .or_else(|| sim.profile_by_velocity_p.get(&velocity_p))
            .unwrap_or(&sim.profile)
            .clone();
        let other = 1 - declared;
        let position_p = sim.channels[usize::from(declared)].gains[2];
        let mut samples: Vec<(i32, i32, i32)> = Vec::new();
        if probe {
            let edge_count = *sim
                .edges_by_position_p
                .get(&position_p)
                .unwrap_or(&profile.probe_edges);
            let zeta = *sim
                .zeta_by_velocity_p
                .get(&velocity_p)
                .or_else(|| sim.zeta_by_position_p.get(&position_p))
                .unwrap_or(&0.2);
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
            let lag = sim.lag_by_velocity_p.get(&velocity_p).copied().unwrap_or(0);
            let ramp = |k: i32| (i64::from(r) * i64::from(k) / 150) as i32;
            let trail = |k: i32, sign: i32| if k > 0 && k < 150 { sign * lag } else { 0 };
            samples.extend((0..=150).map(|k| (ramp(k), ramp(k) - trail(k, 1), 0)));
            samples.extend((0..400).map(|_| (r, r, 0)));
            samples.extend((0..=150).map(|k| (ramp(150 - k), ramp(150 - k) + trail(k, 1), 0)));
        }
        let mut out = Vec::new();
        let middle = samples.len() / 2;
        for (n, (step, tmc, ff)) in samples.into_iter().enumerate() {
            sim.ticks += TICKS_PER_MS;
            let flags = u8::from(profile.drop_flag && n == middle);
            let (seq, ticks) = Self::next(&mut sim);
            let mut payload = critical_payload(step + 1000, tmc + 1000, ff);
            payload[8..12].copy_from_slice(&(step - tmc).to_le_bytes());
            out.push(frame_bytes(
                25,
                1,
                flags,
                (1, declared),
                seq,
                ticks,
                &payload,
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

pub fn settings(electrical_model: bool) -> Value {
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

pub fn fast_config() -> EngineConfig {
    EngineConfig {
        confirm_timeout: Duration::from_millis(300),
        baseline_timeout: Duration::from_millis(300),
        drain: Duration::from_millis(50),
        kill_timeout: Duration::from_millis(300),
        min_free_bytes: 0,
        loss_poll: Duration::from_millis(10),
        shutdown_notice: Duration::from_millis(300),
    }
}

pub fn fast_discovery() -> DiscoveryConfig {
    DiscoveryConfig {
        grace: Duration::from_millis(300),
        retry_start: Duration::from_millis(50),
        retry_max: Duration::from_millis(200),
        poll: Duration::from_millis(10),
    }
}

pub struct Options {
    pub config: EngineConfig,
    pub discovery: Option<DiscoveryConfig>,
    pub announce_b: bool,
    pub electrical_model: bool,
    pub open_a: bool,
    pub initial_hz_a: Option<u32>,
    pub frames_capacity: usize,
    pub homed: &'static str,
    pub b_echoes: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            config: fast_config(),
            discovery: None,
            announce_b: true,
            electrical_model: true,
            open_a: true,
            initial_hz_a: Some(0),
            frames_capacity: 1 << 16,
            homed: "xyz",
            b_echoes: false,
        }
    }
}

pub struct Rig {
    pub fake: FakeMoonraker,
    pub client: Arc<MoonrakerClient>,
    pub engine: Arc<Engine<MoonrakerClient>>,
    pub store: Store,
    pub a: Arc<Board>,
    pub b: Arc<Board>,
    pub session: SessionId,
    pub stop: Arc<AtomicBool>,
    pub live: Arc<Mutex<LiveState>>,
    pub frames: broadcast::Sender<(String, Bytes)>,
    pub discovery: Option<JoinHandle<()>>,
    pub _dir: TempDir,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(task) = &self.discovery {
            task.abort();
        }
    }
}

pub fn session_record() -> Session {
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

pub async fn rig_with(o: Options) -> Rig {
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
    if o.announce_b {
        b.announce(0);
    }

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
    let engine = Arc::new(
        Engine::start(
            client.clone(),
            live.clone(),
            frames.clone(),
            store.clone(),
            o.config,
        )
        .unwrap(),
    );
    let discovery = o
        .discovery
        .map(|config| tokio::spawn(run_trace_discovery(engine.clone(), live.clone(), config)));
    Rig {
        fake,
        client,
        engine,
        store,
        a,
        b,
        session,
        stop,
        live,
        frames,
        discovery,
        _dir: dir,
    }
}

pub async fn rig() -> Rig {
    rig_with(Options::default()).await
}

pub fn tracking() -> RunKind {
    RunKind::Tracking {
        speeds_mm_s: vec![50.0, 195.0],
        strokes: 1,
    }
}

pub fn probe() -> RunKind {
    RunKind::Probe {
        speed_mm_s: 195.0,
        high_gain: 1.3,
        segments_between_toggles: 3,
        strokes: 2,
    }
}

pub fn pattern() -> Pattern {
    Pattern {
        start: (100.0, 100.0),
        end: (200.0, 200.0),
    }
}

impl Rig {
    pub fn request(&self, kind: RunKind) -> RunRequest {
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

    pub fn ladder(&self, kind: RunKind, axis: &str, values: &[f64], repeats: u32) -> LadderRequest {
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

    pub fn scripts(&self) -> Vec<String> {
        self.fake.scripts()
    }

    pub fn stimuli(&self) -> usize {
        self.scripts()
            .iter()
            .filter(|s| s.starts_with("G90"))
            .count()
    }

    pub fn run(&self, id: &RunId) -> Run {
        self.store.read_run(&self.session, id).unwrap()
    }

    pub async fn finish(&self, id: &RunId) -> Run {
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

    pub async fn run_once(&self, kind: RunKind) -> Run {
        let id = self.engine.submit(self.request(kind)).await.unwrap();
        self.finish(&id).await
    }

    /// Waits until `run` is moving and its stimulus has reached Moonraker.
    pub async fn wait_moving(&self, events: &mut broadcast::Receiver<EngineEvent>, run: &RunId) {
        wait_for_state(events, run, &RunState::Moving).await;
        let deadline = Instant::now() + WAIT;
        while self.stimuli() == 0 {
            assert!(Instant::now() < deadline, "stimulus not sent");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Runs one stimulus and kills it with an emergency stop that fails, leaving the kill
    /// unconfirmed; returns the killed run.
    pub async fn unconfirmed_kill(&self) -> RunId {
        self.fake
            .script_emergency_stop(Some(500), Duration::ZERO, None);
        let gate = self.hold_stimulus();
        let mut events = self.engine.events();
        let id = self.engine.submit(self.request(tracking())).await.unwrap();
        self.wait_moving(&mut events, &id).await;
        self.engine.kill().await;
        assert_eq!(self.finish(&id).await.state, RunState::KillUnconfirmed);
        self.a.sim(|s| s.gate = None);
        gate.notify_one();
        id
    }

    pub fn hold_stimulus(&self) -> Arc<Notify> {
        let gate = Arc::new(Notify::new());
        self.a.sim(|s| s.gate = Some(gate.clone()));
        gate
    }
}

pub async fn wait_for_state(
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

pub fn failed(reason: &str) -> RunState {
    RunState::Failed(reason.into())
}

pub fn not_comparable(reason: &str) -> RunState {
    RunState::NotComparable(reason.into())
}
