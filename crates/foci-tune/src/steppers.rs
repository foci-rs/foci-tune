//! Stepper table derived from Moonraker's `configfile.settings`.

use serde::Serialize;
use serde_json::Value;

const STEP_PINS: [(&str, u8); 2] = [("STEP0", 0), ("STEP1", 1)];

/// Outer gains and filter cutoffs as written in the `foci <stepper>` section.
///
/// `None` means the option is absent; `klipper-foci` has no default for any of them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ConfigGains {
    /// `pid_position_p`.
    pub position_p: Option<u32>,
    /// `pid_position_i`.
    pub position_i: Option<u32>,
    /// `pid_velocity_p`.
    pub velocity_p: Option<u32>,
    /// `pid_velocity_i`.
    pub velocity_i: Option<u32>,
    /// `velocity_filter_hz`.
    pub velocity_filter_hz: Option<u32>,
    /// `torque_filter_hz`.
    pub torque_filter_hz: Option<u32>,
    /// `position_filter_hz`.
    pub position_filter_hz: Option<u32>,
    /// `flux_filter_hz`.
    pub flux_filter_hz: Option<u32>,
}

/// One FOCI-driven stepper channel.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stepper {
    /// Klipper section name, for example `stepper_x`.
    pub name: String,
    /// MCU name from the step pin.
    pub mcu: String,
    /// Channel index on that MCU.
    pub channel: u8,
    /// USB serial of the MCU's trace interface.
    pub serial: String,
    /// `printer.kinematics`.
    pub kinematics: String,
    /// Rotation distance divided by the gear ratio product, mm per motor revolution.
    pub motor_rotation_distance_mm: f64,
    /// Axis letter from the section name.
    pub axis: char,
    /// `position_min`, 0 when unset as in Klipper.
    pub position_min: f64,
    /// `position_max`.
    pub position_max: f64,
    /// Encoder pulses per revolution.
    pub encoder_ppr: u32,
    /// Whether `identified_r_count_milli` is present.
    pub electrical_model_present: bool,
    /// Gains and filters as configured.
    pub config_gains: ConfigGains,
    /// Why the channel cannot be used, when it cannot.
    pub unusable: Option<String>,
}

/// Axis travel limits for the bed, from the X and Y steppers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BedLimits {
    /// X `(min, max)` in mm.
    pub x: (f64, f64),
    /// Y `(min, max)` in mm.
    pub y: (f64, f64),
}

impl BedLimits {
    /// Widest limits across the X and Y steppers; `None` when either axis is missing.
    pub fn from_steppers(steppers: &[Stepper]) -> Option<Self> {
        let span = |axis: char| {
            steppers
                .iter()
                .filter(|s| s.axis == axis)
                .map(|s| (s.position_min, s.position_max))
                .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
        };
        Some(Self {
            x: span('x')?,
            y: span('y')?,
        })
    }
}

