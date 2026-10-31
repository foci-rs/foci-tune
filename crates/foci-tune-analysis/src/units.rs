//! Position units and the motion regime that converts them to millimetres.

/// TMC4671 position units per motor revolution, independent of encoder resolution.
pub const POSITION_UNITS_PER_REV: f64 = 65536.0;

/// Motion regime of one channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Regime {
    /// Linear travel per motor revolution, with any gear ratio already folded in.
    pub motor_rotation_distance_mm: f64,
    /// Encoder counts per motor revolution.
    pub encoder_counts_per_rev: u32,
    /// PWM frequency in Hz.
    pub pwm_hz: u32,
    /// MCU clock frequency in Hz.
    pub clock_freq: u32,
}

impl Regime {
    /// Converts position units to micrometres of travel.
    pub fn position_um(&self, units: i32) -> f64 {
        f64::from(units) * self.motor_rotation_distance_mm / POSITION_UNITS_PER_REV * 1000.0
    }

    /// Size of one encoder count in position units.
    pub fn quantum_units(&self) -> f64 {
        POSITION_UNITS_PER_REV / f64::from(self.encoder_counts_per_rev)
    }

    /// Size of one encoder count in micrometres of travel.
    pub fn quantum_um(&self) -> f64 {
        self.quantum_units() * self.motor_rotation_distance_mm / POSITION_UNITS_PER_REV * 1000.0
    }
}
