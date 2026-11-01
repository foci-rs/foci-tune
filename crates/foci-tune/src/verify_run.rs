//! The verification checks of the run sequence, over the frames of one recorded window.

use std::collections::{BTreeMap, HashMap};

use foci_tune_analysis::protocol::{
    ControlState, FLAG_DROP_GAP_BEFORE_THIS_FRAME, Frame, FrameHeader,
};
use foci_tune_analysis::units::POSITION_UNITS_PER_REV;

use crate::model::{Verification, VerificationCheck};
use crate::run_analysis::StimulusWindow;
use crate::stimulus::MIN_TOGGLE_SPACING_S;

/// `applied_request` of a velocity feedforward write, the only write a stimulus makes.
pub const FEEDFORWARD_REQUEST: u8 = 7;
/// Probe runs need at least this many located edges.
pub const MIN_PROBE_EDGES: usize = 20;
/// Floor of another channel's allowed travel, in its encoder counts.
const OTHER_CHANNEL_COUNTS: f64 = 16.0;
/// Another channel may travel this fraction of the declared channel's measured range: on a
/// corexy machine the idle motor deflects through the belt coupling.
const OTHER_CHANNEL_FRACTION: f64 = 0.02;

/// One recorded window: the frames from the metadata frame on, in arrival order.
#[derive(Debug, Clone, Copy)]
pub struct Window<'a> {
    /// Source index of the declared channel.
    pub declared: u8,
    /// Decoded frames of the window.
    pub frames: &'a [Frame],
    /// Baseline control state per channel.
    pub baselines: &'a HashMap<u8, ControlState>,
    /// Located edges for a probe run, `None` for other run types.
    pub probe_edges: Option<usize>,
    /// Where the stimulus runs; channel travel is measured over it.
    pub stimulus: &'a StimulusWindow,
}

fn header(frame: &Frame) -> &FrameHeader {
    match frame {
        Frame::Critical(h, _) | Frame::ControlState(h, _) | Frame::Metadata(h, _) => h,
        Frame::Other(h) => h,
    }
}

fn outcome(check: VerificationCheck, failure: Option<String>, pass: String) -> Verification {
    Verification {
        check,
        passed: failure.is_none(),
        detail: failure.unwrap_or(pass),
    }
}

/// One encoder count in position units; infinite for a channel reporting zero counts, so
/// such a channel never counts as moved.
fn quantum(w: &Window<'_>, channel: u8) -> f64 {
    let counts = w
        .baselines
        .get(&channel)
        .or_else(|| w.baselines.get(&w.declared))
        .map_or(0, |b| b.encoder_counts_per_rev);
    POSITION_UNITS_PER_REV / f64::from(counts)
}

#[derive(Clone, Copy)]
struct Span(i32, i32);

impl Span {
    fn new(v: i32) -> Self {
        Self(v, v)
    }

    fn add(&mut self, v: i32) {
        *self = Self(self.0.min(v), self.1.max(v));
    }

    fn range(self) -> f64 {
        (i64::from(self.1) - i64::from(self.0)) as f64
    }
}

/// Step and actual-position spans per channel over the frames within `ticks`, all when `None`.
fn travel(frames: &[Frame], ticks: Option<(u64, u64)>) -> BTreeMap<u8, (Span, Span)> {
    let mut ranges: BTreeMap<u8, (Span, Span)> = BTreeMap::new();
    for frame in frames {
        let Frame::Critical(h, c) = frame else {
            continue;
        };
        if ticks.is_some_and(|(from, to)| !(from..=to).contains(&h.timestamp_ticks)) {
            continue;
        }
        let (step, tmc) = ranges
            .entry(h.source_index)
            .or_insert((Span::new(c.step_position), Span::new(c.tmc_position_actual)));
        step.add(c.step_position);
        tmc.add(c.tmc_position_actual);
    }
    ranges
}

