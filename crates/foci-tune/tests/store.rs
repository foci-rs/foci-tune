use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use foci_tune::capture::Capture;
use foci_tune::model::{
    CaptureInfo, ChannelBaseline, ControlSnapshot, FORMAT_VERSION, Filters, Gains, Metrics,
    ProbeRecord, Run, RunId, RunRegime, RunRequest, RunState, Session, SessionId, TrackingRecord,
    Verification, VerificationCheck,
};
use foci_tune::pattern::Pattern;
use foci_tune::stimulus::RunKind;
use foci_tune::store::{Store, StoreError, new_id, write_json_atomic};
use foci_tune_analysis::fit::FitError;
use foci_tune_analysis::probe::ProbeMetrics;
use foci_tune_analysis::tracking::TrackingMetrics;
use serde::ser::{Error as _, Serialize, Serializer};

const CLOCK: u32 = 260_000_000;

fn kind() -> RunKind {
    RunKind::Tracking {
        speeds_mm_s: vec![50.0, 195.0],
        strokes: 8,
    }
}

fn pattern() -> Pattern {
    Pattern {
        start: (50.0, 50.0),
        end: (150.0, 150.0),
    }
}

fn gains() -> Gains {
    Gains {
        velocity_p: 1500,
        velocity_i: 40,
        position_p: 141,
        position_i: 0,
    }
}

fn filters() -> Filters {
    Filters {
        velocity_hz: 500,
        torque_hz: 1000,
        position_hz: 500,
        flux_hz: 1000,
    }
}

fn session() -> Session {
    Session {
        format_version: FORMAT_VERSION,
        name: "ladder".into(),
        created_unix_s: 1_793_382_312,
        stepper: "stepper_x".into(),
        kind: kind(),
        pattern: pattern(),
        gains: gains(),
        filters: filters(),
        ladder: None,
    }
}

fn regime() -> RunRegime {
    RunRegime {
        motor_rotation_distance_mm: 40.0,
        encoder_counts_per_rev: 4000,
        pwm_hz: 50_000,
        clock_freq: CLOCK,
    }
}

fn run(session: &SessionId, id: &RunId) -> Run {
    Run {
        format_version: FORMAT_VERSION,
        id: id.clone(),
        request: RunRequest {
            session: session.clone(),
            stepper: "stepper_x".into(),
            kind: kind(),
            pattern: pattern(),
            gains: gains(),
            filters: filters(),
            ladder_value: None,
        },
        state: RunState::Done,
        created_unix_s: 1_793_382_400,
        declared_channel: 0,
        regime: Some(regime()),
        baselines: vec![ChannelBaseline {
            channel: 0,
            control: ControlSnapshot::default(),
        }],
        stream: None,
        steps: Vec::new(),
        clamped_speeds: vec![(400.0, 300.0)],
        applied_speeds: vec![50.0, 195.0, 300.0],
        capture: Some(CaptureInfo {
            frames: 10,
            file_bytes: 100,
        }),
        verification: vec![Verification {
            check: VerificationCheck::NoDropGaps,
            passed: true,
            detail: String::new(),
        }],
        metrics: None,
        notes: Vec::new(),
    }
}

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().to_owned()).unwrap();
    (dir, store)
}

#[test]
fn ids_sort_by_time_and_differ_on_the_suffix() {
    let t = UNIX_EPOCH + Duration::from_secs(1_793_382_312);
    let a = new_id(t);
    let b = new_id(t);
    let later = new_id(t + Duration::from_secs(1));
    assert_eq!(a.len(), "20261030T174512Z-k3q9".len());
    assert!(a.starts_with("20261030T174512Z-"), "{a}");
    assert!(a[..16] < later[..16]);
    assert!(a < later);
    assert_ne!(a, b, "suffixes must differ");
    assert!(
        a[17..]
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    );
}

struct Failing;

impl Serialize for Failing {
    fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(S::Error::custom("serializer refused"))
    }
}

fn names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect()
}

#[test]
fn write_run_leaves_no_temp_file_on_success() {
    let (_d, store) = store();
    let sid = store.new_session(&session()).unwrap();
    let rid = RunId::generate(UNIX_EPOCH + Duration::from_secs(1_793_382_400));
    store.write_run(&sid, &run(&sid, &rid)).unwrap();
    let dir = store.capture_path(&sid, &rid).parent().unwrap().to_owned();
    assert_eq!(names(&dir), vec!["run.json".to_owned()]);
}

