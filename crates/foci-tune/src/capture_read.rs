//! Reader for `foci-trace` capture files, plain or zstd-compressed.

use std::io::Read;
use std::path::Path;

use foci_trace::capture_file::{
    FILE_HEADER_LEN, FILE_MAGIC, FILE_VERSION, RECORD_HEADER_LEN, RECORD_MAGIC,
    RECORD_TYPE_DEVICE_TABLE, RECORD_TYPE_FIRMWARE_FRAME, RECORD_VERSION,
};
use structured_zstd::decoding::StreamingDecoder;
use thiserror::Error;

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// One known record of a capture file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureRecord {
    /// Record type: device table or firmware frame.
    pub record_type: u8,
    /// Index of the USB device the record came from.
    pub device_index: u16,
    /// Host receive time in monotonic nanoseconds.
    pub host_recv_ns: u64,
    /// Record payload; for a firmware frame, one complete trace frame.
    pub payload: Vec<u8>,
}

/// A parsed capture file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    /// Capture start in UNIX nanoseconds.
    pub start_unix_ns: u64,
    /// Capture start in monotonic nanoseconds.
    pub start_mono_ns: u64,
    /// Known records in file order; unknown types and versions are skipped.
    pub records: Vec<CaptureRecord>,
    /// Bytes of a partial final record, 0 when the file ends on a record boundary.
    pub truncated_tail_bytes: usize,
}

impl CaptureRecord {
    /// The first metadata frame embedded in a device-table record; `None` for any other
    /// record or a malformed table.
    pub fn device_table_metadata(&self) -> Option<&[u8]> {
        if self.record_type != RECORD_TYPE_DEVICE_TABLE {
            return None;
        }
        let p = self.payload.as_slice();
        let mut at = 0;
        for _ in 0..3 {
            let len = usize::from(u16::from_le_bytes(p.get(at..at + 2)?.try_into().ok()?));
            at += 2 + len;
        }
        let len = u32::from_le_bytes(p.get(at..at + 4)?.try_into().ok()?) as usize;
        p.get(at + 4..at + 4 + len)
    }
}

impl Capture {
    /// The firmware-frame records in file order.
    pub fn firmware_frames(&self) -> impl Iterator<Item = &CaptureRecord> {
        self.records
            .iter()
            .filter(|r| r.record_type == RECORD_TYPE_FIRMWARE_FRAME)
    }
}

/// Reasons a capture cannot be read.
#[derive(Debug, Error)]
pub enum CaptureError {
    /// The file could not be read.
    #[error("cannot read capture: {0}")]
    Io(#[from] std::io::Error),
    /// The zstd stream could not be decoded.
    #[error("cannot decompress capture: {0}")]
    Zstd(String),
    /// The file header is short or its magic, version or length is wrong.
    #[error("bad capture file header")]
    BadHeader,
    /// A record header does not start with the record magic.
    #[error("bad record magic at byte {0}")]
    BadRecordMagic(usize),
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("four bytes"))
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("eight bytes"))
}

/// Parses an uncompressed capture held in memory.
///
/// A partial final record ends parsing and is counted in
/// [`Capture::truncated_tail_bytes`], as a capture stopped mid-write leaves one.
///
/// # Errors
///
/// [`CaptureError::BadHeader`] for a wrong file header, [`CaptureError::BadRecordMagic`]
/// when a complete record header lacks the record magic.
pub fn parse_capture(bytes: &[u8]) -> Result<Capture, CaptureError> {
    let file_header_len = usize::from(FILE_HEADER_LEN);
    let record_header_len = usize::from(RECORD_HEADER_LEN);
    if bytes.len() < file_header_len
        || u32_at(bytes, 0) != FILE_MAGIC
        || u16_at(bytes, 4) != FILE_VERSION
        || u16_at(bytes, 6) != FILE_HEADER_LEN
    {
        return Err(CaptureError::BadHeader);
    }
    let mut capture = Capture {
        start_unix_ns: u64_at(bytes, 8),
        start_mono_ns: u64_at(bytes, 16),
        records: Vec::new(),
        truncated_tail_bytes: 0,
    };
    let mut at = file_header_len;
    while at < bytes.len() {
        let rest = &bytes[at..];
        if rest.len() < record_header_len {
            capture.truncated_tail_bytes = rest.len();
            break;
        }
        if u32_at(rest, 0) != RECORD_MAGIC {
            return Err(CaptureError::BadRecordMagic(at));
        }
        let payload_len = u32_at(rest, 20) as usize;
        let Some(payload) = rest.get(record_header_len..record_header_len + payload_len) else {
            capture.truncated_tail_bytes = rest.len();
            break;
        };
        at += record_header_len + payload_len;
        let (version, record_type, flags) = (rest[4], rest[5], u16_at(rest, 10));
        if version != RECORD_VERSION
            || flags != 0
            || u16_at(rest, 6) != RECORD_HEADER_LEN
            || !matches!(
                record_type,
                RECORD_TYPE_DEVICE_TABLE | RECORD_TYPE_FIRMWARE_FRAME
            )
        {
            continue;
        }
        capture.records.push(CaptureRecord {
            record_type,
            device_index: u16_at(rest, 8),
            host_recv_ns: u64_at(rest, 12),
            payload: payload.to_vec(),
        });
    }
    Ok(capture)
}

/// Reads a capture file, decompressing it first when it is a zstd stream.
///
/// Compression is detected from the zstd frame magic, not the file name.
///
/// # Errors
///
/// [`CaptureError::Io`] when the file cannot be read, [`CaptureError::Zstd`] for a
/// corrupt zstd stream, otherwise as [`parse_capture`].
pub fn read_capture(path: &Path) -> Result<Capture, CaptureError> {
    let bytes = std::fs::read(path)?;
    if bytes.starts_with(&ZSTD_MAGIC) {
        let mut plain = Vec::new();
        StreamingDecoder::new(bytes.as_slice())
            .read_to_end(&mut plain)
            .map_err(|e| CaptureError::Zstd(e.to_string()))?;
        return parse_capture(&plain);
    }
    parse_capture(&bytes)
}
