//! Damped-cosine-plus-exponential fit of an ensemble step response.

use core::f64::consts::TAU;

use levenberg_marquardt::{LeastSquaresProblem, LevenbergMarquardt};
use nalgebra::storage::Owned;
use nalgebra::{DVector, Dyn, OMatrix, U6, Vector6};

use crate::probe::FIT_RANGE;

const MAX_EVALUATIONS: usize = 20_000;
const PARAMS: usize = 6;
const OSCILLATORY_PARAMS: usize = 4;

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
    /// The model parameters are not identifiable from the data.
    #[error("fit Jacobian is singular")]
    SingularJacobian,
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

/// Fits `A exp(-z w t) cos(w sqrt(1 - z^2) t + phi) + B exp(-t / tau)` to
/// `mean[2..55]` sampled every `dt_s` seconds.
///
/// # Errors
///
/// [`FitError::NoConvergence`] when the input is shorter than the fit range, holds
/// non-finite samples, or the solver fails; [`FitError::SingularJacobian`] when the
/// parameters are not identifiable, including a solve that never leaves the initial
/// guess; [`FitError::ZetaOutOfRange`] when the damping ratio is outside `(0, 1)`.
pub fn fit_damped(mean: &[f64], dt_s: f64) -> Result<DampedFit, FitError> {
    let (start, end) = FIT_RANGE;
    if mean.len() < end || !dt_s.is_finite() || dt_s <= 0.0 {
        return Err(FitError::NoConvergence);
    }
    let y = &mean[start..end];
    if y.iter().chain([&mean[10]]).any(|v| !v.is_finite()) {
        return Err(FitError::NoConvergence);
    }
    let initial = Vector6::new(0.1, 0.1, TAU * 150.0, 0.0, mean[10], 0.02);
    let problem = Problem {
        t: (start..end).map(|k| k as f64 * dt_s).collect(),
        y,
        p: initial,
    };
    let solver = LevenbergMarquardt::new().with_patience(MAX_EVALUATIONS / (PARAMS + 1));
    let (problem, report) = solver.minimize(problem);
    if !report.termination.was_successful() {
        return Err(FitError::NoConvergence);
    }
    let p = problem.p;
    if p.iter().any(|v| !v.is_finite()) {
        return Err(FitError::NoConvergence);
    }
    if p == initial || is_singular(&problem.jacobian_at(&p)) {
        return Err(FitError::SingularJacobian);
    }
    let zeta = p[1];
    if !(zeta > 0.0 && zeta < 1.0) {
        return Err(FitError::ZetaOutOfRange(zeta));
    }
    let r = problem.residuals_at(&p);
    Ok(DampedFit {
        damped_hz: p[2] * (1.0 - zeta * zeta).sqrt() / TAU,
        zeta,
        amplitude: p[0],
        phase: p[3],
        offset: p[4],
        tau_s: p[5],
        residual_rms: (r.norm_squared() / r.len() as f64).sqrt(),
    })
}
