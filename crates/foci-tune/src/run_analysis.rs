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

/// The approach and the speed blocks of a script are separated by 500 ms dwells; reversals
/// inside a block hold the commanded position for a few milliseconds only.
const SPEED_BLOCK_GAP_S: f64 = 0.4;

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

/// Where the stimulus runs inside the recorded window. Every script starts with an approach
/// move to the pattern start and a settle dwell; both lie before the stimulus.
#[derive(Debug, Clone, PartialEq)]
pub enum StimulusWindow {
    /// No commanded motion bounds the stimulus: the whole recorded window.
    Whole,
    /// Tick ranges of the declared channel's stroke blocks in time order: one per speed of a
    /// tracking run, one for a probe run.
    Strokes {
        /// `(first, last)` tick of each block, inclusive.
        blocks: Vec<(u64, u64)>,
    },
    /// Ticks of a standstill's rests, from the start of the hold after the approach to the
    /// last frame, inclusive.
    Rests {
        /// First tick.
        from: u64,
        /// Last tick.
        to: u64,
    },
    /// The declared channel's motion blocks do not match the script.
    Unmatched(String),
}

impl StimulusWindow {
    /// The `(first, last)` ticks the stimulus spans, `None` for the whole window or none.
    pub fn ticks(&self) -> Option<(u64, u64)> {
        match self {
            Self::Strokes { blocks } => Some((blocks.first()?.0, blocks.last()?.1)),
            Self::Rests { from, to } => Some((*from, *to)),
            Self::Whole | Self::Unmatched(_) => None,
        }
    }
}

/// The declared channel's stroke blocks, when `expected` of them follow an optional approach.
fn stroke_blocks(frames: &ChannelFrames, regime: &Regime, expected: usize) -> StimulusWindow {
    let series = channel_series(frames, regime);
    let spans = split_speed_segments(&series, SPEED_BLOCK_GAP_S);
    let blocks = match spans.len() {
        n if n == expected => &spans[..],
        n if n == expected + 1 => &spans[1..],
        n => {
            let plural = if expected == 1 { "" } else { "s" };
            return StimulusWindow::Unmatched(format!(
                "found {n} motion blocks for {expected} speed{plural}"
            ));
        }
    };
    StimulusWindow::Strokes {
        blocks: blocks
            .iter()
            .map(|&(start, end)| {
                (
                    frames[start].0.timestamp_ticks,
                    frames[end - 1].0.timestamp_ticks,
                )
            })
            .collect(),
    }
}

/// A standstill's rests: from the last commanded change, on any channel, before the first hold
/// of at least the block gap that follows motion, to the last frame.
///
/// Without an approach the first such hold follows the first rest move, so the window starts
/// one rest late; it never starts inside an approach.
fn rest_window(by_channel: &BTreeMap<u8, ChannelFrames>, clock_freq: u32) -> StimulusWindow {
    let mut changes: Vec<u64> = by_channel
        .values()
        .flat_map(|frames| {
            frames
                .windows(2)
                .filter(|w| w[0].1.step_position != w[1].1.step_position)
                .map(|w| w[1].0.timestamp_ticks)
        })
        .collect();
    let Some(last) = by_channel
        .values()
        .filter_map(|frames| frames.last())
        .map(|(h, _)| h.timestamp_ticks)
        .max()
    else {
        return StimulusWindow::Whole;
    };
    changes.sort_unstable();
    changes.dedup();
    changes.push(last);
    let gap = (SPEED_BLOCK_GAP_S * f64::from(clock_freq)) as u64;
    changes
        .windows(2)
        .find(|w| w[1] - w[0] >= gap)
        .map_or(StimulusWindow::Whole, |w| StimulusWindow::Rests {
            from: w[0],
            to: last,
        })
}

/// Locates the stimulus in the recorded window from the commanded positions.
///
/// Tracking and probe runs expect one block per applied speed or one block, after an optional
/// approach; any other count is [`StimulusWindow::Unmatched`]. Without critical frames or a
/// regime for the declared channel the window is [`StimulusWindow::Whole`].
pub fn stimulus_window(input: &AnalysisInput<'_>) -> StimulusWindow {
    let by_channel = critical_by_channel(input.frames);
    let (Some(frames), Some(regime)) = (
        by_channel.get(&input.declared),
        input.regimes.get(&input.declared),
    ) else {
        return StimulusWindow::Whole;
    };
    match input.kind {
        RunKind::Tracking { .. } => stroke_blocks(frames, regime, input.applied_speeds.len()),
        RunKind::Probe { .. } => stroke_blocks(frames, regime, 1),
        RunKind::Standstill { .. } => rest_window(&by_channel, regime.clock_freq),
    }
}

/// Computes the run type's metrics for every channel with critical frames, the declared
/// channel first. Tracking metrics are per block of `window`, labelled with the applied speeds
/// in order. Remarks about data that could not be analysed go to `notes`.
pub fn analyze(
    input: &AnalysisInput<'_>,
    window: &StimulusWindow,
    notes: &mut Vec<String>,
) -> Metrics {
    let by_channel = critical_by_channel(input.frames);
    let blocks: &[(u64, u64)] = match window {
        StimulusWindow::Strokes { blocks } => blocks,
        StimulusWindow::Whole | StimulusWindow::Rests { .. } | StimulusWindow::Unmatched(_) => &[],
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
