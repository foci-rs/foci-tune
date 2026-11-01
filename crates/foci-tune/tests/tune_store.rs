use std::time::SystemTime;

use foci_tune::model::{FORMAT_VERSION, Filters, Gains, RunId, SessionId};
use foci_tune::pattern::Pattern;
use foci_tune::store::Store;
use foci_tune::tune::{
    ProbeSettings, RungRecord, StandstillSettings, StepSessions, Tune, TuneId, TuneSetup,
    TuneState, TuneStep,
};
use foci_tune_analysis::ladder::{LadderAxis, RungOutcome};
use foci_tune_analysis::verdict::{Gates, RepeatResult};

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
            }],
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
