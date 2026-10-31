use foci_tune::config_block::render;
use foci_tune::model::{Filters, Gains};

const GAINS: Gains = Gains {
    velocity_p: 841,
    velocity_i: 618,
    position_p: 141,
    position_i: 0,
};

const NO_FILTERS: Filters = Filters {
    velocity_hz: 0,
    torque_hz: 0,
    position_hz: 0,
    flux_hz: 0,
};

#[test]
fn renders_raw_values_with_physical_units_in_comments() {
    let block = render("stepper_x", &GAINS, &NO_FILTERS, Some(50_000));

    assert_eq!(
        block,
        "[foci stepper_x]\n\
         pid_velocity_p: 841      # 3.285 Q8.8\n\
         pid_velocity_i: 618      # 75.0 Hz corner at 50 kHz PWM\n\
         pid_position_p: 141      # 0.551 Q8.8\n\
         pid_position_i: 0\n\
         velocity_filter_hz: 0\n\
         torque_filter_hz: 0\n\
         position_filter_hz: 0\n\
         flux_filter_hz: 0\n"
    );
}

#[test]
fn the_corner_follows_the_pwm_frequency_and_is_omitted_without_one() {
    let filters = Filters {
        velocity_hz: 500,
        torque_hz: 1000,
        position_hz: 500,
        flux_hz: 1000,
    };

    let at_25k = render("stepper_y", &GAINS, &filters, Some(25_000));
    let unknown = render("stepper_y", &GAINS, &filters, None);

    assert!(at_25k.starts_with("[foci stepper_y]\n"));
    assert!(
        at_25k.contains("pid_velocity_i: 618      # 37.5 Hz corner at 25 kHz PWM\n"),
        "{at_25k}"
    );
    assert!(at_25k.contains("torque_filter_hz: 1000\n"));
    assert!(unknown.contains("pid_velocity_i: 618\n"), "{unknown}");
}
