//! The `[foci <stepper>]` block an operator pastes into `printer.cfg` to keep a run's values.

use std::fmt::Write;

use foci_tune_analysis::signals::corner_hz;

use crate::model::{Filters, Gains, Run};

const COMMENT_COLUMN: usize = 25;

fn line(out: &mut String, key: &str, value: impl std::fmt::Display, comment: Option<String>) {
    let entry = format!("{key}: {value}");
    match comment {
        Some(c) => writeln!(out, "{entry:<COMMENT_COLUMN$}# {c}"),
        None => writeln!(out, "{entry}"),
    }
    .expect("writing to a String cannot fail");
}

fn khz(pwm_hz: u32) -> String {
    let s = format!("{:.3}", f64::from(pwm_hz) / 1000.0);
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn q8_8(raw: i32) -> String {
    format!("{:.3} Q8.8", f64::from(raw) / 256.0)
}

/// Renders the block: raw values, with the P gains in Q8.8 and velocity I as its integrator
/// corner in trailing comments. Without `pwm_hz` the corner is unknown and left out.
pub fn render(stepper: &str, gains: &Gains, filters: &Filters, pwm_hz: Option<u32>) -> String {
    let mut out = format!("[foci {stepper}]\n");
    line(
        &mut out,
        "pid_velocity_p",
        gains.velocity_p,
        Some(q8_8(gains.velocity_p)),
    );
    let corner = pwm_hz.map(|hz| {
        format!(
            "{:.1} Hz corner at {} kHz PWM",
            corner_hz(gains.velocity_i, hz),
            khz(hz)
        )
    });
    line(&mut out, "pid_velocity_i", gains.velocity_i, corner);
    line(
        &mut out,
        "pid_position_p",
        gains.position_p,
        Some(q8_8(gains.position_p)),
    );
    line(&mut out, "pid_position_i", gains.position_i, None);
    line(&mut out, "velocity_filter_hz", filters.velocity_hz, None);
    line(&mut out, "torque_filter_hz", filters.torque_hz, None);
    line(&mut out, "position_filter_hz", filters.position_hz, None);
    line(&mut out, "flux_filter_hz", filters.flux_hz, None);
    out
}

/// The block for the gains and filters a run wrote, with the PWM frequency its regime or its
/// declared channel's baseline frame recorded.
pub fn for_run(run: &Run) -> String {
    let pwm_hz = run.regime.as_ref().map(|r| r.pwm_hz).or_else(|| {
        run.baselines
            .iter()
            .find(|b| b.channel == run.declared_channel)
            .map(|b| b.control.pwm_hz)
    });
    render(
        &run.request.stepper,
        &run.request.gains,
        &run.request.filters,
        pwm_hz.filter(|&hz| hz > 0),
    )
}
