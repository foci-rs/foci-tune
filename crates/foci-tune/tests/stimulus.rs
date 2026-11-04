use foci_tune::pattern::{Pattern, PatternError};
use foci_tune::steppers::BedLimits;
use foci_tune::stimulus::{
    MAX_SCRIPT_S, MAX_SPEEDS, MAX_STROKES, MIN_TOGGLE_SPACING_S, REST_OFFSETS_MM, RunKind,
    Stimulus, StimulusError, compose,
};

const FF: &str = "FOCI_SET_VELOCITY_FEEDFORWARD STEPPER=stepper_x ENABLE=1 GAIN=";

fn bed() -> BedLimits {
    BedLimits {
        x: (0.0, 301.0),
        y: (0.0, 301.0),
    }
}

fn diag() -> Pattern {
    Pattern {
        start: (100.5, 100.5),
        end: (200.5, 200.5),
    }
}

fn along_x() -> Pattern {
    Pattern {
        start: (100.5, 150.5),
        end: (200.5, 150.5),
    }
}

fn corexy(kind: &RunKind) -> Stimulus {
    compose(kind, &diag(), "stepper_x", "corexy", &bed(), 500.0).unwrap()
}

fn cartesian(kind: &RunKind) -> Stimulus {
    compose(kind, &along_x(), "stepper_x", "cartesian", &bed(), 500.0).unwrap()
}

fn lines(s: &str) -> Vec<&str> {
    s.lines().collect()
}

fn moves(s: &str) -> Vec<&str> {
    s.lines().filter(|l| l.starts_with("G1 ")).collect()
}

fn tracking(speeds: &[f64], strokes: usize) -> RunKind {
    RunKind::Tracking {
        speeds_mm_s: speeds.to_vec(),
        strokes,
    }
}

fn probe(speed: f64, every: usize) -> RunKind {
    RunKind::Probe {
        speed_mm_s: speed,
        high_gain: 1.3,
        segments_between_toggles: every,
        strokes: 1,
    }
}

#[test]
fn the_predicted_duration_grows_with_the_strokes_at_their_feed() {
    let four = cartesian(&tracking(&[100.0], 4)).predicted_s;
    let eight = cartesian(&tracking(&[100.0], 8)).predicted_s;

    assert!(
        (eight - four - 4.0).abs() < 1e-9,
        "100 mm strokes at 100 mm/s add 1 s each: {four} to {eight}"
    );
}

#[test]
fn the_predicted_duration_covers_a_probe_and_a_standstill_too() {
    let probe_one = cartesian(&probe(100.0, 3)).predicted_s;
    let two_strokes = cartesian(&RunKind::Probe {
        speed_mm_s: 100.0,
        high_gain: 1.3,
        segments_between_toggles: 3,
        strokes: 2,
    })
    .predicted_s;
    assert!((two_strokes - probe_one - 1.0).abs() < 1e-9);

    let rests = cartesian(&RunKind::Standstill {
        rests: 3,
        dwell_s: 5.0,
    })
    .predicted_s;
    assert!(rests >= 15.0, "three 5 s rests take at least 15 s: {rests}");
}

#[test]
fn cartesian_tracking_clamps_to_max_velocity_and_reports_it() {
    let st = cartesian(&tracking(&[50.0, 195.0, 600.0], 2));
    assert_eq!(st.clamped_speeds, vec![(600.0, 500.0)]);
    assert_eq!(st.applied_speeds, vec![50.0, 195.0, 500.0]);
    assert!(st.gcode.contains(" F30000"));
    assert!(!st.gcode.contains("F36000"));
}

#[test]
fn corexy_feed_is_stepper_speed_over_root_two_rounded_to_integer() {
    let st = corexy(&tracking(&[50.0, 195.0], 1));
    assert!(
        st.gcode.contains("G1 X105.5 Y105.5 F2121\n"),
        "50 mm/s -> 3000/sqrt2"
    );
    assert!(
        st.gcode.contains(" F8273\n") || st.gcode.ends_with(" F8273"),
        "195 mm/s"
    );
    assert!(st.clamped_speeds.is_empty());
}

