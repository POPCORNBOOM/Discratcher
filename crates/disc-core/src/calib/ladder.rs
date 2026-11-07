//! Calibration ladder: measure phases near the reference, then walk outward, each rung's
//! whole-turn count resolved from the model the previous rungs built.
//!
//! Rewritten in Rust from `geo.py` (GPL-3.0-or-later): the measurement flow, settling and
//! most thresholds follow it. Point selection and the cubic fit are our own.
//! - <https://github.com/Ashley-QwQ/disc-probe/blob/main/discprobe/geo.py>
//! - <https://github.com/NaiveTomcat/discprobe-macos/blob/main/discprobe/geo.py>

use super::rayleigh::{circular_stats, estimate_period, phase_deg, refine_period, CircularStats};
use super::solver::*;
use crate::drive::TimedDrive;
use crate::ecma267::SECTOR_CHANNEL_BITS;
use crate::preset::DiscGeometryPreset;
use crate::rng::SplitMix64;
use crate::spiral::Spiral;
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, Ordering};

pub const MIN_SPACING_SECTORS: u32 = 2000;
pub const MIN_COHERENCE: f64 = 0.98;
/// 4x, the one speed the method has been validated at on hardware.
pub const READ_SPEED_KBPS: u16 = 5540;
const BIT_MM: f64 = 133.3e-6;
const NOMINAL_RI: f64 = 24.1;
const NOMINAL_PITCH: f64 = 0.74e-3;
/// A model cannot pick a turn count if its prediction is this unsure.
const SAFE_SIGMA_DEG: f64 = 25.0;
/// The constant-pitch model misses a real disc by a degree or two.
const MODEL_FLOOR_DEG: f64 = 3.0;
const RETRIES: usize = 4;
/// Reads in one settling round; about 1.5 s.
const SETTLE_READS: usize = 16;
/// A cold start took three rounds on the GUD1N, so this leaves plenty of room.
const SETTLE_ROUNDS: usize = 30;
const SETTLED_COHERENCE: f64 = 0.99;
/// Two rounds this close mean the spindle stopped changing.
const PERIOD_TOLERANCE: f64 = 2e-3;
/// Fit rms above this many read-noise units is treated as a misfit.
const MISFIT_FACTOR: f64 = 2.5;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibRungEvent {
    pub rung_index: usize,
    pub total_rungs: usize,
    pub lba: u32,
    pub radius_mm: f64,
    pub phase_deg: f64,
    pub sigma_deg: f64,
    pub unwrapped_turns: i64,
    pub const_pitch_residual_deg: f64,
    pub cubic_pitch_residual_deg: Option<f64>,
    pub spindle_period_ms: f64,
    pub chi2_per_dof: f64,
}

#[derive(Clone, Debug)]
pub struct LadderConfig {
    pub max_targets: usize,
    pub reps: usize,
    pub range_pct: (f64, f64),
    pub seed: u64,
}

impl Default for LadderConfig {
    fn default() -> Self {
        Self { max_targets: 56, reps: 40, range_pct: (0.02, 0.96), seed: 0x1234 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationResult {
    pub preset: DiscGeometryPreset,
    /// Final table sorted by radius, with both residuals filled in.
    pub rungs: Vec<CalibRungEvent>,
    pub constant_rms_deg: f64,
    pub cubic_rms_deg: f64,
    pub spindle_period_ms: f64,
    /// The fit stayed well above read noise: a rung may be a turn out. Do not carve with
    /// this geometry; re-measure.
    pub suspect: bool,
}

struct Session<'a> {
    drive: &'a mut dyn TimedDrive,
    cancel: &'a AtomicBool,
    rng: SplitMix64,
    period_ns: f64,
    total: u32,
    far_lba: u32,
}

/// One measured target: wrapped phase relative to the reference.
pub struct Measurement {
    pub stats: CircularStats,
    pub period_ns: f64,
}

fn align(lba: u32) -> u32 {
    lba / 16 * 16
}

impl Session<'_> {
    fn check(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Relaxed) {
            Err("已取消".into())
        } else {
            Ok(())
        }
    }

