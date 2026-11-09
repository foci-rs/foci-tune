use std::time::SystemTime;

use foci_tune::model::{FORMAT_VERSION, Filters, Gains, RunId, SessionId};
use foci_tune::pattern::Pattern;
use foci_tune::store::Store;
use foci_tune::tune::{
    PauseRecord, ProbeSettings, RungRecord, StandstillSettings, StepSessions, Tune, TuneId,
    TuneSetup, TuneState, TuneStep, Waiver,
};
use foci_tune_analysis::ladder::{LadderAxis, RungOutcome};
use foci_tune_analysis::verdict::{Gates, Halt, Pause, RepeatResult};

fn tune(stepper: &str, state: TuneState) -> Tune {
    let session = SessionId::generate(SystemTime::now());
    Tune {
        format_version: FORMAT_VERSION,
        id: TuneId::generate(SystemTime::now()),
        setup: TuneSetup {
            stepper: stepper.to_owned(),
            speeds_mm_s: vec![50.0, 195.0],
            pattern: Pattern {
                start: (50.0, 50.0),
                end: (150.0, 150.0),
            },
            probe: ProbeSettings {
                speed_mm_s: 100.0,
                high_gain: 3000.0,
                segments_between_toggles: 2,
                strokes: 8,
            },
            tracking_strokes: 8,
            standstill: StandstillSettings {
                rests: 4,
                dwell_s: 2.5,
            },
            gates: Gates::default(),
            include_marginal: false,
        },
        snapshot: (
            Gains {
                velocity_p: 1500,
                velocity_i: 40,
                position_p: 141,
                position_i: 0,
            },
            Filters {
                velocity_hz: 400,
                torque_hz: 1000,
                position_hz: 200,
                flux_hz: 1000,
            },
        ),
        steps: vec![TuneStep {
            axis: LadderAxis::VelocityP,
            seed_raw: 1500,
            repeats: 2,
            lower_k: -3,
            upper_k: 3,
            octave_divisions: 3,
            pwm_hz: 50_000,
            base: (
                Gains {
                    velocity_p: 1500,
                    velocity_i: 40,
                    position_p: 141,
                    position_i: 0,
                },
                Filters {
                    velocity_hz: 400,
                    torque_hz: 1000,
                    position_hz: 200,
                    flux_hz: 1000,
                },
            ),
            sessions: StepSessions {
                probe: session.clone(),
                tracking: session.clone(),
                standstill: session,
            },
            notes: Vec::new(),
            rungs: vec![RungRecord {
                shown: 1500.0,
                raw: 1500,
                run_ids: vec![RunId::generate(SystemTime::now())],
                started_unix_ms: Some(1_700_000_000_000),
                ended_unix_ms: Some(1_700_000_030_000),
                skipped: None,
                repeat_values: vec![RepeatResult {
                    zeta: RungOutcome::Assessed(0.11),
                    p99_by_speed: vec![Some(12.5), None],
                    worst_torque_sd: Some(3.25),
                }],
                run_times: Vec::new(),
                waived: None,
                superseded: Vec::new(),
            }],
            paused: None,
            standstill_gate_off: None,
            chosen: Some(1500),
            chosen_shown: Some(1500.0),
            starts: 1,
            started_unix_ms: Some(1_700_000_000_000),
            ended_unix_ms: None,
            failed: None,
            run_count: None,
            assessment: None,
            recommendation: None,
            drift_rung: None,
            drift: Vec::new(),
        }],
        state,
        created_unix_s: 1_700_000_000,
        notes: Vec::new(),
    }
}

fn all_states() -> Vec<TuneState> {
    vec![
        TuneState::Ready,
        TuneState::Announced,
        TuneState::Running,
        TuneState::AwaitingChoice,
        TuneState::Paused,
        TuneState::Done,
        TuneState::Abandoned,
        TuneState::RestorePending,
        TuneState::RestoreFailed("no response".to_owned()),
    ]
}

#[test]
fn tune_round_trips_through_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().to_owned()).unwrap();
    let t = tune("stepper_x", TuneState::AwaitingChoice);

    store.write_tune(&t).unwrap();

    assert_eq!(store.read_tune(&t.id).unwrap(), t);
}