/// Extract the USB serial from a `/dev/serial/by-id/...` path.
///
/// Takes the last `_`-separated token of the file name with the `-if00` suffix removed.
pub fn usb_serial_from_path(serial_path: &str) -> Option<String> {
    let file = serial_path.rsplit('/').next()?;
    let (stem, _) = file.rsplit_once("-if")?;
    let token = stem.rsplit('_').next()?;
    (!token.is_empty() && token != stem).then(|| token.to_owned())
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn unsigned(value: &Value) -> Option<u32> {
    number(value).and_then(|n| (n >= 0.0 && n.fract() == 0.0).then_some(n as u32))
}

fn gear_pair_ratio(motor: f64, output: f64) -> Option<f64> {
    let ratio = motor / output;
    (ratio.is_finite() && ratio > 0.0).then_some(ratio)
}

fn gear_ratio_product(value: &Value) -> Option<f64> {
    match value {
        Value::Null => Some(1.0),
        Value::String(text) => text
            .split([',', '\n'])
            .map(str::trim)
            .filter(|pair| !pair.is_empty())
            .try_fold(1.0, |product, pair| {
                let (motor, output) = pair.split_once(':')?;
                let ratio =
                    gear_pair_ratio(motor.trim().parse().ok()?, output.trim().parse().ok()?)?;
                Some(product * ratio)
            }),
        Value::Array(pairs) => pairs.iter().try_fold(1.0, |product, pair| {
            let [motor, output] = pair.as_array()?.as_slice() else {
                return None;
            };
            Some(product * gear_pair_ratio(motor.as_f64()?, output.as_f64()?)?)
        }),
        Value::Bool(_) | Value::Number(_) | Value::Object(_) => None,
    }
}

fn axis_of(section: &str) -> Option<char> {
    let rest = section.strip_prefix("stepper_")?;
    let mut chars = rest.chars();
    let axis = chars.next()?;
    (axis.is_ascii_alphabetic() && chars.all(|c| c.is_ascii_digit())).then_some(axis)
}

fn channel_of(step_pin: &str) -> Option<(&str, u8)> {
    let pin = step_pin.trim().trim_start_matches(['!', '^', '~']);
    let (mcu, name) = pin.split_once(':')?;
    STEP_PINS
        .iter()
        .find(|(pin_name, _)| *pin_name == name)
        .map(|(_, channel)| (mcu, *channel))
}

fn gains_of(foci: &Value) -> ConfigGains {
    let get = |key: &str| unsigned(&foci[key]);
    ConfigGains {
        position_p: get("pid_position_p"),
        position_i: get("pid_position_i"),
        velocity_p: get("pid_velocity_p"),
        velocity_i: get("pid_velocity_i"),
        velocity_filter_hz: get("velocity_filter_hz"),
        torque_filter_hz: get("torque_filter_hz"),
        position_filter_hz: get("position_filter_hz"),
        flux_filter_hz: get("flux_filter_hz"),
    }
}

fn derive_one(
    settings: &Value,
    kinematics: &str,
    name: &str,
    axis: char,
    open_serials: &[String],
) -> Option<Stepper> {
    let foci = settings.get(format!("foci {name}"))?;
    let section = &settings[name];
    let (mcu, channel) = channel_of(section["step_pin"].as_str()?)?;
    let mcu_section = if mcu == "mcu" {
        "mcu".to_owned()
    } else {
        format!("mcu {mcu}")
    };
    let serial = settings[&mcu_section]["serial"]
        .as_str()
        .and_then(usb_serial_from_path)
        .unwrap_or_default();

    let rotation = number(&section["rotation_distance"]);
    let ratio = gear_ratio_product(&section["gear_ratio"]);
    let position_max = number(&section["position_max"]);
    let encoder_ppr = unsigned(&foci["encoder_ppr"]);

    let unusable = if serial.is_empty() {
        Some(format!("no usable USB serial for mcu {mcu}"))
    } else if rotation.is_none_or(|r| r <= 0.0) {
        Some("rotation_distance is missing or invalid".to_owned())
    } else if ratio.is_none() {
        Some("gear_ratio is not understood".to_owned())
    } else if position_max.is_none() {
        Some("position_max is missing".to_owned())
    } else if encoder_ppr.is_none() {
        Some("encoder_ppr is missing in the foci section".to_owned())
    } else if !open_serials.contains(&serial) {
        Some(format!("trace interface {serial} not open"))
    } else {
        None
    };

    Some(Stepper {
        name: name.to_owned(),
        mcu: mcu.to_owned(),
        channel,
        serial,
        kinematics: kinematics.to_owned(),
        motor_rotation_distance_mm: rotation.unwrap_or(0.0) / ratio.unwrap_or(1.0),
        axis,
        position_min: number(&section["position_min"]).unwrap_or(0.0),
        position_max: position_max.unwrap_or(0.0),
        encoder_ppr: encoder_ppr.unwrap_or(0),
        electrical_model_present: foci.get("identified_r_count_milli").is_some(),
        config_gains: gains_of(foci),
        unusable,
    })
}

/// Build the stepper table from `configfile.settings`.
///
/// A stepper is listed when its step pin is a FOCI `STEPn` pin and a `foci <stepper>` section
/// exists. It is usable only when the MCU's trace interface serial is in `open_serials` and the
/// section carries what the tool needs. Entries are ordered by `(serial, channel)`; a second
/// stepper claiming an already-claimed `(serial, channel)` is marked unusable.
pub fn derive_steppers(settings: &Value, open_serials: &[String]) -> Vec<Stepper> {
    let kinematics = settings["printer"]["kinematics"]
        .as_str()
        .unwrap_or_default();
    let mut steppers: Vec<Stepper> = settings
        .as_object()
        .into_iter()
        .flat_map(|sections| sections.keys())
        .filter_map(|name| {
            let axis = axis_of(name)?;
            derive_one(settings, kinematics, name, axis, open_serials)
        })
        .collect();
    steppers.sort_by(|a, b| (&a.serial, a.channel, &a.name).cmp(&(&b.serial, b.channel, &b.name)));

    let mut claimed: Vec<(String, u8, String)> = Vec::new();
    for stepper in &mut steppers {
        if let Some((_, _, owner)) = claimed
            .iter()
            .find(|(serial, channel, _)| *serial == stepper.serial && *channel == stepper.channel)
        {
            stepper.unusable = Some(format!(
                "channel {} of {} is already claimed by {owner}",
                stepper.channel, stepper.serial
            ));
        } else {
            claimed.push((
                stepper.serial.clone(),
                stepper.channel,
                stepper.name.clone(),
            ));
        }
    }
    steppers
}
