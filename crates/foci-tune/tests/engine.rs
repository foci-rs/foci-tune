mod fake_moonraker;
mod rig;

use std::f64::consts::TAU;
use std::time::{Duration, Instant};

use foci_tune::engine::{
    Engine, EngineConfig, LadderRejected, MAX_LADDER_REPEATS, MAX_LADDER_RUNS, MAX_LADDER_VALUES,
    Precondition,
};
use foci_tune::model::{RunId, RunState};
use foci_tune::moonraker::{KlippyState, MoonrakerApi};
use foci_tune::stimulus::RunKind;
use foci_tune_analysis::ladder::LadderError;
use rig::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_writes_exact_gains_records_and_reaches_done() {
    let rig = rig().await;

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, RunState::Done, "{:?}", run.verification);
    let scripts = rig.scripts();
    assert_eq!(scripts[0], "M400");
    assert_eq!(scripts[1], GAINS_LINE);
    assert_eq!(scripts[2], FILTERS_LINE);
    assert_eq!(scripts[3], CRITICAL_LINE);
    assert!(scripts[4].starts_with("G90"));
    assert_eq!(scripts[5], DORMANT_LINE);
    assert_eq!(scripts.len(), 6);
    assert_eq!(run.request.gains.position_i, 0);
    assert_eq!(run.baselines.len(), 2);
    assert_eq!(run.baselines[0].channel, 0);
    assert_eq!(run.baselines[0].control.velocity_p, 841);
    assert_eq!(run.baselines[0].control.velocity_i, 412);
    assert_eq!(run.baselines[0].control.velocity_filter_hz, 500);
    assert_eq!(run.baselines[1].channel, 1);
    let steps: Vec<&str> = run.steps.iter().map(|s| s.step.as_str()).collect();
    for step in [
        "preparing",
        "writing_gains",
        "baseline",
        "recording",
        "moving",
        "draining",
        "verifying",
        "analyzing",
        "done",
    ] {
        assert!(steps.contains(&step), "{step} missing from {steps:?}");
    }
    assert!(run.steps.windows(2).all(|w| w[0].unix_ms <= w[1].unix_ms));
    assert_eq!(run.applied_speeds, vec![50.0, 195.0]);
    assert_eq!(run.stream.as_ref().unwrap().protocol_version, 25);
    assert_eq!(
        run.regime.as_ref().unwrap().encoder_counts_per_rev,
        COUNTS_PER_REV
    );
    assert!(run.verification.iter().all(|v| v.passed));
    let metrics = run.metrics.as_ref().unwrap();
    assert_eq!(metrics.channels[0].channel, 0);
    assert!(metrics.channels[0].declared);
    assert!(run.capture.as_ref().unwrap().frames > 1000);
    assert!(rig.store.capture_path(&rig.session, &run.id).exists());
    let series = rig.store.series(&rig.session, &run.id).unwrap();
    assert_eq!(series.channels.len(), 2);
    assert!(run.notes.is_empty(), "{:?}", run.notes);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_speed_above_max_velocity_is_clamped_in_run_json() {
    let rig = rig().await;

    let run = rig
        .run_once(RunKind::Tracking {
            speeds_mm_s: vec![50.0, 1000.0],
            strokes: 1,
        })
        .await;

    let limit = 300.0 * std::f64::consts::SQRT_2;
    assert_eq!(run.clamped_speeds.len(), 1);
    assert_eq!(run.clamped_speeds[0].0, 1000.0);
    assert!((run.clamped_speeds[0].1 - limit).abs() < 1e-9);
    assert_eq!(run.applied_speeds.len(), 2);
}

