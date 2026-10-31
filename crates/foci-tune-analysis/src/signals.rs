//! Derived physical series and gain unit conversions.

use core::f64::consts::TAU;

use crate::protocol::{Critical, FLAG_DROP_GAP_BEFORE_THIS_FRAME, FrameHeader};
use crate::units::{POSITION_UNITS_PER_REV, Regime};

/// Per-frame series for one channel, all vectors the same length.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChannelSeries {
    /// Seconds since the first frame.
    pub t_s: Vec<f64>,
    /// Commanded step position in position units.
    pub step_position: Vec<i32>,
    /// TMC4671 actual position in position units.
    pub tmc_position: Vec<i32>,
    /// Position error in micrometres.
    pub position_error_um: Vec<f64>,
    /// Finite-difference velocity of the actual position in mm/s; element 0 is 0.
    pub velocity_mm_s: Vec<f64>,
    /// Interim PID-input target torque.
    pub torque: Vec<i32>,
    /// Cached velocity feedforward offset.
    pub ff_offset: Vec<i32>,
    /// True where frames were dropped immediately before this one.
    pub drop_gap: Vec<bool>,
}

/// Builds the derived series for one channel from its decoded critical frames.
pub fn channel_series(frames: &[(FrameHeader, Critical)], regime: &Regime) -> ChannelSeries {
    let mut s = ChannelSeries::default();
    let Some((first, _)) = frames.first() else {
        return s;
    };
    let t0 = first.timestamp_ticks;
    let clock = f64::from(regime.clock_freq);
    for (k, (h, c)) in frames.iter().enumerate() {
        let t = (h.timestamp_ticks.wrapping_sub(t0)) as f64 / clock;
        let velocity = match k {
            0 => 0.0,
            _ => {
                let dt = t - s.t_s[k - 1];
                let du = c.tmc_position_actual.wrapping_sub(s.tmc_position[k - 1]);
                regime.position_um(du) / 1000.0 / dt
            }
        };
        s.t_s.push(t);
        s.step_position.push(c.step_position);
        s.tmc_position.push(c.tmc_position_actual);
        s.position_error_um
            .push(regime.position_um(c.position_error));
        s.velocity_mm_s.push(velocity);
        s.torque.push(c.interim_pidin_target_torque);
        s.ff_offset.push(c.cached_pid_velocity_offset);
        s.drop_gap
            .push(h.flags & FLAG_DROP_GAP_BEFORE_THIS_FRAME != 0);
    }
    s
}

/// Velocity-loop integral corner frequency in Hz for a raw I gain.
pub fn corner_hz(velocity_i_raw: i32, pwm_hz: u32) -> f64 {
    f64::from(velocity_i_raw) * f64::from(pwm_hz) / (TAU * POSITION_UNITS_PER_REV)
}

/// Raw velocity I gain for a corner frequency, rounded to the nearest integer.
pub fn velocity_i_raw(corner_hz: f64, pwm_hz: u32) -> i32 {
    (corner_hz * TAU * POSITION_UNITS_PER_REV / f64::from(pwm_hz)).round() as i32
}

/// Formats a raw P gain (units of 1/256) as a decimal config value.
pub fn p_gain_param(raw: i32) -> String {
    format!("{:.8}", f64::from(raw) / 256.0)
}

/// Formats a raw I gain (units of 1/4096) as a decimal config value.
pub fn i_gain_param(raw: i32) -> String {
    format!("{:.12}", f64::from(raw) / 4096.0)
}
