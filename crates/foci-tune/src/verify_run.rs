//! The verification checks of the run sequence, over the frames of one recorded window.

use std::collections::{BTreeMap, HashMap};

use foci_tune_analysis::protocol::{
    ControlState, FLAG_DROP_GAP_BEFORE_THIS_FRAME, Frame, FrameHeader,
};
use foci_tune_analysis::units::POSITION_UNITS_PER_REV;

use crate::model::{Verification, VerificationCheck};

/// `applied_request` of a velocity feedforward write, the only write a stimulus makes.
pub const FEEDFORWARD_REQUEST: u8 = 7;
/// Probe runs need at least this many located edges.
pub const MIN_PROBE_EDGES: usize = 20;
const OTHER_CHANNEL_COUNTS: f64 = 16.0;

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

fn channel_moved(w: &Window<'_>) -> Verification {
    let mut ranges: BTreeMap<u8, (Span, Span)> = BTreeMap::new();
    for frame in w.frames {
        if let Frame::Critical(h, c) = frame {
            let (step, tmc) = ranges
                .entry(h.source_index)
                .or_insert((Span::new(c.step_position), Span::new(c.tmc_position_actual)));
            step.add(c.step_position);
            tmc.add(c.tmc_position_actual);
        }
    }
    let declared_q = quantum(w, w.declared);
    let declared_range = ranges
        .get(&w.declared)
        .map_or(0.0, |(step, _)| step.range());
    let failure = if declared_range > declared_q {
        ranges
            .iter()
            .filter(|(channel, _)| **channel != w.declared)
            .find(|(channel, (_, tmc))| tmc.range() >= OTHER_CHANNEL_COUNTS * quantum(w, **channel))
            .map(|(channel, _)| format!("channel {channel} moved"))
    } else {
        Some(format!("channel {} did not move", w.declared))
    };
    outcome(
        VerificationCheck::ChannelMoved,
        failure,
        format!(
            "channel {} step range {declared_range} > {declared_q}; others under 16 counts",
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
        let failure = (edges < MIN_PROBE_EDGES).then(|| format!("{edges} edges"));
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
