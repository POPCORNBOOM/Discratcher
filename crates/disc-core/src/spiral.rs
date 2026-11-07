//! Variable-pitch spiral: maps a channel-bit position to (radius, cumulative angle).
//!
//! p(x) = p0 * (1 + c1 x + c2 x^2 + c3 x^3), x = (r - ri) / (ro - ri).
//! Arc length and angle are integrated on a dense uniform radius grid; p0 follows from
//! requiring the track to be exactly total_sectors * 38688 * bit long.

use crate::ecma267::SECTOR_CHANNEL_BITS;
use serde::{Deserialize, Serialize};
use std::f64::consts::TAU;

/// Source: `GRID` in discprobe `disexport.py`.
pub const DEFAULT_GRID: usize = 40_001;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpiralParams {
    pub total_sectors: u32,
    pub bit_mm: f64,
    pub ri: f64,
    pub ro: f64,
    pub c1: f64,
    pub c2: f64,
    pub c3: f64,
}

#[derive(Clone, Debug)]
pub struct Spiral {
    pub params: SpiralParams,
    /// Base pitch in mm.
    pub p0: f64,
    dr: f64,
    /// Cumulative channel bits at each grid radius.
    n: Vec<f64>,
    /// Cumulative angle in radians at each grid radius.
    theta: Vec<f64>,
}

impl Spiral {
    pub fn new(params: SpiralParams) -> Result<Self, String> {
        Self::with_grid(params, DEFAULT_GRID)
    }

    pub fn with_grid(p: SpiralParams, grid: usize) -> Result<Self, String> {
        if !(p.ro > p.ri && p.ri > 0.0) || p.bit_mm <= 0.0 || p.total_sectors == 0 {
            return Err(format!("invalid spiral geometry: ri={} ro={}", p.ri, p.ro));
        }
        let dr = (p.ro - p.ri) / (grid - 1) as f64;
        let mut n = Vec::with_capacity(grid);
        let mut theta = Vec::with_capacity(grid);
        let (mut s_unit, mut t_unit) = (0.0f64, 0.0f64);
        let mut prev: Option<(f64, f64)> = None;
        for k in 0..grid {
            let r = p.ri + k as f64 * dr;
            let x = k as f64 / (grid - 1) as f64;
            let s = 1.0 + x * (p.c1 + x * (p.c2 + x * p.c3));
            if s <= 0.0 {
                return Err("pitch shape s(x) must stay positive".into());
            }
            let cur = (TAU * r / s, TAU / s);
            if let Some(q) = prev {
                s_unit += 0.5 * (cur.0 + q.0) * dr;
                t_unit += 0.5 * (cur.1 + q.1) * dr;
            }
            prev = Some(cur);
            n.push(s_unit);
            theta.push(t_unit);
        }
        let total_bits = p.total_sectors as f64 * SECTOR_CHANNEL_BITS as f64;
        let p0 = s_unit / (total_bits * p.bit_mm);
        for v in &mut n {
            *v /= p0 * p.bit_mm;
        }
        for v in &mut theta {
            *v /= p0;
        }
        Ok(Self { params: p, p0, dr, n, theta })
    }

    pub fn total_bits(&self) -> f64 {
        *self.n.last().unwrap()
    }

    /// (radius mm, cumulative angle rad) at channel-bit position `bit`.
    #[inline]
    pub fn locate(&self, bit: f64) -> (f64, f64) {
        let last = self.n.len() - 2;
        let k = self.n.partition_point(|&v| v <= bit).saturating_sub(1).min(last);
        let t = (bit - self.n[k]) / (self.n[k + 1] - self.n[k]);
        (
            self.params.ri + (k as f64 + t) * self.dr,
            self.theta[k] + t * (self.theta[k + 1] - self.theta[k]),
        )
    }

    pub fn theta_at_bit(&self, bit: f64) -> f64 {
        self.locate(bit).1
    }

    /// (channel bits, angle rad) on the spiral at radius `r`.
    #[inline]
    pub fn at_radius(&self, r: f64) -> (f64, f64) {
        let f = ((r - self.params.ri) / self.dr).clamp(0.0, (self.n.len() - 1) as f64);
        let k = (f as usize).min(self.n.len() - 2);
        let t = f - k as f64;
        (
            self.n[k] + t * (self.n[k + 1] - self.n[k]),
            self.theta[k] + t * (self.theta[k + 1] - self.theta[k]),
        )
    }

    /// Radius of a logical sector's start.
    pub fn radius_of_lba(&self, lba: u32) -> f64 {
        self.locate(lba as f64 * SECTOR_CHANNEL_BITS as f64).0
    }

    /// LBA reached at radius `r`, rounded down to a sector.
    pub fn lba_at_radius(&self, r: f64) -> u32 {
        (self.at_radius(r).0 / SECTOR_CHANNEL_BITS as f64) as u32
    }

    /// Sectors per full turn at radius `r`.
    pub fn sectors_per_turn(&self, r: f64) -> f64 {
        TAU * r / (SECTOR_CHANNEL_BITS as f64 * self.params.bit_mm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::reference_preset;

    fn reference() -> Spiral {
        Spiral::new(reference_preset().spiral_params()).unwrap()
    }

    #[test]
    fn reproduces_reference_base_pitch_and_span() {
        let s = reference();
        assert!((s.p0 * 1000.0 - 0.73406).abs() < 2e-5, "{}", s.p0 * 1000.0);
        let bits = 2_295_104f64 * 38688.0;
        assert!((s.locate(bits * 0.02).0 - 25.292).abs() < 0.01);
        assert!((s.locate(bits * 0.96).0 - 56.914).abs() < 0.01);
    }

    #[test]
    fn locate_and_at_radius_round_trip() {
        let s = reference();
        for r in [24.5, 31.0, 44.4, 57.5] {
            let (n, th) = s.at_radius(r);
            let (r2, th2) = s.locate(n);
            assert!((r - r2).abs() < 1e-7);
            assert!((th - th2).abs() < 1e-3);
        }
    }

    #[test]
    fn constant_pitch_matches_closed_form() {
        let p = SpiralParams { total_sectors: 2_295_104, bit_mm: 133.3e-6, ri: 24.0, ro: 58.0, c1: 0.0, c2: 0.0, c3: 0.0 };
        let s = Spiral::new(p).unwrap();
        let (u, v) = (p.ri / p.bit_mm, s.p0 / p.bit_mm);
        for f in [0.1, 0.5, 0.97] {
            let bit = s.total_bits() * f;
            let want = TAU / v * ((u * u + v * bit / std::f64::consts::PI).sqrt() - u);
            assert!((s.theta_at_bit(bit) - want).abs().to_degrees() < 0.05, "f {f}");
        }
    }

    #[test]
    fn rejects_bad_geometry() {
        let p = SpiralParams { total_sectors: 16, bit_mm: 1e-4, ri: 30.0, ro: 20.0, c1: 0.0, c2: 0.0, c3: 0.0 };
        assert!(Spiral::new(p).is_err());
        let p = SpiralParams { ri: 20.0, ro: 30.0, c1: -3.0, ..p };
        assert!(Spiral::new(p).is_err());
    }
}