#[test]
fn incomplete_tunes_are_found_by_scan() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().to_owned()).unwrap();
    let mut written = Vec::new();
    for state in all_states() {
        let mut t = tune("stepper_x", state);
        t.id = TuneId::parse(&format!("t{}", written.len())).unwrap();
        store.write_tune(&t).unwrap();
        written.push(t);
    }

    let found = store.scan_tunes().unwrap();
    let mut nonterminal: Vec<_> = found
        .iter()
        .filter(|t| !t.state.is_terminal())
        .map(|t| t.state.clone())
        .collect();
    nonterminal.sort_by_key(|s| format!("{s:?}"));

    assert_eq!(found.len(), written.len());
    let mut expected: Vec<_> = all_states()
        .into_iter()
        .filter(|s| !matches!(s, TuneState::Done | TuneState::Abandoned))
        .collect();
    expected.sort_by_key(|s| format!("{s:?}"));
    assert_eq!(nonterminal, expected);
}

#[test]
fn nonterminal_tune_for_survives_reopen_and_ignores_finished_tunes() {
    let dir = tempfile::tempdir().unwrap();
    for state in all_states() {
        let terminal = state.is_terminal();
        let store = Store::open(dir.path().to_owned()).unwrap();
        let t = tune("stepper_x", state);
        store.write_tune(&t).unwrap();
        let other = tune("stepper_y", TuneState::Ready);
        store.write_tune(&other).unwrap();

        let reopened = Store::open(dir.path().to_owned()).unwrap();
        let found = reopened.nonterminal_tune_for("stepper_x").unwrap();

        assert_eq!(found, (!terminal).then(|| t.id.clone()));
        std::fs::remove_file(dir.path().join("tunes").join(format!("{}.json", t.id))).unwrap();
        std::fs::remove_file(dir.path().join("tunes").join(format!("{}.json", other.id))).unwrap();
    }
}

#[test]
fn tune_files_do_not_disturb_session_scanning() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().to_owned()).unwrap();
    store
        .write_tune(&tune("stepper_x", TuneState::Ready))
        .unwrap();

    assert!(store.scan().unwrap().is_empty());
}

fn three_rung_tune() -> Tune {
    let mut t = tune("stepper_x", TuneState::Ready);
    let rung = t.steps[0].rungs[0].clone();
    t.steps[0].rungs = vec![rung.clone(), rung.clone(), rung];
    t
}

#[test]
fn a_tune_record_without_the_pause_fields_loads_with_defaults() {
    let mut v = serde_json::to_value(three_rung_tune()).unwrap();
    let step = &mut v["steps"][0];
    step.as_object_mut().unwrap().remove("paused");
    step.as_object_mut().unwrap().remove("standstill_gate_off");
    for rung in step["rungs"].as_array_mut().unwrap() {
        rung.as_object_mut().unwrap().remove("waived");
        rung.as_object_mut().unwrap().remove("superseded");
    }
    step["assessment"] = serde_json::json!({
        "verdicts": [],
        "medians": [],
        "halt": [1, "DampingUnassessable"],
        "best": null,
        "runner_up": null,
        "tie": [],
        "spreads": [],
    });

    let tune: Tune = serde_json::from_value(v).unwrap();

    assert_eq!(tune.steps[0].paused, None);
    assert_eq!(tune.steps[0].standstill_gate_off, None);
    assert!(
        tune.steps[0]
            .rungs
            .iter()
            .all(|r| r.waived.is_none() && r.superseded.is_empty())
    );
    assert_eq!(
        tune.steps[0].assessment.as_ref().unwrap().halt,
        Some((1, Halt::DampingUnassessable))
    );
}

#[test]
fn a_paused_tune_round_trips() {
    let mut tune = three_rung_tune();
    tune.state = TuneState::Paused;
    tune.steps[0].paused = Some(PauseRecord {
        rung: 2,
        reason: Pause::DampingUnassessable,
        unix_ms: 7,
    });
    tune.steps[0].rungs[2].waived = Some(Waiver {
        unix_ms: 8,
        reason: Pause::DampingUnassessable,
    });

    let json = serde_json::to_string(&tune).unwrap();

    assert_eq!(serde_json::from_str::<Tune>(&json).unwrap(), tune);
}

fn with_step_sessions(step: &mut TuneStep) -> [SessionId; 3] {
    let ids = [
        SessionId::generate(SystemTime::now()),
        SessionId::generate(SystemTime::now()),
        SessionId::generate(SystemTime::now()),
    ];
    step.sessions = StepSessions {
        probe: ids[0].clone(),
        tracking: ids[1].clone(),
        standstill: ids[2].clone(),
    };
    ids
}

