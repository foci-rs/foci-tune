//! The event stream at `/api/events`.
//!
//! Server messages are JSON objects tagged by `type`:
//! - `printer`: the [`PrinterView`], the latest shared snapshot on connect, then every time
//!   the snapshot task publishes a new one (at most once a second). Control state per channel
//!   travels inside it (`boards[].channels`).
//! - `run_state`: `{session, run, state, reason?}` for every run state change.
//! - `run_result`: `{session, run}` with the stored record once a run reaches its final state.
//! - `queue`: `{runs}`, the queued run ids in order.
//! - `live`: `{serial, clock_freq, channels: [{channel, ticks, step_position, tmc_position,
//!   position_error, torque, ff_offset}]}`, raw critical samples batched at 20 Hz, sent only
//!   to a client that subscribed and only while a run records.
//! - `subscribed` / `unsubscribed`: `{stream: "live"}`, acknowledging a client request.
//! - `resync`: `{reason}`. The connection fell behind and missed messages; the client should
//!   refetch `/api/printer` and the sessions it shows. The connection stays open.
//!
//! Client messages: `{"subscribe": "live"}` and `{"unsubscribe": "live"}`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use bytes::Bytes;
use foci_tune_analysis::protocol::{Frame, decode_frame};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{Instant, MissedTickBehavior};

use crate::api::{App, PrinterView};
use crate::engine::EngineEvent;
use crate::model::{Run, RunId, RunState, SessionId};
use crate::moonraker::MoonrakerApi;

const LIVE_PERIOD: Duration = Duration::from_millis(50);
const RECORDING_RECHECK: Duration = Duration::from_millis(10);
const LIVE: &str = "live";

/// Raw critical samples of one channel.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LiveChannel {
    /// Source index.
    pub channel: u8,
    /// Frame timestamps in MCU ticks.
    pub ticks: Vec<u64>,
    /// Commanded step position, position units.
    pub step_position: Vec<i32>,
    /// TMC4671 actual position, position units.
    pub tmc_position: Vec<i32>,
    /// Position error, position units.
    pub position_error: Vec<i32>,
    /// Interim PID-input target torque, raw.
    pub torque: Vec<i32>,
    /// Cached velocity feedforward offset, raw.
    pub ff_offset: Vec<i32>,
}

/// One board's critical samples since the previous batch.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LiveBatch {
    /// USB serial of the board.
    pub serial: String,
    /// MCU clock in Hz from the board's metadata, to convert ticks; 0 when unknown.
    pub clock_freq: u32,
    /// Channels by source index.
    pub channels: Vec<LiveChannel>,
}

/// A message to a client.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Printer snapshot.
    Printer(Arc<PrinterView>),
    /// A run changed state.
    RunState {
        /// Owning session.
        session: SessionId,
        /// The run.
        run: RunId,
        /// New state, flattened to `state` and `reason`.
        #[serde(flatten)]
        state: RunState,
    },
    /// A run reached its final state.
    RunResult {
        /// Owning session.
        session: SessionId,
        /// The record as stored.
        run: Box<Run>,
    },
    /// The queued runs.
    Queue {
        /// Run ids in execution order.
        runs: Vec<RunId>,
    },
    /// Critical samples of one board.
    Live(LiveBatch),
    /// A subscription is active.
    Subscribed {
        /// Stream name.
        stream: &'static str,
    },
    /// A subscription ended.
    Unsubscribed {
        /// Stream name.
        stream: &'static str,
    },
    /// Messages were missed; refetch state.
    Resync {
        /// What was missed.
        reason: String,
    },
}

impl From<EngineEvent> for ServerMessage {
    fn from(e: EngineEvent) -> Self {
        match e {
            EngineEvent::State {
                session,
                run,
                state,
            } => Self::RunState {
                session,
                run,
                state,
            },
            EngineEvent::Finished(run) => Self::RunResult {
                session: run.request.session.clone(),
                run,
            },
            EngineEvent::Queue(runs) => Self::Queue { runs },
        }
    }
}

/// `GET /api/events`: upgrades to the event stream.
pub async fn events<M: MoonrakerApi + 'static>(
    ws: WebSocketUpgrade,
    State(api): State<App<M>>,
) -> Response {
    ws.on_upgrade(move |socket| connection(socket, api))
}

struct LiveFeed {
    rx: broadcast::Receiver<(String, Bytes)>,
    batches: BTreeMap<String, BTreeMap<u8, LiveChannel>>,
    recording: bool,
    checked_at: Option<Instant>,
}

fn is_recording(state: &RunState) -> bool {
    matches!(
        state,
        RunState::Baseline | RunState::Recording | RunState::Moving | RunState::Draining
    )
}

impl LiveFeed {
    fn new(rx: broadcast::Receiver<(String, Bytes)>) -> Self {
        Self {
            rx,
            batches: BTreeMap::new(),
            recording: false,
            checked_at: None,
        }
    }