    /// Reads finish a fixed whole number of turns apart, which would hide the phase, so
    /// every read starts after a random fraction of a turn.
    fn read(&mut self, lba: u32) -> Result<f64, String> {
        let wait = (self.rng.unit() * self.period_ns) as u64;
        self.drive.host_wait(wait);
        let fua = self.drive.supports_fua();
        if !fua {
            let far = if lba > self.total / 2 { self.far_lba / 4 } else { self.far_lba };
            self.drive.read_timed(far, false)?;
        }
        Ok(self.drive.read_timed(lba, fua)?.t_end_ns)
    }

    fn measure(&mut self, reference: u32, target: u32, reps: usize) -> Result<Measurement, String> {
        for _ in 0..RETRIES {
            self.check()?;
            let (mut refs, mut tgts) = (Vec::with_capacity(reps), Vec::with_capacity(reps));
            for _ in 0..reps {
                self.check()?;
                refs.push(self.read(reference)?);
                tgts.push(self.read(target)?);
            }
            let fit = refine_period(&refs, self.period_ns, 0.05);
            if fit.coherence < MIN_COHERENCE {
                self.settle(reference)?;
                continue;
            }
            self.period_ns = fit.period_ns;
            let phases: Vec<f64> = tgts.iter().map(|t| phase_deg(*t, &fit)).collect();
            return Ok(Measurement { stats: circular_stats(&phases), period_ns: fit.period_ns });
        }
        Err("这个位置读取不稳定，已跳过".into())
    }

    /// Waits for the spindle by reading, never by idling: the GUD1N slows down after
    /// 30–60 s without a read, so a quiet wait ends with the speed it was meant to avoid.
    fn settle(&mut self, lba: u32) -> Result<(), String> {
        // No period known yet: draw waits from one slowest-plausible turn.
        self.period_ns = 60e9 / 1200.0;
        let mut previous: Option<f64> = None;
        for _ in 0..SETTLE_ROUNDS {
            let mut ts = Vec::with_capacity(SETTLE_READS);
            for _ in 0..SETTLE_READS {
                self.check()?;
                ts.push(self.read(lba)?);
            }
            let fit = estimate_period(&ts, 1500.0, 12_000.0);
            if fit.coherence <= SETTLED_COHERENCE {
                previous = None;
                continue;
            }
            if previous.is_some_and(|p| (fit.period_ns / p - 1.0).abs() < PERIOD_TOLERANCE) {
                self.period_ns = fit.period_ns;
                return Ok(());
            }
            previous = Some(fit.period_ns);
        }
        Err("光驱转速一直不稳定，无法标定。请重试，或换一台光驱。".into())
    }
}

fn nominal_uv() -> (f64, f64) {
    (NOMINAL_RI / BIT_MM, NOMINAL_PITCH / BIT_MM)
}

fn priors() -> (Prior, Prior) {
    let (u, v) = nominal_uv();
    (Prior { value: u, sigma: 0.6 / BIT_MM }, Prior { value: v, sigma: 0.02e-3 / BIT_MM })
}

/// Sectors per turn at the reference on a nominal disc.
fn sectors_per_turn_near(lba: u32) -> f64 {
    let (u, v) = nominal_uv();
    let n = completion_bit(lba);
    let r_bits = (u * u + v * n / PI).sqrt();
    2.0 * PI * r_bits / SECTOR_CHANNEL_BITS as f64
}