#[test]
fn a_serializer_error_leaves_no_partial_file() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("run.json");
    assert!(write_json_atomic(&target, &Failing).is_err());
    assert!(names(dir.path()).is_empty());

    write_json_atomic(&target, &serde_json::json!({"a": 1})).unwrap();
    assert!(write_json_atomic(&target, &Failing).is_err());
    assert_eq!(
        std::fs::read_to_string(&target).unwrap().trim(),
        "{\n  \"a\": 1\n}"
    );
    assert_eq!(names(dir.path()), vec!["run.json".to_owned()]);
}

#[test]
fn run_and_session_round_trip_with_format_version() {
    let (_d, store) = store();
    let sid = store.new_session(&session()).unwrap();
    let rid = RunId::generate(UNIX_EPOCH + Duration::from_secs(1_793_382_400));
    let mut r = run(&sid, &rid);
    r.state = RunState::Skipped("damping unassessable".into());
    store.write_run(&sid, &r).unwrap();

    let dir = store.capture_path(&sid, &rid).parent().unwrap().to_owned();
    let text = std::fs::read_to_string(dir.join("run.json")).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["format_version"], 1);
    assert_eq!(value["state"], "skipped");
    assert_eq!(value["reason"], "damping unassessable");
    assert_eq!(value["clamped_speeds"], serde_json::json!([[400.0, 300.0]]));

    let summaries = store.scan().unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].id, sid);
    assert_eq!(summaries[0].session.as_ref().unwrap(), &session());
    let back = summaries[0].runs[0].run.as_ref().unwrap();
    assert_eq!(back, &r);
    assert_eq!(
        summaries[0].runs[0].state,
        RunState::Skipped("damping unassessable".into())
    );
}

#[test]
fn run_state_serialises_as_a_name_with_an_optional_reason() {
    let unit = serde_json::to_value(RunState::WritingGains).unwrap();
    assert_eq!(unit, serde_json::json!({"state": "writing_gains"}));
    let with_reason = serde_json::to_value(RunState::NotComparable("gap".into())).unwrap();
    assert_eq!(
        with_reason,
        serde_json::json!({"state": "not_comparable", "reason": "gap"})
    );
    let killed: RunState = serde_json::from_str(r#"{"state":"kill_unconfirmed"}"#).unwrap();
    assert_eq!(killed, RunState::KillUnconfirmed);
}

#[test]
fn non_finite_metrics_round_trip_as_null() {
    let tracking = TrackingMetrics {
        cruise_mean_abs_um: f64::NAN,
        sd_um: f64::NAN,
        p99_um: f64::NAN,
        reversal_peak_um: f64::NAN,
        reversal_count: 0,
        moving_samples: 0,
    };
    let probe = ProbeMetrics {
        velocity: Err(FitError::NoConvergence),
        torque: Err(FitError::ZetaOutOfRange(1.4)),
        position_error: Err(FitError::NoConvergence),
        edges: 0,
        stderr_at_peak: f64::NAN,
        torque_psd_peak_hz: f64::INFINITY,
    };
    let metrics = Metrics::for_tracking_and_probe(
        0,
        vec![TrackingRecord::new(195.0, &tracking)],
        Some(ProbeRecord::from(&probe)),
    );
    let text = serde_json::to_string(&metrics).unwrap();
    assert!(text.contains("null"));
    assert!(!text.contains("NaN") && !text.contains("inf"));
    let back: Metrics = serde_json::from_str(&text).unwrap();
    assert_eq!(back, metrics);
    assert_eq!(back.channels[0].tracking[0].reversal_peak_um, None);
}

#[test]
fn a_run_directory_without_run_json_is_listed_as_failed() {
    let (dir, store) = store();
    let sid = store.new_session(&session()).unwrap();
    let good = RunId::generate(UNIX_EPOCH + Duration::from_secs(1_793_382_400));
    store.write_run(&sid, &run(&sid, &good)).unwrap();
    let broken = dir
        .path()
        .join("sessions")
        .join(sid.as_str())
        .join("20261030T180000Z-dead");
    std::fs::create_dir(&broken).unwrap();
    std::fs::write(broken.join("capture.bin.zst"), b"x").unwrap();

    let runs = &store.scan().unwrap()[0].runs;
    assert_eq!(runs.len(), 2);
    let bad = runs
        .iter()
        .find(|r| r.id.as_str() == "20261030T180000Z-dead")
        .unwrap();
    assert!(bad.run.is_none());
    match &bad.state {
        RunState::Failed(reason) => assert!(reason.contains("run.json"), "{reason}"),
        other => panic!("expected failed, got {other:?}"),
    }
}

fn frame(frame_type: u8, payload: &[u8], source_index: u8, seq: u32, ts: u64) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&0x4352_5446u32.to_le_bytes());
    v.extend_from_slice(&[25, 32, frame_type, 0]);
    v.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    v.extend_from_slice(&[1, source_index]);
    v.extend_from_slice(&seq.to_le_bytes());
    v.extend_from_slice(&ts.to_le_bytes());
    v.extend_from_slice(&7u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(payload);
    v
}

