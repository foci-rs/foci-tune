use foci_tune_analysis::probe::{POST, ensemble, locate_edges, probe_ensembles, probe_metrics};
use foci_tune_analysis::signals::ChannelSeries;

struct Gaussian(u64);

impl Gaussian {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn sample(&mut self) -> f64 {
        let (u1, u2) = (self.uniform(), self.uniform());
        (-2.0 * u1.ln()).sqrt() * (core::f64::consts::TAU * u2).cos()
    }
}

fn levels(blocks: &[(i32, usize)]) -> Vec<i32> {
    blocks
        .iter()
        .flat_map(|&(v, n)| std::iter::repeat_n(v, n))
        .collect()
}

#[test]
fn edges_need_flat_windows_on_both_sides() {
    let clean = levels(&[(1000, 100), (1300, 100), (1000, 100)]);
    assert_eq!(locate_edges(&clean), vec![100, 200]);

    let mut drifting = levels(&[(1000, 100), (1300, 100)]);
    for (k, v) in drifting[100..160].iter_mut().enumerate() {
        *v += (k as i32 * 39) / 59;
    }
    assert!(locate_edges(&drifting).is_empty());

    let late = levels(&[(1000, 100), (1300, 50)]);
    assert!(locate_edges(&late).is_empty());
}

#[test]
fn response_is_normalised_by_the_step_and_baselined() {
    let ff = levels(&[(1000, 100), (1300, 100)]);
    let signal: Vec<f64> = (0..200).map(|k| if k < 100 { 0.0 } else { 30.0 }).collect();
    let edges = locate_edges(&ff);
    assert_eq!(edges, vec![100]);

    let e = ensemble(&signal, &ff, &edges);

    assert_eq!(e.edges, 1);
    assert_eq!(e.mean.len(), POST);
    assert!(e.mean.iter().all(|&v| (v - 0.1).abs() < 1e-12));
}

fn noisy_run(edge_count: usize, rng: &mut Gaussian) -> (Vec<f64>, Vec<i32>) {
    let blocks: Vec<(i32, usize)> = (0..=edge_count)
        .map(|k| (if k % 2 == 0 { 1000 } else { 1300 }, 100))
        .collect();
    let ff = levels(&blocks);
    let signal = ff
        .iter()
        .map(|&v| 0.01 * f64::from(v) + rng.sample())
        .collect();
    (signal, ff)
}

#[test]
fn ensemble_stderr_falls_with_edge_count() {
    let mut rng = Gaussian(0x9e37_79b9_7f4a_7c15);
    let (few_signal, few_ff) = noisy_run(4, &mut rng);
    let (many_signal, many_ff) = noisy_run(16, &mut rng);
    let few_edges = locate_edges(&few_ff);
    let many_edges = locate_edges(&many_ff);
    assert_eq!(few_edges.len(), 4);
    assert_eq!(many_edges.len(), 16);

    let few = ensemble(&few_signal, &few_ff, &few_edges);
    let many = ensemble(&many_signal, &many_ff, &many_edges);

    assert_eq!((few.edges, many.edges), (4, 16));
    assert!(few.stderr_at_peak.is_finite() && many.stderr_at_peak.is_finite());
    assert!(
        many.stderr_at_peak < 0.75 * few.stderr_at_peak,
        "16 edges {} vs 4 edges {}",
        many.stderr_at_peak,
        few.stderr_at_peak
    );
}

#[test]
fn empty_and_nan_series_do_not_panic() {
    let empty = probe_metrics(&ChannelSeries::default());
    assert_eq!(empty.edges, 0);
    assert!(empty.velocity.is_err() && empty.torque.is_err() && empty.position_error.is_err());
    assert!(empty.torque_psd_peak_hz.is_nan());

    let ff = levels(&[(1000, 100), (1300, 100), (1000, 100)]);
    let e = ensemble(&vec![f64::NAN; ff.len()], &ff, &locate_edges(&ff));
    assert_eq!(e.edges, 2);
    assert!(e.stderr_at_peak.is_nan());
}

#[test]
fn probe_ensembles_are_the_ensembles_the_metrics_fit() {
    let ff = levels(&[(1000, 100), (1300, 100), (1000, 100)]);
    let n = ff.len();
    let s = ChannelSeries {
        t_s: (0..n).map(|k| k as f64 * 0.002).collect(),
        velocity_mm_s: ff.iter().map(|&v| f64::from(v) * 0.1).collect(),
        torque: ff.iter().map(|&v| v / 10).collect(),
        position_error_um: vec![0.0; n],
        ff_offset: ff.clone(),
        ..ChannelSeries::default()
    };

    let e = probe_ensembles(&s);

    assert!((e.dt_s - 0.002).abs() < 1e-12, "{}", e.dt_s);
    assert_eq!(e.velocity.edges, 2);
    assert_eq!(
        e.velocity,
        ensemble(&s.velocity_mm_s, &ff, &locate_edges(&ff))
    );
    assert!(e.velocity.mean.iter().all(|&v| (v - 0.1).abs() < 1e-12));
    assert_eq!(e.torque.edges, 2);
    assert_eq!(probe_metrics(&s).edges, e.velocity.edges);
}
