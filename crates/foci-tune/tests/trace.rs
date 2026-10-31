use std::collections::{HashSet, VecDeque};
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use bytes::Bytes;
use foci_tune::capture::Capture;
use foci_tune::capture_read::read_capture;
use foci_tune::live::{LiveState, TraceMode};
use foci_tune::trace::{FrameSource, PumpEnd, pump_source, serials_to_open};
use tokio::sync::broadcast;

const SERIAL: &str = "2600abcd";

fn frame_v(
    version: u8,
    frame_type: u8,
    payload: &[u8],
    source_index: u8,
    seq: u32,
    ts: u64,
) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&0x4352_5446u32.to_le_bytes());
    v.extend_from_slice(&[version, 32, frame_type, 0]);
    v.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    v.extend_from_slice(&[1, source_index]);
    v.extend_from_slice(&seq.to_le_bytes());
    v.extend_from_slice(&ts.to_le_bytes());
    v.extend_from_slice(&7u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(payload);
    v
}

fn frame(frame_type: u8, payload: &[u8], source_index: u8, seq: u32, ts: u64) -> Vec<u8> {
    frame_v(25, frame_type, payload, source_index, seq, ts)
}

fn control_state_payload(velocity_p: i32) -> Vec<u8> {
    let mut p = vec![0u8; 88];
    p[36..40].copy_from_slice(&velocity_p.to_le_bytes());
    p
}

fn push_str(p: &mut Vec<u8>, s: &str) {
    p.push(s.len() as u8);
    p.extend_from_slice(s.as_bytes());
}

fn metadata_payload(critical_hz: u32) -> Vec<u8> {
    let mut p = vec![1, 0, 2, 0];
    for v in [260_000_000u32, critical_hz, 100, 10, 0, 0, 0, 0] {
        p.extend_from_slice(&v.to_le_bytes());
    }
    push_str(&mut p, "fw");
    push_str(&mut p, "FOCI Ouroboros");
    push_str(&mut p, SERIAL);
    for (kind, idx, label) in [(1u8, 0u8, "channel0"), (1, 1, "channel1")] {
        p.extend_from_slice(&[kind, idx, label.len() as u8]);
        p.extend_from_slice(label.as_bytes());
    }
    p
}

fn critical(seq: u32, ts: u64) -> Vec<u8> {
    frame(1, &[0u8; 88], 0, seq, ts)
}

#[test]
fn metadata_then_two_control_states_fill_both_channels() {
    let mut live = LiveState::default();
    let now = Instant::now();

    live.ingest(SERIAL, &frame(5, &metadata_payload(1000), 0, 1, 10), now);
    live.ingest(
        SERIAL,
        &frame(10, &control_state_payload(841), 0, 2, 20),
        now,
    );
    live.ingest(
        SERIAL,
        &frame(10, &control_state_payload(900), 1, 3, 30),
        now,
    );

    let board = &live.boards[SERIAL];
    assert_eq!(board.metadata.as_ref().unwrap().serial, SERIAL);
    assert_eq!(board.trace_mode, TraceMode::Streaming);
    assert_eq!(board.protocol_version, Some(25));
    assert_eq!(board.channels[&0].velocity_p, 841);
    assert_eq!(board.channels[&1].velocity_p, 900);
    assert!(board.open);
    assert!(board.unusable_reason().is_none());
}

#[test]
fn metadata_with_zero_critical_rate_is_dormant() {
    let mut live = LiveState::default();
    live.ingest(
        SERIAL,
        &frame(5, &metadata_payload(0), 0, 1, 10),
        Instant::now(),
    );

    assert_eq!(live.boards[SERIAL].trace_mode, TraceMode::Dormant);
}

#[test]
fn protocol_24_frame_is_recorded_not_decoded_and_board_stays_open() {
    let mut live = LiveState::default();
    let now = Instant::now();
    live.ingest(SERIAL, &frame(10, &control_state_payload(5), 0, 1, 1), now);

    live.ingest(
        SERIAL,
        &frame_v(24, 10, &control_state_payload(777), 0, 2, 2),
        now,
    );

    let board = &live.boards[SERIAL];
    assert_eq!(board.protocol_version, Some(24));
    assert!(board.open);
    assert_eq!(board.channels[&0].velocity_p, 5);
    let reason = board.unusable_reason().expect("reason is available");
    assert!(reason.contains("24") && reason.contains("25"), "{reason}");
}

#[test]
fn a_protocol_24_board_seen_first_has_no_channels() {
    let mut live = LiveState::default();

    live.ingest(
        SERIAL,
        &frame_v(24, 10, &control_state_payload(777), 0, 2, 2),
        Instant::now(),
    );

    let board = &live.boards[SERIAL];
    assert!(board.channels.is_empty());
    assert!(board.metadata.is_none());
    assert!(board.unusable_reason().is_some());
}

