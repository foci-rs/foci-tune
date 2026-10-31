use std::io::Cursor;
use std::path::PathBuf;

use foci_trace::capture_file::{
    CaptureWriter, RECORD_TYPE_DEVICE_TABLE, RECORD_TYPE_FIRMWARE_FRAME,
};
use foci_trace::clock::CaptureClock;
use foci_tune::capture_read::{Capture, CaptureRecord, parse_capture, read_capture};
use structured_zstd::encoding::{CompressionLevel, compress_slice_to_vec};

const FRAMES: [&[u8]; 3] = [b"FTRCfirst", b"FTRCsecond-frame", b""];

fn synthetic_capture() -> Vec<u8> {
    let clock = CaptureClock::new().unwrap();
    let mut out = Cursor::new(Vec::new());
    let mut writer = CaptureWriter::new(&mut out, &clock).unwrap();
    writer
        .write_device_table(0, 10, "1-1.2", "FOCI Ouroboros", "SER1", b"FTRCmeta")
        .unwrap();
    for (k, frame) in FRAMES.iter().enumerate() {
        writer
            .write_firmware_frame(k as u16 % 2, 100 + k as u64, frame)
            .unwrap();
    }
    writer.flush().unwrap();
    out.into_inner()
}

fn assert_synthetic(capture: &Capture) {
    assert_eq!(capture.truncated_tail_bytes, 0);
    assert_eq!(capture.records.len(), 4);
    assert_eq!(capture.records[0].record_type, RECORD_TYPE_DEVICE_TABLE);
    assert_eq!(capture.records[0].host_recv_ns, 10);
    assert_eq!(
        capture.records[0].device_table_metadata(),
        Some(&b"FTRCmeta"[..])
    );
    let frames: Vec<&CaptureRecord> = capture.firmware_frames().collect();
    assert_eq!(frames.len(), FRAMES.len());
    for (k, (record, expected)) in frames.iter().zip(FRAMES).enumerate() {
        assert_eq!(record.record_type, RECORD_TYPE_FIRMWARE_FRAME);
        assert_eq!(record.device_index, k as u16 % 2);
        assert_eq!(record.host_recv_ns, 100 + k as u64);
        assert_eq!(record.payload, expected);
    }
}

fn temp_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

#[test]
fn plain_capture_round_trips() {
    let path = temp_path("capture_read_plain.bin");
    std::fs::write(&path, synthetic_capture()).unwrap();

    let capture = read_capture(&path).unwrap();

    assert_synthetic(&capture);
}

#[test]
fn zstd_capture_round_trips() {
    let path = temp_path("capture_read_zstd.bin.zst");
    let compressed = compress_slice_to_vec(&synthetic_capture(), CompressionLevel::from_level(3));
    std::fs::write(&path, compressed).unwrap();

    let capture = read_capture(&path).unwrap();

    assert_synthetic(&capture);
}

#[test]
fn partial_final_record_is_reported_not_fatal() {
    let mut bytes = synthetic_capture();
    bytes.truncate(bytes.len() - 3);

    let capture = parse_capture(&bytes).unwrap();

    assert_eq!(capture.firmware_frames().count(), 2);
    assert_eq!(capture.truncated_tail_bytes, 32 - 3);
}

#[test]
fn bad_file_magic_is_an_error() {
    let mut bytes = synthetic_capture();
    bytes[0] ^= 0xFF;

    assert!(parse_capture(&bytes).is_err());
}
