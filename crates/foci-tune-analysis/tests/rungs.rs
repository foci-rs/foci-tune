use foci_tune_analysis::ladder::{LadderAxis, PID_GAIN_MAX_RAW};
use foci_tune_analysis::rungs::{
    DEFAULT_LOWER_K, DEFAULT_UPPER_K, RungNote, SEED_CORNER_HZ, SeedRefused, rung_values, seed_from,
};

const PWM_HZ: u32 = 50_000;

fn raws(r: &foci_tune_analysis::rungs::Rungs) -> Vec<i32> {
    r.values.iter().map(|v| v.raw).collect()
}

#[test]
fn seven_third_octave_rungs() {
    let r = rung_values(
        LadderAxis::VelocityP,
        840,
        PWM_HZ,
        DEFAULT_LOWER_K,
        DEFAULT_UPPER_K,
    );

    assert_eq!(raws(&r), [420, 529, 667, 840, 1058, 1333, 1680]);
    assert!(r.notes.is_empty());
}

#[test]
fn seed_from_refuses_zero_and_rounds_to_zero() {
    for axis in [LadderAxis::VelocityP, LadderAxis::PositionP] {
        assert_eq!(seed_from(axis, 0.0, PWM_HZ), Err(SeedRefused::Zero));
        assert_eq!(seed_from(axis, 0.4, PWM_HZ), Err(SeedRefused::RoundsToZero));
        assert_eq!(seed_from(axis, 840.0, PWM_HZ), Ok(840));
    }
    assert_eq!(seed_from(LadderAxis::VelocityICornerHz, 0.0, PWM_HZ), Ok(0));
    assert_eq!(
        seed_from(LadderAxis::VelocityICornerHz, 0.0001, PWM_HZ),
        Err(SeedRefused::RoundsToZero)
    );
}

#[test]
fn seed_from_refuses_out_of_range() {
    assert!(matches!(
        seed_from(LadderAxis::VelocityP, 40000.0, PWM_HZ),
        Err(SeedRefused::OutOfRange(_))
    ));
}

#[test]
fn velocity_i_zero_seed_uses_fifty_hz() {
    let r = rung_values(
        LadderAxis::VelocityICornerHz,
        0,
        PWM_HZ,
        DEFAULT_LOWER_K,
        DEFAULT_UPPER_K,
    );

    let shown: Vec<f64> = r.values.iter().map(|v| v.shown).collect();
    let expected: Vec<f64> = (DEFAULT_LOWER_K..=DEFAULT_UPPER_K)
        .map(|k| SEED_CORNER_HZ * 2f64.powf(f64::from(k) / 3.0))
        .collect();
    assert_eq!(shown.len(), 7);
    for (s, e) in shown.iter().zip(&expected) {
        assert!((s - e).abs() < 1e-9, "{s} vs {e}");
    }
}

#[test]
fn rungs_over_range_are_omitted_and_noted() {
    let r = rung_values(
        LadderAxis::VelocityP,
        30000,
        PWM_HZ,
        DEFAULT_LOWER_K,
        DEFAULT_UPPER_K,
    );

    assert!(r.values.iter().all(|v| v.raw <= PID_GAIN_MAX_RAW));
    assert_eq!(r.values.len(), 4);
    let over = r
        .notes
        .iter()
        .filter(|n| matches!(n, RungNote::OutOfRange(_)))
        .count();
    assert_eq!(over, 3);
}

#[test]
fn rounding_duplicates_merge_and_zero_rungs_are_omitted() {
    let r = rung_values(
        LadderAxis::VelocityP,
        2,
        PWM_HZ,
        DEFAULT_LOWER_K,
        DEFAULT_UPPER_K,
    );
    let got = raws(&r);
    assert!(got.windows(2).all(|w| w[0] < w[1]), "{got:?}");
    assert!(r.notes.iter().any(|n| matches!(n, RungNote::Merged { .. })));

    let r = rung_values(LadderAxis::VelocityP, 1, PWM_HZ, -6, DEFAULT_UPPER_K);
    assert!(r.values.iter().all(|v| v.raw >= 1));
    assert!(
        r.notes
            .iter()
            .any(|n| matches!(n, RungNote::RoundedToZero(_)))
    );
}

#[test]
fn k_range_is_editable() {
    let r = rung_values(LadderAxis::VelocityP, 840, PWM_HZ, -1, 5);

    assert_eq!(r.values.len(), 7);
    let first = r.values[0].shown / 840.0;
    let last = r.values[6].shown / 840.0;
    assert!((first - 0.7937).abs() < 1e-3);
    assert!((last - 3.1748).abs() < 1e-3);
}