#[test]
fn corexy_clamp_bounds_path_speed_at_max_velocity() {
    let st = corexy(&tracking(&[400.0, 800.0], 1));
    let limit = 500.0 * std::f64::consts::SQRT_2;
    assert_eq!(st.clamped_speeds.len(), 1);
    assert_eq!(st.clamped_speeds[0].0, 800.0);
    assert!((st.clamped_speeds[0].1 - limit).abs() < 1e-9);
    assert_eq!(st.applied_speeds[0], 400.0);
    assert!((st.applied_speeds[1] - limit).abs() < 1e-9);
    assert!(
        st.gcode.contains(" F30000"),
        "path feed is exactly max_velocity"
    );
    assert!(!st.gcode.contains("F33941"));
}

#[test]
fn every_script_opens_with_the_fixed_prologue() {
    let t = corexy(&tracking(&[50.0], 1)).gcode;
    let l = lines(&t);
    assert_eq!(l[0], "G90");
    assert_eq!(l[1], format!("{FF}1.0"));
    assert_eq!(&l[2..5], ["G1 X100.5 Y100.5 F3000", "M400", "G4 P500"]);
    let s = corexy(&RunKind::Standstill {
        rests: 8,
        dwell_s: 3.0,
    })
    .gcode;
    assert_eq!(
        &lines(&s)[..4],
        ["G90", "G1 X100.5 Y100.5 F3000", "M400", "G4 P500"]
    );
}

#[test]
fn tracking_closes_with_gain_restored() {
    let g = corexy(&tracking(&[50.0], 1)).gcode;
    let l = lines(&g);
    let n = l.len();
    assert_eq!(
        &l[n - 3..],
        [format!("{FF}1.0"), "M400".to_string(), format!("{FF}1.0")]
    );
}

#[test]
fn one_hundred_mm_stroke_is_twenty_segments_each_way() {
    let g = corexy(&tracking(&[50.0], 2)).gcode;
    let m: Vec<&str> = moves(&g).into_iter().skip(1).collect();
    assert_eq!(m.len(), 40);
    assert_eq!(m[0], "G1 X105.5 Y105.5 F2121");
    assert_eq!(m[19], "G1 X200.5 Y200.5 F2121");
    assert_eq!(m[20], "G1 X195.5 Y195.5 F2121");
    assert_eq!(m[39], "G1 X100.5 Y100.5 F2121");
}

#[test]
fn tracking_runs_slowest_first_with_dwell_and_keeps_duplicates() {
    let st = cartesian(&tracking(&[195.0, 50.0, 600.0, 700.0], 1));
    assert_eq!(st.applied_speeds, vec![50.0, 195.0, 500.0, 500.0]);
    let l = lines(&st.gcode);
    let first_fast = l.iter().position(|x| x.ends_with("F11700")).unwrap();
    assert_eq!(l[first_fast - 1], "G4 P500");
    assert_eq!(l.iter().filter(|x| **x == "G4 P500").count(), 1 + 3);
}

#[test]
fn probe_toggles_every_third_segment_alternating() {
    let g = corexy(&probe(195.0, 3)).gcode;
    let l = lines(&g);
    let body = &l[5..l.len() - 3];
    let mut seg = 0;
    let mut toggles = Vec::new();
    for (i, line) in body.iter().enumerate() {
        if line.starts_with("G1 ") {
            seg += 1;
        } else {
            assert_eq!(seg % 3, 0);
            assert!(body[i - 1].starts_with("G1 "));
            toggles.push(*line);
        }
    }
    assert_eq!(seg, 20);
    assert_eq!(toggles.len(), 6);
    for (i, t) in toggles.iter().enumerate() {
        let want = if i % 2 == 0 { "1.3" } else { "1.0" };
        assert_eq!(*t, format!("{FF}{want}"));
    }
}

#[test]
fn probe_ends_with_gain_restored_whatever_the_last_toggle() {
    let g = corexy(&probe(195.0, 4)).gcode;
    let l = lines(&g);
    let n = l.len();
    assert_eq!(
        &l[n - 3..],
        [format!("{FF}1.0"), "M400".to_string(), format!("{FF}1.0")]
    );
}

