//! Damped least squares for the spiral models. QR on scaled columns: the u/v columns of
//! this problem are almost parallel, and normal equations would throw that precision away.
//! The constant-pitch model and `completion_bit` follow discprobe `calib.py` and `geo.py`.

use crate::spiral::{Spiral, SpiralParams};
use std::f64::consts::{PI, TAU};

/// One unwrapped measurement, relative to the reference.
#[derive(Clone, Copy, Debug)]
pub struct PhasePoint {
    pub lba: u32,
    /// Channel bits at the read's completion point.
    pub n: f64,
    pub theta_deg: f64,
    pub sigma_deg: f64,
}

/// Where a DVD read ends; see `Media::completion_bit` for both families.
pub fn completion_bit(lba: u32) -> f64 {
    crate::media::Media::Dvd.completion_bit(lba)
}

/// Solve min ||A x - b|| by Householder QR. Returns x and the upper-triangular R.
fn qr_solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> (Vec<f64>, Vec<Vec<f64>>) {
    let (m, n) = (a.len(), a[0].len());
    for k in 0..n {
        let norm = (k..m).map(|i| a[i][k] * a[i][k]).sum::<f64>().sqrt();
        if norm == 0.0 {
            continue;
        }
        let alpha = if a[k][k] > 0.0 { -norm } else { norm };
        let mut v: Vec<f64> = (k..m).map(|i| a[i][k]).collect();
        v[0] -= alpha;
        let vv: f64 = v.iter().map(|x| x * x).sum();
        if vv == 0.0 {
            continue;
        }
        for j in k..n {
            let d: f64 = 2.0 * v.iter().enumerate().map(|(i, x)| x * a[k + i][j]).sum::<f64>() / vv;
            for (i, x) in v.iter().enumerate() {
                a[k + i][j] -= d * x;
            }
        }
        let d: f64 = 2.0 * v.iter().enumerate().map(|(i, x)| x * b[k + i]).sum::<f64>() / vv;
        for (i, x) in v.iter().enumerate() {
            b[k + i] -= d * x;
        }
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let s: f64 = (i + 1..n).map(|j| a[i][j] * x[j]).sum();
        x[i] = if a[i][i].abs() < 1e-300 { 0.0 } else { (b[i] - s) / a[i][i] };
    }
    let r = (0..n).map(|i| a[i][..n].to_vec()).collect();
    (x, r)
}

/// (R^T R)^-1 from an upper-triangular R.
fn inverse_rtr(r: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = r.len();
    let mut inv = vec![vec![0.0; n]; n];
    for c in 0..n {
        for i in (0..=c).rev() {
            let e = if i == c { 1.0 } else { 0.0 };
            let s: f64 = (i + 1..=c).map(|j| r[i][j] * inv[j][c]).sum();
            inv[i][c] = (e - s) / r[i][i];
        }
    }
    let mut out = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            out[i][j] = (0..n).map(|k| inv[i][k] * inv[j][k]).sum();
        }
    }
    out
}

struct LmResult {
    x: Vec<f64>,
    cov: Vec<Vec<f64>>,
}

