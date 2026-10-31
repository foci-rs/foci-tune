//! Decoder for protocol 25 trace frames (little-endian, 32-byte header).

use thiserror::Error;

/// Trace protocol version this decoder accepts.
pub const PROTOCOL_VERSION: u8 = 25;
/// Length of the fixed frame header in bytes.
pub const FRAME_HEADER_LEN: usize = 32;
/// Header flag: frames were dropped immediately before this one.
pub const FLAG_DROP_GAP_BEFORE_THIS_FRAME: u8 = 1;

const FRAME_MAGIC: u32 = 0x4352_5446;
const CRITICAL_LEN: usize = 88;
const CONTROL_STATE_LEN: usize = 88;
const METADATA_FIXED_LEN: usize = 36;
const METADATA_SCHEMA_VERSION: u8 = 1;
const MAX_SOURCES: u8 = 16;
const MAX_STRING_LEN: usize = 31;

/// Frame types this decoder understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    /// High-rate critical sample.
    Critical = 1,
    /// Stream metadata.
    Metadata = 5,
    /// Applied control-state snapshot.
    ControlState = 10,
}

/// Fixed frame header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameHeader {
    /// Protocol version byte.
    pub protocol_version: u8,
    /// Raw frame type byte.
    pub frame_type: u8,
    /// Frame flags; see [`FLAG_DROP_GAP_BEFORE_THIS_FRAME`].
    pub flags: u8,
    /// Payload length in bytes.
    pub payload_len: u16,
    /// Source kind byte.
    pub source_kind: u8,
    /// Source index within its kind.
    pub source_index: u8,
    /// Per-source frame sequence number.
    pub sequence: u32,
    /// MCU timestamp in clock ticks.
    pub timestamp_ticks: u64,
    /// Boot identifier of the emitting MCU session.
    pub boot_id: u32,
}

/// Critical sample payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Critical {
    /// Commanded step position.
    pub step_position: i32,
    /// TMC4671 actual position.
    pub tmc_position_actual: i32,
    /// Position error.
    pub position_error: i32,
    /// Target velocity.
    pub target_velocity: i32,
    /// TMC4671 actual velocity.
    pub pid_velocity_actual: i32,
    /// Cached velocity feedforward offset.
    pub cached_pid_velocity_offset: i32,
    /// Velocity error.
    pub velocity_error: i32,
    /// Sample flag bits.
    pub sample_flags: u8,
    /// Interim PID-input target torque (signed).
    pub interim_pidin_target_torque: i32,
}

/// Applied control-state payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlState {
    /// Change sequence counter.
    pub change_sequence: u32,
    /// Tick at which the change was applied.
    pub applied_tick: u64,
    /// Applied request code.
    pub applied_request: u8,
    /// Encoder counts per revolution.
    pub encoder_counts_per_rev: u32,
    /// PWM frequency in Hz.
    pub pwm_hz: u32,
    /// Velocity meter selector.
    pub velocity_meter: u8,
    /// Velocity filter corner in Hz.
    pub velocity_filter_hz: u16,
    /// Torque filter corner in Hz.
    pub torque_filter_hz: u16,
    /// Position filter corner in Hz.
    pub position_filter_hz: u16,
    /// Flux filter corner in Hz.
    pub flux_filter_hz: u16,
    /// Velocity loop P gain.
    pub velocity_p: i32,
    /// Velocity loop I gain.
    pub velocity_i: i32,
    /// Position loop P gain.
    pub position_p: i32,
    /// Position loop I gain.
    pub position_i: i32,
    /// Torque loop P gain.
    pub torque_p: i32,
    /// Torque loop I gain.
    pub torque_i: i32,
    /// Flux loop P gain.
    pub flux_p: i32,
    /// Flux loop I gain.
    pub flux_i: i32,
    /// Velocity feedforward enabled.
    pub velocity_ff_enable: bool,
    /// Velocity feedforward in permille.
    pub velocity_ff_permille: u16,
    /// Acceleration feedforward enabled.
    pub accel_ff_enable: bool,
    /// Acceleration feedforward gain.
    pub accel_ff_gain: u16,
    /// Deceleration feedforward gain.
    pub decel_ff_gain: u16,
    /// Run current setting.
    pub run_current: u32,
    /// Voltage limit setting.
    pub voltage_limit: u16,
}

