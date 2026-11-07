//! Spindle period from completion timestamps, and robust circular statistics.
//! The 0.2° floor and the 0.9 harmonic check follow discprobe `geo.py` and `analysis.py`.

use std::f64::consts::TAU;

#[derive(Clone, Copy, Debug, Default)]
pub struct PeriodFit {
    pub period_ns: f64,
    pub coherence: f64,
    /// A completion instant of the reference: phases are measured from here.
    pub t0_ns: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CircularStats {
    pub mean_deg: f64,
    pub std_deg: f64,
    /// Standard error, floored at `SIGMA_FLOOR_DEG`.
    pub sigma_deg: f64,
    pub used: usize,
}

pub const SIGMA_FLOOR_DEG: f64 = 0.20;
/// Reads retried by the drive land anywhere, so they are cut before judging coherence.
const OUTLIER_DEG: f64 = 12.0;

fn resultant(ts: &[f64], f_per_ns: f64) -> (f64, f64) {
    let (mut re, mut im) = (0.0, 0.0);
    let origin = ts[0];
    for t in ts {
        let (s, c) = (TAU * f_per_ns * (t - origin)).sin_cos();
        re += c;
        im += s;
    }
    ((re * re + im * im).sqrt() / ts.len() as f64, im.atan2(re))
}

fn peak(ts: &[f64], lo: f64, hi: f64, step: f64) -> (f64, f64) {
    let mut best = (lo, -1.0);
    let n = ((hi - lo) / step).ceil() as usize;
    for i in 0..=n {
        let f = lo + i as f64 * step;
        let c = resultant(ts, f).0;
        if c > best.1 {
            best = (f, c);
        }
    }
    best
}

fn finish(ts: &[f64], f: f64) -> PeriodFit {
    // Second pass with retried reads removed: coherence should describe the spindle.
    let (_, mean) = resultant(ts, f);
    let keep: Vec<f64> = ts
        .iter()
        .copied()
        .filter(|t| {
            let d = wrap_deg((TAU * f * (t - ts[0]) - mean).to_degrees());
            d.abs() <= OUTLIER_DEG
        })
        .collect();
    let used = if keep.len() >= ts.len() * 2 / 3 { &keep[..] } else { ts };
    let (coherence, mean) = resultant(used, f);
    PeriodFit { period_ns: 1.0 / f, coherence, t0_ns: used[0] + mean / (TAU * f) }
}

/// Three-level refinement around a known period, each level 20x finer.
pub fn refine_period(ts: &[f64], prior_ns: f64, window: f64) -> PeriodFit {
    let span = (ts[ts.len() - 1] - ts[0]).max(1.0);
    let f0 = 1.0 / prior_ns;
    let (mut lo, mut hi) = (f0 * (1.0 - window), f0 * (1.0 + window));
    let mut step = 1.0 / (10.0 * span);
    let mut f = f0;
    for _ in 0..3 {
        f = peak(ts, lo, hi, step).0;
        lo = f - step;
        hi = f + step;
        step /= 20.0;
    }
    finish(ts, f)
}

/// Period with no prior. Every multiple of the rotation frequency is also coherent, so
/// this takes the lowest frequency that comes close to the best.
pub fn estimate_period(ts: &[f64], rpm_lo: f64, rpm_hi: f64) -> PeriodFit {
    let span = (ts[ts.len() - 1] - ts[0]).max(1.0);
    let (lo, hi) = (rpm_lo / 60e9, rpm_hi / 60e9);
    let step = 1.0 / (10.0 * span);
    let n = ((hi - lo) / step).ceil() as usize;
    let scan: Vec<(f64, f64)> = (0..=n)
        .map(|i| {
            let f = lo + i as f64 * step;
            (f, resultant(ts, f).0)
        })
        .collect();
    let best = scan.iter().fold(0.0f64, |a, q| a.max(q.1));
    let first = scan.iter().find(|q| q.1 >= 0.9 * best).map(|q| q.0).unwrap_or(lo);
    let local = scan
        .iter()
        .filter(|q| (q.0 - first).abs() <= first * 0.02)
        .fold((first, -1.0), |a, q| if q.1 > a.1 { *q } else { a });
    refine_period(ts, 1.0 / local.0, 0.01)
}

/// Wrap to (-180, 180].
#[inline]
pub fn wrap_deg(d: f64) -> f64 {
    let w = d.rem_euclid(360.0);
    if w > 180.0 {
        w - 360.0
    } else {
        w
    }
}

/// Phase of an instant in degrees, in [0, 360).
#[inline]
pub fn phase_deg(t_ns: f64, fit: &PeriodFit) -> f64 {
    ((t_ns - fit.t0_ns) / fit.period_ns).rem_euclid(1.0) * 360.0
}

fn circular_mean(v: &[f64]) -> f64 {
    let (mut s, mut c) = (0.0, 0.0);
    for d in v {
        let (a, b) = d.to_radians().sin_cos();
        s += a;
        c += b;
    }
    s.atan2(c).to_degrees()
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Mean angle after rejecting samples beyond three robust sigmas of the circular median.
pub fn circular_stats(phases: &[f64]) -> CircularStats {
    let anchor = circular_mean(phases);
    let mut offs: Vec<f64> = phases.iter().map(|p| wrap_deg(p - anchor)).collect();
    let centre = median(&mut offs.clone());
    for o in &mut offs {
        *o -= centre;
    }
    let mut abs: Vec<f64> = offs.iter().map(|o| o.abs()).collect();
    let sigma = (1.4826 * median(&mut abs)).max(0.05);
    let keep: Vec<f64> = offs.iter().copied().filter(|o| o.abs() <= 3.0 * sigma).collect();
    let n = keep.len().max(1) as f64;
    let m = keep.iter().sum::<f64>() / n;
    let var = keep.iter().map(|o| (o - m).powi(2)).sum::<f64>() / (n - 1.0).max(1.0);
    let std_deg = var.sqrt();
    CircularStats {
        mean_deg: (anchor + centre + m).rem_euclid(360.0),
        std_deg,
        sigma_deg: (std_deg / n.sqrt()).max(SIGMA_FLOOR_DEG),
        used: keep.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SplitMix64;
    use std::collections::BTreeMap;

    #[test]
    fn wraps() {
        assert_eq!(wrap_deg(190.0), -170.0);
        assert_eq!(wrap_deg(-190.0), 170.0);
        assert_eq!(wrap_deg(180.0), 180.0);
    }

    #[test]
    fn circular_mean_survives_the_zero_crossing_and_outliers() {
        let mut g = SplitMix64(4);
        let mut v: Vec<f64> = (0..40).map(|_| (359.5 + g.gauss() * 0.6).rem_euclid(360.0)).collect();
        v[3] = 170.0;
        v[9] = 45.0;
        let s = circular_stats(&v);
        assert!(wrap_deg(s.mean_deg - 359.5).abs() < 0.4, "{}", s.mean_deg);
        assert!((35..=38).contains(&s.used), "{}", s.used);
    }

    #[test]
    fn finds_fundamental_not_a_harmonic() {
        let period = 25.474e6;
        let mut g = SplitMix64(9);
        let mut t = 0.0;
        let ts: Vec<f64> = (0..60)
            .map(|_| {
                t += period * (2 + (g.unit() * 4.0) as u64) as f64;
                t + g.gauss() * 40_000.0
            })
            .collect();
        let fit = estimate_period(&ts, 1200.0, 12_000.0);
        assert!((fit.period_ns / period - 1.0).abs() < 1e-4, "{}", fit.period_ns);
        assert!(fit.coherence > 0.98);
    }

    /// Replays the 2240 real ref/target read pairs of the 56-rung run on the GUD1N and
    /// checks each rung's phase against the published table.
    #[test]
    fn real_run_phases_reproduce() {
        let Some(root) = crate::probe_root() else { return };
        let root = root.join("results/geo-disk8-20261106-211940");
        let Ok(events) = std::fs::read_to_string(root.join("events.csv")) else { return };
        let targets = std::fs::read_to_string(root.join("targets.csv")).unwrap();
        let mut by: BTreeMap<u32, (Vec<f64>, Vec<f64>)> = BTreeMap::new();
        let mut last_ref = Vec::new();
        for l in events.lines().skip(1) {
            let f: Vec<&str> = l.split(',').collect();
            if f[1] == "ref" && f[8] == "0" {
                last_ref.push(f[6].parse::<f64>().unwrap());
            } else if f[1] == "target" && f[8] == "0" {
                let e = by.entry(f[2].parse().unwrap()).or_default();
                e.0.append(&mut last_ref);
                e.1.push(f[6].parse::<f64>().unwrap());
            }
        }
        let (mut worst, mut rungs, mut min_c) = (0.0f64, 0, 1.0f64);
        for l in targets.lines().skip(1) {
            let f: Vec<&str> = l.split(',').collect();
            let lba: u32 = f[0].parse().unwrap();
            let want: f64 = f[2].parse().unwrap();
            let want_period: f64 = f[10].parse().unwrap();
            let (refs, tgts) = by.get(&lba).unwrap();
            let fit = refine_period(refs, want_period * 1e6, 0.05);
            let s = circular_stats(&tgts.iter().map(|t| phase_deg(*t, &fit)).collect::<Vec<_>>());
            worst = worst.max(wrap_deg(s.mean_deg - want).abs());
            min_c = min_c.min(fit.coherence);
            rungs += 1;
        }
        eprintln!("rungs {rungs}, worst phase difference {worst:.3} deg, min coherence {min_c:.4}");
        assert_eq!(rungs, 56);
        assert!(worst < 1.5, "{worst}");
    }
}
