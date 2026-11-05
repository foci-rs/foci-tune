//! Damped-cosine-plus-exponential fit of an ensemble step response.

use core::f64::consts::{FRAC_PI_2, TAU};

use levenberg_marquardt::{LeastSquaresProblem, LevenbergMarquardt};
use nalgebra::storage::Owned;
use nalgebra::{DVector, Dyn, OMatrix, U6, Vector6};

use crate::probe::{FIT_RANGE, PSD_BAND_HZ};

const MAX_EVALUATIONS: usize = 20_000;
const PARAMS: usize = 6;
const SOLVER_TOLERANCE: f64 = 1e-8;
const OSCILLATORY_PARAMS: usize = 4;
const AMPLITUDE_SIGNS: [f64; 2] = [1.0, -1.0];
const START_ZETAS: [f64; 2] = [0.05, 0.1];
const START_HZ: [f64; 3] = [120.0, 150.0, 180.0];
const START_PHASES: [f64; 2] = [0.0, -FRAC_PI_2];
const START_TAU_S: f64 = 0.02;
const START_COUNT: usize = 24;

type Jacobian = OMatrix<f64, Dyn, U6>;

/// Parameters of a successful damped-response fit.
#[derive(Debug, Clone, PartialEq)]
pub struct DampedFit {
    /// Damped oscillation frequency `w sqrt(1 - z^2) / 2 pi` in Hz.
    pub damped_hz: f64,
    /// Damping ratio, strictly inside `(0, 1)`.
    pub zeta: f64,
    /// Amplitude of the damped cosine.
    pub amplitude: f64,
    /// Phase of the damped cosine in radians.
    pub phase: f64,
    /// Amplitude of the exponential term.
    pub offset: f64,
    /// Time constant of the exponential term in seconds.
    pub tau_s: f64,
    /// Root-mean-square residual over the fitted samples.
    pub residual_rms: f64,
    /// Starts, out of the fixed set, that converged to an admissible solution.
    pub admissible_starts: usize,
}

/// Reasons a damped-response fit is rejected.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FitError {
    /// The solver did not converge, or the input was too short or not finite.
    #[error("fit did not converge")]
    NoConvergence,
    /// The fitted damping ratio lies outside `(0, 1)`.
    #[error("fitted damping ratio {0} is outside (0, 1)")]
    ZetaOutOfRange(f64),
    /// The damped frequency lies outside the analysis band.
    #[error("damped frequency {0} Hz is outside 60 to 490 Hz")]
    FrequencyOffBand(f64),
    /// The cosine amplitude is implausibly large for the data.
    #[error("cosine amplitude {0} exceeds twice the ensemble peak")]
    AmplitudeExcessive(f64),
    /// The model parameters are not identifiable from the data.
    #[error("fit Jacobian is singular")]
    SingularJacobian,
}

impl DampedFit {
    /// The fitted model at `t_s` seconds after the edge.
    pub fn value_at(&self, t_s: f64) -> f64 {
        let w = TAU * self.damped_hz / (1.0 - self.zeta * self.zeta).sqrt();
        let p = Vector6::new(
            self.amplitude,
            self.zeta,
            w,
            self.phase,
            self.offset,
            self.tau_s,
        );
        model(&p, t_s)
    }
}

fn model(p: &Vector6<f64>, t: f64) -> f64 {
    let (a, z, w, phi, b, tau) = (p[0], p[1], p[2], p[3], p[4], p[5]);
    let wd = w * (1.0 - z * z).max(1e-6).sqrt();
    a * (-z * w * t).exp() * (wd * t + phi).cos() + b * (-t / tau).exp()
}

struct Problem<'a> {
    t: Vec<f64>,
    y: &'a [f64],
    p: Vector6<f64>,
}