#[test]
fn every_session_of_every_step_is_listed_with_its_step_counted_from_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().to_owned()).unwrap();
    let mut t = tune("stepper_x", TuneState::Ready);
    let second = t.steps[0].clone();
    t.steps.push(second);
    let first_ids = with_step_sessions(&mut t.steps[0]);
    let second_ids = with_step_sessions(&mut t.steps[1]);
    store.write_tune(&t).unwrap();

    let owners = store.tune_sessions().unwrap();

    assert_eq!(owners.len(), 6);
    for id in &first_ids {
        assert_eq!(owners[id], (t.id.clone(), 1));
    }
    for id in &second_ids {
        assert_eq!(owners[id], (t.id.clone(), 2));
    }
}

#[test]
fn a_session_in_two_tunes_belongs_to_the_older_tune_every_time() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().to_owned()).unwrap();
    let mut older = tune("stepper_x", TuneState::Done);
    let mut newer = tune("stepper_x", TuneState::Done);
    older.id = TuneId::parse("20261101T000000Z-aaaa").unwrap();
    newer.id = TuneId::parse("20261102T000000Z-bbbb").unwrap();
    let shared = with_step_sessions(&mut older.steps[0]);
    newer.steps[0].sessions = older.steps[0].sessions.clone();
    store.write_tune(&newer).unwrap();
    store.write_tune(&older).unwrap();

    for _ in 0..5 {
        let owners = store.tune_sessions().unwrap();
        assert_eq!(owners[&shared[0]], (older.id.clone(), 1));
    }
}

#[test]
fn a_tune_bundle_holds_the_record_its_sessions_and_names_the_missing_ones() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().to_owned()).unwrap();
    let mut t = tune("stepper_x", TuneState::Done);
    let present = SessionId::generate(SystemTime::now());
    let gone = SessionId::parse("20990101T000000Z-zzzz").unwrap();
    t.steps[0].sessions = StepSessions {
        probe: present.clone(),
        tracking: present.clone(),
        standstill: gone.clone(),
    };
    store.write_tune(&t).unwrap();
    let session_dir = store.session_dir(&present);
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(session_dir.join("session.json"), b"{}").unwrap();

    let bytes = foci_tune::export::tune_bundle(&store, &t.id).unwrap();

    let decoder = structured_zstd::decoding::StreamingDecoder::new(&bytes[..]);
    let mut archive = tar::Archive::new(decoder);
    let mut entries = Vec::new();
    let mut manifest = String::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().into_owned();
        if path == "manifest.json" {
            std::io::Read::read_to_string(&mut entry, &mut manifest).unwrap();
        }
        entries.push(path);
    }
    assert_eq!(
        entries,
        [
            format!("{}/tune.json", t.id),
            format!("{}/sessions/{present}/session.json", t.id),
            "manifest.json".to_owned(),
        ]
    );
    let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
    assert_eq!(manifest["kind"], "tune");
    assert_eq!(
        manifest["missing_sessions"],
        serde_json::json!([gone.as_str()])
    );
}

#[test]
fn a_capture_still_being_written_is_left_out_and_named_in_the_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().to_owned()).unwrap();
    let session = SessionId::generate(SystemTime::now());
    let session_dir = store.session_dir(&session);
    for (run, run_json) in [
        ("done", r#"{"capture":{"bytes":3}}"#),
        ("live", r#"{"capture":null}"#),
    ] {
        std::fs::create_dir_all(session_dir.join(run)).unwrap();
        std::fs::write(session_dir.join(run).join("run.json"), run_json).unwrap();
        std::fs::write(session_dir.join(run).join("capture.bin.zst"), b"zst").unwrap();
    }

    let bytes = foci_tune::export::session_bundle(&store, &session).unwrap();

    let decoder = structured_zstd::decoding::StreamingDecoder::new(&bytes[..]);
    let mut archive = tar::Archive::new(decoder);
    let mut entries = Vec::new();
    let mut manifest = String::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().into_owned();
        if path == "manifest.json" {
            std::io::Read::read_to_string(&mut entry, &mut manifest).unwrap();
        }
        entries.push(path);
    }
    assert!(entries.contains(&format!("{session}/done/capture.bin.zst")));
    assert!(entries.contains(&format!("{session}/live/run.json")));
    assert!(!entries.contains(&format!("{session}/live/capture.bin.zst")));
    let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
    assert_eq!(
        manifest["unfinished_captures"],
        serde_json::json!([format!("{session}/live")])
    );
}