    fn push(&mut self, serial: String, frame: &[u8]) {
        let Ok(Frame::Critical(h, c)) = decode_frame(frame) else {
            return;
        };
        let ch = self
            .batches
            .entry(serial)
            .or_default()
            .entry(h.source_index)
            .or_insert_with(|| LiveChannel {
                channel: h.source_index,
                ..LiveChannel::default()
            });
        ch.ticks.push(h.timestamp_ticks);
        ch.step_position.push(c.step_position);
        ch.tmc_position.push(c.tmc_position_actual);
        ch.position_error.push(c.position_error);
        ch.torque.push(c.interim_pidin_target_torque);
        ch.ff_offset.push(c.cached_pid_velocity_offset);
    }
}

async fn send(socket: &mut WebSocket, message: &ServerMessage) -> bool {
    let Ok(text) = serde_json::to_string(message) else {
        tracing::warn!("event not serialisable");
        return true;
    };
    socket.send(Message::Text(text.into())).await.is_ok()
}

enum Request {
    Subscribe,
    Unsubscribe,
}

fn parse_request(text: &str) -> Option<Request> {
    let v: Value = serde_json::from_str(text).ok()?;
    if v["subscribe"] == LIVE {
        Some(Request::Subscribe)
    } else if v["unsubscribe"] == LIVE {
        Some(Request::Unsubscribe)
    } else {
        None
    }
}

async fn next_frame(live: &mut Option<LiveFeed>) -> Result<(String, Bytes), RecvError> {
    match live {
        Some(feed) => feed.rx.recv().await,
        None => std::future::pending().await,
    }
}

async fn connection<M: MoonrakerApi + 'static>(mut socket: WebSocket, api: App<M>) {
    let mut events = api.engine.events();
    let mut printer = api.printer_feed();
    printer.mark_changed();
    let mut printer_alive = true;
    let mut flush = tokio::time::interval(LIVE_PERIOD);
    flush.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut live: Option<LiveFeed> = None;
    loop {
        let ok = tokio::select! {
            incoming = socket.recv() => match incoming {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(Message::Text(text))) => match parse_request(text.as_str()) {
                    Some(Request::Subscribe) => {
                        if live.is_none() {
                            live = Some(LiveFeed::new(api.frames.subscribe()));
                        }
                        send(&mut socket, &ServerMessage::Subscribed { stream: LIVE }).await
                    }
                    Some(Request::Unsubscribe) => {
                        live = None;
                        send(&mut socket, &ServerMessage::Unsubscribed { stream: LIVE }).await
                    }
                    None => true,
                },
                Some(Ok(_)) => true,
            },
            event = events.recv() => match event {
                Ok(e) => send(&mut socket, &ServerMessage::from(e)).await,
                Err(RecvError::Lagged(n)) => {
                    let reason = format!("missed {n} run events");
                    send(&mut socket, &ServerMessage::Resync { reason }).await
                }
                Err(RecvError::Closed) => return,
            },
            changed = printer.changed(), if printer_alive => {
                printer_alive = changed.is_ok();
                let snapshot = printer.borrow_and_update().clone();
                match snapshot {
                    Some(view) => send(&mut socket, &ServerMessage::Printer(view)).await,
                    None => true,
                }
            }
            frame = next_frame(&mut live) => match frame {
                Ok((serial, frame)) => {
                    if let Some(feed) = live.as_mut() {
                        let now = Instant::now();
                        if feed.checked_at.is_none_or(|at| now - at >= RECORDING_RECHECK) {
                            feed.recording = api
                                .engine
                                .status()
                                .current
                                .is_some_and(|(_, _, state)| is_recording(&state));
                            feed.checked_at = Some(now);
                        }
                        if feed.recording {
                            feed.push(serial, &frame);
                        }
                    }
                    true
                }
                Err(RecvError::Lagged(n)) => {
                    let reason = format!("live stream fell behind; {n} trace frames dropped");
                    send(&mut socket, &ServerMessage::Resync { reason }).await
                }
                Err(RecvError::Closed) => {
                    live = None;
                    true
                }
            },
            _ = flush.tick(), if live.is_some() => {
                let mut ok = true;
                if let Some(feed) = live.as_mut() {
                    for (serial, channels) in std::mem::take(&mut feed.batches) {
                        let clock_freq = api
                            .live
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .boards
                            .get(&serial)
                            .and_then(|b| b.metadata.as_ref())
                            .map_or(0, |m| m.clock_freq);
                        let batch = LiveBatch {
                            serial,
                            clock_freq,
                            channels: channels.into_values().collect(),
                        };
                        ok &= send(&mut socket, &ServerMessage::Live(batch)).await;
                    }
                }
                ok
            }
        };
        if !ok {
            return;
        }
    }
}