pub fn run_ladder(
    drive: &mut dyn TimedDrive,
    cfg: &LadderConfig,
    cancel: &AtomicBool,
    on_rung: &mut dyn FnMut(&CalibRungEvent),
    on_status: &mut dyn FnMut(&str),
) -> Result<CalibrationResult, String> {
    let total = drive.capacity()?;
    if total < 500_000 {
        return Err(format!("这张光盘只有 {total} 个扇区，无法标定，先刻入校准镜像"));
    }
    let lo = align((total as f64 * cfg.range_pct.0) as u32);
    let hi = align((total as f64 * cfg.range_pct.1) as u32);
    let reference = align((total as f64 * 0.49) as u32);
    let n_ref = completion_bit(reference);

    on_status("等待光驱转速稳定");
    // Auto speed changes ruin the timing, so a refused speed has to be visible.
    if let Err(e) = drive.set_read_speed(READ_SPEED_KBPS) {
        on_status(&format!("读速没有设上，测量可能不稳定：{e}"));
    }
    let mut s = Session { drive, cancel, rng: SplitMix64(cfg.seed), period_ns: 25e6, total, far_lba: align(total - 2048) };
    for lba in [0, total / 4, total / 2, total / 2 + total / 4] {
        s.check()?;
        s.drive.read_timed(align(lba), false)?;
    }
    s.settle(reference)?;

    let turn = sectors_per_turn_near(reference);
    let near: Vec<u32> = [1.5f64, -1.5, 3.0, -3.0, 6.0, -6.0, 12.0, -12.0, 24.0, -24.0]
        .iter()
        .map(|t| align((reference as f64 + t * turn).round() as u32 + 8))
        .filter(|l| (lo..hi).contains(l) && l.abs_diff(reference) >= 32)
        .collect();

    let mut points: Vec<PhasePoint> = Vec::new();
    let mut rows: Vec<CalibRungEvent> = Vec::new();
    let (mut u, mut v) = nominal_uv();
    let n_grid = (cfg.max_targets * 2).max(40);
    let grid: Vec<u32> = (0..n_grid)
        .map(|i| align(lo + ((hi - 16 - lo) as u64 * i as u64 / (n_grid as u64 - 1)) as u32))
        .collect();
    let mut model: Option<ConstFit> = None;
    let mut rejected: Vec<u32> = Vec::new();

    while rows.len() < cfg.max_targets {
        let lba = if rows.len() < near.len() {
            near[rows.len()]
        } else {
            let Some(l) = pick_next(&grid, &points, &rejected, model.as_ref(), reference) else { break };
            l
        };
        on_status(&format!("第 {} / {} 阶", rows.len() + 1, cfg.max_targets));
        let m = match s.measure(reference, lba, cfg.reps) {
            Ok(m) => m,
            Err(e) if e == "已取消" => return Err(e),
            Err(e) => {
                on_status(&e);
                rejected.push(lba);
                if rejected.len() > cfg.max_targets {
                    return Err("被放弃的阶梯太多，光驱对这张光盘读取不稳定".into());
                }
                continue;
            }
        };
        let n = completion_bit(lba);
        let predicted = match &model {
            Some(f) => f.predict_deg(n) + local_correction(f, &points, n),
            None => theta_const_deg(n, u, v) - theta_const_deg(n_ref, u, v),
        };
        let turns = ((predicted - m.stats.mean_deg) / 360.0).round() as i64;
        let unwrapped = m.stats.mean_deg + 360.0 * turns as f64;
        let point = PhasePoint { lba, n, theta_deg: unwrapped, sigma_deg: m.stats.sigma_deg };

        // Trial fit: a point the model cannot absorb means the turn count is wrong, and
        // keeping it would poison every later rung.
        points.push(point);
        let enough = points.len() >= 8;
        let pri = if enough { None } else { Some(priors()) };
        let fit = fit_constant(&points, n_ref, (u, v), pri)?;
        let residual = fit.predict_deg(n) - unwrapped;
        if residual.abs() > 45.0 && points.len() > 4 {
            points.pop();
            rejected.push(lba);
            on_status("跳过一个不可靠的测量点");
            if rejected.len() > cfg.max_targets {
                return Err("被放弃的阶梯太多，光驱对这张光盘读取不稳定".into());
            }
            continue;
        }
        u = fit.u;
        v = fit.v;
        let radius_mm = BIT_MM * (fit.u * fit.u + fit.v * n / PI).sqrt();
        let row = CalibRungEvent {
            rung_index: rows.len(),
            total_rungs: cfg.max_targets,
            lba,
            radius_mm,
            phase_deg: m.stats.mean_deg,
            sigma_deg: m.stats.sigma_deg,
            unwrapped_turns: turns,
            const_pitch_residual_deg: residual,
            cubic_pitch_residual_deg: None,
            spindle_period_ms: m.period_ns / 1e6,
            chi2_per_dof: fit.chi2_per_dof,
        };
        on_rung(&row);
        rows.push(row);
        model = Some(fit);
    }

    if points.len() < 12 {
        return Err(format!("只测到 {} 阶，不足以拟合", points.len()));
    }
    on_status("拟合三次轨距");
    let fin = finish_fit(&mut points, n_ref, (u, v), total)?;
    let (c, cubic) = (&fin.constant, &fin.cubic);
    let noise = fin.noise_deg;

    rows.sort_by(|a, b| a.radius_mm.total_cmp(&b.radius_mm));
    for (row, (p, res)) in rows.iter_mut().zip(points.iter().zip(&cubic.residuals_deg)) {
        row.const_pitch_residual_deg = c.predict_deg(p.n) - p.theta_deg;
        row.cubic_pitch_residual_deg = Some(*res);
    }
    let spiral = Spiral::new(cubic.params)?;
    let span = (rows[0].radius_mm, rows[rows.len() - 1].radius_mm);
    let p = cubic.params;
    let preset = DiscGeometryPreset {
        id: String::new(),
        name: String::new(),
        drive_model: String::new(),
        media_id: String::new(),
        stamper_code: String::new(),
        brand: String::new(),
        burned_at: String::new(),
        total_sectors: total,
        channel_bit_length_nm: BIT_MM * 1e6,
        inner_radius_mm: p.ri,
        outer_radius_mm: p.ro,
        pitch_linear: p.c1,
        pitch_quadratic: p.c2,
        pitch_cubic: p.c3,
        base_pitch_um: spiral.p0 * 1000.0,
        rms_residual_deg: cubic.rms_deg,
        measured_span_mm: span,
        created_at: String::new(),
    };
    let suspect = is_suspect(cubic.rms_deg, noise);
    Ok(CalibrationResult {
        suspect,
        preset,
        rungs: rows,
        constant_rms_deg: c.rms_deg,
        cubic_rms_deg: cubic.rms_deg,
        spindle_period_ms: s.period_ns / 1e6,
    })
}

