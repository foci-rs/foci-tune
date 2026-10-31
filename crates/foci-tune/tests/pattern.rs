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
            start: (100.5, 100.5),
            end: (200.5, 200.5)
        }
    );
}

#[test]
fn corexy_stepper_y_proposes_the_anti_diagonal() {
    let p = propose_pattern("corexy", "stepper_y", &bed()).unwrap();
    assert_eq!(
        p,
        Pattern {
            start: (100.5, 200.5),
            end: (200.5, 100.5)
        }
    );
}

#[test]
fn cartesian_moves_one_axis_at_bed_centre_of_the_other() {
    let x = propose_pattern("cartesian", "stepper_x", &bed()).unwrap();
    assert_eq!(
        x,
        Pattern {
            start: (100.5, 150.5),
            end: (200.5, 150.5)
        }
    );
    let y = propose_pattern("cartesian", "stepper_y", &bed()).unwrap();
    assert_eq!(
        y,
        Pattern {
            start: (150.5, 100.5),
            end: (150.5, 200.5)
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

#[test]
fn bed_too_small_for_the_span_is_rejected() {
    let small = BedLimits {
        x: (0.0, 110.0),
        y: (0.0, 110.0),
    };
    assert_eq!(
        propose_pattern("corexy", "stepper_x", &small),
        Err(PatternError::OutsideLimits)
    );
}
