use foci_tune::steppers::{BedLimits, derive_steppers, usb_serial_from_path};
use serde_json::{Value, json};

const OUROBOROS_PATH: &str = "/dev/serial/by-id/usb-foci_FOCI_Ouroboros_2600AB12CD34-if00";
const OUROBOROS_SERIAL: &str = "2600AB12CD34";
const OFFBOARD_PATH: &str = "/dev/serial/by-id/usb-foci_FOCI_OpenFFBoard_3300EF56-if00";
const OFFBOARD_SERIAL: &str = "3300EF56";

fn serials(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

fn corexy_settings() -> Value {
    json!({
        "printer": {"kinematics": "corexy"},
        "mcu": {"serial": "/dev/serial/by-id/usb-Klipper_stm32f446xx_host-if00"},
        "mcu ouroboros": {"serial": OUROBOROS_PATH},
        "stepper_x": {
            "step_pin": "ouroboros:STEP0",
            "rotation_distance": 40.0,
            "gear_ratio": "5:1",
            "position_min": -2.0,
            "position_max": 350.0,
        },
        "stepper_y": {
            "step_pin": "ouroboros:STEP1",
            "rotation_distance": 40.0,
            "position_min": 0.0,
            "position_max": 330.0,
        },
        "stepper_z": {"step_pin": "mcu:PF13", "rotation_distance": 8.0, "position_max": 300.0},
        "foci stepper_x": {
            "encoder_ppr": 1000,
            "identified_r_count_milli": 812,
            "pid_position_p": 120,
            "pid_position_i": 3,
            "pid_velocity_p": 900,
            "pid_velocity_i": 40,
            "velocity_filter_hz": 200,
            "torque_filter_hz": 4000,
        },
        "foci stepper_y": {"encoder_ppr": 1000},
    })
}

#[test]
fn usb_serial_is_the_last_token_without_the_if_suffix() {
    assert_eq!(
        usb_serial_from_path(OUROBOROS_PATH).as_deref(),
        Some(OUROBOROS_SERIAL)
    );
    assert_eq!(usb_serial_from_path("/dev/ttyACM0"), None);
}

#[test]
fn corexy_table_lists_the_two_foci_channels() {
    let steppers = derive_steppers(&corexy_settings(), &serials(&[OUROBOROS_SERIAL]));

    assert_eq!(steppers.len(), 2);
    let x = &steppers[0];
    assert_eq!((x.name.as_str(), x.axis, x.channel), ("stepper_x", 'x', 0));
    assert_eq!(x.mcu, "ouroboros");
    assert_eq!(x.serial, OUROBOROS_SERIAL);
    assert_eq!(x.kinematics, "corexy");
    assert_eq!(x.motor_rotation_distance_mm, 8.0);
    assert_eq!((x.position_min, x.position_max), (-2.0, 350.0));
    assert_eq!(x.encoder_ppr, 1000);
    assert!(x.electrical_model_present);
    assert_eq!(x.unusable, None);
    assert_eq!(x.config_gains.position_p, Some(120));
    assert_eq!(x.config_gains.velocity_i, Some(40));
    assert_eq!(x.config_gains.velocity_filter_hz, Some(200));
    assert_eq!(x.config_gains.position_filter_hz, None);

    let y = &steppers[1];
    assert_eq!((y.name.as_str(), y.axis, y.channel), ("stepper_y", 'y', 1));
    assert_eq!(y.motor_rotation_distance_mm, 40.0);
    assert!(!y.electrical_model_present);
    assert_eq!(y.unusable, None);
    assert_eq!(y.config_gains.position_p, None);
}

#[test]
fn steppers_without_a_foci_section_are_absent() {
    let steppers = derive_steppers(&corexy_settings(), &serials(&[OUROBOROS_SERIAL]));

    assert!(steppers.iter().all(|s| s.name != "stepper_z"));
}

#[test]
fn unmatched_serial_marks_the_channel_unusable() {
    let steppers = derive_steppers(&corexy_settings(), &serials(&["DEADBEEF"]));

    assert_eq!(steppers.len(), 2);
    for stepper in &steppers {
        assert_eq!(
            stepper.unusable.as_deref(),
            Some("trace interface 2600AB12CD34 not open")
        );
    }
}

#[test]
fn cartesian_variant_keeps_the_axes_and_records_the_kinematics() {
    let mut settings = corexy_settings();
    settings["printer"]["kinematics"] = json!("cartesian");

    let steppers = derive_steppers(&settings, &serials(&[OUROBOROS_SERIAL]));

    assert_eq!(steppers.len(), 2);
    assert!(steppers.iter().all(|s| s.kinematics == "cartesian"));
    assert_eq!(
        steppers.iter().map(|s| s.axis).collect::<Vec<_>>(),
        vec!['x', 'y']
    );
}

#[test]
fn two_boards_with_four_channels_are_keyed_by_serial_and_channel() {
    let mut settings = corexy_settings();
    settings["mcu offboard"] = json!({"serial": OFFBOARD_PATH});
    settings["stepper_x1"] = json!({
        "step_pin": "offboard:STEP0", "rotation_distance": 40.0, "position_max": 350.0
    });
    settings["stepper_y1"] = json!({
        "step_pin": "offboard:STEP1", "rotation_distance": 40.0, "position_max": 330.0
    });
    settings["foci stepper_x1"] = json!({"encoder_ppr": 4000});
    settings["foci stepper_y1"] = json!({"encoder_ppr": 4000});

    let steppers = derive_steppers(&settings, &serials(&[OUROBOROS_SERIAL, OFFBOARD_SERIAL]));

    let mut keys: Vec<(String, u8)> = steppers
        .iter()
        .map(|s| (s.serial.clone(), s.channel))
        .collect();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), 4);
    assert_eq!(
        keys,
        vec![
            (OUROBOROS_SERIAL.to_owned(), 0),
            (OUROBOROS_SERIAL.to_owned(), 1),
            (OFFBOARD_SERIAL.to_owned(), 0),
            (OFFBOARD_SERIAL.to_owned(), 1),
        ]
    );
    let x1 = steppers.iter().find(|s| s.name == "stepper_x1").unwrap();
    assert_eq!(
        (x1.axis, x1.mcu.as_str(), x1.encoder_ppr),
        ('x', "offboard", 4000)
    );
    assert!(steppers.iter().all(|s| s.unusable.is_none()));
}

#[test]
fn two_steppers_claiming_one_channel_leave_the_second_unusable() {
    let mut settings = corexy_settings();
    settings["stepper_x1"] = json!({
        "step_pin": "ouroboros:STEP0", "rotation_distance": 40.0, "position_max": 350.0
    });
    settings["foci stepper_x1"] = json!({"encoder_ppr": 1000});

    let steppers = derive_steppers(&settings, &serials(&[OUROBOROS_SERIAL]));

    let usable: Vec<_> = steppers.iter().filter(|s| s.unusable.is_none()).collect();
    assert_eq!(usable.len(), 2);
    let duplicate = steppers.iter().find(|s| s.name == "stepper_x1").unwrap();
    assert_eq!(
        duplicate.unusable.as_deref(),
        Some("channel 0 of 2600AB12CD34 is already claimed by stepper_x")
    );
}

#[test]
fn bed_limits_come_from_the_x_and_y_steppers() {
    let steppers = derive_steppers(&corexy_settings(), &serials(&[OUROBOROS_SERIAL]));

    let limits = BedLimits::from_steppers(&steppers).unwrap();

    assert_eq!(limits.x, (-2.0, 350.0));
    assert_eq!(limits.y, (0.0, 330.0));
}
