//! Live per-board control state decoded from the trace stream.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use foci_tune_analysis::protocol::{
    ControlState, DecodeError, Frame, Metadata, PROTOCOL_VERSION, decode_frame,
};

const CADENCE_WINDOW_S: f64 = 1.0;

/// Trace mode inferred from a board's metadata, which reports a critical rate but not the
/// mode itself.
///
/// `Streaming` covers both the critical and the diagnostic mode, which share a rate. A caller
/// restoring a board's prior mode may do so only when it was a known `Dormant`; `Streaming`
/// and `Unknown` are left alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceMode {
    /// No metadata seen yet, or the board rebooted since the last one.
    Unknown,
    /// The critical rate is 0: no critical frames are emitted.
    Dormant,
    /// A nonzero critical rate: critical or diagnostic mode.
    Streaming,
}

/// Everything known about one board's trace stream.
#[derive(Debug, Clone)]
pub struct BoardLive {
    /// Latest metadata frame.
    pub metadata: Option<Metadata>,
    /// Protocol version of the latest frame with a valid header.
    pub protocol_version: Option<u8>,
    /// Trace mode derived from the latest metadata.
    pub trace_mode: TraceMode,
    /// Highest per-channel critical frame rate over the last second of MCU time (Hz per
    /// channel), 0 when unmeasurable or dormant.
    pub critical_hz_measured: f64,
    /// Latest control state per channel (the frame's source index).
    pub channels: HashMap<u8, ControlState>,
    /// When the latest frame arrived.
    pub last_frame_at: Instant,
    /// Whether the USB interface is currently open.
    pub open: bool,
    /// Boot identifier of the latest frame.
    pub boot_id: Option<u32>,
    /// Sequence number of the latest frame.
    pub last_sequence: Option<u32>,
    /// How many times the boot identifier changed.
    pub boot_changes: u64,
    /// Frames that could not be decoded for a reason other than a protocol mismatch.
    pub undecodable_frames: u64,
    unusable_reason: Option<String>,
    critical_ticks: HashMap<u8, VecDeque<u64>>,
}

impl BoardLive {
    fn new(now: Instant) -> Self {
        Self {
            metadata: None,
            protocol_version: None,
            trace_mode: TraceMode::Unknown,
            critical_hz_measured: 0.0,
            channels: HashMap::new(),
            last_frame_at: now,
            open: false,
            boot_id: None,
            last_sequence: None,
            boot_changes: 0,
            undecodable_frames: 0,
            unusable_reason: None,
            critical_ticks: HashMap::new(),
        }
    }

    /// Why runs must not start on this board, `None` when it is usable.
    pub fn unusable_reason(&self) -> Option<&str> {
        self.unusable_reason.as_deref()
    }

    fn reset_stream_state(&mut self) {
        self.channels.clear();
        self.critical_ticks.clear();
        self.critical_hz_measured = 0.0;
        self.trace_mode = TraceMode::Unknown;
    }

    fn note_critical(&mut self, channel: u8, ticks: u64) {
        if self.metadata.is_none() {
            return;
        }
        let window = self.critical_ticks.entry(channel).or_default();
        if window.back().is_some_and(|&last| ticks < last) {
            window.clear();
        }
        window.push_back(ticks);
    }

    fn refresh_cadence(&mut self, now_ticks: u64) {
        let Some(clock_freq) = self
            .metadata
            .as_ref()
            .map(|m| f64::from(m.clock_freq))
            .filter(|&f| f > 0.0)
        else {
            self.critical_hz_measured = 0.0;
            return;
        };
        let span_limit = (clock_freq * CADENCE_WINDOW_S) as u64;
        let mut best = 0.0f64;
        for window in self.critical_ticks.values_mut() {
            while window
                .front()
                .is_some_and(|&first| now_ticks.saturating_sub(first) > span_limit)
            {
                window.pop_front();
            }
            if let (Some(&first), Some(&last)) = (window.front(), window.back()) {
                let span_s = (last - first) as f64 / clock_freq;
                if window.len() > 1 && span_s > 0.0 {
                    best = best.max((window.len() - 1) as f64 / span_s);
                }
            }
        }
        self.critical_hz_measured = best;
    }
}

/// Live state of every board seen, keyed by USB serial.
#[derive(Debug, Clone, Default)]
pub struct LiveState {
    /// Boards by serial.
    pub boards: HashMap<String, BoardLive>,
}

impl LiveState {
    /// Records that the interface for `serial` opened.
    pub fn mark_open(&mut self, serial: &str, now: Instant) {
        self.boards
            .entry(serial.to_owned())
            .or_insert_with(|| BoardLive::new(now))
            .open = true;
    }

    /// Records that the interface for `serial` was lost; the last known state is kept.
    pub fn mark_closed(&mut self, serial: &str) {
        if let Some(board) = self.boards.get_mut(serial) {
            board.open = false;
        }
    }

    /// Applies one assembled frame from the board with USB serial `serial`.
    ///
    /// A frame whose protocol version is not 25 is never decoded as data: the board
    /// stays listed and open, with [`BoardLive::unusable_reason`] set.
    pub fn ingest(&mut self, serial: &str, frame: &[u8], now: Instant) {
        let board = self
            .boards
            .entry(serial.to_owned())
            .or_insert_with(|| BoardLive::new(now));
        board.open = true;
        let decoded = match decode_frame(frame) {
            Ok(decoded) => decoded,
            Err(DecodeError::UnsupportedVersion(version)) => {
                board.protocol_version = Some(version);
                board.last_frame_at = now;
                board.unusable_reason = Some(format!(
                    "board speaks trace protocol {version}, this tool needs {PROTOCOL_VERSION}; \
                     reflash the board"
                ));
                return;
            }
            Err(_) => {
                board.undecodable_frames += 1;
                return;
            }
        };
        board.protocol_version = Some(PROTOCOL_VERSION);
        board.unusable_reason = None;
        board.last_frame_at = now;
        let header = match &decoded {
            Frame::Critical(h, _) | Frame::ControlState(h, _) | Frame::Metadata(h, _) => h,
            Frame::Other(h) => h,
        };
        if board.boot_id.is_some_and(|boot| boot != header.boot_id) {
            board.boot_changes += 1;
            board.reset_stream_state();
        }
        board.boot_id = Some(header.boot_id);
        board.last_sequence = Some(header.sequence);
        let ticks = header.timestamp_ticks;
        match decoded {
            Frame::Metadata(_, metadata) => {
                board.trace_mode = if metadata.critical_hz == 0 {
                    TraceMode::Dormant
                } else {
                    TraceMode::Streaming
                };
                board.critical_ticks.clear();
                board.critical_hz_measured = 0.0;
                board.metadata = Some(metadata);
            }
            Frame::ControlState(h, state) => {
                board.channels.insert(h.source_index, state);
            }
            Frame::Critical(h, _) => board.note_critical(h.source_index, h.timestamp_ticks),
            Frame::Other(_) => {}
        }
        board.refresh_cadence(ticks);
    }
}
