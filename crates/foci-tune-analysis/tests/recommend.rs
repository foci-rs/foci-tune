use foci_tune_analysis::recommend::{
    Extension, NoRecommendation, Recommendation, Recommended, recommend,
};
use foci_tune_analysis::verdict::{Halt, LadderAssessment, RungMedians, RungVerdict};

fn assessment(
    scores: &[Option<f64>],
    deciding: &[Option<usize>],
    tie: Vec<usize>,
    halt: Option<(usize, Halt)>,
) -> LadderAssessment {
    let best = scores
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.map(|s| (i, s)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i);
    LadderAssessment {
        verdicts: scores
            .iter()
            .zip(deciding)
            .map(|(&score, &deciding_speed)| RungVerdict {
                score,
                deciding_speed,
                ..RungVerdict::default()
            })
            .collect(),
        medians: scores.iter().map(|_| RungMedians::default()).collect(),
        halt,
        pause: None,
        best,
        runner_up: None,
        tie,
        spreads: Vec::new(),
    }
}

fn uniform(scores: &[Option<f64>], halt: Option<(usize, Halt)>) -> LadderAssessment {
    let best = scores
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.map(|s| (i, s)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i);
    assessment(
        scores,
        &vec![Some(0); scores.len()],
        best.into_iter().collect(),
        halt,
    )
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
    let a = uniform(&quadratic(&gains, 0.5, vertex, 1.0), None);

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
    let a = uniform(&quadratic(&gains, 0.5, 0.05, 1.0), None);

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
    let a = uniform(&[Some(3.0), Some(2.0), None], None);

    let got = recommend(&a, &gains);

    assert_eq!(got.choice, Recommendation::Measured { rung: 1 });
}

#[test]
fn edge_best_rung_offers_extension_and_does_not_extend() {
    let gains = [1.0, 2.0, 4.0];

    let top = recommend(&uniform(&[Some(3.0), Some(2.0), Some(1.0)], None), &gains);
    assert_eq!(top.choice, Recommendation::Measured { rung: 2 });
    assert_eq!(top.extend, Some(Extension::Above));

    let halted = recommend(
        &uniform(
            &[Some(3.0), Some(1.0), None],
            Some((2, Halt::DampingUnassessable)),
        ),
        &gains,
    );
    assert_eq!(halted.choice, Recommendation::Measured { rung: 1 });
    assert_eq!(halted.extend, None);

    let bottom = recommend(&uniform(&[Some(1.0), Some(2.0), Some(3.0)], None), &gains);
    assert_eq!(bottom.choice, Recommendation::Measured { rung: 0 });
    assert_eq!(bottom.extend, Some(Extension::Below));
}

#[test]
fn no_scored_rung_gives_no_recommendation() {
    let gains = [1.0, 2.0];
    let got = recommend(&uniform(&[None, None], None), &gains);

    assert_eq!(
        got.choice,
        Recommendation::None(NoRecommendation::NoScoredRung)
    );
    assert_eq!(got.extend, None);
}

#[test]
fn never_extrapolates() {
    let gains = [1.0, 2.0, 4.0, 8.0];
    let a = uniform(&[Some(1.0), Some(1.2), Some(2.0), Some(5.0)], None);

    let got = recommend(&a, &gains);

    match got.choice {
        Recommendation::Measured { rung } => assert!(rung < gains.len()),
        Recommendation::Interpolated { shown, .. } => {
            assert!((gains[0]..=gains[3]).contains(&shown))
        }
        Recommendation::None(_) => panic!("scored rungs exist"),
    }
}

#[test]
fn a_tie_recommends_the_lowest_gain_measured_without_interpolation() {
    let gains = [1.0, 2.0, 4.0];
    let a = assessment(
        &[Some(66.0), Some(65.0), Some(80.0)],
        &[Some(1); 3],
        vec![0, 1],
        None,
    );
    let r = recommend(&a, &gains);
    assert_eq!(r.choice, Recommendation::Measured { rung: 0 });
    assert_eq!(r.extend, None);
}

#[test]
fn refinement_needs_one_deciding_speed_across_the_three_points() {
    let gains = [1.0, 2.0, 4.0];
    let scores = quadratic(&gains, 1.0, 2.0f64.ln() + 0.2, 60.0);
    let same = assessment(&scores, &[Some(1); 3], vec![1], None);
    assert!(matches!(
        recommend(&same, &gains).choice,
        Recommendation::Interpolated { .. }
    ));
    let corner = assessment(&scores, &[Some(1), Some(1), Some(0)], vec![1], None);
    assert_eq!(
        recommend(&corner, &gains).choice,
        Recommendation::Measured { rung: 1 }
    );
}

#[test]
fn the_extension_offer_follows_the_best_rung_even_in_a_tie() {
    let gains = [1.0, 2.0, 4.0];
    let a = assessment(
        &[Some(70.0), Some(65.5), Some(65.0)],
        &[Some(1); 3],
        vec![1, 2],
        None,
    );
    let r = recommend(&a, &gains);
    assert_eq!(r.choice, Recommendation::Measured { rung: 1 });
    assert_eq!(r.extend, Some(Extension::Above));
}