pub fn is_suspect(rms_deg: f64, noise_deg: f64) -> bool {
    rms_deg > 3.0 * noise_deg.max(0.5)
}

pub struct FinalFit {
    pub constant: ConstFit,
    pub cubic: CubicFit,
    pub noise_deg: f64,
    pub turn_repairs: usize,
}

/// Fit the finished rung table. `points` are sorted here and may have turns repaired.
pub fn finish_fit(
    points: &mut Vec<PhasePoint>,
    n_ref: f64,
    init: (f64, f64),
    total: u32,
) -> Result<FinalFit, String> {
    points.sort_by(|a, b| a.n.total_cmp(&b.n));
    let c = fit_constant(points, n_ref, init, None)?;
    let ri = c.u * BIT_MM;
    let bits = total as f64 * SECTOR_CHANNEL_BITS as f64;
    let ro0 = BIT_MM * (c.u * c.u + c.v * bits / PI).sqrt();
    let mut cubic = fit_cubic(points, n_ref, ri, ro0, total, BIT_MM, CUBIC_LAMBDA)?;
    // Real rungs carry a little more than their floored standard error, so the bar sits
    // well clear of a healthy disc: only a plain misfit may move ri or touch a turn count.
    let noise = points.iter().map(|p| p.sigma_deg).sum::<f64>() / points.len() as f64;
    let bar = MISFIT_FACTOR * noise.max(0.25);
    let free = |pts: &[PhasePoint], from: &CubicFit| {
        fit_cubic_scan_ri(pts, n_ref, from.params.ri, from.params.ro, total, BIT_MM, CUBIC_LAMBDA)
    };
    if cubic.rms_deg > bar {
        let f = free(points, &cubic)?;
        if f.rms_deg < cubic.rms_deg {
            cubic = f;
        }
    }
    // The fit absorbs a rung that is a full turn out and spreads the error over its
    // neighbours, so the culprit need not even be the largest residual. Try the worst few
    // rungs one turn either way and keep what helps most.
    let mut turn_repairs = 0;
    for _ in 0..8 {
        if cubic.rms_deg <= bar {
            break;
        }
        let mut order: Vec<usize> = (0..points.len()).collect();
        order.sort_by(|a, b| cubic.residuals_deg[*b].abs().total_cmp(&cubic.residuals_deg[*a].abs()));
        let mut best: Option<(usize, f64, CubicFit)> = None;
        for &i in order.iter().take(6) {
            for k in [360.0, -360.0] {
                let mut trial = points.clone();
                trial[i].theta_deg += k;
                let f = free(&trial, &cubic)?;
                if f.rms_deg < 0.7 * cubic.rms_deg && best.as_ref().map_or(true, |b| f.rms_deg < b.2.rms_deg) {
                    best = Some((i, k, f));
                }
            }
        }
        let Some((i, k, f)) = best else { break };
        points[i].theta_deg += k;
        turn_repairs += 1;
        cubic = f;
    }
    Ok(FinalFit { constant: c, cubic, noise_deg: noise, turn_repairs })
}