fn levenberg(
    x0: Vec<f64>,
    residual: &dyn Fn(&[f64]) -> Vec<f64>,
    jacobian: &dyn Fn(&[f64], &[f64]) -> Vec<Vec<f64>>,
) -> LmResult {
    let cost = |r: &[f64]| r.iter().map(|v| v * v).sum::<f64>();
    let n = x0.len();
    let mut x = x0;
    let mut r = residual(&x);
    let mut c = cost(&r);
    let mut mu: f64 = 1e-8;
    let mut last_rt = None;
    for _ in 0..200 {
        let j = jacobian(&x, &r);
        let scale: Vec<f64> = (0..n)
            .map(|k| j.iter().map(|row| row[k] * row[k]).sum::<f64>().sqrt().max(1e-300))
            .collect();
        let mut improved = false;
        while mu < 1e12 {
            let mut a: Vec<Vec<f64>> =
                j.iter().map(|row| row.iter().zip(&scale).map(|(v, s)| v / s).collect()).collect();
            let mut b: Vec<f64> = r.iter().map(|v| -v).collect();
            for k in 0..n {
                let mut row = vec![0.0; n];
                row[k] = mu.sqrt();
                a.push(row);
                b.push(0.0);
            }
            let (y, _) = qr_solve(a, b);
            let trial: Vec<f64> = x.iter().zip(y.iter().zip(&scale)).map(|(v, (d, s))| v + d / s).collect();
            let tr = residual(&trial);
            let tc = cost(&tr);
            if tc.is_finite() && tc < c {
                let small = (c - tc) <= 1e-12 * c;
                x = trial;
                r = tr;
                c = tc;
                mu = (mu * 0.2).max(1e-14);
                improved = !small;
                break;
            }
            mu *= 8.0;
        }
        let a: Vec<Vec<f64>> = j.iter().map(|row| row.iter().zip(&scale).map(|(v, s)| v / s).collect()).collect();
        last_rt = Some((a, scale));
        if !improved {
            break;
        }
    }
    let (a, scale) = last_rt.unwrap();
    let (_, rm) = qr_solve(a, vec![0.0; r.len()]);
    let mut cov = inverse_rtr(&rm);
    for i in 0..n {
        for j in 0..n {
            cov[i][j] /= scale[i] * scale[j];
        }
    }
    LmResult { x, cov }
}

/// Constant-pitch model in bit units: u = r0 / b, v = p / b.
#[inline]
pub fn theta_const_deg(n: f64, u: f64, v: f64) -> f64 {
    (TAU / v * ((u * u + v * n / PI).sqrt() - u)).to_degrees()
}

fn grad_const(n: f64, u: f64, v: f64) -> (f64, f64) {
    let s = (u * u + v * n / PI).sqrt();
    let du = TAU / v * (u / s - 1.0);
    let dv = -TAU / (v * v) * (s - u) + TAU / v * n / (TAU * s);
    (du.to_degrees(), dv.to_degrees())
}

#[derive(Clone, Copy, Debug)]
pub struct Prior {
    pub value: f64,
    pub sigma: f64,
}

#[derive(Clone, Debug)]
pub struct ConstFit {
    pub u: f64,
    pub v: f64,
    pub c_deg: f64,
    pub n_ref: f64,
    pub rms_deg: f64,
    pub chi2_per_dof: f64,
    pub cov: Vec<Vec<f64>>,
}

impl ConstFit {
    pub fn predict_deg(&self, n: f64) -> f64 {
        theta_const_deg(n, self.u, self.v) - theta_const_deg(self.n_ref, self.u, self.v) + self.c_deg
    }

    /// 1-sigma prediction uncertainty at `n`.
    pub fn predict_sigma_deg(&self, n: f64) -> f64 {
        let (a, b) = grad_const(n, self.u, self.v);
        let (e, f) = grad_const(self.n_ref, self.u, self.v);
        let g = [a - e, b - f, 1.0];
        let mut s = 0.0;
        for i in 0..3 {
            for j in 0..3 {
                s += g[i] * self.cov[i][j] * g[j];
            }
        }
        s.max(0.0).sqrt()
    }
}

