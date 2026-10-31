//! Decoding a run's capture and computing its metrics for every channel.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use foci_tune_analysis::probe::probe_metrics;
use foci_tune_analysis::protocol::{ControlState, Critical, Frame, FrameHeader, decode_frame};
use foci_tune_analysis::signals::channel_series;
use foci_tune_analysis::standstill::standstill_metrics;
use foci_tune_analysis::tracking::{split_speed_segments, tracking_metrics};
use foci_tune_analysis::units::Regime;

use crate::capture_read::read_capture;
use crate::model::{
    ChannelMetrics, Metrics, ProbeRecord, StandstillRecord, StreamInfo, TrackingRecord,
};
use crate::stimulus::RunKind;
use crate::verify::{STANDSTILL_MIN_LEN, STANDSTILL_SETTLE};

/// Speed blocks of a tracking script are separated by a 500 ms dwell; reversals inside a
/// block stand still for a few milliseconds only.
const SPEED_BLOCK_GAP_S: f64 = 0.25;

/// A capture decoded from its first metadata frame on.
#[derive(Debug, Clone)]
pub struct Decoded {
    /// Frames from the first metadata frame on, in file order.
    pub frames: Vec<Frame>,
    /// The first metadata frame's strings and protocol version.
    pub stream: StreamInfo,
    /// First control state per channel after the metadata frame: the baseline.
    pub baselines: HashMap<u8, ControlState>,
}

/// Reads and decodes the capture at `path`; undecodable frames are skipped.
///
/// # Errors
/// The capture cannot be read, or holds no metadata frame.
pub fn decode_capture(path: &Path) -> Result<Decoded, String> {
    let capture = read_capture(path).map_err(|e| e.to_string())?;
    let mut frames = Vec::new();
    let mut stream = None;
    let mut baselines = HashMap::new();
    for record in capture.firmware_frames() {
        let Ok(frame) = decode_frame(&record.payload) else {
            continue;
        };
        match (&stream, &frame) {
            (None, Frame::Metadata(h, m)) => stream = Some(StreamInfo::new(h.protocol_version, m)),
            (None, _) => continue,
            (Some(_), Frame::ControlState(h, c)) => {
                baselines.entry(h.source_index).or_insert_with(|| c.clone());
            }
            (Some(_), _) => {}
        }
        frames.push(frame);
    }
    let stream = stream.ok_or_else(|| "capture holds no metadata frame".to_owned())?;
    Ok(Decoded {
        frames,
        stream,
        baselines,
    })
}

type ChannelFrames = Vec<(FrameHeader, Critical)>;

fn critical_by_channel(frames: &[Frame]) -> BTreeMap<u8, ChannelFrames> {
    let mut by_channel: BTreeMap<u8, ChannelFrames> = BTreeMap::new();
    for frame in frames {
        if let Frame::Critical(h, c) = frame {
            by_channel
                .entry(h.source_index)
                .or_default()
                .push((h.clone(), c.clone()));
        }
    }
    for frames in by_channel.values_mut() {
        frames.sort_by_key(|(h, _)| h.timestamp_ticks);
    }
    by_channel
}

/// What the analysis of one run needs.
#[derive(Debug, Clone, Copy)]
pub struct AnalysisInput<'a> {
    /// Run type and parameters.
    pub kind: &'a RunKind,
    /// Source index of the declared channel.
    pub declared: u8,
    /// Decoded window frames.
    pub frames: &'a [Frame],
    /// Conversion constants per channel; channels without one are not analysed.
    pub regimes: &'a HashMap<u8, Regime>,
    /// Applied stepper speeds in script block order, for tracking runs.
    pub applied_speeds: &'a [f64],
}

/// Tick ranges of the declared channel's speed blocks, the last `speeds` motion spans:
/// an approach move to the start point, when there was one, comes first.
fn speed_blocks(
    frames: &ChannelFrames,
    regime: &Regime,
    speeds: usize,
    notes: &mut Vec<String>,
) -> Vec<(u64, u64)> {
    let series = channel_series(frames, regime);
    let spans = split_speed_segments(&series, SPEED_BLOCK_GAP_S);
    if spans.len() < speeds {
        notes.push(format!(
            "tracking: found {} motion blocks for {speeds} speeds; no per-speed metrics",
            spans.len()
        ));
        return Vec::new();
    }
    spans[spans.len() - speeds..]
        .iter()
        .map(|&(start, end)| {
            (
                frames[start].0.timestamp_ticks,
                frames[end - 1].0.timestamp_ticks,
            )
        })
        .collect()
}

/// Computes the run type's metrics for every channel with critical frames, the declared
/// channel first. Remarks about data that could not be analysed go to `notes`.
pub fn analyze(input: &AnalysisInput<'_>, notes: &mut Vec<String>) -> Metrics {
    let by_channel = critical_by_channel(input.frames);
    let blocks = match (input.kind, by_channel.get(&input.declared)) {
        (RunKind::Tracking { .. }, Some(frames)) => input
            .regimes
            .get(&input.declared)
            .map(|r| speed_blocks(frames, r, input.applied_speeds.len(), notes))
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut order: Vec<u8> = by_channel.keys().copied().collect();
    order.sort_by_key(|&c| (c != input.declared, c));
    let mut channels = Vec::new();
    for channel in order {
        let Some(regime) = input.regimes.get(&channel) else {
            notes.push(format!(
                "channel {channel}: no baseline frame, not analysed"
            ));
            continue;
        };
        let frames = &by_channel[&channel];
        let mut m = ChannelMetrics {
            channel,
            declared: channel == input.declared,
            tracking: Vec::new(),
            probe: None,
            standstill: None,
        };
        match input.kind {
            RunKind::Tracking { .. } => {
                m.tracking = blocks
                    .iter()
                    .zip(input.applied_speeds)
                    .map(|(&(from, to), &speed)| {
                        let block: ChannelFrames = frames
                            .iter()
                            .filter(|(h, _)| (from..=to).contains(&h.timestamp_ticks))
                            .cloned()
                            .collect();
                        let series = channel_series(&block, regime);
                        TrackingRecord::new(speed, &tracking_metrics(&series, regime))
                    })
                    .collect();
            }
            RunKind::Probe { .. } => {
                let series = channel_series(frames, regime);
                m.probe = Some(ProbeRecord::from(&probe_metrics(&series)));
            }
            RunKind::Standstill { .. } => {
                let series = channel_series(frames, regime);
                m.standstill = Some(StandstillRecord::from(&standstill_metrics(
                    &series,
                    regime,
                    STANDSTILL_SETTLE,
                    STANDSTILL_MIN_LEN,
                )));
            }
        }
        channels.push(m);
    }
    Metrics { channels }
}
