//! Damped-cosine-plus-exponential fit of an ensemble step response.

use core::f64::consts::{FRAC_PI_2, TAU};

use levenberg_marquardt::{LeastSquaresProblem, LevenbergMarquardt};
use nalgebra::storage::Owned;
use nalgebra::{DVector, Dyn, OMatrix, U6, Vector6};

use crate::probe::{FIT_RANGE, PSD_BAND_HZ};

const MAX_EVALUATIONS: usize = 20_000;
const PARAMS: usize = 6;
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

fn solve_from(problem: Problem<'_>, initial: Vector6<f64>, peak: f64) -> SolveOutcome {
    let solver = LevenbergMarquardt::new().with_patience(MAX_EVALUATIONS / (PARAMS + 1));
    let (problem, report) = solver.minimize(Problem {
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
    // A is the t = 0 amplitude, so the reference peak includes samples 0 and 1.
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