/// Fit u, v and the constant offset. `priors` hold u and v in place while only the local
/// slope has been seen; the earliest rungs cannot separate them.
pub fn fit_constant(
    points: &[PhasePoint],
    n_ref: f64,
    init: (f64, f64),
    priors: Option<(Prior, Prior)>,
) -> Result<ConstFit, String> {
    if points.is_empty() || (points.len() < 3 && priors.is_none()) {
        return Err("点数不足，无法拟合".into());
    }
    let residual = |x: &[f64]| -> Vec<f64> {
        let base = theta_const_deg(n_ref, x[0], x[1]);
        let mut r: Vec<f64> = points
            .iter()
            .map(|p| (theta_const_deg(p.n, x[0], x[1]) - base + x[2] - p.theta_deg) / p.sigma_deg)
            .collect();
        if let Some((pu, pv)) = priors {
            r.push((x[0] - pu.value) / pu.sigma);
            r.push((x[1] - pv.value) / pv.sigma);
        }
        r
    };
    let jacobian = |x: &[f64], _: &[f64]| -> Vec<Vec<f64>> {
        let (bu, bv) = grad_const(n_ref, x[0], x[1]);
        let mut j: Vec<Vec<f64>> = points
            .iter()
            .map(|p| {
                let (a, b) = grad_const(p.n, x[0], x[1]);
                vec![(a - bu) / p.sigma_deg, (b - bv) / p.sigma_deg, 1.0 / p.sigma_deg]
            })
            .collect();
        if let Some((pu, pv)) = priors {
            j.push(vec![1.0 / pu.sigma, 0.0, 0.0]);
            j.push(vec![0.0, 1.0 / pv.sigma, 0.0]);
        }
        j
    };
    let LmResult { x, cov } = levenberg(vec![init.0, init.1, 0.0], &residual, &jacobian);
    let r = residual(&x);
    let chi2: f64 = r.iter().take(points.len()).map(|v| v * v).sum();
    let base = theta_const_deg(n_ref, x[0], x[1]);
    let rms = (points
        .iter()
        .map(|p| (theta_const_deg(p.n, x[0], x[1]) - base + x[2] - p.theta_deg).powi(2))
        .sum::<f64>()
        / points.len() as f64)
        .sqrt();
    let dof = (points.len() as f64 - 3.0).max(1.0);
    Ok(ConstFit {
        u: x[0],
        v: x[1],
        c_deg: x[2],
        n_ref,
        rms_deg: rms,
        chi2_per_dof: chi2 / dof,
        cov,
    })
}

#[derive(Clone, Debug)]
pub struct CubicFit {
    pub params: SpiralParams,
    pub p0_um: f64,
    pub offset_deg: f64,
    pub rms_deg: f64,
    pub residuals_deg: Vec<f64>,
}

/// Default weight on the shape terms, in deg^2 per (1e-4)^2. Weak: it only stops the edges
/// beyond the measured span from swinging.
pub const CUBIC_LAMBDA: f64 = 1e-3;

fn cubic_grid_angle(p: SpiralParams, n_ref: f64, points: &[PhasePoint]) -> Option<Vec<f64>> {
    let s = Spiral::with_grid(p, 8001).ok()?;
    let base = s.theta_at_bit(n_ref);
    Some(points.iter().map(|q| (s.theta_at_bit(q.n) - base).to_degrees()).collect())
}