/// What the constant-pitch model is getting wrong near `n`, read off the two closest
/// rungs. The misfit curve is smooth, so this stays good where the fit itself has drifted
/// by more than half a turn, which is what would miscount a rung on a curved disc.
fn local_correction(fit: &ConstFit, points: &[PhasePoint], n: f64) -> f64 {
    if points.len() < 12 {
        return 0.0;
    }
    let miss = |p: &PhasePoint| p.theta_deg - fit.predict_deg(p.n);
    let left = points.iter().filter(|p| p.n <= n).max_by(|a, b| a.n.total_cmp(&b.n));
    let right = points.iter().filter(|p| p.n >= n).min_by(|a, b| a.n.total_cmp(&b.n));
    let line = |a: &PhasePoint, b: &PhasePoint| miss(a) + (miss(b) - miss(a)) * (n - a.n) / (b.n - a.n);
    match (left, right) {
        (Some(a), Some(b)) if a.n != b.n => line(a, b),
        (Some(a), Some(_)) => miss(a),
        (Some(a), None) | (None, Some(a)) => {
            // Extrapolating: only trust a slope over a baseline as long as the step, or
            // the noise of two close rungs gets multiplied.
            let reach = (n - a.n).abs();
            let side = if n > a.n { -1.0 } else { 1.0 };
            points
                .iter()
                .filter(|p| (p.n - a.n) * side >= reach)
                .min_by(|x, y| (x.n - a.n).abs().total_cmp(&(y.n - a.n).abs()))
                .map_or_else(|| miss(a), |b| line(a, b))
        }
        (None, None) => 0.0,
    }
}

