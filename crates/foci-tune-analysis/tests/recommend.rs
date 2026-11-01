use foci_tune_analysis::recommend::{
    Extension, NoRecommendation, Recommendation, Recommended, recommend,
};
use foci_tune_analysis::verdict::{Halt, LadderAssessment, RungMedians, RungVerdict};

fn assessment(scores: &[Option<f64>], halt: Option<(usize, Halt)>) -> LadderAssessment {
    LadderAssessment {
        verdicts: scores
            .iter()
            .map(|&score| RungVerdict {
                score,
                ..RungVerdict::default()
            })
            .collect(),
        medians: scores.iter().map(|_| RungMedians::default()).collect(),
        halt,
    }
}

fn quadratic(gains: &[f64], a: f64, vertex: f64, floor: f64) -> Vec<Option<f64>> {
    gains
        .iter()
        .map(|g| Some(a * (g.ln() - vertex).powi(2) + floor))
        .collect()
}

#[test]
fn vertex_lands_between_neighbours() {
    let gains = [1.0, 2.0, 4.0];
    let vertex = 0.6;
    let a = assessment(&quadratic(&gains, 0.5, vertex, 1.0), None);

    let Recommended {
        choice: Recommendation::Interpolated { shown, between },
        extend,
    } = recommend(&a, &gains)
    else {
        panic!("expected an interpolated recommendation");
    };

    assert!((shown.ln() - vertex).abs() < 1e-6);
    assert_eq!(between, (0, 1));
    assert_eq!(extend, None);
}

#[test]
fn vertex_stays_within_the_neighbour_span() {
    let gains = [1.0, 1.1, 8.0];
    let a = assessment(&quadratic(&gains, 0.5, 0.05, 1.0), None);

    let Recommended {
        choice: Recommendation::Interpolated { shown, .. },
        ..
    } = recommend(&a, &gains)
    else {
        panic!("expected an interpolated recommendation");
    };

    assert!((gains[0]..=gains[2]).contains(&shown));
}

#[test]
fn fewer_than_three_points_returns_best_rung() {
    let gains = [1.0, 2.0, 4.0];
    let a = assessment(&[Some(3.0), Some(2.0), None], None);

    let got = recommend(&a, &gains);

    assert_eq!(got.choice, Recommendation::Measured { rung: 1 });
}

#[test]
fn edge_best_rung_offers_extension_and_does_not_extend() {
    let gains = [1.0, 2.0, 4.0];

    let top = recommend(
        &assessment(&[Some(3.0), Some(2.0), Some(1.0)], None),
        &gains,
    );
    assert_eq!(top.choice, Recommendation::Measured { rung: 2 });
    assert_eq!(top.extend, Some(Extension::Above));

    let halted = recommend(
        &assessment(
            &[Some(3.0), Some(1.0), None],
            Some((2, Halt::DampingUnassessable)),
        ),
        &gains,
    );
    assert_eq!(halted.choice, Recommendation::Measured { rung: 1 });
    assert_eq!(halted.extend, None);

    let bottom = recommend(
        &assessment(&[Some(1.0), Some(2.0), Some(3.0)], None),
        &gains,
    );
    assert_eq!(bottom.choice, Recommendation::Measured { rung: 0 });
    assert_eq!(bottom.extend, Some(Extension::Below));
}

#[test]
fn no_scored_rung_gives_no_recommendation() {
    let gains = [1.0, 2.0];
    let got = recommend(&assessment(&[None, None], None), &gains);

    assert_eq!(
        got.choice,
        Recommendation::None(NoRecommendation::NoScoredRung)
    );
    assert_eq!(got.extend, None);
}

#[test]
fn never_extrapolates() {
    let gains = [1.0, 2.0, 4.0, 8.0];
    let a = assessment(&[Some(1.0), Some(1.2), Some(2.0), Some(5.0)], None);

    let got = recommend(&a, &gains);

    match got.choice {
        Recommendation::Measured { rung } => assert!(rung < gains.len()),
        Recommendation::Interpolated { shown, .. } => {
            assert!((gains[0]..=gains[3]).contains(&shown))
        }
        Recommendation::None(_) => panic!("scored rungs exist"),
    }
}