#[test]
fn cadence_is_measured_from_timestamp_ticks() {
    let mut live = LiveState::default();
    let now = Instant::now();
    live.ingest(SERIAL, &frame(5, &metadata_payload(1000), 0, 0, 0), now);

    for k in 0..1000u32 {
        live.ingest(
            SERIAL,
            &critical(k + 1, 1_000_000 + u64::from(k) * 260_000),
            now,
        );
    }

    let hz = live.boards[SERIAL].critical_hz_measured;
    assert!((hz - 1000.0).abs() <= 1.0, "measured {hz}");
}

#[test]
fn old_critical_frames_leave_the_cadence_window() {
    let mut live = LiveState::default();
    let now = Instant::now();
    live.ingest(SERIAL, &frame(5, &metadata_payload(1000), 0, 0, 0), now);
    for k in 0..500u32 {
        live.ingest(SERIAL, &critical(k, u64::from(k) * 260_000), now);
    }
    let later = 10_000_000_000u64;
    for k in 0..200u32 {
        live.ingest(SERIAL, &critical(k, later + u64::from(k) * 2_600_000), now);
    }

    let hz = live.boards[SERIAL].critical_hz_measured;
    assert!((hz - 100.0).abs() <= 1.0, "measured {hz}");
}

#[test]
fn boot_id_change_is_visible_and_drops_stale_channels() {
    let mut live = LiveState::default();
    let now = Instant::now();
    live.ingest(SERIAL, &frame(10, &control_state_payload(5), 0, 1, 1), now);
    let mut rebooted = frame(10, &control_state_payload(6), 1, 1, 1);
    rebooted[24..28].copy_from_slice(&99u32.to_le_bytes());

    live.ingest(SERIAL, &rebooted, now);

    let board = &live.boards[SERIAL];
    assert_eq!((board.boot_id, board.boot_changes), (Some(99), 1));
    assert!(!board.channels.contains_key(&0));
    assert_eq!(board.channels[&1].velocity_p, 6);
}

struct FakeSource {
    chunks: VecDeque<Vec<u8>>,
}

impl FrameSource for FakeSource {
    fn serial(&self) -> &str {
        SERIAL
    }

    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        match self.chunks.pop_front() {
            Some(chunk) => {
                buf[..chunk.len()].copy_from_slice(&chunk);
                Ok(Some(chunk.len()))
            }
            None => Err(io::Error::other("unplugged")),
        }
    }
}

#[test]
fn garbage_does_not_stop_later_frames_and_loss_closes_the_board() {
    let mut bad_header = b"FTRC".to_vec();
    bad_header.extend_from_slice(&[25, 7]);
    bad_header.resize(40, 0xEE);
    let good = frame(10, &control_state_payload(841), 0, 1, 1);
    let mut source = FakeSource {
        chunks: VecDeque::from([
            vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x11],
            bad_header,
            good[..20].to_vec(),
            good[20..].to_vec(),
            frame(10, &control_state_payload(900), 1, 2, 2),
        ]),
    };
    let live = Mutex::new(LiveState::default());
    let (tx, mut rx) = broadcast::channel::<(String, Bytes)>(16);

    let end = pump_source(&mut source, &live, &tx, &AtomicBool::new(false));

    assert!(matches!(end, PumpEnd::Lost(_)));
    let state = live.lock().unwrap();
    let board = &state.boards[SERIAL];
    assert!(!board.open);
    assert_eq!(board.channels[&0].velocity_p, 841);
    assert_eq!(board.channels[&1].velocity_p, 900);
    let (serial, first) = rx.try_recv().unwrap();
    assert_eq!((serial.as_str(), first.as_ref()), (SERIAL, good.as_slice()));
}