impl Problem<'_> {
    fn residuals_at(&self, p: &Vector6<f64>) -> DVector<f64> {
        DVector::from_iterator(
            self.t.len(),
            self.t.iter().zip(self.y).map(|(&t, &y)| model(p, t) - y),
        )
    }

    fn jacobian_at(&self, p: &Vector6<f64>) -> Jacobian {
        let (a, z, w, phi, b, tau) = (p[0], p[1], p[2], p[3], p[4], p[5]);
        let root = (1.0 - z * z).max(1e-6).sqrt();
        let wd = w * root;
        let d_wd_dz = if 1.0 - z * z > 1e-6 {
            -w * z / root
        } else {
            0.0
        };
        let mut j = Jacobian::zeros(self.t.len());
        for (row, &t) in self.t.iter().enumerate() {
            let envelope = (-z * w * t).exp();
            let (sin, cos) = (wd * t + phi).sin_cos();
            let decay = (-t / tau).exp();
            j[(row, 0)] = envelope * cos;
            j[(row, 1)] = a * envelope * (-w * t * cos - t * d_wd_dz * sin);
            j[(row, 2)] = a * envelope * (-z * t * cos - t * root * sin);
            j[(row, 3)] = -a * envelope * sin;
            j[(row, 4)] = decay;
            j[(row, 5)] = b * decay * t / (tau * tau);
        }
        j
    }
}

impl LeastSquaresProblem<f64, Dyn, U6> for Problem<'_> {
    type ResidualStorage = Owned<f64, Dyn>;
    type JacobianStorage = Owned<f64, Dyn, U6>;
    type ParameterStorage = Owned<f64, U6>;

    fn set_params(&mut self, x: &Vector6<f64>) {
        self.p = *x;
    }

    fn params(&self) -> Vector6<f64> {
        self.p
    }

    fn residuals(&self) -> Option<DVector<f64>> {
        Some(self.residuals_at(&self.p))
    }

    fn jacobian(&self) -> Option<Jacobian> {
        Some(self.jacobian_at(&self.p))
    }
}

/// Rank test, with numpy's default tolerance `s_max * max(m, n) * eps`, of the
/// columns for `A, z, w, phi`.
///
/// The exponential term is a nuisance: it can collapse (`tau` -> 0) and leave `B` and
/// `tau` unidentifiable while the reported oscillation stays well determined.
fn is_singular(j: &Jacobian) -> bool {
    let j = j.columns(0, OSCILLATORY_PARAMS);
    if j.iter().any(|v| !v.is_finite()) {
        return true;
    }
    let s = j.svd(false, false).singular_values;
    let s_max = s.max();
    let tol = s_max * j.nrows().max(j.ncols()) as f64 * f64::EPSILON;
    s_max <= 0.0 || s.iter().any(|&v| v <= tol)
}

enum SolveOutcome {
    Admissible { params: Vector6<f64>, residual: f64 },
    Inadmissible { error: FitError, residual: f64 },
    Failed(FitError),
}

fn starts(peak: f64, b: f64) -> Vec<Vector6<f64>> {
    let mut out = Vec::with_capacity(START_COUNT);
    for sign in AMPLITUDE_SIGNS {
        for zeta in START_ZETAS {
            for hz in START_HZ {
                for phase in START_PHASES {
                    out.push(Vector6::new(
                        sign * peak,
                        zeta,
                        TAU * hz,
                        phase,
                        b,
                        START_TAU_S,
                    ));
                }
            }
        }
    }
    out
}

/// Machine-epsilon stopping tolerances make the solver creep along a flat valley for thousands
/// of evaluations after the fit has settled.
fn solver() -> LevenbergMarquardt<f64> {
    LevenbergMarquardt::new()
        .with_patience(MAX_EVALUATIONS / (PARAMS + 1))
        .with_ftol(SOLVER_TOLERANCE)
        .with_xtol(SOLVER_TOLERANCE)
}

fn solve_from(problem: Problem<'_>, initial: Vector6<f64>, peak: f64) -> SolveOutcome {
    let (problem, report) = solver().minimize(Problem {
        p: initial,
        ..problem
    });
    if !report.termination.was_successful() {
        return SolveOutcome::Failed(FitError::NoConvergence);
    }
    let p = problem.p;
    if p.iter().any(|v| !v.is_finite()) {
        return SolveOutcome::Failed(FitError::NoConvergence);
    }
    if p == initial || is_singular(&problem.jacobian_at(&p)) {
        return SolveOutcome::Failed(FitError::SingularJacobian);
    }
    let r = problem.residuals_at(&p);
    let residual = (r.norm_squared() / r.len() as f64).sqrt();
    let zeta = p[1];
    let damped_hz = p[2] * (1.0 - zeta * zeta).max(0.0).sqrt() / TAU;
    let error = if !(zeta > 0.0 && zeta < 1.0) {
        Some(FitError::ZetaOutOfRange(zeta))
    } else if !(PSD_BAND_HZ.0..=PSD_BAND_HZ.1).contains(&damped_hz) {
        Some(FitError::FrequencyOffBand(damped_hz))
    } else if p[0].abs() > 2.0 * peak {
        Some(FitError::AmplitudeExcessive(p[0]))
    } else {
        None
    };
    match error {
        Some(error) => SolveOutcome::Inadmissible { error, residual },
        None => SolveOutcome::Admissible {
            params: p,
            residual,
        },
    }
}