/// Travel is measured over the stimulus window. A standstill's rests move the declared channel
/// by less than an encoder count by design, so its "moved" rule keeps the whole window.
fn channel_moved(w: &Window<'_>) -> Verification {
    if let StimulusWindow::Unmatched(reason) = w.stimulus {
        return outcome(
            VerificationCheck::ChannelMoved,
            Some(reason.clone()),
            String::new(),
        );
    }
    let ranges = travel(w.frames, w.stimulus.ticks());
    let moved = match w.stimulus {
        StimulusWindow::Rests { .. } => travel(w.frames, None),
        StimulusWindow::Whole | StimulusWindow::Strokes { .. } | StimulusWindow::Unmatched(_) => {
            ranges.clone()
        }
    };
    let declared_q = quantum(w, w.declared);
    let declared_range = moved.get(&w.declared).map_or(0.0, |(step, _)| step.range());
    let declared_travel = ranges.get(&w.declared).map_or(0.0, |(_, tmc)| tmc.range());
    if declared_range <= declared_q {
        return outcome(
            VerificationCheck::ChannelMoved,
            Some(format!("channel {} did not move", w.declared)),
            String::new(),
        );
    }
    let declared_counts = declared_travel / declared_q;
    let mut failure = None;
    let mut others = Vec::new();
    for (&channel, (_, tmc)) in ranges.iter().filter(|(c, _)| **c != w.declared) {
        let q = quantum(w, channel);
        let limit = (OTHER_CHANNEL_COUNTS * q).max(OTHER_CHANNEL_FRACTION * declared_travel);
        let text = format!(
            "channel {channel} moved {:.1} counts against a limit of {:.1} (declared channel \
             {declared_counts:.1})",
            tmc.range() / q,
            limit / q
        );
        if tmc.range() >= limit {
            failure = Some(text);
            break;
        }
        others.push(format!("{text}, margin {:.1}", (limit - tmc.range()) / q));
    }
    let others = if others.is_empty() {
        "no other channel".to_owned()
    } else {
        others.join("; ")
    };
    outcome(
        VerificationCheck::ChannelMoved,
        failure,
        format!(
            "channel {} step range {declared_range} > {declared_q}; {others}",
            w.declared
        ),
    )
}

fn baseline_contiguous(w: &Window<'_>) -> Verification {
    let mut writes = 0;
    let mut previous: Option<u32> = None;
    let mut failure = None;
    for frame in w.frames {
        let Frame::ControlState(h, c) = frame else {
            continue;
        };
        if h.source_index != w.declared {
            continue;
        }
        let Some(prev) = previous.replace(c.change_sequence) else {
            continue;
        };
        match c.change_sequence.wrapping_sub(prev) {
            0 => {}
            1 if c.applied_request == FEEDFORWARD_REQUEST => writes += 1,
            1 => {
                failure = Some(format!(
                    "channel {} changed by request {} from outside the tool",
                    w.declared, c.applied_request
                ));
                break;
            }
            delta => {
                failure = Some(format!(
                    "change_sequence on channel {} advanced by {delta}",
                    w.declared
                ));
                break;
            }
        }
    }
    outcome(
        VerificationCheck::BaselineContiguous,
        failure,
        format!("{writes} feedforward writes after the baseline"),
    )
}

fn no_drop_gaps(w: &Window<'_>) -> Verification {
    let failure = w
        .frames
        .iter()
        .map(header)
        .find(|h| h.flags & FLAG_DROP_GAP_BEFORE_THIS_FRAME != 0)
        .map(|h| {
            format!(
                "drop gap before frame {} of source {}",
                h.sequence, h.source_index
            )
        });
    outcome(
        VerificationCheck::NoDropGaps,
        failure,
        format!("{} frames without a drop gap", w.frames.len()),
    )
}

/// Runs every check that applies to the window.
pub fn verify(w: &Window<'_>) -> Vec<Verification> {
    let mut checks = vec![channel_moved(w), baseline_contiguous(w), no_drop_gaps(w)];
    if let Some(edges) = w.probe_edges {
        let failure = (edges < MIN_PROBE_EDGES).then(|| {
            format!(
                "{edges} edges (need at least {MIN_PROBE_EDGES}); toggles closer than ~{:.0} ms \
                 or a feedforward offset that never steps produce none: check \
                 segments_between_toggles against the speed",
                MIN_TOGGLE_SPACING_S * 1000.0
            )
        });
        checks.push(outcome(
            VerificationCheck::EdgeCount,
            failure,
            format!("{edges} edges"),
        ));
    }
    checks
}

/// The reason of the first failed check, the run's `not comparable` reason.
pub fn first_failure(checks: &[Verification]) -> Option<&str> {
    checks.iter().find(|c| !c.passed).map(|c| c.detail.as_str())
}
