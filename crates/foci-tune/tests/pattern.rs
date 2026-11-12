use foci_tune::pattern::{Pattern, PatternError, propose_pattern, validate_pattern};
use foci_tune::steppers::BedLimits;

fn bed() -> BedLimits {
    BedLimits {
        x: (0.0, 301.0),
        y: (0.0, 301.0),
    }
}

#[test]
fn corexy_stepper_x_proposes_the_positive_diagonal() {
    let p = propose_pattern("corexy", "stepper_x", &bed()).unwrap();
    assert_eq!(
        p,
        Pattern {
            start: (80.0, 80.0),
            end: (221.0, 221.0)
        }
    );
}

#[test]
fn corexy_stepper_y_proposes_the_anti_diagonal() {
    let p = propose_pattern("corexy", "stepper_y", &bed()).unwrap();
    assert_eq!(
        p,
        Pattern {
            start: (80.0, 221.0),
            end: (221.0, 80.0)
        }
    );
}

#[test]
fn cartesian_moves_one_axis_at_bed_centre_of_the_other() {
    let x = propose_pattern("cartesian", "stepper_x", &bed()).unwrap();
    assert_eq!(
        x,
        Pattern {
            start: (50.5, 150.5),
            end: (250.5, 150.5)
        }
    );
    let y = propose_pattern("cartesian", "stepper_y", &bed()).unwrap();
    assert_eq!(
        y,
        Pattern {
            start: (150.5, 50.5),
            end: (150.5, 250.5)
        }
    );
}

#[test]
fn other_kinematics_and_steppers_are_refused() {
    assert_eq!(
        propose_pattern("delta", "stepper_a", &bed()),
        Err(PatternError::UnsupportedKinematics("delta".into()))
    );
    assert!(matches!(
        propose_pattern("corexy", "stepper_z", &bed()),
        Err(PatternError::UnsupportedStepper(_))
    ));
}

#[test]
fn pattern_ending_inside_the_margin_is_rejected() {
    let p = Pattern {
        start: (195.0, 100.0),
        end: (295.0, 100.0),
    };
    assert_eq!(
        validate_pattern(&p, &bed()),
        Err(PatternError::OutsideLimits)
    );
    let start_low = Pattern {
        start: (5.0, 100.0),
        end: (100.0, 100.0),
    };
    assert_eq!(
        validate_pattern(&start_low, &bed()),
        Err(PatternError::OutsideLimits)
    );
    let ok = Pattern {
        start: (10.0, 100.0),
        end: (291.0, 100.0),
    };
    assert_eq!(validate_pattern(&ok, &bed()), Ok(()));
}

fn square(max: f64) -> BedLimits {
    BedLimits {
        x: (0.0, max),
        y: (0.0, max),
    }
}

#[test]
fn a_120_mm_corexy_bed_proposes_the_full_inner_diagonal() {
    let p = propose_pattern("corexy", "stepper_x", &square(120.0)).unwrap();
    assert_eq!(
        p,
        Pattern {
            start: (10.0, 10.0),
            end: (110.0, 110.0)
        }
    );
}

#[test]
fn a_bed_slightly_under_120_mm_shrinks_the_stroke_to_fit() {
    let p = propose_pattern("corexy", "stepper_x", &square(117.0)).unwrap();
    assert_eq!(
        p,
        Pattern {
            start: (10.0, 10.0),
            end: (107.0, 107.0)
        }
    );
}

#[test]
fn a_small_cartesian_bed_shrinks_along_the_steppers_own_axis() {
    let p = propose_pattern("cartesian", "stepper_x", &square(120.0)).unwrap();
    assert_eq!(
        p,
        Pattern {
            start: (10.0, 60.0),
            end: (110.0, 60.0)
        }
    );
}

#[test]
fn a_bed_too_small_for_the_minimum_stroke_is_refused_with_its_own_reason() {
    assert_eq!(
        propose_pattern("corexy", "stepper_x", &square(50.0)),
        Err(PatternError::BedTooSmall)
    );
    let thin = BedLimits {
        x: (0.0, 300.0),
        y: (0.0, 15.0),
    };
    assert_eq!(
        propose_pattern("cartesian", "stepper_x", &thin),
        Err(PatternError::BedTooSmall)
    );
}

#[test]
fn every_proposal_is_valid_and_at_least_the_minimum_stroke() {
    for kinematics in ["corexy", "cartesian"] {
        for stepper in ["stepper_x", "stepper_y"] {
            for tenths in 600..=3200 {
                let limits = square(f64::from(tenths) / 10.0);
                if let Ok(p) = propose_pattern(kinematics, stepper, &limits) {
                    assert_eq!(
                        validate_pattern(&p, &limits),
                        Ok(()),
                        "{kinematics} {limits:?}"
                    );
                    let stroke = (p.end.0 - p.start.0).hypot(p.end.1 - p.start.1);
                    assert!(stroke >= 50.0 - 1e-9, "{kinematics} {limits:?} {stroke}");
                }
            }
        }
    }
}

#[test]
fn zero_and_sub_millimetre_patterns_are_rejected() {
    let same = Pattern {
        start: (100.0, 100.0),
        end: (100.0, 100.0),
    };
    assert_eq!(
        validate_pattern(&same, &bed()),
        Err(PatternError::ZeroLength)
    );
    let tiny = Pattern {
        start: (100.0, 100.0),
        end: (100.5, 100.0),
    };
    assert_eq!(
        validate_pattern(&tiny, &bed()),
        Err(PatternError::ZeroLength)
    );
    let ok = Pattern {
        start: (100.0, 100.0),
        end: (101.0, 100.0),
    };
    assert_eq!(validate_pattern(&ok, &bed()), Ok(()));
}