fn temp_path(name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn capture_round_trips_through_the_capture_reader() {
    let path = temp_path("trace_capture_round_trip.cap.zst");
    let frames = [
        frame(5, &metadata_payload(1000), 0, 1, 1),
        frame(10, &control_state_payload(841), 0, 2, 2),
        frame(10, &control_state_payload(900), 1, 3, 3),
        frame(10, &control_state_payload(999), 0, 4, 4),
        critical(5, 5),
    ];
    let mut capture = Capture::start(&path, SERIAL).unwrap();
    for f in &frames {
        capture.push(f);
    }

    let summary = capture.finish().unwrap();

    assert_eq!(summary.frames, 5);
    assert_eq!(
        summary.frame_bytes,
        frames.iter().map(|f| f.len() as u64).sum::<u64>()
    );
    assert_eq!(summary.first_control_state[&0].velocity_p, 841);
    assert_eq!(summary.first_control_state[&1].velocity_p, 900);
    assert_eq!(summary.file_bytes, std::fs::metadata(&path).unwrap().len());
    let read = read_capture(&path).unwrap();
    assert_eq!(read.truncated_tail_bytes, 0);
    let read_frames: Vec<&[u8]> = read
        .firmware_frames()
        .map(|r| r.payload.as_slice())
        .collect();
    let expected: Vec<&[u8]> = frames.iter().map(Vec::as_slice).collect();
    assert_eq!(read_frames, expected);
}

fn critical_on(channel: u8, seq: u32, ts: u64) -> Vec<u8> {
    frame(1, &[0u8; 88], channel, seq, ts)
}

#[test]
fn cadence_is_per_channel_not_summed_across_channels() {
    let mut live = LiveState::default();
    let now = Instant::now();
    live.ingest(SERIAL, &frame(5, &metadata_payload(1000), 0, 0, 0), now);

    for k in 0..1000u32 {
        let ts = 1_000_000 + u64::from(k) * 260_000;
        live.ingest(SERIAL, &critical_on(0, k, ts), now);
        live.ingest(SERIAL, &critical_on(1, k, ts + 1000), now);
    }

    let hz = live.boards[SERIAL].critical_hz_measured;
    assert!((hz - 1000.0).abs() <= 1.0, "measured {hz}");
}

#[test]
fn cadence_decays_to_zero_when_critical_frames_stop() {
    let mut live = LiveState::default();
    let now = Instant::now();
    live.ingest(SERIAL, &frame(5, &metadata_payload(1000), 0, 0, 0), now);
    for k in 0..1000u32 {
        live.ingest(
            SERIAL,
            &critical(k, 1_000_000 + u64::from(k) * 260_000),
            now,
        );
    }
    assert!(live.boards[SERIAL].critical_hz_measured > 900.0);

    let end = 1_000_000 + 999 * 260_000u64;
    live.ingest(SERIAL, &frame(2, &[0u8; 16], 0, 1, end + 300_000_000), now);

    assert_eq!(live.boards[SERIAL].critical_hz_measured, 0.0);
}

#[test]
fn metadata_frame_clears_the_cadence_window() {
    let mut live = LiveState::default();
    let now = Instant::now();
    live.ingest(SERIAL, &frame(5, &metadata_payload(1000), 0, 0, 0), now);
    for k in 0..100u32 {
        live.ingest(
            SERIAL,
            &critical(k, 1_000_000 + u64::from(k) * 260_000),
            now,
        );
    }

    live.ingest(
        SERIAL,
        &frame(5, &metadata_payload(0), 0, 1, 30_000_000),
        now,
    );

    assert_eq!(live.boards[SERIAL].critical_hz_measured, 0.0);
    assert_eq!(live.boards[SERIAL].trace_mode, TraceMode::Dormant);
}

#[test]
fn critical_frames_without_metadata_leave_no_cadence() {
    let mut live = LiveState::default();
    let now = Instant::now();

    for k in 0..5000u32 {
        live.ingest(SERIAL, &critical(k, u64::from(k) * 260_000), now);
    }

    assert_eq!(live.boards[SERIAL].critical_hz_measured, 0.0);
    live.ingest(SERIAL, &frame(5, &metadata_payload(1000), 0, 0, 0), now);
    live.ingest(SERIAL, &critical(1, 1_000_000), now);
    assert_eq!(live.boards[SERIAL].critical_hz_measured, 0.0);
}

#[test]
fn boot_id_change_resets_trace_mode_to_unknown() {
    let mut live = LiveState::default();
    let now = Instant::now();
    live.ingest(SERIAL, &frame(5, &metadata_payload(0), 0, 1, 1), now);
    assert_eq!(live.boards[SERIAL].trace_mode, TraceMode::Dormant);
    let mut rebooted = frame(10, &control_state_payload(6), 1, 1, 1);
    rebooted[24..28].copy_from_slice(&99u32.to_le_bytes());

    live.ingest(SERIAL, &rebooted, now);

    assert_eq!(live.boards[SERIAL].trace_mode, TraceMode::Unknown);
}

#[test]
fn capture_baseline_ignores_control_state_before_metadata() {
    let path = temp_path("trace_capture_baseline.cap.zst");
    let mut capture = Capture::start(&path, SERIAL).unwrap();
    capture.push(&frame(10, &control_state_payload(111), 0, 1, 1));
    capture.push(&frame(5, &metadata_payload(1000), 0, 2, 2));
    capture.push(&frame(10, &control_state_payload(222), 0, 3, 3));
    capture.push(&frame(10, &control_state_payload(333), 0, 4, 4));

    let summary = capture.finish().unwrap();

    assert_eq!(summary.frames, 4);
    assert_eq!(summary.first_control_state.len(), 1);
    assert_eq!(summary.first_control_state[&0].velocity_p, 222);
}

#[test]
fn rescan_opens_every_candidate_not_already_active() {
    let candidates: Vec<String> = ["A", "B", "C", "C"].map(String::from).to_vec();
    let active: HashSet<String> = HashSet::from(["A".to_owned()]);

    let wanted = serials_to_open(&candidates, &active);

    assert_eq!(wanted, vec!["B".to_owned(), "C".to_owned()]);
    assert!(serials_to_open(&candidates[..1], &active).is_empty());
}