/// Fits `A exp(-z w t) cos(w sqrt(1 - z^2) t + phi) + B exp(-t / tau)` to
/// `mean[2..55]` sampled every `dt_s` seconds.
///
/// The solver runs from a fixed set of 24 starts; the admissible converged solution
/// with the lowest residual wins. A solution is admissible when `0 < z < 1`, the
/// damped frequency lies in the analysis band and `|A|` is at most twice the peak
/// magnitude of `mean[..55]`.
///
/// # Errors
///
/// [`FitError::NoConvergence`] when the input is shorter than the fit range, holds
/// non-finite samples, or no start converges; [`FitError::SingularJacobian`] when no
/// start yields an admissible solution and some start was not identifiable, including
/// a solve that never leaves its initial guess; otherwise the error of the
/// lowest-residual inadmissible solution ([`FitError::ZetaOutOfRange`],
/// [`FitError::FrequencyOffBand`] or [`FitError::AmplitudeExcessive`]).
pub fn fit_damped(mean: &[f64], dt_s: f64) -> Result<DampedFit, FitError> {
    let (start, end) = FIT_RANGE;
    if mean.len() < end || !dt_s.is_finite() || dt_s <= 0.0 {
        return Err(FitError::NoConvergence);
    }
    let y = &mean[start..end];
    if y.iter().chain([&mean[10]]).any(|v| !v.is_finite()) {
        return Err(FitError::NoConvergence);
    }
    let peak = mean[..end].iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let t: Vec<f64> = (start..end).map(|k| k as f64 * dt_s).collect();

    let mut best: Option<(Vector6<f64>, f64)> = None;
    let mut admissible_starts = 0;
    let mut best_inadmissible: Option<(FitError, f64)> = None;
    let mut singular = false;
    for initial in starts(peak, mean[10]) {
        let problem = Problem {
            t: t.clone(),
            y,
            p: initial,
        };
        match solve_from(problem, initial, peak) {
            SolveOutcome::Admissible { params, residual } => {
                admissible_starts += 1;
                if best.is_none_or(|(_, r)| residual < r) {
                    best = Some((params, residual));
                }
            }
            SolveOutcome::Inadmissible { error, residual } => {
                if best_inadmissible
                    .as_ref()
                    .is_none_or(|(_, r)| residual < *r)
                {
                    best_inadmissible = Some((error, residual));
                }
            }
            SolveOutcome::Failed(error) => singular |= error == FitError::SingularJacobian,
        }
    }
    let Some((p, residual_rms)) = best else {
        return Err(match best_inadmissible {
            Some((error, _)) => error,
            None if singular => FitError::SingularJacobian,
            None => FitError::NoConvergence,
        });
    };
    let zeta = p[1];
    Ok(DampedFit {
        damped_hz: p[2] * (1.0 - zeta * zeta).sqrt() / TAU,
        zeta,
        amplitude: p[0],
        phase: p[3],
        offset: p[4],
        tau_s: p[5],
        residual_rms,
        admissible_starts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f64 = 0.001000023076922929;
    const FLAT_VALLEY: [f64; 60] = [
        -0.04357780869086538,
        0.2236374699703829,
        0.6520958851713741,
        0.32054516476080025,
        -0.010643736742149358,
        -0.23101721378885903,
        -0.23315560667068477,
        -0.12123554285488404,
        0.21615516683231797,
        0.5402467707519574,
        0.2098609463768197,
        -0.12499994811779074,
        -0.33969531567746286,
        -0.12326544782997517,
        -0.007926373659334887,
        -0.024744444358144494,
        0.11063923829127778,
        0.21570315890298275,
        -0.12354121339869771,
        -0.33975356697845344,
        -0.2316739846957628,
        0.10518828357676821,
        0.2166262906457127,
        0.09294441511911676,
        -0.12357365166385913,
        -0.0010435364061388934,
        -0.12373582803284712,
        -0.1165489171304674,
        -0.014878739588061763,
        0.323044566089012,
        0.10561794725820006,
        -0.12569618694302626,
        -0.11929578580405828,
        -0.007554469138032813,
        0.0889008488086088,
        -0.012795898831095718,
        0.10016955673697542,
        0.10888475700269282,
        -0.11407225341087862,
        -0.240459211963808,
        -0.11618447700083373,
        0.20944773800817856,
        -0.016381287950001894,
        0.0011969590458382603,
        -0.12266500280040589,
        -0.12399525834215126,
        -0.003399814563514762,
        -0.12342767160376632,
        -0.008661913552908925,
        0.20901573376404486,
        -0.008508003596251177,
        -0.008114581343101829,
        -0.12342767160376632,
        0.09984668894213339,
        0.10047456952941142,
        -0.010677875132035473,
        -0.028389897042671603,
        0.12141442534997174,
        -0.02073067228160868,
        -0.12131681407372209,
    ];

    #[test]
    fn a_response_with_a_flat_valley_stops_once_the_fit_has_settled() {
        let peak = FLAT_VALLEY[..FIT_RANGE.1]
            .iter()
            .fold(0.0f64, |m, v| m.max(v.abs()));
        let t: Vec<f64> = (FIT_RANGE.0..FIT_RANGE.1).map(|k| k as f64 * DT).collect();
        let y = &FLAT_VALLEY[FIT_RANGE.0..FIT_RANGE.1];

        let worst = starts(peak, FLAT_VALLEY[10])
            .into_iter()
            .map(|initial| {
                let problem = Problem {
                    t: t.clone(),
                    y,
                    p: initial,
                };
                solver().minimize(problem).1.number_of_evaluations
            })
            .max()
            .unwrap();

        assert!(worst < 3_000, "a start used {worst} evaluations");
    }

    #[test]
    fn the_flat_valley_response_fits_to_its_settled_damping() {
        let fit = fit_damped(&FLAT_VALLEY, DT).expect("the response fits");

        assert!((fit.zeta - 0.035772069).abs() < 1e-4, "{}", fit.zeta);
        assert!(
            (fit.damped_hz - 151.207872).abs() < 1e-3,
            "{}",
            fit.damped_hz
        );
    }

    impl Problem<'_> {
        fn jacobian_by_differences(&self, p: &Vector6<f64>) -> Jacobian {
            let mut j = Jacobian::zeros(self.t.len());
            for c in 0..PARAMS {
                let h = f64::EPSILON.cbrt() * p[c].abs().max(1e-3);
                let (mut hi, mut lo) = (*p, *p);
                hi[c] += h;
                lo[c] -= h;
                let column = (self.residuals_at(&hi) - self.residuals_at(&lo)) / (2.0 * h);
                j.set_column(c, &column);
            }
            j
        }
    }

    #[test]
    fn the_analytic_jacobian_matches_central_differences() {
        let t: Vec<f64> = (FIT_RANGE.0..FIT_RANGE.1).map(|k| k as f64 * DT).collect();
        let y = &FLAT_VALLEY[FIT_RANGE.0..FIT_RANGE.1];
        let problem = Problem {
            t,
            y,
            p: Vector6::zeros(),
        };
        let points = [
            Vector6::new(0.3, 0.06, TAU * 150.0, 0.2, 0.05, 0.02),
            Vector6::new(-0.8, 0.4, TAU * 90.0, -1.3, -0.2, 0.005),
            Vector6::new(1.2, 0.01, TAU * 400.0, 3.0, 0.0, 0.05),
            Vector6::new(0.5, 0.9, TAU * 200.0, -0.4, 0.3, 0.3),
        ];

        for p in points {
            let analytic = problem.jacobian_at(&p);
            let numeric = problem.jacobian_by_differences(&p);
            for c in 0..PARAMS {
                let scale = numeric.column(c).amax().max(1e-9);
                let worst = (analytic.column(c) - numeric.column(c)).amax();
                assert!(
                    worst / scale < 1e-5,
                    "column {c} of {p:?}: {worst} against scale {scale}"
                );
            }
        }
    }
}