fn push_str(p: &mut Vec<u8>, s: &str) {
    p.push(s.len() as u8);
    p.extend_from_slice(s.as_bytes());
}

fn metadata_payload() -> Vec<u8> {
    let mut p = vec![1, 0, 1, 0];
    for v in [CLOCK, 1000, 100, 10, 0, 0, 0, 0] {
        p.extend_from_slice(&v.to_le_bytes());
    }
    push_str(&mut p, "fw");
    push_str(&mut p, "FOCI");
    push_str(&mut p, "SER");
    p.extend_from_slice(&[1, 0, 8]);
    p.extend_from_slice(b"channel0");
    p
}

fn critical_payload(position: i32, error: i32) -> Vec<u8> {
    let mut p = vec![0u8; 88];
    p[0..4].copy_from_slice(&position.to_le_bytes());
    p[4..8].copy_from_slice(&position.to_le_bytes());
    p[8..12].copy_from_slice(&error.to_le_bytes());
    p
}

fn write_synthetic_capture(path: &Path, criticals: u32) {
    let mut cap = Capture::start(path, "SER").unwrap();
    cap.push(&frame(5, &metadata_payload(), 0, 0, 0));
    for k in 0..criticals {
        let ts = u64::from(k) * u64::from(CLOCK / 1000);
        let payload = critical_payload(k as i32 * 100, 655);
        cap.push(&frame(1, &payload, 0, k + 1, ts));
    }
    cap.finish().unwrap();
}

#[test]
fn series_regenerates_from_the_capture_when_missing() {
    let (_d, store) = store();
    let sid = store.new_session(&session()).unwrap();
    let rid = RunId::generate(UNIX_EPOCH + Duration::from_secs(1_793_382_400));
    store.write_run(&sid, &run(&sid, &rid)).unwrap();
    write_synthetic_capture(&store.capture_path(&sid, &rid), 200);

    let series_path = store.capture_path(&sid, &rid).with_file_name("series.json");
    assert!(!series_path.exists());
    let series = store.series(&sid, &rid).unwrap();
    assert!(series_path.exists(), "regenerated series is persisted");
    assert_eq!(series.format_version, FORMAT_VERSION);
    assert_eq!(series.channels.len(), 1);
    let ch = &series.channels[0];
    assert_eq!(ch.channel, 0);
    assert_eq!(ch.t_s.len(), 200usize.div_ceil(series.decimation as usize));
    assert_eq!(ch.t_s[1], Some(f64::from(series.decimation) / 1000.0));
    // 655 position units of 40 mm / 65536 per unit is about 400 um.
    let err = ch.position_error_um[0].unwrap();
    assert!((err - 399.8).abs() < 0.5, "{err}");

    let again = store.series(&sid, &rid).unwrap();
    assert_eq!(again, series);
}

#[test]
fn an_undecodable_capture_is_a_capture_error_not_an_empty_series() {
    let (_d, store) = store();
    let sid = store.new_session(&session()).unwrap();
    let rid = RunId::generate(UNIX_EPOCH + Duration::from_secs(1_793_382_400));
    store.write_run(&sid, &run(&sid, &rid)).unwrap();
    std::fs::write(
        store.capture_path(&sid, &rid),
        b"\x28\xb5\x2f\xfd not a zstd frame",
    )
    .unwrap();

    match store.series(&sid, &rid) {
        Err(StoreError::Capture(reason)) => assert!(!reason.is_empty()),
        other => panic!("expected StoreError::Capture, got {other:?}"),
    }
}

#[test]
fn a_capture_without_critical_frames_is_a_capture_error() {
    let (_d, store) = store();
    let sid = store.new_session(&session()).unwrap();
    let rid = RunId::generate(UNIX_EPOCH + Duration::from_secs(1_793_382_400));
    store.write_run(&sid, &run(&sid, &rid)).unwrap();
    write_synthetic_capture(&store.capture_path(&sid, &rid), 0);
    assert!(matches!(
        store.series(&sid, &rid),
        Err(StoreError::Capture(_))
    ));
}

#[test]
fn free_bytes_reports_the_data_directory() {
    let (_d, store) = store();
    assert!(store.free_bytes().unwrap() > 0);
}
