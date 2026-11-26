//! Far-field model of one patch of track: which colour reaches the eye from where.
//!
//! Tracks are not aligned to a fraction of a wavelength, so each one is its own 1-D
//! grating along the track and their intensities add. A run of marks with level changes
//! at x_k scatters, at spatial frequency f along the track,
//!
//!   S(f) = |sum_k (-1)^k exp(-2 pi i f x_k)|^2 / (2 pi f)^2 / L
//!
//! (the derivative of a step waveform is a train of +-1 impulses). Light arriving from
//! direction l and leaving towards v picks f = (v + l)_parallel . t / lambda, t the track
//! direction; the radial part only meets the groove's own rainbow.

use std::f64::consts::{PI, TAU};

/// Spectrum sampled on a uniform grid of spatial frequency (cycles per µm).
#[derive(Clone, Debug)]
pub struct Spectrum {
    pub df: f64,
    pub s: Vec<f32>,
}

pub const F_MAX: f64 = 5.0;
pub const BINS: usize = 1000;
/// Below this the along-track spectrum is the run's mean level, which every track shares and
/// which therefore only feeds the groove's own orders: left out.
pub const F_MIN: f64 = 0.12;

impl Spectrum {
    /// Power at `f` (sign ignored: the waveform is real), linear between bins.
    #[inline]
    pub fn at(&self, f: f64) -> f32 {
        let x = f.abs() / self.df;
        let k = x as usize;
        if k + 1 >= self.s.len() {
            return 0.0;
        }
        let t = (x - k as f64) as f32;
        self.s[k] * (1.0 - t) + self.s[k + 1] * t
    }

    /// Gaussian blur in frequency, for a light source of finite size.
    pub fn blurred(&self, sigma_f: f64) -> Spectrum {
        let r = (3.0 * sigma_f / self.df).ceil() as isize;
        if r < 1 {
            return self.clone();
        }
        let w: Vec<f32> = (-r..=r).map(|k| (-0.5 * (k as f64 * self.df / sigma_f).powi(2)).exp() as f32).collect();
        let sum: f32 = w.iter().sum();
        let n = self.s.len() as isize;
        let s = (0..n)
            .map(|i| {
                let mut acc = 0.0;
                for (j, wk) in (-r..=r).zip(&w) {
                    // Mirror at zero: S is even.
                    let idx = (i + j).abs().min(n - 1);
                    acc += self.s[idx as usize] * wk;
                }
                acc / sum
            })
            .collect();
        Spectrum { df: self.df, s }
    }
}

/// Power spectrum of a run whose level changes at `transitions` (channel bits) over
/// `len_bits`, with bits `bit_um` long and edges blurred by a Gaussian of `edge_um`.
pub fn spectrum(transitions: &[u32], len_bits: u32, bit_um: f64, edge_um: f64) -> Spectrum {
    let df = F_MAX / BINS as f64;
    let len_um = len_bits as f64 * bit_um;
    // Phasor recurrence along the frequency grid: one complex multiply per edge per bin.
    let mut cur: Vec<(f64, f64)> = vec![(1.0, 0.0); transitions.len()];
    let step: Vec<(f64, f64)> = transitions
        .iter()
        .map(|&x| {
            let a = -TAU * df * x as f64 * bit_um;
            (a.cos(), a.sin())
        })
        .collect();
    let mut s = Vec::with_capacity(BINS + 1);
    for k in 0..=BINS {
        let f = k as f64 * df;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, c) in cur.iter_mut().enumerate() {
            let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
            re += sign * c.0;
            im += sign * c.1;
            let st = step[i];
            *c = (c.0 * st.0 - c.1 * st.1, c.0 * st.1 + c.1 * st.0);
        }
        let p = if f < F_MIN { 0.0 } else { (re * re + im * im) / (TAU * f).powi(2) / len_um };
        let mtf = (-(TAU * f * edge_um).powi(2)).exp();
        s.push((p * mtf) as f32);
    }
    Spectrum { df, s }
}

/// CIE 1931 2° matching functions, multi-lobe fit of Wyman, Sloan and Shirley (2013).
pub fn cie_xyz(nm: f64) -> [f64; 3] {
    let g = |mu: f64, s1: f64, s2: f64| {
        let t = (nm - mu) / if nm < mu { s1 } else { s2 };
        (-0.5 * t * t).exp()
    };
    [
        1.056 * g(599.8, 37.9, 31.0) + 0.362 * g(442.0, 16.0, 26.7) - 0.065 * g(501.1, 20.4, 26.2),
        0.821 * g(568.8, 46.9, 40.5) + 0.286 * g(530.9, 16.3, 31.1),
        1.217 * g(437.0, 11.8, 36.0) + 0.681 * g(459.0, 26.0, 13.8),
    ]
}