#[test]
fn probe_speed_is_clamped_and_reported() {
    let st = cartesian(&probe(700.0, 10));
    assert_eq!(st.clamped_speeds, vec![(700.0, 500.0)]);
    assert_eq!(st.applied_speeds, vec![500.0]);
    assert!(!st.gcode.contains("F42000"));
}

fn fmt5(v: f64) -> String {
    let c = format!("{v:.5}");
    c.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[test]
fn corexy_standstill_moves_each_axis_half_the_stepper_offset() {
    let g = corexy(&RunKind::Standstill {
        rests: 8,
        dwell_s: 3.0,
    })
    .gcode;
    let rests: Vec<&str> = moves(&g).into_iter().skip(1).collect();
    assert_eq!(rests.len(), 8);
    for (line, off) in rests.iter().zip(REST_OFFSETS_MM) {
        let c = fmt5(100.5 + off / 2.0);
        assert_eq!(*line, format!("G1 X{c} Y{c} F600"), "offset {off}");
    }
    assert_eq!(rests[7], "G1 X100.572 Y100.572 F600");
    assert_eq!(g.matches("G4 P3000").count(), 8);
    assert_eq!(lines(&g).last(), Some(&"M400"));
}

#[test]
fn corexy_anti_diagonal_standstill_signs_follow_the_pattern() {
    let p = Pattern {
        start: (100.5, 200.5),
        end: (200.5, 100.5),
    };
    let st = compose(
        &RunKind::Standstill {
            rests: 8,
            dwell_s: 3.0,
        },
        &p,
        "stepper_y",
        "corexy",
        &bed(),
        500.0,
    )
    .unwrap();
    let m = moves(&st.gcode);
    assert_eq!(m[8], "G1 X100.572 Y200.428 F600");
}

#[test]
fn cartesian_standstill_moves_the_full_offset_along_the_axis() {
    let g = cartesian(&RunKind::Standstill {
        rests: 8,
        dwell_s: 3.0,
    })
    .gcode;
    let m = moves(&g);
    assert_eq!(m[2], "G1 X100.507 Y150.5 F600");
    assert_eq!(m[8], "G1 X100.644 Y150.5 F600");
}

fn err(kind: RunKind) -> StimulusError {
    compose(&kind, &diag(), "stepper_x", "corexy", &bed(), 500.0).unwrap_err()
}

#[test]
fn pattern_outside_limits_and_bad_kinematics_are_refused() {
    let bad = Pattern {
        start: (195.0, 100.0),
        end: (295.0, 100.0),
    };
    let e = compose(
        &tracking(&[50.0], 1),
        &bad,
        "stepper_x",
        "cartesian",
        &bed(),
        500.0,
    );
    assert_eq!(
        e.unwrap_err(),
        StimulusError::Pattern(PatternError::OutsideLimits)
    );
    let e = compose(
        &tracking(&[50.0], 1),
        &diag(),
        "stepper_a",
        "delta",
        &bed(),
        500.0,
    );
    assert!(matches!(
        e.unwrap_err(),
        StimulusError::Pattern(PatternError::UnsupportedKinematics(_))
    ));
}

#[test]
fn invalid_parameters_are_errors_not_corrections() {
    let invalid = |e: StimulusError| matches!(e, StimulusError::InvalidParameter(_));
    assert!(invalid(err(tracking(&[], 1))));
    assert!(invalid(err(tracking(&[f64::NAN], 1))));
    assert!(invalid(err(tracking(&[-5.0], 1))));
    assert!(invalid(err(probe(195.0, 0))));
    assert!(invalid(err(probe(f64::INFINITY, 3))));
    assert!(invalid(err(RunKind::Standstill {
        rests: 0,
        dwell_s: 3.0
    })));
    assert!(invalid(err(RunKind::Standstill {
        rests: 9,
        dwell_s: 3.0
    })));
    assert!(invalid(err(RunKind::Standstill {
        rests: 8,
        dwell_s: -1.0
    })));
    assert!(invalid(err(RunKind::Standstill {
        rests: 8,
        dwell_s: 1.0e9
    })));
    let nan_max = compose(
        &tracking(&[50.0], 1),
        &diag(),
        "stepper_x",
        "corexy",
        &bed(),
        f64::NAN,
    );
    assert!(invalid(nan_max.unwrap_err()));
}

#[test]
fn zero_length_pattern_is_refused_for_every_run_kind() {
    let p = Pattern {
        start: (150.0, 150.0),
        end: (150.0, 150.0),
    };
    let kinds = [
        tracking(&[50.0], 1),
        probe(195.0, 3),
        RunKind::Standstill {
            rests: 8,
            dwell_s: 3.0,
        },
    ];
    for kind in kinds {
        let e = compose(&kind, &p, "stepper_x", "corexy", &bed(), 500.0).unwrap_err();
        assert_eq!(
            e,
            StimulusError::Pattern(PatternError::ZeroLength),
            "{kind:?}"
        );
    }
}

#[test]
fn request_sizes_are_bounded_before_the_script_is_built() {
    let compose_x = |kind: &RunKind| compose(kind, &diag(), "stepper_x", "corexy", &bed(), 500.0);
    let at_limit = tracking(&[400.0; MAX_SPEEDS], MAX_STROKES);

    let strokes = compose_x(&tracking(&[50.0], 1_000_000_000));
    let speeds = compose_x(&tracking(&[50.0; MAX_SPEEDS + 1], 1));
    let probe_strokes = compose_x(&RunKind::Probe {
        speed_mm_s: 195.0,
        high_gain: 1.3,
        segments_between_toggles: 3,
        strokes: MAX_STROKES + 1,
    });

    assert!(compose_x(&at_limit).is_ok());
    assert_eq!(
        strokes,
        Err(StimulusError::TooLarge {
            parameter: "strokes",
            max: MAX_STROKES
        })
    );
    assert_eq!(
        speeds,
        Err(StimulusError::TooLarge {
            parameter: "speeds_mm_s",
            max: MAX_SPEEDS
        })
    );
    assert!(matches!(
        probe_strokes,
        Err(StimulusError::TooLarge {
            parameter: "strokes",
            ..
        })
    ));
}

fn long_x() -> Pattern {
    Pattern {
        start: (20.5, 150.5),
        end: (277.5, 150.5),
    }
}

fn probe_strokes(speed: f64, strokes: usize) -> RunKind {
    RunKind::Probe {
        speed_mm_s: speed,
        high_gain: 1.3,
        segments_between_toggles: 3,
        strokes,
    }
}

#[test]
fn the_predicted_duration_may_reach_the_cap_but_not_pass_it() {
    let at_x = |kind: &RunKind| compose(kind, &long_x(), "stepper_x", "cartesian", &bed(), 500.0);

    // 0.5 s settle plus 28 strokes of 257 mm at 8 mm/s is exactly 900 s.
    let probe_at_cap = at_x(&probe_strokes(8.0, 28));
    let tracking_at_cap = at_x(&tracking(&[8.0], 28));
    let probe_over = at_x(&probe_strokes(7.99, 28));
    let tracking_over = at_x(&tracking(&[400.0, 7.99], 28));

    assert_eq!(MAX_SCRIPT_S, 900.0);
    assert!(probe_at_cap.is_ok(), "{probe_at_cap:?}");
    assert!(tracking_at_cap.is_ok(), "{tracking_at_cap:?}");
    let Err(StimulusError::TooLong {
        parameter,
        predicted_s,
        max_s,
    }) = probe_over
    else {
        panic!("{probe_over:?}");
    };
    assert_eq!((parameter, max_s), ("speed_mm_s", 900.0));
    assert!(predicted_s > 900.0 && predicted_s < 905.0, "{predicted_s}");
    assert!(matches!(
        tracking_over,
        Err(StimulusError::TooLong {
            parameter: "speeds_mm_s",
            ..
        })
    ));
}

#[test]
fn speeds_below_one_mm_s_or_feed_sixty_are_refused() {
    let x = |kind: &RunKind| compose(kind, &along_x(), "stepper_x", "cartesian", &bed(), 500.0);
    let d = |kind: &RunKind| compose(kind, &diag(), "stepper_x", "corexy", &bed(), 500.0);

    assert!(x(&tracking(&[1.0], 1)).is_ok());
    assert!(x(&tracking(&[1.0], 1)).unwrap().gcode.contains(" F60\n"));
    assert!(d(&tracking(&[1.42], 1)).is_ok());
    for (kind, parameter) in [
        (x(&tracking(&[50.0, 0.99], 1)), "speeds_mm_s"),
        (x(&probe_strokes(0.01, 1)), "speed_mm_s"),
        (d(&tracking(&[1.0], 1)), "speeds_mm_s"),
        (d(&probe_strokes(1.4, 1)), "speed_mm_s"),
    ] {
        assert!(
            matches!(kind, Err(StimulusError::TooSlow { parameter: p, .. }) if p == parameter),
            "{kind:?}"
        );
    }
}

#[test]
fn the_default_requests_fit_within_the_duration_cap() {
    let d = |kind: &RunKind| compose(kind, &diag(), "stepper_x", "corexy", &bed(), 500.0);

    assert!(d(&tracking(&[50.0, 195.0, 400.0], 8)).is_ok());
    assert!(d(&probe_strokes(195.0, 8)).is_ok());
    assert!(
        d(&RunKind::Standstill {
            rests: 8,
            dwell_s: 3.0
        })
        .is_ok()
    );
}

fn too_close(result: Result<Stimulus, StimulusError>) -> (f64, usize, String) {
    let e = result.expect_err("toggles too close");
    let text = e.to_string();
    let StimulusError::TogglesTooClose {
        parameter,
        spacing_s,
        min_segments,
        ..
    } = e
    else {
        panic!("{e:?}");
    };
    assert_eq!(parameter, "segments_between_toggles");
    (spacing_s, min_segments, text)
}

#[test]
fn corexy_probe_toggles_closer_than_the_edge_window_are_refused_with_the_minimum() {
    let at = |every| {
        compose(
            &probe(400.0, every),
            &diag(),
            "stepper_x",
            "corexy",
            &bed(),
            500.0,
        )
    };

    let (spacing_s, min_segments, text) = too_close(at(3));

    assert_eq!(MIN_TOGGLE_SPACING_S, 0.1);
    assert!((spacing_s - 0.075).abs() < 1e-9, "{spacing_s}");
    assert_eq!(min_segments, 4);
    assert!(text.contains("75 ms"), "{text}");
    assert!(text.contains("400 mm/s"), "{text}");
    assert!(text.contains("100 ms"), "{text}");
    assert!(text.contains("at least 4 segments"), "{text}");
    assert!(at(4).is_ok(), "{:?}", at(4));
}

#[test]
fn toggle_spacing_exactly_at_the_minimum_is_accepted() {
    let at = |speed| {
        compose(
            &probe(speed, 4),
            &diag(),
            "stepper_x",
            "corexy",
            &bed(),
            500.0,
        )
    };

    let (_, min_segments, _) = too_close(at(401.0));

    assert!(at(400.0).is_ok(), "{:?}", at(400.0));
    assert_eq!(min_segments, 5);
}

#[test]
fn the_default_probe_keeps_its_toggles_far_enough_apart() {
    assert!(corexy(&probe(195.0, 3)).gcode.contains("GAIN=1.3"));
}

#[test]
fn cartesian_toggle_spacing_uses_the_applied_speed() {
    let at = |speed, every| {
        compose(
            &probe(speed, every),
            &along_x(),
            "stepper_x",
            "cartesian",
            &bed(),
            500.0,
        )
    };

    let (spacing_s, min_segments, _) = too_close(at(500.0, 3));
    let clamped = at(700.0, 10);

    assert!((spacing_s - 0.03).abs() < 1e-9, "{spacing_s}");
    assert_eq!(min_segments, 10);
    assert!(at(500.0, 10).is_ok());
    assert_eq!(clamped.unwrap().applied_speeds, vec![500.0]);
}