/// Stream metadata payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    /// MCU clock frequency in Hz.
    pub clock_freq: u32,
    /// Critical sample rate in Hz (0 when dormant).
    pub critical_hz: u32,
    /// Control-detail rate in Hz.
    pub control_detail_hz: u32,
    /// Status rate in Hz.
    pub status_hz: u32,
    /// Firmware identifier string.
    pub firmware_id: String,
    /// Product string.
    pub product: String,
    /// Serial string.
    pub serial: String,
    /// Declared sources as (kind, index, label).
    pub sources: Vec<(u8, u8, String)>,
}

/// A decoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// Critical sample.
    Critical(FrameHeader, Critical),
    /// Control-state snapshot.
    ControlState(FrameHeader, ControlState),
    /// Stream metadata.
    Metadata(FrameHeader, Metadata),
    /// Any other frame type; payload is not decoded.
    Other(FrameHeader),
}

/// Reasons a frame fails to decode.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecodeError {
    /// Fewer bytes than the header or declared payload requires.
    #[error("frame truncated")]
    Truncated,
    /// Magic number mismatch.
    #[error("bad frame magic")]
    BadMagic,
    /// Protocol version other than [`PROTOCOL_VERSION`].
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u8),
    /// Structurally invalid payload.
    #[error("invalid payload: {0}")]
    Payload(&'static str),
}

fn le<const N: usize>(b: &[u8], at: usize) -> Result<[u8; N], DecodeError> {
    let end = at.checked_add(N).ok_or(DecodeError::Truncated)?;
    b.get(at..end)
        .and_then(|s| s.try_into().ok())
        .ok_or(DecodeError::Truncated)
}

fn u8_at(b: &[u8], at: usize) -> Result<u8, DecodeError> {
    Ok(le::<1>(b, at)?[0])
}