async fn refused(rig: &Rig) -> Precondition {
    let err = rig
        .engine
        .submit(rig.request(tracking()))
        .await
        .unwrap_err();
    assert!(rig.scripts().is_empty(), "{:?}", rig.scripts());
    err
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn klipper_not_ready_is_refused() {
    let rig = rig().await;
    rig.fake.push("notify_klippy_shutdown", json!([]));
    let mut state = rig.client.state();
    state
        .wait_for(|s| s.klippy == KlippyState::Shutdown)
        .await
        .unwrap();

    assert_eq!(refused(&rig).await, Precondition::KlippyNotReady);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn printing_is_refused() {
    let rig = rig().await;
    rig.fake
        .update_status(json!({"print_stats": {"state": "printing"}}));
    let mut state = rig.client.state();
    state
        .wait_for(|s| s.print_state == "printing")
        .await
        .unwrap();

    assert_eq!(refused(&rig).await, Precondition::Printing);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unhomed_axis_is_refused() {
    let rig = rig_with(Options {
        homed: "yz",
        ..Options::default()
    })
    .await;

    assert_eq!(refused(&rig).await, Precondition::NotHomed('x'));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_electrical_model_is_refused() {
    let rig = rig_with(Options {
        electrical_model: false,
        ..Options::default()
    })
    .await;

    assert_eq!(refused(&rig).await, Precondition::NoElectricalModel);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_closed_trace_interface_is_refused() {
    let rig = rig_with(Options {
        open_a: false,
        initial_hz_a: None,
        ..Options::default()
    })
    .await;

    assert_eq!(refused(&rig).await, Precondition::TraceNotOpen);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_board_on_another_protocol_is_refused() {
    let rig = rig_with(Options {
        initial_hz_a: None,
        ..Options::default()
    })
    .await;
    let frame = frame_bytes(24, 5, 0, (0, 0), 1, 1, &metadata_payload(SERIAL_A, 0));
    rig.a
        .tx
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .send(frame)
        .unwrap();
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        let seen = rig.a.live.lock().unwrap().boards[SERIAL_A].protocol_version;
        if seen == Some(24) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    assert_eq!(refused(&rig).await, Precondition::ProtocolVersion(24));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_open_board_without_frames_is_refused() {
    let rig = rig_with(Options {
        initial_hz_a: None,
        ..Options::default()
    })
    .await;

    assert_eq!(refused(&rig).await, Precondition::NoTraceFrames);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn low_disk_is_refused() {
    let rig = rig_with(Options {
        config: EngineConfig {
            min_free_bytes: u64::MAX,
            ..fast_config()
        },
        ..Options::default()
    })
    .await;

    assert!(matches!(refused(&rig).await, Precondition::LowDisk(free) if free < u64::MAX));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_run_while_one_is_in_flight_is_busy() {
    let rig = rig().await;
    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let first = rig.engine.submit(rig.request(tracking())).await.unwrap();
    rig.wait_moving(&mut events, &first).await;

    let second = rig.engine.submit(rig.request(tracking())).await;

    assert_eq!(second, Err(Precondition::Busy));
    gate.notify_one();
    assert_eq!(rig.finish(&first).await.state, RunState::Done);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unconfirmed_gain_write_fails_before_motion() {
    let rig = rig().await;
    rig.a.sim(|s| s.confirm_gains = false);

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, failed("gain write unconfirmed"));
    assert_eq!(
        rig.scripts(),
        vec!["M400".to_owned(), GAINS_LINE.to_owned()]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_filter_error_response_fails_with_its_message() {
    let rig = rig().await;
    let message = "!! FLUX_HZ must be 0 (disabled) or 20..4000";
    rig.a
        .sim(|s| s.failures.push(("FOCI_SET_FILTERS".into(), message.into())));

    let run = rig.run_once(tracking()).await;

    let RunState::Failed(reason) = &run.state else {
        panic!("{:?}", run.state);
    };
    assert!(reason.contains(message), "{reason}");
    assert_eq!(rig.stimuli(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_filter_frame_fails_before_the_capture() {
    let rig = rig().await;
    rig.a.sim(|s| s.confirm_filters = [true, true, true, false]);

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, failed("filter write unconfirmed: flux"));
    assert!(!rig.store.capture_path(&rig.session, &run.id).exists());
    assert_eq!(rig.scripts().len(), 3, "{:?}", rig.scripts());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wrong_cutoff_is_unconfirmed() {
    let rig = rig().await;
    rig.a.sim(|s| s.cutoff_offset = [0, 0, 0, 10]);

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, failed("filter write unconfirmed: flux"));
    assert_eq!(rig.stimuli(), 0);
    assert!(!rig.store.capture_path(&rig.session, &run.id).exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metadata_without_control_state_is_a_missing_baseline() {
    let rig = rig().await;
    rig.a.sim(|s| s.baseline = false);

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, failed("baseline missing"));
    assert_eq!(rig.stimuli(), 0);
    assert_eq!(rig.scripts().last().unwrap(), DORMANT_LINE);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_gain_changed_before_the_baseline_fails_the_run_before_motion() {
    let rig = rig().await;
    rig.a
        .sim(|s| s.drift = Some(Box::new(|c| c.gains[0] = 900)));

    let run = rig.run_once(tracking()).await;

    assert_eq!(
        run.state,
        failed("baseline differs: velocity_p requested 841, found 900")
    );
    assert_eq!(rig.stimuli(), 0);
    assert_eq!(rig.scripts().last().unwrap(), DORMANT_LINE);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_filter_changed_before_the_baseline_fails_the_run_before_motion() {
    let rig = rig().await;
    rig.a
        .sim(|s| s.drift = Some(Box::new(|c| c.filters[1] = 2000)));

    let run = rig.run_once(tracking()).await;

    assert_eq!(
        run.state,
        failed("baseline differs: torque_hz requested 1000, found 2000")
    );
    assert_eq!(rig.stimuli(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_other_channel_changing_before_the_baseline_does_not_fail_the_run() {
    let rig = rig().await;
    rig.a.sim(|s| s.channels[1].gains[0] = 900);

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, RunState::Done, "{:?}", run.verification);
    assert_eq!(rig.stimuli(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_channel_moved_thresholds_sit_at_one_and_sixteen_counts() {
    let rig = rig().await;
    let cases = [
        ((Q, 0), not_comparable("channel 0 did not move")),
        ((Q + 1, 0), RunState::Done),
        ((50_000, 16 * Q), not_comparable("channel 1 moved")),
        ((50_000, 16 * Q - 1), RunState::Done),
    ];

    for ((declared, other), expected) in cases {
        rig.a.sim(|s| {
            s.profile.declared_range = declared;
            s.profile.other_range = other;
        });
        let run = rig.run_once(tracking()).await;
        assert_eq!(run.state, expected, "declared {declared} other {other}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sequence_jump_a_drop_flag_and_few_edges_are_not_comparable() {
    let rig = rig().await;

    rig.a.sim(|s| s.profile.sequence_jump = true);
    let jump = rig.run_once(tracking()).await;
    rig.a.sim(|s| {
        s.profile = Profile {
            drop_flag: true,
            ..Profile::default()
        }
    });
    let drop = rig.run_once(tracking()).await;
    rig.a.sim(|s| {
        s.profile = Profile {
            probe_edges: 12,
            ..Profile::default()
        }
    });
    let edges = rig.run_once(probe()).await;

    assert_eq!(
        jump.state,
        not_comparable("change_sequence on channel 0 advanced by 2")
    );
    assert!(
        matches!(&drop.state, RunState::NotComparable(r) if r.starts_with("drop gap")),
        "{:?}",
        drop.state
    );
    assert_eq!(edges.state, not_comparable("12 edges"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_probe_run_with_enough_edges_fits_the_damping() {
    let rig = rig().await;
    rig.a.sim(|s| {
        s.zeta_by_position_p.insert(141, 0.03);
    });

    let run = rig.run_once(probe()).await;

    assert_eq!(run.state, RunState::Done, "{:?}", run.verification);
    let probe = run.metrics.unwrap().channels[0].probe.clone().unwrap();
    assert_eq!(probe.edges, 25);
    let foci_tune::model::FitOutcome::Fit { zeta, .. } = probe.velocity else {
        panic!("{:?}", probe.velocity);
    };
    assert!((zeta - 0.03).abs() < 0.01, "{zeta}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kill_during_motion_is_killed_once_shutdown_is_reported() {
    let rig = rig().await;
    rig.fake
        .script_emergency_stop(None, Duration::ZERO, Some(Duration::ZERO));
    let _gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let ids = rig
        .engine
        .submit_ladder(rig.ladder(tracking(), "velocity_p", &[841.0, 941.0, 1041.0], 1))
        .await
        .unwrap();
    rig.wait_moving(&mut events, &ids[0]).await;
    let sent_before_kill = rig.scripts().len();

    rig.engine.kill().await;

    assert_eq!(rig.finish(&ids[0]).await.state, RunState::Killed);
    assert_eq!(rig.run(&ids[1]).state, RunState::Aborted);
    assert_eq!(rig.run(&ids[2]).state, RunState::Aborted);
    assert!(rig.engine.is_idle());
    assert_eq!(rig.fake.emergency_stops(), 1);
    assert_eq!(rig.scripts().len(), sent_before_kill, "{:?}", rig.scripts());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_emergency_stop_is_kill_unconfirmed() {
    let rig = rig().await;
    rig.fake
        .script_emergency_stop(Some(500), Duration::ZERO, None);
    let _gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let ids = rig
        .engine
        .submit_ladder(rig.ladder(tracking(), "velocity_p", &[841.0, 941.0], 1))
        .await
        .unwrap();
    rig.wait_moving(&mut events, &ids[0]).await;

    rig.engine.kill().await;

    assert_eq!(rig.finish(&ids[0]).await.state, RunState::KillUnconfirmed);
    assert_eq!(rig.run(&ids[1]).state, RunState::Aborted);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn losing_the_socket_during_a_delayed_stop_is_kill_unconfirmed() {
    let rig = rig().await;
    rig.fake
        .script_emergency_stop(None, Duration::from_millis(150), None);
    let _gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let id = rig.engine.submit(rig.request(tracking())).await.unwrap();
    rig.wait_moving(&mut events, &id).await;

    let drop_socket = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        rig.fake.refuse_websockets(true);
        rig.fake.drop_websockets();
    };
    tokio::join!(rig.engine.kill(), drop_socket);
    let run = rig.finish(&id).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(rig.fake.emergency_stops(), 1);
    assert_eq!(run.state, RunState::KillUnconfirmed);
    assert_eq!(rig.run(&id).state, RunState::KillUnconfirmed);
    assert_eq!(
        rig.engine.submit(rig.request(tracking())).await,
        Err(Precondition::KillUnconfirmed)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_run_starts_while_a_kill_is_unconfirmed_until_shutdown_is_seen() {
    let rig = rig().await;
    rig.fake
        .script_emergency_stop(Some(500), Duration::ZERO, None);
    let _gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let id = rig.engine.submit(rig.request(tracking())).await.unwrap();
    rig.wait_moving(&mut events, &id).await;
    rig.engine.kill().await;
    rig.finish(&id).await;

    let refused = rig.engine.submit(rig.request(tracking())).await;
    let ladder = rig
        .engine
        .submit_ladder(rig.ladder(tracking(), "velocity_p", &[841.0], 1))
        .await;
    rig.fake.push("notify_klippy_shutdown", json!([]));
    let deadline = Instant::now() + WAIT;
    while rig.run(&id).state != RunState::Killed {
        assert!(
            Instant::now() < deadline,
            "shutdown did not confirm the kill"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    rig.fake.push("notify_klippy_ready", json!([]));
    let mut state = rig.client.state();
    state
        .wait_for(|s| s.klippy == KlippyState::Ready)
        .await
        .unwrap();
    rig.a.sim(|s| s.gate = None);
    let next = rig.engine.submit(rig.request(tracking())).await;

    assert_eq!(refused, Err(Precondition::KillUnconfirmed));
    assert_eq!(
        ladder,
        Err(LadderRejected::Precondition(Precondition::KillUnconfirmed))
    );
    assert!(next.is_ok(), "{next:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_klipper_restart_cycle_clears_an_unconfirmed_kill() {
    let rig = rig().await;
    let id = rig.unconfirmed_kill().await;
    assert_eq!(
        rig.engine.submit(rig.request(tracking())).await,
        Err(Precondition::KillUnconfirmed)
    );

    rig.fake.push("notify_klippy_disconnected", json!([]));
    rig.fake.push("notify_klippy_ready", json!([]));
    let mut state = rig.client.state();
    state
        .wait_for(|s| s.klippy == KlippyState::Ready)
        .await
        .unwrap();
    let deadline = Instant::now() + WAIT;
    let next = loop {
        match rig.engine.submit(rig.request(tracking())).await {
            Err(Precondition::KillUnconfirmed | Precondition::KlippyNotReady)
                if Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            other => break other,
        }
    };

    assert!(next.is_ok(), "{next:?}");
    assert_eq!(rig.run(&id).state, RunState::KillUnconfirmed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_websocket_reconnect_does_not_clear_an_unconfirmed_kill() {
    let rig = rig().await;
    rig.unconfirmed_kill().await;

    rig.fake.drop_websockets();
    let mut state = rig.client.state();
    state
        .wait_for(|s| s.klippy == KlippyState::Disconnected)
        .await
        .unwrap();
    state
        .wait_for(|s| s.klippy == KlippyState::Ready)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(
        rig.engine.submit(rig.request(tracking())).await,
        Err(Precondition::KillUnconfirmed)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revert_is_refused_while_a_kill_is_unconfirmed() {
    let rig = rig().await;
    rig.unconfirmed_kill().await;
    let sent = rig.scripts().len();

    let reverted = rig.engine.revert(&rig.session).await;

    assert_eq!(reverted, Err(Precondition::KillUnconfirmed.to_string()));
    assert_eq!(rig.scripts().len(), sent);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kill_left_unconfirmed_by_a_crash_gates_the_next_engine() {
    let rig = rig().await;
    let id = rig.engine.submit(rig.request(tracking())).await.unwrap();
    let mut killing = rig.finish(&id).await;
    killing.id = RunId::parse("20261030T120002Z-kill").unwrap();
    killing.state = RunState::Killing;
    rig.store.write_run(&rig.session, &killing).unwrap();

    let restarted = Engine::start(
        rig.client.clone(),
        rig.live.clone(),
        rig.frames.clone(),
        rig.store.clone(),
        fast_config(),
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let refused = restarted.submit(rig.request(tracking())).await;
    rig.fake.push("notify_klippy_shutdown", json!([]));
    let deadline = Instant::now() + WAIT;
    while rig.run(&killing.id).state != RunState::Killed {
        assert!(
            Instant::now() < deadline,
            "shutdown did not confirm the kill"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(refused, Err(Precondition::KillUnconfirmed));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kill_while_a_submit_is_checking_keeps_the_queue_empty() {
    let rig = rig().await;
    rig.fake.delay_settings(Duration::from_millis(200));
    rig.fake
        .script_emergency_stop(None, Duration::ZERO, Some(Duration::ZERO));

    let kill = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        rig.engine.kill().await;
    };
    let (submitted, ()) = tokio::join!(rig.engine.submit(rig.request(tracking())), kill);

    assert_eq!(submitted, Err(Precondition::KillPressed));
    assert!(rig.store.scan().unwrap()[0].runs.is_empty());
    assert!(rig.engine.is_idle());
    assert_eq!(rig.stimuli(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stop_request_and_the_shutdown_report_share_one_deadline() {
    let rig = rig().await;
    rig.fake.script_emergency_stop(
        None,
        Duration::from_millis(200),
        Some(Duration::from_millis(200)),
    );
    let _gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let id = rig.engine.submit(rig.request(tracking())).await.unwrap();
    rig.wait_moving(&mut events, &id).await;

    let started = Instant::now();
    rig.engine.kill().await;
    let elapsed = started.elapsed();
    let at_return = rig.run(&id).state;

    assert_eq!(at_return, RunState::KillUnconfirmed);
    assert!(elapsed < 2 * fast_config().kill_timeout, "{elapsed:?}");
    let deadline = Instant::now() + WAIT;
    while rig.run(&id).state != RunState::Killed {
        assert!(
            Instant::now() < deadline,
            "late shutdown did not confirm the kill"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_emergency_stop_that_times_out_is_kill_unconfirmed() {
    let rig = rig().await;
    rig.fake
        .script_emergency_stop(None, Duration::from_secs(2), Some(Duration::ZERO));
    let _gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let id = rig.engine.submit(rig.request(tracking())).await.unwrap();
    rig.wait_moving(&mut events, &id).await;

    rig.engine.kill().await;

    assert_eq!(rig.run(&id).state, RunState::KillUnconfirmed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abort_during_the_first_of_thirty_runs_finishes_it_and_aborts_the_rest() {
    let rig = rig().await;
    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let values: Vec<f64> = (0..10).map(|k| 841.0 + 100.0 * f64::from(k)).collect();
    let ids = rig
        .engine
        .submit_ladder(rig.ladder(tracking(), "velocity_p", &values, 3))
        .await
        .unwrap();
    assert_eq!(ids.len(), 30);
    rig.wait_moving(&mut events, &ids[0]).await;

    rig.engine.abort().await;
    gate.notify_one();

    assert_eq!(rig.finish(&ids[0]).await.state, RunState::Done);
    for id in &ids[1..] {
        assert_eq!(rig.run(id).state, RunState::Aborted);
    }
    assert_eq!(rig.stimuli(), 1);
    let listed = rig.store.scan().unwrap();
    assert_eq!(listed[0].runs.len(), 30);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn losing_moonraker_mid_run_fails_it_and_empties_the_queue() {
    let rig = rig().await;
    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let ids = rig
        .engine
        .submit_ladder(rig.ladder(tracking(), "velocity_p", &[841.0, 941.0], 1))
        .await
        .unwrap();
    rig.wait_moving(&mut events, &ids[0]).await;

    rig.fake.refuse_websockets(true);
    rig.fake.drop_websockets();
    let first = rig.finish(&ids[0]).await;
    gate.notify_one();

    assert_eq!(first.state, failed("moonraker connection lost"));
    assert!(matches!(rig.run(&ids[1]).state, RunState::Skipped(_)));
    assert_eq!(rig.stimuli(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn losing_the_trace_interface_mid_run_fails_it() {
    let rig = rig().await;
    let gate = rig.hold_stimulus();
    let mut events = rig.engine.events();
    let id = rig.engine.submit(rig.request(tracking())).await.unwrap();
    rig.wait_moving(&mut events, &id).await;

    rig.a.unplug();
    let run = rig.finish(&id).await;
    gate.notify_one();

    assert_eq!(run.state, failed("trace interface lost"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lagging_capture_receiver_fails_the_run() {
    let rig = rig_with(Options {
        frames_capacity: 1024,
        ..Options::default()
    })
    .await;
    rig.a.sim(|s| s.profile.burst = 200_000);

    let run = rig.run_once(tracking()).await;

    assert!(
        matches!(&run.state, RunState::Failed(r) if r.contains("lost")),
        "{:?}",
        run.state
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_stimulus_restores_the_feedforward_gain() {
    let rig = rig().await;
    rig.a.sim(|s| {
        s.failures
            .push(("G90".into(), "!! Must home axis first".into()))
    });

    let run = rig.run_once(probe()).await;

    let RunState::Failed(reason) = &run.state else {
        panic!("{:?}", run.state);
    };
    assert!(reason.contains("Must home axis first"), "{reason}");
    let scripts = rig.scripts();
    let stimulus = scripts.iter().position(|s| s.starts_with("G90")).unwrap();
    assert_eq!(
        scripts[stimulus + 1..].to_vec(),
        vec![RESET_FF_LINE.to_owned(), DORMANT_LINE.to_owned()]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_streaming_board_is_left_in_critical_with_a_note() {
    let rig = rig_with(Options {
        initial_hz_a: Some(1000),
        ..Options::default()
    })
    .await;

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, RunState::Done);
    assert!(!rig.scripts().iter().any(|s| s.contains("MODE=dormant")));
    assert!(
        run.notes.iter().any(|n| n.contains("streaming")),
        "{:?}",
        run.notes
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_board_of_unknown_mode_is_left_in_critical_with_a_note() {
    let rig = rig_with(Options {
        initial_hz_a: None,
        ..Options::default()
    })
    .await;
    rig.a.poke();

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, RunState::Done);
    assert!(!rig.scripts().iter().any(|s| s.contains("MODE=dormant")));
    assert!(
        run.notes.iter().any(|n| n.contains("unknown")),
        "{:?}",
        run.notes
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_on_board_a_never_touches_board_b() {
    let rig = rig_with(Options {
        b_echoes: true,
        ..Options::default()
    })
    .await;
    rig.b.sim(|s| s.cutoff_offset = [1; 4]);

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, RunState::Done);
    let modes: Vec<String> = rig
        .scripts()
        .into_iter()
        .filter(|s| s.starts_with("FOCI_TRACE_MODE"))
        .collect();
    assert_eq!(
        modes,
        vec![CRITICAL_LINE.to_owned(), DORMANT_LINE.to_owned()]
    );
    assert_eq!(run.baselines.len(), 2);
    assert!(
        run.baselines
            .iter()
            .all(|b| b.control.velocity_filter_hz == 500)
    );
    assert_eq!(run.stream.unwrap().serial, SERIAL_A);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_boards_matching_frame_does_not_confirm_a_write() {
    let rig = rig_with(Options {
        b_echoes: true,
        ..Options::default()
    })
    .await;
    rig.a.sim(|s| s.confirm_gains = false);

    let run = rig.run_once(tracking()).await;

    assert_eq!(run.state, failed("gain write unconfirmed"));
}

fn by_value(rig: &Rig, ids: &[RunId]) -> Vec<(f64, RunState)> {
    ids.iter()
        .map(|id| {
            let run = rig.run(id);
            (run.request.ladder_value.unwrap(), run.state)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_probe_ladder_stops_after_a_value_below_the_floor() {
    let rig = rig().await;
    rig.a.sim(|s| {
        s.zeta_by_position_p.insert(200, 0.03);
    });

    let ids = rig
        .engine
        .submit_ladder(rig.ladder(probe(), "position_p", &[141.0, 200.0, 240.0], 1))
        .await
        .unwrap();
    rig.finish(&ids[1]).await;

    let runs = by_value(&rig, &ids);
    assert_eq!(runs[0].1, RunState::Done);
    assert_eq!(runs[1].1, RunState::Done);
    assert!(
        matches!(&runs[2].1, RunState::Skipped(r) if r.contains("below the floor")),
        "{:?}",
        runs[2]
    );
    assert_eq!(rig.stimuli(), 2);
    assert!(
        rig.scripts()
            .iter()
            .any(|s| s.contains("POSITION_P=0.78125000"))
    );
    let listed = rig.store.scan().unwrap();
    assert_eq!(listed[0].runs.len(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_probe_ladder_stops_on_an_unassessable_value() {
    let rig = rig().await;
    rig.a.sim(|s| {
        s.edges_by_position_p.insert(200, 12);
    });

    let ids = rig
        .engine
        .submit_ladder(rig.ladder(probe(), "position_p", &[141.0, 200.0, 240.0], 1))
        .await
        .unwrap();
    rig.finish(&ids[1]).await;

    let runs = by_value(&rig, &ids);
    assert_eq!(runs[0].1, RunState::Done);
    assert_eq!(runs[1].1, not_comparable("12 edges"));
    assert!(
        matches!(&runs[2].1, RunState::Skipped(r) if r.contains("damping unassessable")),
        "{:?}",
        runs[2]
    );
    assert_eq!(rig.stimuli(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stop_rule_sees_one_values_repeats_in_an_interleaved_queue() {
    let rig = rig().await;
    rig.a.sim(|s| {
        s.zeta_by_position_p.insert(200, 0.03);
    });

    let ids = rig
        .engine
        .submit_ladder(rig.ladder(probe(), "position_p", &[141.0, 200.0, 240.0], 2))
        .await
        .unwrap();
    rig.finish(&ids[4]).await;

    let runs = by_value(&rig, &ids);
    let values: Vec<f64> = runs.iter().map(|r| r.0).collect();
    assert_eq!(values, vec![141.0, 200.0, 240.0, 141.0, 200.0, 240.0]);
    assert!(runs[..5].iter().all(|r| r.1 == RunState::Done), "{runs:?}");
    assert!(matches!(&runs[5].1, RunState::Skipped(r) if r.contains("below the floor")));
    assert_eq!(rig.stimuli(), 5);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_not_comparable_run_does_not_stop_a_tracking_ladder() {
    let rig = rig().await;
    rig.a.sim(|s| {
        s.profile_by_velocity_p.insert(
            941,
            Profile {
                declared_range: Q,
                ..Profile::default()
            },
        );
    });

    let ids = rig
        .engine
        .submit_ladder(rig.ladder(tracking(), "velocity_p", &[841.0, 941.0, 1041.0], 1))
        .await
        .unwrap();
    rig.finish(&ids[2]).await;

    let states: Vec<RunState> = by_value(&rig, &ids).into_iter().map(|r| r.1).collect();
    assert_eq!(
        states,
        vec![
            RunState::Done,
            not_comparable("channel 0 did not move"),
            RunState::Done
        ]
    );
    assert_eq!(rig.stimuli(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_run_stops_the_ladder_and_skips_the_rest() {
    let rig = rig().await;
    rig.a.sim(|s| s.unconfirmed_velocity_p.push(941));

    let ids = rig
        .engine
        .submit_ladder(rig.ladder(tracking(), "velocity_p", &[841.0, 941.0, 1041.0], 1))
        .await
        .unwrap();
    rig.finish(&ids[1]).await;

    let runs = by_value(&rig, &ids);
    assert_eq!(runs[0].1, RunState::Done);
    assert_eq!(runs[1].1, failed("gain write unconfirmed"));
    let RunState::Skipped(reason) = &runs[2].1 else {
        panic!("{:?}", runs[2]);
    };
    assert!(
        reason.contains("queue stopped") && reason.contains(ids[1].as_str()),
        "{reason}"
    );
    assert_eq!(rig.stimuli(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_typed_value_outside_the_gain_range_rejects_the_ladder() {
    let rig = rig().await;

    let err = rig
        .engine
        .submit_ladder(rig.ladder(probe(), "position_p", &[141.0, 40_000.0], 1))
        .await
        .unwrap_err();

    assert_eq!(
        err,
        LadderRejected::OutOfRange(LadderError::OutOfRange {
            shown: 40_000.0,
            raw: 40_000
        })
    );
    assert!(rig.store.scan().unwrap()[0].runs.is_empty());
    assert!(rig.scripts().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_corner_ladder_converts_with_the_baseline_pwm() {
    let rig = rig().await;

    let ids = rig
        .engine
        .submit_ladder(rig.ladder(tracking(), "velocity_i_corner_hz", &[75.0], 1))
        .await
        .unwrap();
    let run = rig.finish(&ids[0]).await;

    let raw = (75.0 * TAU * 65536.0 / f64::from(PWM_HZ)).round() as i32;
    assert_eq!(run.request.gains.velocity_i, raw);
    assert_eq!(run.request.ladder_value, Some(75.0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revert_writes_the_sessions_starting_gains_through_the_confirmed_path() {
    let rig = rig().await;

    rig.engine.revert(&rig.session).await.unwrap();

    assert_eq!(
        rig.scripts(),
        vec![
            "M400".to_owned(),
            "FOCI_SET_GAINS STEPPER=stepper_x VELOCITY_P=3.51562500 \
             VELOCITY_I=0.009765625000 POSITION_P=0.46875000 POSITION_I=0"
                .to_owned(),
            "FOCI_SET_FILTERS STEPPER=stepper_x VELOCITY_HZ=200 TORQUE_HZ=4000 POSITION_HZ=500 \
             FLUX_HZ=1000"
                .to_owned(),
        ]
    );

    rig.a.sim(|s| s.confirm_filters = [true, false, true, true]);
    assert_eq!(
        rig.engine.revert(&rig.session).await,
        Err("filter write unconfirmed: torque".to_owned())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runs_left_in_flight_by_a_crash_are_reconciled_to_failed() {
    let rig = rig().await;
    let id = rig.engine.submit(rig.request(tracking())).await.unwrap();
    let mut done = rig.finish(&id).await;
    let mut crashed = done.clone();
    crashed.id = RunId::parse("20261030T120000Z-crsh").unwrap();
    crashed.state = RunState::Moving;
    rig.store.write_run(&rig.session, &crashed).unwrap();
    let mut killing = done.clone();
    killing.id = RunId::parse("20261030T120001Z-kill").unwrap();
    killing.state = RunState::Killing;
    rig.store.write_run(&rig.session, &killing).unwrap();
    done.notes.push("kept".into());
    rig.store.write_run(&rig.session, &done).unwrap();

    let restarted = foci_tune::engine::reconcile_interrupted(&rig.store).unwrap();

    assert_eq!(restarted, vec![crashed.id.clone(), killing.id.clone()]);
    assert_eq!(rig.run(&crashed.id).state, failed("interrupted"));
    assert_eq!(rig.run(&killing.id).state, RunState::KillUnconfirmed);
    assert_eq!(rig.run(&id), done);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stepper_table_is_read_once_and_again_after_klipper_reports_ready() {
    let rig = rig().await;
    let before = rig.engine.steppers().await.unwrap();
    rig.engine.steppers().await.unwrap();
    assert_eq!(rig.fake.settings_queries(), 1);
    let mut changed = settings(true);
    changed["stepper_x"]["rotation_distance"] = json!(32.0);
    rig.fake.set_settings(changed);
    assert_eq!(
        rig.engine.steppers().await.unwrap(),
        before,
        "cached until ready"
    );

    rig.fake.push("notify_klippy_disconnected", json!([]));
    rig.fake.push("notify_klippy_ready", json!([]));

    let deadline = Instant::now() + WAIT;
    loop {
        let x = rig.engine.steppers().await.unwrap();
        if x.iter()
            .any(|s| s.name == "stepper_x" && s.motor_rotation_distance_mm == 32.0)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "table not re-read after klipper ready"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_ladders_are_rejected_before_anything_is_queued() {
    let rig = rig().await;
    let values = |n: usize| -> Vec<f64> { (0..n).map(|k| 800.0 + k as f64).collect() };

    let too_many = rig
        .engine
        .submit_ladder(rig.ladder(tracking(), "velocity_p", &values(MAX_LADDER_VALUES + 1), 1))
        .await;
    let too_often = rig
        .engine
        .submit_ladder(rig.ladder(
            tracking(),
            "velocity_p",
            &values(1),
            MAX_LADDER_REPEATS as u32 + 1,
        ))
        .await;
    let too_long = rig
        .engine
        .submit_ladder(rig.ladder(tracking(), "velocity_p", &values(MAX_LADDER_VALUES), 4))
        .await;

    assert_eq!(
        too_many,
        Err(LadderRejected::TooLarge {
            parameter: "values",
            max: MAX_LADDER_VALUES
        })
    );
    assert_eq!(
        too_often,
        Err(LadderRejected::TooLarge {
            parameter: "repeats",
            max: MAX_LADDER_REPEATS
        })
    );
    assert_eq!(
        too_long,
        Err(LadderRejected::TooManyRuns {
            runs: MAX_LADDER_VALUES * 4,
            max: MAX_LADDER_RUNS
        })
    );
    assert!(rig.engine.is_idle());
    assert!(rig.store.scan().unwrap()[0].runs.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn preconditions_list_every_failing_item_and_check_reports_the_first() {
    let rig = rig_with(Options {
        homed: "y",
        electrical_model: false,
        ..Options::default()
    })
    .await;

    let x = rig.engine.preconditions("stepper_x").await;
    let y = rig.engine.preconditions("stepper_y").await;
    let first = rig.engine.check("stepper_x").await;

    assert_eq!(
        x,
        vec![Precondition::NotHomed('x'), Precondition::NoElectricalModel]
    );
    assert!(y.is_empty(), "{y:?}");
    assert_eq!(first, Err(Precondition::NotHomed('x')));
}