/// Hold ri and fit (ro, c1, c2, c3) plus the offset directly on the measured angles.
pub fn fit_cubic(
    points: &[PhasePoint],
    n_ref: f64,
    ri: f64,
    ro0: f64,
    total_sectors: u32,
    sector_bits: u32,
    bit_mm: f64,
    lambda: f64,
) -> Result<CubicFit, String> {
    let make = |x: &[f64]| SpiralParams { total_sectors, sector_bits, bit_mm, ri, ro: x[0], c1: x[1], c2: x[2], c3: x[3] };
    let reg = lambda.sqrt() / 1e-4;
    let residual = |x: &[f64]| -> Vec<f64> {
        let Some(m) = cubic_grid_angle(make(x), n_ref, points) else {
            return vec![f64::INFINITY; points.len() + 3];
        };
        let mut r: Vec<f64> = m.iter().zip(points).map(|(a, q)| a + x[4] - q.theta_deg).collect();
        r.extend(x[1..4].iter().map(|c| c * reg));
        r
    };
    let steps = [1e-7, 1e-7, 1e-7, 1e-7, 1e-2];
    let jacobian = |x: &[f64], _: &[f64]| -> Vec<Vec<f64>> {
        let cols: Vec<(Vec<f64>, Vec<f64>)> = (0..5)
            .map(|k| {
                let (mut a, mut b) = (x.to_vec(), x.to_vec());
                a[k] += steps[k];
                b[k] -= steps[k];
                (residual(&a), residual(&b))
            })
            .collect();
        (0..points.len() + 3)
            .map(|i| (0..5).map(|k| (cols[k].0[i] - cols[k].1[i]) / (2.0 * steps[k])).collect())
            .collect()
    };
    let LmResult { x, .. } = levenberg(vec![ro0, 0.0, 0.0, 0.0, 0.0], &residual, &jacobian);
    let params = make(&x);
    // Report on the full-resolution grid the carver will use.
    let s = Spiral::new(params)?;
    let base = s.theta_at_bit(n_ref);
    let raw: Vec<f64> = points
        .iter()
        .map(|q| (s.theta_at_bit(q.n) - base).to_degrees() - q.theta_deg)
        .collect();
    let off = raw.iter().sum::<f64>() / raw.len() as f64;
    let residuals_deg: Vec<f64> = raw.iter().map(|v| v - off).collect();
    let rms = (residuals_deg.iter().map(|v| v * v).sum::<f64>() / raw.len() as f64).sqrt();
    Ok(CubicFit { params, p0_um: s.p0 * 1000.0, offset_deg: -off, rms_deg: rms, residuals_deg })
}