fn u16_at(b: &[u8], at: usize) -> Result<u16, DecodeError> {
    Ok(u16::from_le_bytes(le(b, at)?))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, DecodeError> {
    Ok(u32::from_le_bytes(le(b, at)?))
}

fn i32_at(b: &[u8], at: usize) -> Result<i32, DecodeError> {
    Ok(i32::from_le_bytes(le(b, at)?))
}

fn u64_at(b: &[u8], at: usize) -> Result<u64, DecodeError> {
    Ok(u64::from_le_bytes(le(b, at)?))
}

fn parse_header(bytes: &[u8]) -> Result<FrameHeader, DecodeError> {
    if bytes.len() < FRAME_HEADER_LEN {
        return Err(DecodeError::Truncated);
    }
    if u32_at(bytes, 0)? != FRAME_MAGIC {
        return Err(DecodeError::BadMagic);
    }
    let protocol_version = u8_at(bytes, 4)?;
    if protocol_version != PROTOCOL_VERSION {
        return Err(DecodeError::UnsupportedVersion(protocol_version));
    }
    if usize::from(u8_at(bytes, 5)?) != FRAME_HEADER_LEN {
        return Err(DecodeError::Payload("unsupported header length"));
    }
    Ok(FrameHeader {
        protocol_version,
        frame_type: u8_at(bytes, 6)?,
        flags: u8_at(bytes, 7)?,
        payload_len: u16_at(bytes, 8)?,
        source_kind: u8_at(bytes, 10)?,
        source_index: u8_at(bytes, 11)?,
        sequence: u32_at(bytes, 12)?,
        timestamp_ticks: u64_at(bytes, 16)?,
        boot_id: u32_at(bytes, 24)?,
    })
}

fn decode_critical(p: &[u8]) -> Result<Critical, DecodeError> {
    if p.len() < CRITICAL_LEN {
        return Err(DecodeError::Truncated);
    }
    Ok(Critical {
        step_position: i32_at(p, 0)?,
        tmc_position_actual: i32_at(p, 4)?,
        position_error: i32_at(p, 8)?,
        target_velocity: i32_at(p, 12)?,
        pid_velocity_actual: i32_at(p, 16)?,
        cached_pid_velocity_offset: i32_at(p, 20)?,
        velocity_error: i32_at(p, 24)?,
        sample_flags: u8_at(p, 29)?,
        interim_pidin_target_torque: i32_at(p, 60)?,
    })
}

fn decode_control_state(p: &[u8]) -> Result<ControlState, DecodeError> {
    if p.len() < CONTROL_STATE_LEN {
        return Err(DecodeError::Truncated);
    }
    Ok(ControlState {
        change_sequence: u32_at(p, 0)?,
        applied_tick: u64_at(p, 4)?,
        applied_request: u8_at(p, 12)?,
        encoder_counts_per_rev: u32_at(p, 16)?,
        pwm_hz: u32_at(p, 20)?,
        velocity_meter: u8_at(p, 24)?,
        velocity_filter_hz: u16_at(p, 28)?,
        torque_filter_hz: u16_at(p, 30)?,
        position_filter_hz: u16_at(p, 32)?,
        flux_filter_hz: u16_at(p, 34)?,
        velocity_p: i32_at(p, 36)?,
        velocity_i: i32_at(p, 40)?,
        position_p: i32_at(p, 44)?,
        position_i: i32_at(p, 48)?,
        torque_p: i32_at(p, 52)?,
        torque_i: i32_at(p, 56)?,
        flux_p: i32_at(p, 60)?,
        flux_i: i32_at(p, 64)?,
        velocity_ff_enable: u8_at(p, 68)? != 0,
        velocity_ff_permille: u16_at(p, 70)?,
        accel_ff_enable: u8_at(p, 72)? != 0,
        accel_ff_gain: u16_at(p, 74)?,
        decel_ff_gain: u16_at(p, 76)?,
        run_current: u32_at(p, 78)?,
        voltage_limit: u16_at(p, 82)?,
    })
}

fn read_str(p: &[u8], at: usize, what: &'static str) -> Result<(String, usize), DecodeError> {
    let len = usize::from(u8_at(p, at)?);
    if len > MAX_STRING_LEN {
        return Err(DecodeError::Payload(what));
    }
    let start = at + 1;
    let raw = p.get(start..start + len).ok_or(DecodeError::Truncated)?;
    let s = std::str::from_utf8(raw).map_err(|_| DecodeError::Payload("string is not utf-8"))?;
    Ok((s.to_owned(), start + len))
}

fn decode_metadata(p: &[u8]) -> Result<Metadata, DecodeError> {
    if p.len() < METADATA_FIXED_LEN {
        return Err(DecodeError::Truncated);
    }
    if u8_at(p, 0)? != METADATA_SCHEMA_VERSION {
        return Err(DecodeError::Payload("unsupported metadata schema version"));
    }
    let source_count = u8_at(p, 2)?;
    if source_count > MAX_SOURCES {
        return Err(DecodeError::Payload("metadata source count exceeds 16"));
    }
    let (firmware_id, at) = read_str(p, METADATA_FIXED_LEN, "metadata string exceeds 31 bytes")?;
    let (product, at) = read_str(p, at, "metadata string exceeds 31 bytes")?;
    let (serial, mut at) = read_str(p, at, "metadata string exceeds 31 bytes")?;
    let mut sources = Vec::with_capacity(usize::from(source_count));
    for _ in 0..source_count {
        let kind = u8_at(p, at)?;
        let index = u8_at(p, at + 1)?;
        let (label, next) = read_str(p, at + 2, "source label exceeds 31 bytes")?;
        sources.push((kind, index, label));
        at = next;
    }
    Ok(Metadata {
        clock_freq: u32_at(p, 4)?,
        critical_hz: u32_at(p, 8)?,
        control_detail_hz: u32_at(p, 12)?,
        status_hz: u32_at(p, 16)?,
        firmware_id,
        product,
        serial,
        sources,
    })
}

/// Decode one frame from the start of `bytes`.
///
/// Trailing bytes beyond the declared payload are ignored.
///
/// # Errors
///
/// Returns [`DecodeError`] when the header is malformed, the protocol version
/// is not 25, or the payload is short or structurally invalid.
pub fn decode_frame(bytes: &[u8]) -> Result<Frame, DecodeError> {
    let header = parse_header(bytes)?;
    let end = FRAME_HEADER_LEN + usize::from(header.payload_len);
    let payload = bytes
        .get(FRAME_HEADER_LEN..end)
        .ok_or(DecodeError::Truncated)?;
    Ok(match header.frame_type {
        t if t == FrameType::Critical as u8 => Frame::Critical(header, decode_critical(payload)?),
        t if t == FrameType::ControlState as u8 => {
            Frame::ControlState(header, decode_control_state(payload)?)
        }
        t if t == FrameType::Metadata as u8 => Frame::Metadata(header, decode_metadata(payload)?),
        _ => Frame::Other(header),
    })
}
