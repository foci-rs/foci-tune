use foci_tune::pattern::Pattern;
use foci_tune::stimulus::{REST_OFFSETS_MM, RunKind, compose};

fn diag() -> Pattern {
    Pattern {
        start: (100.5, 100.5),
        end: (200.5, 200.5),
    }
}

fn lines(s: &str) -> Vec<&str> {
    s.lines().collect()
}

fn moves(s: &str) -> Vec<&str> {
    s.lines().filter(|l| l.starts_with("G1 ")).collect()
}

const FF: &str = "FOCI_SET_VELOCITY_FEEDFORWARD STEPPER=stepper_x ENABLE=1 GAIN=";

#[test]
fn tracking_clamps_speeds_and_reports_it() {
    let kind = RunKind::Tracking {
        speeds_mm_s: vec![50.0, 195.0, 600.0],
        strokes: 2,
    };
    let st = compose(&kind, &diag(), "stepper_x", 500.0);
    assert_eq!(st.clamped_speeds, vec![(600.0, 500.0)]);
    assert!(
        st.gcode.contains(" F30000"),
        "600 clamps to 500 mm/s = F30000"
    );
    assert!(!st.gcode.contains("F36000"));
    assert!(st.gcode.contains(" F3000\n") && st.gcode.contains(" F11700"));
}

#[test]
fn tracking_script_opens_and_closes_as_specified() {
    let kind = RunKind::Tracking {
        speeds_mm_s: vec![50.0],
        strokes: 1,
    };
    let g = compose(&kind, &diag(), "stepper_x", 500.0).gcode;
    let l = lines(&g);
    assert_eq!(l[0], "G90");
    assert_eq!(l[1], format!("{FF}1.0"));
    assert_eq!(&l[2..5], ["G1 X100.5 Y100.5 F3000", "M400", "G4 P500"]);
    let n = l.len();
    assert_eq!(
        &l[n - 3..],
        [format!("{FF}1.0"), "M400".to_string(), format!("{FF}1.0")]
    );
}

#[test]
fn one_hundred_mm_stroke_is_twenty_segments_each_way() {
    let kind = RunKind::Tracking {
        speeds_mm_s: vec![50.0],
        strokes: 2,
    };
    let g = compose(&kind, &diag(), "stepper_x", 500.0).gcode;
    let m: Vec<&str> = moves(&g).into_iter().skip(1).collect();
    assert_eq!(m.len(), 40);
    assert_eq!(m[0], "G1 X105.5 Y105.5 F3000");
    assert_eq!(m[19], "G1 X200.5 Y200.5 F3000");
    assert_eq!(m[20], "G1 X195.5 Y195.5 F3000");
    assert_eq!(m[39], "G1 X100.5 Y100.5 F3000");
}

#[test]
fn tracking_runs_slowest_first_with_dwell_between_speeds() {
    let kind = RunKind::Tracking {
        speeds_mm_s: vec![195.0, 50.0],
        strokes: 2,
    };
    let g = compose(&kind, &diag(), "stepper_x", 500.0).gcode;
    let l = lines(&g);
    let first_fast = l.iter().position(|x| x.ends_with("F11700")).unwrap();
    assert!(
        l[..first_fast]
            .iter()
            .any(|x| x.ends_with("F3000") && x.starts_with("G1 X105.5"))
    );
    assert_eq!(l[first_fast - 1], "G4 P500");
}

#[test]
fn probe_toggles_every_third_segment_alternating() {
    let kind = RunKind::Probe {
        speed_mm_s: 195.0,
        high_gain: 1.3,
        segments_between_toggles: 3,
        strokes: 1,
    };
    let g = compose(&kind, &diag(), "stepper_x", 500.0).gcode;
    let l = lines(&g);
    let body = &l[5..l.len() - 3];
    let mut seg = 0;
    let mut toggles = Vec::new();
    for (i, line) in body.iter().enumerate() {
        if line.starts_with("G1 ") {
            seg += 1;
        } else {
            assert_eq!(seg % 3, 0, "toggle after a non-multiple of three segments");
            assert!(i > 0 && body[i - 1].starts_with("G1 "));
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
    let kind = RunKind::Probe {
        speed_mm_s: 195.0,
        high_gain: 1.3,
        segments_between_toggles: 4,
        strokes: 1,
    };
    let g = compose(&kind, &diag(), "stepper_x", 500.0).gcode;
    let l = lines(&g);
    let n = l.len();
    assert_eq!(
        &l[n - 3..],
        [format!("{FF}1.0"), "M400".to_string(), format!("{FF}1.0")]
    );
}

#[test]
fn probe_speed_is_clamped_and_reported() {
    let kind = RunKind::Probe {
        speed_mm_s: 700.0,
        high_gain: 1.3,
        segments_between_toggles: 3,
        strokes: 1,
    };
    let st = compose(&kind, &diag(), "stepper_x", 500.0);
    assert_eq!(st.clamped_speeds, vec![(700.0, 500.0)]);
    assert!(!st.gcode.contains("F42000"));
}

#[test]
fn standstill_emits_projected_rests_with_dwell() {
    let kind = RunKind::Standstill {
        rests: 8,
        dwell_s: 3.0,
    };
    let g = compose(&kind, &diag(), "stepper_x", 500.0).gcode;
    let l = lines(&g);
    assert_eq!(l[0], "G90");
    assert_eq!(l[1], "G1 X100.5 Y100.5 F6000");
    let rests: Vec<&str> = moves(&g).into_iter().skip(1).collect();
    assert_eq!(rests.len(), 8);
    let k = std::f64::consts::FRAC_1_SQRT_2;
    for (line, off) in rests.iter().zip(REST_OFFSETS_MM) {
        let p = 100.5 + off * k;
        let c = format!("{p:.5}");
        let c = c.trim_end_matches('0').trim_end_matches('.');
        assert_eq!(*line, format!("G1 X{c} Y{c} F600"), "offset {off}");
    }
    assert_eq!(g.matches("G4 P3000").count(), 8);
    assert_eq!(l[l.len() - 1], "M400");
}