/// As `fit_cubic`, searching ri as well. The constant-pitch ri is only right for a nearly
/// straight disc. ri and c1 are almost degenerate, so a joint fit does not converge; a
/// one-dimensional scan around the well-behaved four-parameter fit does.
pub fn fit_cubic_scan_ri(
    points: &[PhasePoint],
    n_ref: f64,
    ri0: f64,
    ro0: f64,
    total_sectors: u32,
    sector_bits: u32,
    bit_mm: f64,
    lambda: f64,
) -> Result<CubicFit, String> {
    // Keeps the area, and so the track length, roughly fixed as ri moves.
    let at = |ri: f64| {
        let ro = (ro0 * ro0 + ri * ri - ri0 * ri0).sqrt();
        fit_cubic(points, n_ref, ri, ro, total_sectors, sector_bits, bit_mm, lambda)
    };
    const SPAN: f64 = 0.36;
    const STEPS: i32 = 9;
    let step = SPAN / STEPS as f64;
    let mut best = (ri0, at(ri0)?);
    for k in -STEPS..=STEPS {
        let ri = ri0 + k as f64 * step;
        if k != 0 {
            let f = at(ri)?;
            if f.rms_deg < best.1.rms_deg {
                best = (ri, f);
            }
        }
    }
    let (mut lo, mut hi) = (best.0 - step, best.0 + step);
    for _ in 0..24 {
        let (a, b) = (lo + (hi - lo) * 0.382, lo + (hi - lo) * 0.618);
        let (fa, fb) = (at(a)?, at(b)?);
        if fa.rms_deg < fb.rms_deg {
            hi = b;
            if fa.rms_deg < best.1.rms_deg {
                best = (a, fa);
            }
        } else {
            lo = a;
            if fb.rms_deg < best.1.rms_deg {
                best = (b, fb);
            }
        }
    }
    Ok(best.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_recovers_exact_solution() {
        let a = vec![vec![1.0, 2.0], vec![3.0, 4.0], vec![5.0, 6.5]];
        let b = vec![5.0, 11.0, 18.0];
        let (x, _) = qr_solve(a, b);
        assert!((x[0] - 1.0).abs() < 1e-9 && (x[1] - 2.0).abs() < 1e-9, "{x:?}");
    }

    #[test]
    fn constant_fit_recovers_synthetic_truth() {
        let (u, v) = (181_000.0, 5.55);
        let n_ref = 1_100_000.0 * 38688.0;
        let pts: Vec<PhasePoint> = (0..20)
            .map(|k| {
                let n = (60_000.0 + k as f64 * 100_000.0) * 38688.0;
                PhasePoint {
                    lba: 0,
                    n,
                    theta_deg: theta_const_deg(n, u, v) - theta_const_deg(n_ref, u, v) + 1.5,
                    sigma_deg: 0.3,
                }
            })
            .collect();
        let f = fit_constant(&pts, n_ref, (180_400.0, 5.6), None).unwrap();
        assert!((f.u - u).abs() < 1e-2, "{}", f.u);
        assert!((f.v - v).abs() < 1e-6, "{}", f.v);
        assert!((f.c_deg - 1.5).abs() < 1e-4);
        assert!(f.rms_deg < 1e-3);
    }

    fn real_points() -> Option<(Vec<PhasePoint>, f64)> {
        let root = crate::probe_root()?;
        let t = std::fs::read_to_string(root.join("results/geo-disk8-20261106-211940/targets.csv")).ok()?;
        let mut v: Vec<PhasePoint> = t
            .lines()
            .skip(1)
            .map(|l| {
                let f: Vec<&str> = l.split(',').collect();
                PhasePoint {
                    lba: f[0].parse().unwrap(),
                    n: f[1].parse().unwrap(),
                    theta_deg: f[7].parse().unwrap(),
                    sigma_deg: f[3].parse().unwrap(),
                }
            })
            .collect();
        v.sort_by(|a, b| a.n.total_cmp(&b.n));
        Some((v, completion_bit(1_124_544)))
    }

    /// Fits the measured 56-rung table and must land on the shipped preset's angle map.
    #[test]
    fn real_run_reproduces_golden_preset() {
        let Some((pts, n_ref)) = real_points() else { return };
        assert_eq!(completion_bit(1_124_640), pts.iter().find(|p| p.lba == 1_124_640).unwrap().n);
        let bit = 133.3e-6;
        let c = fit_constant(&pts, n_ref, (24.17 / bit, 0.734e-3 / bit), None).unwrap();
        eprintln!("const u {:.3} v {:.7} c {:.3} rms {:.3}", c.u, c.v, c.c_deg, c.rms_deg);
        assert!((c.u - 181_345.276).abs() < 0.2, "{}", c.u);
        assert!((c.v - 5.5068311).abs() < 2e-5, "{}", c.v);
        assert!((c.rms_deg - 0.715).abs() < 0.02, "{}", c.rms_deg);

        let ri = c.u * bit;
        let bits = 2_295_104f64 * 38688.0;
        let ro0 = bit * (c.u * c.u + c.v * bits / PI).sqrt();
        let f = fit_cubic(&pts, n_ref, ri, ro0, 2_295_104, 38_688, bit, CUBIC_LAMBDA).unwrap();
        let p = f.params;
        eprintln!("cubic ro {:.6} c {:.4e} {:.4e} {:.4e} p0 {:.6} rms {:.3}", p.ro, p.c1, p.c2, p.c3, f.p0_um, f.rms_deg);

        let golden = Spiral::new(crate::preset::reference_preset().spiral_params()).unwrap();
        let mine = Spiral::new(p).unwrap();
        let (gb, mb) = (golden.theta_at_bit(n_ref), mine.theta_at_bit(n_ref));
        let mut worst = 0.0f64;
        for k in 0..=200 {
            let n = bits * (0.02 + 0.94 * k as f64 / 200.0);
            let d = (mine.theta_at_bit(n) - mb - (golden.theta_at_bit(n) - gb)).to_degrees();
            worst = worst.max(d.abs());
        }
        eprintln!("worst angle difference to golden preset, 25.3-56.9 mm: {worst:.3} deg");
        assert!((f.rms_deg - 0.402).abs() < 0.06, "{}", f.rms_deg);
        assert!(worst < 1.0, "{worst}");
    }
}