/// Farthest candidate the model can still count turns to; once the whole span is safe,
/// fill the widest remaining gap so rungs end up evenly spread.
fn pick_next(
    grid: &[u32],
    points: &[PhasePoint],
    rejected: &[u32],
    model: Option<&ConstFit>,
    reference: u32,
) -> Option<u32> {
    let model = model?;
    let free: Vec<u32> = grid
        .iter()
        .copied()
        .filter(|l| {
            points.iter().all(|p| p.lba.abs_diff(*l) >= MIN_SPACING_SECTORS) && !rejected.contains(l)
        })
        .collect();
    let scale = model.chi2_per_dof.sqrt().max(1.0);
    let sigma = |l: u32| model.predict_sigma_deg(completion_bit(l)) * scale + MODEL_FLOOR_DEG;
    // Walk into uncertain territory from its edge, in steps that double as the coverage
    // grows, rather than saving the hardest region for one long jump at the end.
    let nearest = |l: u32| points.iter().map(|p| p.lba.abs_diff(l)).min().unwrap_or(u32::MAX);
    let unsure = free.iter().copied().filter(|l| sigma(*l) > SAFE_SIGMA_DEG).min_by_key(|l| nearest(*l));
    if let Some(edge) = unsure {
        let side = |l: u32| (l > reference) == (edge > reference);
        let reach = points.iter().filter(|p| side(p.lba)).map(|p| p.lba.abs_diff(reference)).max().unwrap_or(0);
        let step = nearest(edge).max(reach / 3);
        let target = free
            .iter()
            .copied()
            .filter(|l| side(*l) && l.abs_diff(reference) > reach && l.abs_diff(reference) <= reach + step)
            .max_by_key(|l| l.abs_diff(reference));
        return target.or(Some(edge));
    }
    free.into_iter().max_by_key(|l| points.iter().map(|p| p.lba.abs_diff(*l)).min().unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drive::sim::SimDrive;
    use crate::preset::reference_preset;
    use crate::spiral::SpiralParams;

    fn run(mut d: SimDrive) -> Result<(CalibrationResult, Spiral), String> {
        let truth = Spiral::new(reference_preset().spiral_params()).unwrap();
        let cfg = LadderConfig::default();
        let r = run_ladder(&mut d, &cfg, &AtomicBool::new(false), &mut |_| {}, &mut |_| {})?;
        Ok((r, truth))
    }

    /// Error of a recovered geometry against the true one, as angle relative to the
    /// reference over the measured span. That is what decides whether lines stay straight.
    fn worst_error(truth: &Spiral, p: SpiralParams) -> f64 {
        let m = Spiral::new(p).unwrap();
        let bits = truth.total_bits();
        let nref = completion_bit(1_124_592);
        let (a, b) = (truth.theta_at_bit(nref), m.theta_at_bit(nref));
        (0..=300)
            .map(|k| {
                let n = bits * (0.02 + 0.94 * k as f64 / 300.0);
                ((m.theta_at_bit(n) - b) - (truth.theta_at_bit(n) - a)).to_degrees().abs()
            })
            .fold(0.0, f64::max)
    }

    #[test]
    fn recovers_geometry_on_simulated_disc() {
        let d = SimDrive::new(reference_preset().spiral_params(), 7);
        let (r, truth) = run(d).unwrap();
        assert_eq!(r.rungs.len(), 56);
        let span = r.preset.measured_span_mm;
        assert!(span.0 < 26.0 && span.1 > 56.0, "{span:?}");
        let err = worst_error(&truth, r.preset.spiral_params());
        eprintln!("rms {:.3}, worst {:.3} deg, period {:.3}", r.cubic_rms_deg, err, r.spindle_period_ms);
        assert!(r.cubic_rms_deg < 0.5, "{}", r.cubic_rms_deg);
        assert!(err < 1.0, "{err}");
        assert!((r.spindle_period_ms - 25.474).abs() < 0.01);
    }

    /// Unwrapping is the fragile step: a wrong prior must not silently miscount turns.
    #[test]
    fn survives_discs_far_from_nominal() {
        // Up to about twice the measured curvature. At three times a sweep of seeds got
        // 2 of 24 right, and 6 of the misses were a whole turn off with a clean-looking
        // fit: a turn error spread over the outer rungs fits as curvature, so no residual
        // test catches it.
        for (seed, ri, ro, c2) in [(1, 23.95, 58.4, 0.0), (2, 24.25, 57.7, 2e-4), (3, 24.05, 58.1, -2e-4)] {
            let p = SpiralParams { total_sectors: 2_295_104, bit_mm: 133.3e-6, ri, ro, c1: 0.0, c2, c3: 0.0 };
            let (r, _) = run(SimDrive::new(p, seed)).unwrap();
            let truth = Spiral::new(p).unwrap();
            let err = worst_error(&truth, r.preset.spiral_params());
            eprintln!("seed {seed}: worst {err:.3} deg, rms {:.3}", r.cubic_rms_deg);
            assert!(err < 1.5, "seed {seed}: {err}");
        }
    }

    #[test]
    fn works_without_fua_via_distant_eviction() {
        let mut d = SimDrive::new(reference_preset().spiral_params(), 11);
        d.fua = false;
        let (r, truth) = run(d).unwrap();
        assert!(worst_error(&truth, r.preset.spiral_params()) < 1.0);
    }

    /// Hardware will run this code path, so it must leave a healthy real disc alone: the
    /// measured table may not move ri, repair a turn or read as suspect.
    #[test]
    fn finishing_stage_leaves_real_run_alone() {
        let Some(root) = crate::probe_root() else { return };
        let Ok(t) = std::fs::read_to_string(root.join("results/geo-disk8-20261106-211940/targets.csv")) else { return };
        let mut pts: Vec<PhasePoint> = t
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
        let n_ref = completion_bit(1_124_544);
        let f = finish_fit(&mut pts, n_ref, (24.17 / BIT_MM, 0.734e-3 / BIT_MM), 2_295_104).unwrap();
        eprintln!("noise {:.3}, rms {:.3}, repairs {}, ri {:.6}", f.noise_deg, f.cubic.rms_deg, f.turn_repairs, f.cubic.params.ri);
        assert_eq!(f.turn_repairs, 0);
        assert!((f.cubic.params.ri - 24.173325).abs() < 1e-5, "{}", f.cubic.params.ri);
        assert!(!is_suspect(f.cubic.rms_deg, f.noise_deg));
        let truth = Spiral::new(reference_preset().spiral_params()).unwrap();
        let err = worst_error(&truth, f.cubic.params);
        eprintln!("worst angle error to golden {err:.3}");
        assert!(err < 0.1, "{err}");
    }

    #[test]
    fn cancels_promptly() {
        let mut d = SimDrive::new(reference_preset().spiral_params(), 1);
        let cfg = LadderConfig::default();
        let e = run_ladder(&mut d, &cfg, &AtomicBool::new(true), &mut |_| {}, &mut |_| {}).unwrap_err();
        assert_eq!(e, "已取消");
    }
}