pub fn xyz_to_linear_srgb(c: [f64; 3]) -> [f64; 3] {
    [
        3.2406 * c[0] - 1.5372 * c[1] - 0.4986 * c[2],
        -0.9689 * c[0] + 1.8758 * c[1] + 0.0415 * c[2],
        0.0557 * c[0] - 0.2040 * c[1] + 1.0570 * c[2],
    ]
}

pub const NM_FIRST: f64 = 400.0;
pub const NM_STEP: f64 = 10.0;
pub const NM_COUNT: usize = 31;

/// Matching functions on the wavelength grid, scaled so a flat unit spectrum has Y = 1.
pub fn cmf_table() -> Vec<[f64; 3]> {
    let raw: Vec<[f64; 3]> = (0..NM_COUNT).map(|i| cie_xyz(NM_FIRST + i as f64 * NM_STEP)).collect();
    let y: f64 = raw.iter().map(|c| c[1]).sum();
    raw.iter().map(|c| [c[0] / y, c[1] / y, c[2] / y]).collect()
}

/// XYZ seen when `spec` is lit and viewed so that (v + l) projected onto the track
/// direction is `g_t` (unitless, -2..2). Flat illuminant.
pub fn xyz_at(spec: &Spectrum, g_t: f64, cmf: &[[f64; 3]]) -> [f64; 3] {
    let mut out = [0.0; 3];
    for (i, c) in cmf.iter().enumerate() {
        let lam_um = (NM_FIRST + i as f64 * NM_STEP) * 1e-3;
        let p = spec.at(g_t / lam_um) as f64;
        for k in 0..3 {
            out[k] += p * c[k];
        }
    }
    out
}

/// Normal incidence, light straight down, diffraction angle `deg` from the normal in the
/// track direction: the setting of Hans Chiu's published plots.
pub fn g_for_angle(deg: f64) -> f64 {
    deg.to_radians().sin()
}

/// Degrees to radians helper for callers that think in degrees.
pub fn deg(x: f64) -> f64 {
    x * PI / 180.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holo::efm;

    const BIT_UM: f64 = 0.1333;

    fn spec_of(bytes: &[u8]) -> Spectrum {
        let e = efm::encode_after_sync(bytes);
        spectrum(&efm::transitions(&e.codes), bytes.len() as u32 * 16, BIT_UM, 0.0)
    }

    #[test]
    fn square_wave_peaks_at_its_period() {
        // 0x92 repeated: 8T marks and 8T spaces, period 16T = 2.13 µm.
        let s = spec_of(&[0x92; 81]);
        let (k, _) = s.s.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap();
        let f = k as f64 * s.df;
        assert!((f - 1.0 / (16.0 * BIT_UM)).abs() < 0.02, "{f}");
    }

    fn rgb_curve(s: &Spectrum) -> Vec<[f64; 3]> {
        let cmf = cmf_table();
        (0..=50).map(|d| xyz_to_linear_srgb(xyz_at(s, g_for_angle(d as f64), &cmf))).collect()
    }

    fn argmax(c: &[[f64; 3]], ch: usize, lo: usize, hi: usize) -> usize {
        (lo..=hi).max_by(|&a, &b| c[a][ch].total_cmp(&c[b][ch])).unwrap()
    }

    /// Hans Chiu's "Blue" sequence (transcribed from his screenshot) puts blue at 20-24°,
    /// green near 27° and red near 32° in his plot. The same physics must agree.
    #[test]
    fn published_blue_sequence_lands_where_his_plot_says() {
        let blue: Vec<u8> = "58 BB C6 66 67 6F 94 CB BF FC 67 C8 CE 58 CB 7D CF CC CF CC 67 B7 89 67 94 BF C0 CE \
             67 94 A3 81 FC 98 AD 7F 98 64 98 64 5C 66 89 67 CC 5D A4 7F FC 86 CF CC 67 CC 82 7D \
             98 CD B7 81 C6 67 6F AD 6F 94 BF A5 98 BF FC C2 C6 66 6F 6F 94 BB E2 7D A2"
            .split_whitespace()
            .map(|h| u8::from_str_radix(h, 16).unwrap())
            .collect();
        assert_eq!(blue.len(), 81);
        let c = rgb_curve(&spec_of(&blue).blurred(0.01));
        let (b, g, r) = (argmax(&c, 2, 10, 45), argmax(&c, 1, 10, 45), argmax(&c, 0, 10, 45));
        assert!((19..=25).contains(&b), "blue peak {b}");
        assert!((24..=31).contains(&g), "green peak {g}");
        assert!((29..=37).contains(&r), "red peak {r}");
    }

    #[test]
    fn white_has_unit_luminance() {
        let cmf = cmf_table();
        let y: f64 = cmf.iter().map(|c| c[1]).sum();
        assert!((y - 1.0).abs() < 1e-9);
        let w = xyz_to_linear_srgb([cmf.iter().map(|c| c[0]).sum(), 1.0, cmf.iter().map(|c| c[2]).sum()]);
        assert!(w.iter().all(|&v| v > 0.7 && v < 1.4), "{w:?}");
    }
}
