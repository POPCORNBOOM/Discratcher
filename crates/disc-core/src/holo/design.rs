//! Designer: pictures for several viewing angles in, one byte run per sync frame out.
//!
//! The disc is cut into square cells. For each cell, every view asks for a colour, and
//! every codebook entry (plus plain data last) shows some colour at that view; a converged
//! FISTA simplex solver gives the cell non-negative weights summing to one that best match
//! all views at once. Tracks add incoherently, so a weight is simply the share of the
//! cell's sync frames given to that entry, handed out with a low-discrepancy sequence.
//!
//! Codebook: [`swatch_codebook`] holds the fixed 12-entry SA calibration codebook (used
//! verbatim by the swatch disc). [`shared_codebook`] extends it via
//! [`codebook::designer_extend`] with high-purity zero-prefix periodic choice-free entries
//! (0.469..1.250 /µm) that suppress the broadband background haze without altering the
//! calibration swatch entries. Masked-off (`alpha = 0`) and ring-edge cells default to
//! the locally darkest non-plain entry rather than plain data; the ISO 9660 system area
//! remains strictly plain in [`build_iso`].
//!
//! Viewing model: by default (`LightMode::Flash`), the eye is at `distance_mm` (default
//! 350 mm) from the disc centre, tilted `angle_deg` from the normal toward `dir_deg`
//! (image angle, -90 = towards the top of the picture), with a phone flashlight ~10 mm
//! beside the eye; every cell gets its own `(g_t, g_r)` from `scene::g_at`, and marks
//! include the radial envelope `exp(-(g_r/lambda)^2 / (2 sigma_r^2))`. `LightMode::Far`
//! preserves the legacy far-field model with light at `light_deg` and no radial envelope.

use super::codebook::{self, Codebook, CORE_BYTES};
use super::efm;
use super::frame::{frame_slots, main_byte_bit};
use super::optics::{self, Spectrum};
use super::scene;
use crate::carver::{create_mapped, CarveProgress};
use crate::ecma267::*;
use crate::preset::DiscGeometryPreset;
use crate::spiral::Spiral;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Resolution of the (g_t, g_r) lookup.
const G_BINS: usize = 400;
const GR_BINS: usize = 64;
pub const G_MAX: f64 = 2.0;
/// Light-source blur in spatial frequency, per µm: a 1° source at 550 nm.
pub const DEFAULT_SOURCE_BLUR: f64 = 0.03;
/// Radial envelope of mark scattering, per µm (1 sigma).
pub const DEFAULT_SIGMA_R: f64 = 1.0;
/// Default eye distance from the disc centre, mm.
pub const DEFAULT_DISTANCE_MM: f64 = 350.0;
/// Default phone flashlight offset from the eye, mm (lateral to the view plane).
pub const FLASH_OFFSET_MM: f64 = 10.0;
/// Default fixed ceiling lamp position in disc-frame mm (1.2 m above, 200 mm behind disc,
/// opposite the viewer at `dir = -90°` where `eye_y < 0`).
pub const DEFAULT_LAMP_MM: [f64; 3] = [0.0, 200.0, 1200.0];
/// Reference fixed phone-flashlight position in disc-frame mm (25 cm to the viewer's right,
/// 40 cm above the disc, aligned in y with the disc centre) so tangential `g_t ≈ (l + v)_x`
/// at the near/far bands keeps a single sign across ±80 mm horizontal head motion.
pub const HELD_FLASHLIGHT_MM: [f64; 3] = [250.0, 0.0, 400.0];
/// Default head-motion elevation tilt from the disc normal, degrees.
pub const DEFAULT_HEAD_TILT_DEG: f64 = 30.0;
/// Default horizontal head-motion half-span (±H), mm.
pub const DEFAULT_HEAD_HALF_SPAN_MM: f64 = 80.0;
/// Lamp angular blur in g for the groove rainbow (unitless, 1 sigma).
pub const GROOVE_SOURCE: f64 = 0.017;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LightMode {
    /// Near-field phone flashlight: lamp sits at the eye, offset by ~10 mm.
    #[default]
    Flash,
    /// Legacy far-field model: light at `light_deg` and eye at `angle_deg`, infinitely far.
    Far,
    /// Fixed lamp at `light_mm` while the viewer's eye moves independently.
    Lamp,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ViewSpec {
    /// Square picture covering the disc, like a full-disc layer when carving.
    pub image_path: String,
    /// Eye angle from the disc normal, degrees.
    pub angle_deg: f64,
    #[serde(default)]
    pub light_deg: f64,
    #[serde(default = "default_dir")]
    pub dir_deg: f64,
    /// Eye distance from the disc centre, mm.
    #[serde(default = "default_distance")]
    pub distance_mm: f64,
    /// Illumination geometry: `"flash"` (default), `"far"`, or `"lamp"`.
    #[serde(default)]
    pub light_mode: LightMode,
    /// Optional lamp offset from the eye in disc-frame mm (overrides the ~10 mm flash offset).
    #[serde(default)]
    pub light_offset_mm: Option<[f64; 3]>,
    /// Absolute lamp position in disc-frame mm (`LightMode::Lamp`; defaults to `DEFAULT_LAMP_MM`).
    /// Disc coordinates: +x right, +y down in picture (same as `y = (v - 0.5) * 2 * ro`), +z up.
    #[serde(default)]
    pub light_mm: Option<[f64; 3]>,
    /// Exact eye position in disc-frame mm; if `None`, derived from `(angle_deg, dir_deg, distance_mm)`.
    #[serde(default)]
    pub eye_mm: Option<[f64; 3]>,
}

fn default_dir() -> f64 {
    -90.0
}
fn default_distance() -> f64 {
    DEFAULT_DISTANCE_MM
}

/// Exact eye position in disc-frame mm for a viewer at distance `distance_mm` from the disc
/// centre, tilted `tilt_deg` from the disc normal along `dir_deg` (`-90°` = from `-y`), then
/// shifted horizontally by `head_x_mm` along the line perpendicular to `dir_deg` (`+x` = right
/// for `dir_deg = -90°`).
pub fn head_eye_mm(distance_mm: f64, tilt_deg: f64, dir_deg: f64, head_x_mm: f64) -> [f64; 3] {
    let a = tilt_deg.to_radians();
    let d = dir_deg.to_radians();
    let e0 = [distance_mm * a.sin() * d.cos(), distance_mm * a.sin() * d.sin(), distance_mm * a.cos()];
    [e0[0] - head_x_mm * d.sin(), e0[1] + head_x_mm * d.cos(), e0[2]]
}

impl ViewSpec {
    pub fn flash(image_path: impl Into<String>, angle_deg: f64) -> Self {
        ViewSpec {
            image_path: image_path.into(),
            angle_deg,
            light_deg: 0.0,
            dir_deg: default_dir(),
            distance_mm: DEFAULT_DISTANCE_MM,
            light_mode: LightMode::Flash,
            light_offset_mm: None,
            light_mm: None,
            eye_mm: None,
        }
    }

    /// Fixed-lamp view with exact eye and lamp coordinates in disc-frame mm.
    pub fn lamp(image_path: impl Into<String>, eye_mm: [f64; 3], light_mm: [f64; 3]) -> Self {
        let d = (eye_mm[0] * eye_mm[0] + eye_mm[1] * eye_mm[1] + eye_mm[2] * eye_mm[2]).sqrt();
        let angle_deg = if d > 0.0 { (eye_mm[2] / d).clamp(-1.0, 1.0).acos().to_degrees() } else { 0.0 };
        let dir_deg = if eye_mm[0] != 0.0 || eye_mm[1] != 0.0 { eye_mm[1].atan2(eye_mm[0]).to_degrees() } else { default_dir() };
        ViewSpec {
            image_path: image_path.into(),
            angle_deg,
            light_deg: 0.0,
            dir_deg,
            distance_mm: d,
            light_mode: LightMode::Lamp,
            light_offset_mm: None,
            light_mm: Some(light_mm),
            eye_mm: Some(eye_mm),
        }
    }

    /// Build `n` views for a head moving horizontally by `±half_span_mm` in `n` evenly spaced
    /// steps under a fixed lamp at `light_mm`.
    pub fn head_motion_views(
        n: usize,
        distance_mm: f64,
        tilt_deg: f64,
        dir_deg: f64,
        half_span_mm: f64,
        light_mm: [f64; 3],
    ) -> Vec<Self> {
        (0..n)
            .map(|i| {
                let h = if n <= 1 {
                    0.0
                } else {
                    -half_span_mm + 2.0 * half_span_mm * i as f64 / (n - 1) as f64
                };
                Self::lamp("", head_eye_mm(distance_mm, tilt_deg, dir_deg, h), light_mm)
            })
            .collect()
    }

    pub fn g(&self) -> f64 {
        self.angle_deg.to_radians().sin() + self.light_deg.to_radians().sin()
    }

    /// Exact eye position in disc-frame mm.
    pub fn eye_pos(&self) -> [f64; 3] {
        if let Some(eye) = self.eye_mm {
            return eye;
        }
        let a = self.angle_deg.to_radians();
        let dir = self.dir_deg.to_radians();
        let d = self.distance_mm;
        [d * a.sin() * dir.cos(), d * a.sin() * dir.sin(), d * a.cos()]
    }

    /// Near-field scene geometry corresponding to this view (only `camera_mm` and `light_mm` are set).
    pub fn scene(&self) -> scene::Scene {
        let cam = self.eye_pos();
        let light = match self.light_mode {
            LightMode::Lamp => self.light_mm.unwrap_or(DEFAULT_LAMP_MM),
            LightMode::Flash | LightMode::Far => {
                let dir = self.dir_deg.to_radians();
                let off = self.light_offset_mm.unwrap_or([-FLASH_OFFSET_MM * dir.sin(), FLASH_OFFSET_MM * dir.cos(), 0.0]);
                [cam[0] + off[0], cam[1] + off[1], cam[2] + off[2]]
            }
        };
        scene::Scene { light_mm: light, camera_mm: cam, ..Default::default() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DesignInput {
    pub views: Vec<ViewSpec>,
    /// 0 = match brightness only, 1 = colour counts as much as brightness.
    #[serde(default = "default_chroma")]
    pub chroma_weight: f64,
    /// Target brightness scale, 1 = the brightest a single entry can be.
    #[serde(default = "default_exposure")]
    pub exposure: f64,
    /// Cells across the disc diameter.
    #[serde(default = "default_grid")]
    pub grid: usize,
}

fn default_chroma() -> f64 {
    0.3
}
fn default_exposure() -> f64 {
    0.6
}
fn default_grid() -> usize {
    256
}

/// Linear-RGB response of every option at every `(g_t, g_r)`: options are the codebook
/// entries (`0..cb.entries.len()`) and, last, plain data (`cb.entries.len()`).
///
/// In [`shared_codebook`], luminance is normalised to the brightest swatch codebook entry
/// so extended high-purity periodic entries retain their physical brightness advantage
/// (> 1.0 at peak) without shifting the default `exposure = 0.6` target scale.
#[derive(Clone, Debug)]
pub struct Responses {
    pub options: usize,
    pub gt_bins: usize,
    pub gr_bins: usize,
    /// `[option][it * gr_bins + ir]` -> linear RGB
    pub rgb: Vec<Vec<[f32; 3]>>,
}

impl Responses {
    #[inline]
    pub fn at(&self, k: usize, gt: f64, gr: f64) -> [f32; 3] {
        let xt = (gt.abs() / G_MAX * (self.gt_bins - 1) as f64).min((self.gt_bins - 1) as f64);
        let xr = (gr.abs() / G_MAX * (self.gr_bins - 1) as f64).min((self.gr_bins - 1) as f64);
        let it = (xt as usize).min(self.gt_bins - 2);
        let ir = (xr as usize).min(self.gr_bins - 2);
        let tt = (xt - it as f64) as f32;
        let tr = (xr - ir as f64) as f32;
        let row = &self.rgb[k];
        let i0 = it * self.gr_bins + ir;
        let i1 = (it + 1) * self.gr_bins + ir;
        let (a00, a01, a10, a11) = (row[i0], row[i0 + 1], row[i1], row[i1 + 1]);
        let mut out = [0.0f32; 3];
        for ch in 0..3 {
            let b0 = a00[ch] + (a01[ch] - a00[ch]) * tr;
            let b1 = a10[ch] + (a11[ch] - a10[ch]) * tr;
            out[ch] = b0 + (b1 - b0) * tt;
        }
        out
    }
}

/// Spectrum of what the drive records for plain (all-zero host) data: the scrambler's own
/// stream, encoded in prescribed States. Close enough for a background.
pub fn plain_spectrum(bit_um: f64) -> Spectrum {
    let lut = build_scrambler_lut();
    let e = efm::encode_after_sync(&lut[0][..1024]);
    optics::spectrum(&efm::transitions(&e.codes), 1024 * 16, bit_um, 0.0)
}

pub fn entry_spectrum(bytes: &[u8], bit_um: f64) -> Spectrum {
    let e = efm::encode_after_sync(&bytes[..CORE_BYTES]);
    optics::spectrum(&efm::transitions(&e.codes), CORE_BYTES as u32 * 16, bit_um, 0.0)
}

/// Build responses for `cb` + plain data, normalising luminance to the peak of the first
/// `ref_entries` codebook entries (plus plain data).
pub fn responses_with_ref_peak(
    cb: &Codebook,
    ref_entries: usize,
    bit_um: f64,
    source_blur: f64,
    sigma_r: f64,
) -> Responses {
    let cmf = optics::cmf_table();
    let mut specs: Vec<Spectrum> = cb.entries.iter().map(|e| entry_spectrum(&e.bytes, bit_um)).collect();
    specs.push(plain_spectrum(bit_um));
    let lams: Vec<f64> = (0..cmf.len()).map(|i| (optics::NM_FIRST + i as f64 * optics::NM_STEP) * 1e-3).collect();
    let inv_2s2 = 1.0 / (2.0 * sigma_r * sigma_r);
    let env: Vec<Vec<f64>> = (0..GR_BINS)
        .map(|ir| {
            let gr = ir as f64 / (GR_BINS - 1) as f64 * G_MAX;
            lams.iter()
                .map(|&lam| {
                    let fr = gr / lam;
                    (-fr * fr * inv_2s2).exp()
                })
                .collect()
        })
        .collect();
    let raw: Vec<Vec<[f64; 3]>> = specs
        .par_iter()
        .map(|s| {
            let s = s.blurred(source_blur);
            let mut table = Vec::with_capacity(G_BINS * GR_BINS);
            let mut p_lam = vec![0.0f64; lams.len()];
            for it in 0..G_BINS {
                let gt = it as f64 / (G_BINS - 1) as f64 * G_MAX;
                for (il, &lam) in lams.iter().enumerate() {
                    p_lam[il] = s.at(gt / lam) as f64;
                }
                for e_row in &env {
                    let mut xyz = [0.0f64; 3];
                    for (il, c) in cmf.iter().enumerate() {
                        let p = p_lam[il] * e_row[il];
                        xyz[0] += p * c[0];
                        xyz[1] += p * c[1];
                        xyz[2] += p * c[2];
                    }
                    table.push(optics::xyz_to_linear_srgb(xyz));
                }
            }
            table
        })
        .collect();
    // Normalise to the brightest reference entry (first `ref_entries` entries plus plain).
    let n_ref = ref_entries.min(cb.entries.len());
    let plain_idx = cb.entries.len();
    let peak = raw[..n_ref]
        .iter()
        .chain(std::iter::once(&raw[plain_idx]))
        .flatten()
        .map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2])
        .fold(1e-12, f64::max);
    let rgb = raw
        .into_iter()
        .map(|v| {
            v.into_iter()
                .map(|c| [(c[0] / peak).max(0.0) as f32, (c[1] / peak).max(0.0) as f32, (c[2] / peak).max(0.0) as f32])
                .collect()
        })
        .collect();
    Responses { options: specs.len(), gt_bins: G_BINS, gr_bins: GR_BINS, rgb }
}

pub fn responses_with_params(cb: &Codebook, bit_um: f64, source_blur: f64, sigma_r: f64) -> Responses {
    responses_with_ref_peak(cb, cb.entries.len(), bit_um, source_blur, sigma_r)
}

pub fn responses(cb: &Codebook, source_blur: f64) -> Responses {
    responses_with_params(cb, cb.bit_um, source_blur, DEFAULT_SIGMA_R)
}

/// The fixed 12-entry calibration swatch codebook, built once per bit length.
/// Never altered by designer-only extensions so burned swatch discs stay reproducible.
pub fn swatch_codebook(bit_um: f64) -> Arc<Codebook> {
    static CACHE: OnceLock<Mutex<HashMap<u64, Arc<Codebook>>>> = OnceLock::new();
    let key = bit_um.to_bits();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().unwrap().get(&key) {
        return v.clone();
    }
    let cb = codebook::build(
        super::swatch::CODEBOOK_ENTRIES,
        bit_um,
        super::swatch::CODEBOOK_ITERS,
        super::swatch::CODEBOOK_SEED,
    );
    let v = Arc::new(cb);
    cache.lock().unwrap().insert(key, v.clone());
    v
}

/// The designer's extended codebook (12 swatch entries + high-purity periodic entries +
/// plain data) and its responses on the 12-entry swatch luminance scale, built once per
/// bit length.
pub fn shared_codebook(bit_um: f64) -> Arc<(Codebook, Responses)> {
    static CACHE: OnceLock<Mutex<HashMap<u64, Arc<(Codebook, Responses)>>>> = OnceLock::new();
    let key = bit_um.to_bits();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().unwrap().get(&key) {
        return v.clone();
    }
    let swatch = swatch_codebook(bit_um);
    let cb = codebook::designer_extend(&swatch);
    let r = responses_with_ref_peak(&cb, swatch.entries.len(), cb.bit_um, DEFAULT_SOURCE_BLUR, DEFAULT_SIGMA_R);
    let v = Arc::new((cb, r));
    cache.lock().unwrap().insert(key, v.clone());
    v
}

/// Light fractions the carve page's dithered bytes are simulated at, dark to light.
pub const CARVE_LEVELS: usize = 9;

/// The bytes the carve page writes for a light fraction `p`, one codebook core long.
pub fn carve_bytes(p: f64) -> Vec<u8> {
    let mut bytes = vec![0u8; CORE_BYTES];
    let mut acc = 0.0f32;
    let tone = crate::carver::Tone { servo_safe: true, min_luma: 0.0 };
    crate::carver::tone_run(tone, (p.clamp(0.0, 1.0) * 255.0) as f32, &mut acc, &mut bytes);
    bytes
}

/// What carved bytes look like at each of `CARVE_LEVELS` light fractions: their dithered
/// sequences through the same encoding and optics as the codebook, on `shared_codebook`'s scale.
pub fn carve_looks(bit_um: f64) -> Arc<Vec<Vec<[f32; 3]>>> {
    static CACHE: OnceLock<Mutex<HashMap<u64, Arc<Vec<Vec<[f32; 3]>>>>>> = OnceLock::new();
    let key = bit_um.to_bits();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().unwrap().get(&key) {
        return v.clone();
    }
    let shared = shared_codebook(bit_um);
    let cb = &shared.0;
    let mut with_carve = cb.clone();
    for l in 0..CARVE_LEVELS {
        let bytes = carve_bytes(l as f64 / (CARVE_LEVELS - 1) as f64);
        with_carve.entries.push(codebook::Entry { bytes, f0: 0.0, purity: 0.0, dsv_peak: 0 });
    }
    // Normalised against the same reference entries as the codebook itself.
    let r = responses_with_ref_peak(&with_carve, swatch_codebook(bit_um).entries.len(), cb.bit_um, DEFAULT_SOURCE_BLUR, DEFAULT_SIGMA_R);
    let v = Arc::new(r.rgb[cb.entries.len()..cb.entries.len() + CARVE_LEVELS].to_vec());
    cache.lock().unwrap().insert(key, v.clone());
    v
}

/// Targets of one view on the cell grid: linear RGB and a weight (the picture's alpha).
struct Target {
    rgb: Vec<[f32; 3]>,
    weight: Vec<f32>,
}

fn srgb_to_linear(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn load_target(path: &str, grid: usize, exposure: f64) -> Result<Target, String> {
    let img = image::open(path).map_err(|e| format!("无法读取图像 {path}：{e}"))?;
    let img = img.resize_exact(grid as u32, grid as u32, image::imageops::FilterType::Triangle).to_rgba8();
    let mut rgb = Vec::with_capacity(grid * grid);
    let mut weight = Vec::with_capacity(grid * grid);
    for p in img.pixels() {
        let e = exposure as f32;
        rgb.push([srgb_to_linear(p[0]) * e, srgb_to_linear(p[1]) * e, srgb_to_linear(p[2]) * e]);
        weight.push(p[3] as f32 / 255.0);
    }
    Ok(Target { rgb, weight })
}

/// Euclidean projection onto the probability simplex in `f64`, using `buf` as scratch.
fn project_simplex_f64(w: &mut [f64], buf: &mut [f64]) {
    buf.copy_from_slice(w);
    buf.sort_by(|a, b| b.total_cmp(a));
    let (mut css, mut theta) = (0.0f64, 0.0f64);
    for (j, &v) in buf.iter().enumerate() {
        css += v;
        let t = (css - 1.0) / (j + 1) as f64;
        if v - t > 0.0 {
            theta = t;
        }
    }
    for x in w.iter_mut() {
        *x = (*x - theta).max(0.0);
    }
}

/// Euclidean projection onto the probability simplex.
#[cfg(test)]
fn project_simplex(w: &mut [f32]) {
    let mut w64: Vec<f64> = w.iter().map(|&x| x as f64).collect();
    let mut buf = vec![0.0f64; w.len()];
    project_simplex_f64(&mut w64, &mut buf);
    for (dst, &src) in w.iter_mut().zip(&w64) {
        *dst = src as f32;
    }
}

/// Solve `min_{w in Delta_k} w^T G w - 2 h^T w` to KKT optimality via FISTA with
/// power-iteration Lipschitz step, best-vertex warm start, and adaptive gradient restart.
pub(crate) fn solve_simplex_qp(gram: &[f64], rhs: &[f64], k: usize) -> Vec<f64> {
    let mut w = vec![0.0f64; k];
    // Best single-vertex warm start.
    let mut best_j = 0usize;
    let mut best_obj = f64::INFINITY;
    for j in 0..k {
        let obj = gram[j * k + j] - 2.0 * rhs[j];
        if obj < best_obj {
            best_obj = obj;
            best_j = j;
        }
    }
    w[best_j] = 1.0;

    let trace: f64 = (0..k).map(|j| gram[j * k + j]).sum();
    if trace <= 1e-18 {
        return w;
    }
    // Tight spectral norm of symmetric PSD non-negative Gram matrix via power iteration.
    let inv_sqrt_k = 1.0 / (k as f64).sqrt();
    let mut pv = vec![inv_sqrt_k; k];
    let mut pu = vec![0.0f64; k];
    let mut lmax = trace;
    for _ in 0..10 {
        let mut norm2 = 0.0f64;
        for j in 0..k {
            let g_row = &gram[j * k..(j + 1) * k];
            let mut s = 0.0f64;
            for m in 0..k {
                s += g_row[m] * pv[m];
            }
            pu[j] = s;
            norm2 += s * s;
        }
        let norm = norm2.sqrt();
        if norm <= 1e-18 {
            break;
        }
        lmax = norm;
        let inv = 1.0 / norm;
        for j in 0..k {
            pv[j] = pu[j] * inv;
        }
    }
    let lip = (lmax * 1.01).min(trace.max(lmax)).max(1e-12);
    let step = 1.0 / lip;

    let mut y = w.clone();
    let mut w_next = vec![0.0f64; k];
    let mut buf = vec![0.0f64; k];
    let mut t = 1.0f64;
    for _ in 0..600 {
        for j in 0..k {
            let g_row = &gram[j * k..(j + 1) * k];
            let mut g = -rhs[j];
            for m in 0..k {
                g += g_row[m] * y[m];
            }
            w_next[j] = y[j] - step * g;
        }
        project_simplex_f64(&mut w_next, &mut buf);

        let mut diff2 = 0.0f64;
        let mut step2 = 0.0f64;
        let mut restart_dot = 0.0f64;
        for j in 0..k {
            let dw = w_next[j] - w[j];
            let dy = y[j] - w_next[j];
            diff2 += dw * dw;
            step2 += dy * dy;
            restart_dot += dy * dw;
        }
        if restart_dot > 0.0 {
            t = 1.0;
            y.copy_from_slice(&w_next);
            w.copy_from_slice(&w_next);
        } else {
            let t_next = 0.5 * (1.0 + (1.0 + 4.0 * t * t).sqrt());
            let beta = (t - 1.0) / t_next;
            for j in 0..k {
                y[j] = w_next[j] + beta * (w_next[j] - w[j]);
            }
            w.copy_from_slice(&w_next);
            t = t_next;
        }
        if diff2 < 1e-13 && step2 < 1e-13 {
            break;
        }
    }
    w
}

/// Weights of every cell, row-major over the grid; masked-off and ring-boundary cells
/// default to the locally darkest non-plain codebook entry rather than plain data.
#[derive(Clone, Debug)]
pub struct Solved {
    pub grid: usize,
    pub options: usize,
    pub weights: Vec<f32>,
    /// Mean squared error over all views and cells that matter.
    pub error: f64,
}

impl Solved {
    #[inline]
    pub fn cell(&self, u: f64, v: f64) -> Option<&[f32]> {
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
            return None;
        }
        let (x, y) = ((u * self.grid as f64) as usize, (v * self.grid as f64) as usize);
        let i = (y * self.grid + x) * self.options;
        Some(&self.weights[i..i + self.options])
    }
}

/// Disc coordinates of a picture point: as in `Composition::sample`, u = 0.5 + r cos t / 2ro.
#[inline]
fn track_angle(u: f64, v: f64) -> (f64, f64) {
    let (x, y) = (u - 0.5, v - 0.5);
    (x.hypot(y), y.atan2(x))
}

/// Precomputed per-view geometry for `solve` and `render`.
enum ViewGeom {
    Far { g: f64, dir: f64 },
    Near(scene::Scene),
}

impl ViewGeom {
    fn from_view(v: &ViewSpec) -> Self {
        match v.light_mode {
            LightMode::Far => ViewGeom::Far { g: v.g(), dir: v.dir_deg.to_radians() },
            LightMode::Flash | LightMode::Lamp => ViewGeom::Near(v.scene()),
        }
    }

    /// `(g_t, g_r_marks, g_r_groove)` at picture point `(u, v)` for outer radius `ro_mm`.
    /// In `Far` mode, `g_r_marks = 0.0` reproduces the legacy far-field model (no radial
    /// envelope on marks), while `g_r_groove` still tracks the radial component.
    #[inline]
    fn g_at(&self, u: f64, v: f64, ro_mm: f64) -> (f64, f64, f64) {
        match self {
            ViewGeom::Far { g, dir } => {
                let (_, t) = track_angle(u, v);
                let gt = g * (dir - t).sin();
                let gr = g * (dir - t).cos();
                (gt, 0.0, gr)
            }
            ViewGeom::Near(sc) => {
                let x = (u - 0.5) * 2.0 * ro_mm;
                let y = (v - 0.5) * 2.0 * ro_mm;
                let (gt, gr) = scene::g_at(sc, x, y);
                (gt, gr, gr)
            }
        }
    }
}

pub fn solve(input: &DesignInput, resp: &Responses, preset: &DiscGeometryPreset) -> Result<Solved, String> {
    if input.views.is_empty() {
        return Err("至少需要一张图".into());
    }
    let n = input.grid.clamp(16, 1024);
    let targets: Vec<Target> = input.views.iter().map(|v| load_target(&v.image_path, n, input.exposure)).collect::<Result<_, _>>()?;
    let k = resp.options;
    let plain = k.saturating_sub(1);
    let ro = preset.outer_radius_mm;
    let (r_in, r_out) = (preset.inner_radius_mm / (2.0 * ro), 0.5);
    let c = input.chroma_weight.max(0.0).sqrt();
    let views: Vec<ViewGeom> = input.views.iter().map(ViewGeom::from_view).collect();
    let luma = |p: [f32; 3]| 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64;
    // Fallback dark entry over non-plain codebook entries if a cell sits at exact disc centre.
    let dark_default: usize = if plain == 0 {
        0
    } else {
        (0..plain)
            .min_by(|&a, &b| {
                let ea: f64 = resp.rgb[a].iter().map(|&p| { let y = luma(p); y * y }).sum();
                let eb: f64 = resp.rgb[b].iter().map(|&p| { let y = luma(p); y * y }).sum();
                ea.total_cmp(&eb)
            })
            .unwrap_or(0)
    };
    let darkest_option = |u: f64, v: f64| -> usize {
        if plain == 0 {
            return 0;
        }
        let mut best_j = dark_default;
        let mut best_e = f64::INFINITY;
        for j in 0..plain {
            let mut e = 0.0f64;
            for geom in &views {
                let (gt, gr_marks, _) = geom.g_at(u, v, ro);
                let y = luma(resp.at(j, gt, gr_marks));
                e += y * y;
            }
            if e < best_e {
                best_e = e;
                best_j = j;
            }
        }
        best_j
    };
    let cells: Vec<(Vec<f32>, f64, f64)> = (0..n * n)
        .into_par_iter()
        .map(|i| {
            let (u, v) = (((i % n) as f64 + 0.5) / n as f64, ((i / n) as f64 + 0.5) / n as f64);
            let (r, th) = track_angle(u, v);
            let mut w = vec![0.0f32; k];
            if r < r_in || r > r_out {
                let pick = if r < 1e-6 {
                    dark_default
                } else {
                    let rc = r.clamp(r_in, r_out);
                    darkest_option(0.5 + rc * th.cos(), 0.5 + rc * th.sin())
                };
                w[pick] = 1.0;
                return (w, 0.0, 0.0);
            }
            // Normal equations G = A^T A, h = A^T b over per-view Y and c * (R, G, B) rows.
            let mut gram = vec![0.0f64; k * k];
            let mut rhs = vec![0.0f64; k];
            let mut b_sq = 0.0f64;
            let mut n_rows = 0usize;
            let mut row = vec![0.0f64; k];
            let mut cols = vec![[0.0f32; 3]; k];
            let mut add_row = |row: &[f64], bi: f64| {
                for r in 0..k {
                    let ar = row[r];
                    rhs[r] += ar * bi;
                    let gr_row = &mut gram[r * k..(r + 1) * k];
                    for col in 0..k {
                        gr_row[col] += ar * row[col];
                    }
                }
                b_sq += bi * bi;
                n_rows += 1;
            };
            for (vi, geom) in views.iter().enumerate() {
                let alpha = targets[vi].weight[i] as f64;
                if alpha <= 0.0 {
                    continue;
                }
                let (gt, gr_marks, _) = geom.g_at(u, v, ro);
                let tg = targets[vi].rgb[i];
                let tg_y = luma(tg);
                for j in 0..k {
                    cols[j] = resp.at(j, gt, gr_marks);
                    row[j] = alpha * luma(cols[j]);
                }
                add_row(&row, alpha * tg_y);
                // Black (target-0) rows have undefined chromaticity: keep them luma-only so
                // chroma_weight does not penalise R/B leakage off the dark anchors [0x83]/[0xA7].
                if c > 0.0 && tg_y > 1e-6 {
                    for ch in 0..3 {
                        for j in 0..k {
                            row[j] = alpha * c * cols[j][ch] as f64;
                        }
                        add_row(&row, alpha * c * tg[ch] as f64);
                    }
                }
            }
            if n_rows == 0 {
                w[darkest_option(u, v)] = 1.0;
                return (w, 0.0, 0.0);
            }
            let w64 = solve_simplex_qp(&gram, &rhs, k);
            let mut err = b_sq;
            for j in 0..k {
                let mut gw = 0.0f64;
                let g_row = &gram[j * k..(j + 1) * k];
                for m in 0..k {
                    gw += g_row[m] * w64[m];
                }
                err += w64[j] * (gw - 2.0 * rhs[j]);
                w[j] = w64[j] as f32;
            }
            (w, err.max(0.0), n_rows as f64)
        })
        .collect();
    let (mut err, mut rows) = (0.0, 0.0);
    let mut weights = Vec::with_capacity(n * n * k);
    for (w, e, r) in cells {
        weights.extend(w);
        err += e;
        rows += r;
    }
    Ok(Solved { grid: n, options: k, weights, error: err / rows.max(1.0) })
}

/// The sequence a scene's empty cells are given: one byte repeated, 1.172 lines per µm, dark
/// wherever `|g_t|` is below about 0.5.
pub const BACKGROUND_BYTE: u8 = 0xA7;

/// Gives every cell that is black in all views the same background sequence.
///
/// Solved alone, such cells take whichever sequences happen to be dark at the designed poses.
/// Those differ from place to place and put all their light into narrow lines, so from any
/// other pose the background turns into bright rainbow streaks. One high-frequency sequence
/// is dark for every lamp and eye whose `|g_t|` stays below 0.5, all round the disc.
pub fn uniform_background(solved: &mut Solved, input: &DesignInput, cb: &Codebook) -> Result<(), String> {
    let Some(bg) = cb.entries.iter().position(|e| e.bytes.iter().all(|&b| b == BACKGROUND_BYTE)) else {
        return Err("码本里没有背景序列".into());
    };
    let (n, k) = (solved.grid, solved.options);
    let targets: Vec<Target> = input.views.iter().map(|v| load_target(&v.image_path, n, 1.0)).collect::<Result<_, _>>()?;
    for i in 0..n * n {
        if targets.iter().all(|t| t.rgb[i].iter().all(|&c| c < 0.004)) {
            let w = &mut solved.weights[i * k..(i + 1) * k];
            w.fill(0.0);
            w[bg] = 1.0;
        }
    }
    Ok(())
}

/// The groove's first-order rainbow at `g_t = 0`, linear RGB: `|g_r| = lambda / pitch`. Away
/// from zero it falls off as `exp(-g_t^2 / (2 GROOVE_SOURCE^2))`.
pub fn groove_rainbow(gr: f64, pitch_um: f64) -> [f32; 3] {
    let s2 = 2.0 * GROOVE_SOURCE * GROOVE_SOURCE;
    let mut xyz = [0.0f64; 3];
    for (i, w) in optics::cmf_table().iter().enumerate() {
        let lam = (optics::NM_FIRST + i as f64 * optics::NM_STEP) * 1e-3;
        let d = gr.abs() - lam / pitch_um;
        let p = (-(d * d) / s2).exp();
        for c in 0..3 {
            xyz[c] += p * w[c];
        }
    }
    optics::xyz_to_linear_srgb(xyz).map(|v| v.max(0.0) as f32)
}

/// What the disc looks like from one view, as RGBA, `size` square. The groove's own
/// rainbow (radial, 0.74 µm) is drawn too, since it is there on every real disc.
pub fn render(solved: &Solved, resp: &Responses, preset: &DiscGeometryPreset, view: &ViewSpec, size: usize, groove: f32) -> Vec<u8> {
    let geom = ViewGeom::from_view(view);
    let ro = preset.outer_radius_mm;
    let r_in = preset.inner_radius_mm / (2.0 * ro);
    let cmf = optics::cmf_table();
    let pitch_um = preset.base_pitch_um;
    let s2 = 2.0 * GROOVE_SOURCE * GROOVE_SOURCE;
    let groove_rgb = |gt: f64, gr: f64| -> [f32; 3] {
        if groove == 0.0 {
            return [0.0; 3];
        }
        if view.light_mode == LightMode::Far {
            // Legacy far-field narrow spectral line at lambda = |g_r| * pitch.
            let nm = gr.abs() * pitch_um * 1000.0;
            if !(380.0..=720.0).contains(&nm) {
                return [0.0; 3];
            }
            let x = optics::xyz_to_linear_srgb(optics::cie_xyz(nm));
            return [x[0].max(0.0) as f32 * 0.3, x[1].max(0.0) as f32 * 0.3, x[2].max(0.0) as f32 * 0.3];
        }
        // Near-field groove first-order rainbow: |g_r| = lambda / pitch and g_t ≈ 0.
        if gt * gt > 16.0 * s2 {
            return [0.0; 3];
        }
        let mut xyz = [0.0f64; 3];
        for (i, w) in cmf.iter().enumerate() {
            let lam = (optics::NM_FIRST + i as f64 * optics::NM_STEP) * 1e-3;
            let d = gr.abs() - lam / pitch_um;
            let p = (-(gt * gt + d * d) / s2).exp();
            xyz[0] += p * w[0];
            xyz[1] += p * w[1];
            xyz[2] += p * w[2];
        }
        let x = optics::xyz_to_linear_srgb(xyz);
        [x[0].max(0.0) as f32, x[1].max(0.0) as f32, x[2].max(0.0) as f32]
    };
    let enc = |l: f32| -> u8 {
        let l = l.clamp(0.0, 1.0);
        let s = if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0).round() as u8
    };
    let rows: Vec<Vec<u8>> = (0..size)
        .into_par_iter()
        .map(|y| {
            let mut row = Vec::with_capacity(size * 4);
            for x in 0..size {
                let (u, v) = ((x as f64 + 0.5) / size as f64, (y as f64 + 0.5) / size as f64);
                let (r, _) = track_angle(u, v);
                if r > 0.5 || r < r_in {
                    row.extend([0, 0, 0, 0]);
                    continue;
                }
                let (gt, gr_marks, gr_groove) = geom.g_at(u, v, ro);
                let mut c = [0.0f32; 3];
                if let Some(w) = solved.cell(u, v) {
                    for (k, &wk) in w.iter().enumerate() {
                        if wk > 0.0 {
                            let p = resp.at(k, gt, gr_marks);
                            for ch in 0..3 {
                                c[ch] += wk * p[ch];
                            }
                        }
                    }
                }
                let gr = groove_rgb(gt, gr_groove);
                row.extend([enc(c[0] + groove * gr[0]), enc(c[1] + groove * gr[1]), enc(c[2] + groove * gr[2]), 255]);
            }
            row
        })
        .collect();
    rows.concat()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignSummary {
    pub output_iso_path: String,
    pub total_sectors: u32,
    /// Share of sync frames given to each option, plain data last.
    pub usage: Vec<f64>,
}

/// Low-discrepancy threshold for the `j`-th sync frame on the disc.
#[inline]
fn frame_threshold(j: u64) -> f32 {
    ((j.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 40) as f32) / (1u64 << 24) as f32
}

/// Build a DVD ISO image from a solved cell-weight grid and its matching codebook.
///
/// Option indices `0..cb.entries.len()` write their codebook byte runs into each frame
/// slot (after SYNC); option index `cb.entries.len()` leaves the slot as plain host zero
/// data. ISO 9660 system sectors (head + tail) are always left plain regardless of weights.
pub fn build_iso(
    preset: &DiscGeometryPreset,
    cb: &Codebook,
    solved: &Solved,
    flat: Option<(&super::flat::Flat, crate::carver::Tone)>,
    output_iso_path: &str,
    cancel: &AtomicBool,
    progress: &(dyn Fn(CarveProgress) + Sync),
) -> Result<DesignSummary, String> {
    if preset.media != crate::media::Media::Dvd {
        return Err("全息设计只支持 DVD".into());
    }
    let plain = cb.entries.len();
    let k = solved.options;
    if k != plain + 1 {
        return Err(format!("码本条目数 ({plain}) 与求解选项数 ({k}) 不匹配"));
    }
    if solved.weights.len() != solved.grid * solved.grid * k {
        return Err("求解权重尺寸不匹配".into());
    }
    let spiral = Spiral::new(preset.spiral_params())?;
    let total = preset.total_sectors;
    let path = Path::new(output_iso_path);
    let mut map = create_mapped(path, total as u64 * SECTOR_BYTES as u64)?;
    let lut = build_scrambler_lut();
    let slots = frame_slots();
    let ro2 = 2.0 * spiral.params.ro;
    let usage: Vec<AtomicU64> = (0..k).map(|_| AtomicU64::new(0)).collect();
    let done = AtomicU64::new(0);
    let blocks = (total as u64).div_ceil(ECC_BLOCK_SECTORS as u64);
    let tick = (blocks / 200).max(1);
    let run = map.par_chunks_mut(ECC_BLOCK_BYTES).enumerate().try_for_each(|(b, chunk)| {
        if cancel.load(Ordering::Relaxed) {
            return Err(());
        }
        let mut local = vec![0u64; k];
        let mut acc = crate::carver::block_phase(b as u32);
        for (i, s) in chunk.chunks_exact_mut(SECTOR_BYTES).enumerate() {
            let lba = (b * ECC_BLOCK_SECTORS + i) as u32;
            if crate::iso9660::is_system_sector(lba, total) {
                local[plain] += slots.len() as u64;
                continue;
            }
            let key = &lut[seed_index(lba)];
            // Plain data unless a slot is given an entry.
            s.copy_from_slice(key);
            for (si, slot) in slots.iter().enumerate() {
                let mid = lba as f64 * SECTOR_CHANNEL_BITS as f64 + main_byte_bit(slot.first) as f64 + slot.len as f64 * 8.0;
                let (r, th) = spiral.locate(mid);
                let (u, v) = (0.5 + r * th.cos() / ro2, 0.5 + r * th.sin() / ro2);
                let Some(w) = solved.cell(u, v) else {
                    local[plain] += 1;
                    continue;
                };
                let mut pick = plain;
                let mut acc = 0.0f32;
                let thr = frame_threshold(lba as u64 * 32 + si as u64);
                for (o, &wo) in w.iter().enumerate() {
                    acc += wo;
                    if thr < acc {
                        pick = o;
                        break;
                    }
                }
                local[pick] += 1;
                if let Some(entry) = cb.entries.get(pick) {
                    s[slot.first..slot.first + slot.len].copy_from_slice(&entry.bytes[..slot.len]);
                }
            }
            // Flat positions' pictures, carved over whatever the slots held there.
            if let Some((f, tone)) = flat {
                crate::carver::for_each_sample(&spiral, lba, |r, x, y| {
                    if let Some(grey) = f.sample(x, y) {
                        crate::carver::tone_run(tone, grey, &mut acc, crate::carver::run_of(s, r));
                    }
                });
            }
            xor_sector(&lut, lba, s);
        }
        for (u, l) in usage.iter().zip(local) {
            u.fetch_add(l, Ordering::Relaxed);
        }
        let d = done.fetch_add(1, Ordering::Relaxed) + 1;
        if d % tick == 0 {
            progress(CarveProgress { percent: d as f64 / blocks as f64 * 100.0, ..Default::default() });
        }
        Ok(())
    });
    if run.is_err() {
        drop(map);
        let _ = std::fs::remove_file(path);
        return Err("已取消".into());
    }
    crate::iso9660::write_system_area(&mut map, crate::iso9660::HOLO_VOLUME, total);
    map.flush().map_err(|e| e.to_string())?;
    let all: u64 = usage.iter().map(|u| u.load(Ordering::Relaxed)).sum();
    Ok(DesignSummary {
        output_iso_path: output_iso_path.into(),
        total_sectors: total,
        usage: usage.iter().map(|u| u.load(Ordering::Relaxed) as f64 / all.max(1) as f64).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::reference_preset;

    #[test]
    fn carved_bytes_are_simulated_by_their_share_of_light() {
        for l in 0..CARVE_LEVELS {
            let p = l as f64 / (CARVE_LEVELS - 1) as f64;
            let light = carve_bytes(p).iter().filter(|&&b| b == crate::ecma267::LIGHT_BYTE).count() as f64 / CORE_BYTES as f64;
            assert!((light - p).abs() <= 1.0 / CORE_BYTES as f64 + 1e-9, "level {l}: {light} light, wanted {p}");
        }
        let looks = carve_looks(0.1333);
        assert_eq!(looks.len(), CARVE_LEVELS);
        let luma = |v: &Vec<[f32; 3]>| v.iter().map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]).sum::<f32>();
        let all: Vec<f32> = looks.iter().map(luma).collect();
        assert!(all.iter().all(|v| v.is_finite() && *v > 0.0), "{all:?}");
        // Dark and light bytes are different marks, so they diffract differently.
        assert!((all[0] - all[CARVE_LEVELS - 1]).abs() > 0.05 * all[0].max(all[CARVE_LEVELS - 1]), "{all:?}");
    }

    #[test]
    fn simplex_projection() {
        let mut w = [0.5f32, 0.8, -0.2];
        project_simplex(&mut w);
        assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(w.iter().all(|&x| x >= 0.0));
    }

    #[test]
    fn view_spec_serde_is_backward_compatible() {
        let legacy = r#"{"imagePath":"a.png","angleDeg":20.0,"lightDeg":5.0,"dirDeg":-90.0}"#;
        let v: ViewSpec = serde_json::from_str(legacy).unwrap();
        assert_eq!(v.distance_mm, DEFAULT_DISTANCE_MM);
        assert_eq!(v.light_mode, LightMode::Flash);
        assert_eq!(v.light_offset_mm, None);
        assert_eq!(v.light_mm, None);
        assert_eq!(v.eye_mm, None);

        let far_json = r#"{"imagePath":"a.png","angleDeg":20.0,"lightDeg":5.0,"lightMode":"far"}"#;
        let f: ViewSpec = serde_json::from_str(far_json).unwrap();
        assert_eq!(f.light_mode, LightMode::Far);
        assert_eq!(f.dir_deg, -90.0);
        assert_eq!(f.light_mm, None);
        assert_eq!(f.eye_mm, None);

        let lamp_json = r#"{"imagePath":"a.png","angleDeg":30.0,"lightMode":"lamp","lightMm":[0.0,200.0,1200.0],"eyeMm":[-80.0,-175.0,303.1]}"#;
        let l: ViewSpec = serde_json::from_str(lamp_json).unwrap();
        assert_eq!(l.light_mode, LightMode::Lamp);
        assert_eq!(l.light_mm, Some([0.0, 200.0, 1200.0]));
        assert_eq!(l.eye_mm, Some([-80.0, -175.0, 303.1]));
    }

    /// As distance -> infinity, near-field flash and lamp geometry must reproduce the old
    /// far-field g_t = (sin eye + sin light) * |sin(dir - track angle)| and leave existing
    /// Flash / Far modes unchanged.
    #[test]
    fn far_limit_reproduces_old_gt() {
        let ro = reference_preset().outer_radius_mm;
        for &(angle_deg, dir_deg) in &[(15.0, -90.0), (32.0, -90.0), (25.0, 20.0), (40.0, -135.0)] {
            let near = ViewSpec {
                image_path: String::new(),
                angle_deg,
                light_deg: angle_deg,
                dir_deg,
                distance_mm: 1e9,
                light_mode: LightMode::Flash,
                light_offset_mm: None,
                light_mm: None,
                eye_mm: None,
            };
            let far = ViewSpec { light_mode: LightMode::Far, ..near.clone() };
            let gn = ViewGeom::from_view(&near);
            let gf = ViewGeom::from_view(&far);
            for &(u, v) in &[(0.15, 0.5), (0.85, 0.5), (0.5, 0.15), (0.7, 0.25), (0.3, 0.78)] {
                let (gt_n, gr_n, _) = gn.g_at(u, v, ro);
                let (gt_f, _, gr_f) = gf.g_at(u, v, ro);
                let (_, t) = track_angle(u, v);
                let expected_gt = 2.0 * angle_deg.to_radians().sin() * (dir_deg.to_radians() - t).sin().abs();
                assert!((gt_n.abs() - expected_gt).abs() < 1e-5, "gt near {gt_n} vs old {expected_gt}");
                assert!((gt_n - gt_f).abs() < 1e-5, "gt near {gt_n} vs far {gt_f}");
                assert!((gr_n - gr_f).abs() < 1e-5, "gr near {gr_n} vs far {gr_f}");
            }
        }
        // Lamp mode far-field limit with distinct eye and lamp angles in the same azimuth plane.
        for &(eye_deg, light_deg, dir_deg) in &[(30.0_f64, -10.0_f64, -90.0_f64), (25.0, 15.0, -90.0), (35.0, 5.0, 20.0)] {
            let far_d = 1e9_f64;
            let dir = dir_deg.to_radians();
            let ea = eye_deg.to_radians();
            let la = light_deg.to_radians();
            let eye = [far_d * ea.sin() * dir.cos(), far_d * ea.sin() * dir.sin(), far_d * ea.cos()];
            let lamp_pos = [far_d * la.sin() * dir.cos(), far_d * la.sin() * dir.sin(), far_d * la.cos()];
            let lamp_view = ViewSpec::lamp("", eye, lamp_pos);
            let far_view = ViewSpec {
                angle_deg: eye_deg,
                light_deg,
                dir_deg,
                light_mode: LightMode::Far,
                ..ViewSpec::flash("", eye_deg)
            };
            let gl = ViewGeom::from_view(&lamp_view);
            let gf = ViewGeom::from_view(&far_view);
            for &(u, v) in &[(0.15, 0.5), (0.85, 0.5), (0.5, 0.15), (0.7, 0.25), (0.3, 0.78)] {
                let (gt_l, gr_l, _) = gl.g_at(u, v, ro);
                let (gt_f, _, gr_f) = gf.g_at(u, v, ro);
                assert!((gt_l - gt_f).abs() < 1e-5, "lamp far gt {gt_l} vs far {gt_f}");
                assert!((gr_l - gr_f).abs() < 1e-5, "lamp far gr {gr_l} vs far {gr_f}");
            }
        }
    }

    #[test]
    fn head_motion_views_exact_positions() {
        let views = ViewSpec::head_motion_views(5, 350.0, 30.0, -90.0, 80.0, DEFAULT_LAMP_MM);
        assert_eq!(views.len(), 5);
        let expected_y = -350.0 * 30.0_f64.to_radians().sin();
        let expected_z = 350.0 * 30.0_f64.to_radians().cos();
        for (i, v) in views.iter().enumerate() {
            assert_eq!(v.light_mode, LightMode::Lamp);
            assert_eq!(v.light_mm, Some(DEFAULT_LAMP_MM));
            let eye = v.eye_pos();
            let expected_x = -80.0 + 40.0 * i as f64;
            assert!((eye[0] - expected_x).abs() < 1e-12, "view {i} x: {}", eye[0]);
            assert!((eye[1] - expected_y).abs() < 1e-12, "view {i} y: {}", eye[1]);
            assert!((eye[2] - expected_z).abs() < 1e-12, "view {i} z: {}", eye[2]);
            assert_eq!(v.scene().camera_mm, eye);
            assert_eq!(v.scene().light_mm, DEFAULT_LAMP_MM);
        }
    }

    fn write_png(path: &Path, f: impl Fn(u32, u32) -> [u8; 4]) {
        let img = image::RgbaImage::from_fn(64, 64, |x, y| image::Rgba(f(x, y)));
        img.save(path).unwrap();
    }

    /// Two views, two opposite pictures: the left half bright in view A only, the right
    /// half bright in view B only. The solution must show each where it was asked, in both
    /// the default near-field flash mode and the legacy far-field mode.
    #[test]
    fn two_views_are_kept_apart() {
        let dir = std::env::temp_dir().join(format!("holo-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.png"), dir.join("b.png"));
        write_png(&a, |x, _| if x < 32 { [255; 4] } else { [0, 0, 0, 255] });
        write_png(&b, |x, _| if x >= 32 { [255; 4] } else { [0, 0, 0, 255] });
        let shared = shared_codebook(0.1333);
        let (_, resp) = &*shared;
        let preset = reference_preset();
        let ro = preset.outer_radius_mm;

        let check = |label: &str, views: Vec<ViewSpec>| {
            let input = DesignInput { views: views.clone(), chroma_weight: 0.0, exposure: 0.5, grid: 64 };
            let solved = solve(&input, resp, &preset).unwrap();
            let luma = |view: &ViewSpec, u: f64| {
                let w = solved.cell(u, 0.5).unwrap();
                let (gt, gr, _) = ViewGeom::from_view(view).g_at(u, 0.5, ro);
                let mut y = 0.0;
                for (k, &wk) in w.iter().enumerate() {
                    let p = resp.at(k, gt, gr);
                    y += wk * (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]);
                }
                y
            };
            let (l_a, r_a, l_b, r_b) = (luma(&views[0], 0.15), luma(&views[0], 0.85), luma(&views[1], 0.15), luma(&views[1], 0.85));
            assert!(l_a > 2.0 * r_a, "{label} view A: left {l_a} right {r_a}");
            assert!(r_b > 2.0 * l_b, "{label} view B: left {l_b} right {r_b}");
            let px = render(&solved, resp, &preset, &views[0], 64, 0.0);
            assert_eq!(px.len(), 64 * 64 * 4);
        };

        // Default near-field flash mode (distance 350 mm): g ≈ 2 sin(angle), so 10° and 22°
        // sit cleanly inside the codebook band at the horizontal equator.
        check("flash", vec![ViewSpec::flash(a.to_string_lossy(), 10.0), ViewSpec::flash(b.to_string_lossy(), 22.0)]);
        // Legacy far-field mode: g = sin(angle) + sin(light), reproducing the original test.
        check(
            "far",
            vec![
                ViewSpec { light_mode: LightMode::Far, ..ViewSpec::flash(a.to_string_lossy(), 15.0) },
                ViewSpec { light_mode: LightMode::Far, ..ViewSpec::flash(b.to_string_lossy(), 40.0) },
            ],
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn near_field_groove_rainbow_lands_where_gt_is_zero() {
        let shared = shared_codebook(0.1333);
        let (_, resp) = &*shared;
        let preset = reference_preset();
        let grid = 32;
        // Zero mark weights so only the groove rainbow contributes.
        let solved = Solved { grid, options: resp.options, weights: vec![0.0; grid * grid * resp.options], error: 0.0 };
        let view = ViewSpec::flash("", 22.0);
        let size = 128;
        let px = render(&solved, resp, &preset, &view, size, 1.0);
        let luma_at = |u: f64, v: f64| -> u32 {
            let x = (u * size as f64) as usize;
            let y = (v * size as f64) as usize;
            let i = (y * size + x) * 4;
            px[i] as u32 + px[i + 1] as u32 + px[i + 2] as u32
        };
        // Along the vertical midline (x ≈ 0, g_t ≈ 0, |g_r| ≈ 2 sin 22° ≈ 0.75 -> green),
        // the groove rainbow must shine; at the horizontal equator (|g_t| ≈ 0.75), it must vanish.
        let top_mid = luma_at(0.5, 0.22);
        let left_eq = luma_at(0.22, 0.5);
        assert!(top_mid > 100, "expected bright groove rainbow at vertical midline, got {top_mid}");
        assert_eq!(left_eq, 0, "expected no groove rainbow at horizontal equator, got {left_eq}");
    }

    #[test]
    fn build_iso_writes_readable_system_area() {
        let shared = shared_codebook(0.1333);
        let (cb, resp) = &*shared;
        let total = 16 * 80;
        let preset = DiscGeometryPreset { total_sectors: total, ..reference_preset() };
        // Every cell puts 100% weight on codebook entry 0, so without the system area
        // guard every sector from LBA 0 onward would carry diffraction bytes.
        let grid = 16;
        let k = resp.options;
        let mut weights = vec![0.0f32; grid * grid * k];
        for c in 0..grid * grid {
            weights[c * k] = 1.0;
        }
        let solved = Solved { grid, options: k, weights, error: 0.0 };
        let dir = std::env::temp_dir().join(format!("holo-design-sys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let iso = dir.join("design.iso");
        let sum = build_iso(&preset, cb, &solved, None, iso.to_str().unwrap(), &AtomicBool::new(false), &|_| {}).unwrap();
        assert_eq!(sum.total_sectors, total);
        let bytes = std::fs::read(&iso).unwrap();
        crate::iso9660::tests::assert_system_area(&bytes, crate::iso9660::HOLO_VOLUME, total);
        // Middle sectors still carry the codebook pattern (non-zero host bytes after XOR).
        let mid = &bytes[600 * SECTOR_BYTES..601 * SECTOR_BYTES];
        assert!(mid.iter().any(|&b| b != 0), "middle sector must hold hologram data");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn build_iso_writes_extended_options_and_preserves_system_area() {
        let shared = shared_codebook(0.1333);
        let (cb, resp) = &*shared;
        assert!(cb.entries.len() > super::super::swatch::CODEBOOK_ENTRIES);
        assert_eq!(resp.options, cb.entries.len() + 1);

        let total = 16 * 80;
        let preset = DiscGeometryPreset { total_sectors: total, ..reference_preset() };
        let grid = 16;
        let k = resp.options;
        let extra_idx = cb.entries.len() - 1; // last extended periodic entry
        let plain_idx = cb.entries.len();

        // Put 100% weight on an extra option (>= 12) and verify every middle sector slot
        // decodes (after descrambling) to that exact entry's bytes, while system sectors stay plain.
        let mut weights = vec![0.0f32; grid * grid * k];
        for c in 0..grid * grid {
            weights[c * k + extra_idx] = 1.0;
        }
        let solved = Solved { grid, options: k, weights, error: 0.0 };
        let dir = std::env::temp_dir().join(format!("holo-design-ext-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let iso = dir.join("design-ext.iso");
        let sum = build_iso(&preset, cb, &solved, None, iso.to_str().unwrap(), &AtomicBool::new(false), &|_| {}).unwrap();
        assert_eq!(sum.usage.len(), k);
        let sys_sectors = (crate::iso9660::SYSTEM_HEAD_SECTORS + crate::iso9660::SYSTEM_TAIL_SECTORS) as f64;
        let expected_extra = (total as f64 - sys_sectors) / total as f64;
        let expected_plain = sys_sectors / total as f64;
        assert!(
            (sum.usage[extra_idx] - expected_extra).abs() < 1e-6,
            "expected extra option usage {expected_extra:.6}, got {:.6}",
            sum.usage[extra_idx]
        );
        assert!(
            (sum.usage[plain_idx] - expected_plain).abs() < 1e-6,
            "expected system plain usage {expected_plain:.6}, got {:.6}",
            sum.usage[plain_idx]
        );

        let bytes = std::fs::read(&iso).unwrap();
        crate::iso9660::tests::assert_system_area(&bytes, crate::iso9660::HOLO_VOLUME, total);

        let lut = build_scrambler_lut();
        let slots = frame_slots();
        let lba = 600u32;
        let mut sec = bytes[lba as usize * SECTOR_BYTES..(lba as usize + 1) * SECTOR_BYTES].to_vec();
        xor_sector(&lut, lba, &mut sec);
        for slot in &slots {
            assert_eq!(
                &sec[slot.first..slot.first + slot.len],
                &cb.entries[extra_idx].bytes[..slot.len],
                "slot {} must carry extended entry {extra_idx}",
                slot.first
            );
        }

        // Mismatched codebook vs solved option count must return an error rather than panic.
        let swatch = swatch_codebook(0.1333);
        assert!(build_iso(&preset, &swatch, &solved, None, iso.to_str().unwrap(), &AtomicBool::new(false), &|_| {}).is_err());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn masked_off_cells_use_dark_entry_not_plain() {
        let dir = std::env::temp_dir().join(format!("holo-masked-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img_path = dir.join("masked.png");
        // Left half opaque white, right half fully transparent (alpha = 0).
        write_png(&img_path, |x, _| if x < 32 { [255, 255, 255, 255] } else { [0, 0, 0, 0] });
        let shared = shared_codebook(0.1333);
        let (_, resp) = &*shared;
        let preset = reference_preset();
        let views = ViewSpec::head_motion_views(8, 350.0, 20.0, -90.0, 80.0, HELD_FLASHLIGHT_MM)
            .into_iter()
            .map(|v| ViewSpec { image_path: img_path.to_string_lossy().into_owned(), ..v })
            .collect();
        let input = DesignInput { views, chroma_weight: 0.0, exposure: 0.6, grid: 32 };
        let solved = solve(&input, resp, &preset).unwrap();
        let plain = resp.options - 1;
        // Sample masked-off cell inside the ring on the right side (u = 0.78, v = 0.5).
        let w = solved.cell(0.78, 0.5).unwrap();
        assert_eq!(w[plain], 0.0, "masked-off cell should use a dark codebook entry, not plain");
        assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn fista_converges_to_kkt_optimum() {
        // Construct a realistic ill-conditioned simplex QP from the extended codebook responses.
        let shared = shared_codebook(0.1333);
        let (_, resp) = &*shared;
        let preset = reference_preset();
        let ro = preset.outer_radius_mm;
        let views: Vec<ViewGeom> = ViewSpec::head_motion_views(8, 350.0, 20.0, -90.0, 80.0, HELD_FLASHLIGHT_MM)
            .iter()
            .map(ViewGeom::from_view)
            .collect();
        let k = resp.options;
        let luma = |p: [f32; 3]| 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64;
        // Parallax edge target at near band (u = 0.5, v = 0.15): bright in first 4 views, dark in last 4.
        let mut gram = vec![0.0f64; k * k];
        let mut rhs = vec![0.0f64; k];
        for (vi, geom) in views.iter().enumerate() {
            let (gt, gr, _) = geom.g_at(0.5, 0.15, ro);
            let bi = if vi < 4 { 0.6 } else { 0.0 };
            let row: Vec<f64> = (0..k).map(|j| luma(resp.at(j, gt, gr))).collect();
            for r in 0..k {
                rhs[r] += row[r] * bi;
                for col in 0..k {
                    gram[r * k + col] += row[r] * row[col];
                }
            }
        }
        let w = solve_simplex_qp(&gram, &rhs, k);
        assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!(w.iter().all(|&x| x >= -1e-12));
        // KKT optimality on the simplex: min_j g_j >= w^T g - eps (Frank-Wolfe duality gap).
        let mut grad = vec![0.0f64; k];
        for j in 0..k {
            let mut g = -rhs[j];
            for m in 0..k {
                g += gram[j * k + m] * w[m];
            }
            grad[j] = g;
        }
        let wg: f64 = w.iter().zip(&grad).map(|(a, b)| a * b).sum();
        let min_g = grad.iter().copied().fold(f64::INFINITY, f64::min);
        let fw_gap = wg - min_g;
        assert!(fw_gap < 1e-5, "FISTA Frank-Wolfe duality gap too large: {fw_gap}");
    }

    #[test]
    fn empty_cells_share_one_background_that_stays_dark() {
        let dir = std::env::temp_dir().join(format!("holo-bg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("half.png");
        write_png(&img, |_, y| if y < 32 { [200, 200, 200, 255] } else { [0, 0, 0, 255] });
        let shared = shared_codebook(0.1333);
        let (cb, resp) = (&shared.0, &shared.1);
        let views = vec![ViewSpec { image_path: img.to_string_lossy().into_owned(), ..ViewSpec::flash("", 15.0) }];
        let input = DesignInput { views, chroma_weight: 0.0, exposure: 0.6, grid: 32 };
        let mut s = solve(&input, resp, &reference_preset()).unwrap();
        let lit = s.cell(0.5, 0.15).unwrap().to_vec();
        uniform_background(&mut s, &input, cb).unwrap();
        assert_eq!(s.cell(0.5, 0.15).unwrap(), &lit[..], "the subject keeps its weights");
        let bg = s.cell(0.5, 0.85).unwrap().iter().position(|&w| w == 1.0).unwrap();
        for g in 0..=45 {
            let p = resp.at(bg, g as f64 / 100.0, 0.0);
            assert!(0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2] < 0.06, "g_t {g}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn target_zero_rows_stay_luma_only_with_chroma_weight() {
        let dir = std::env::temp_dir().join(format!("holo-subj-chroma-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img_path = dir.join("half.png");
        // Top half (near band, y < 32) opaque neutral grey; bottom half opaque black.
        write_png(&img_path, |_, y| if y < 32 { [200, 200, 200, 255] } else { [0, 0, 0, 255] });
        let shared = shared_codebook(0.1333);
        let (_, resp) = &*shared;
        let preset = reference_preset();
        let views: Vec<ViewSpec> = ViewSpec::head_motion_views(8, 350.0, 20.0, -90.0, 80.0, HELD_FLASHLIGHT_MM)
            .into_iter()
            .map(|v| ViewSpec { image_path: img_path.to_string_lossy().into_owned(), ..v })
            .collect();
        let s0 = solve(&DesignInput { views: views.clone(), chroma_weight: 0.0, exposure: 0.6, grid: 32 }, resp, &preset).unwrap();
        let s3 = solve(&DesignInput { views: views.clone(), chroma_weight: 0.3, exposure: 0.6, grid: 32 }, resp, &preset).unwrap();

        // Dark background cell in far band (u = 0.5, v = 0.85): weights must match luma-only exactly.
        let w0_dark = s0.cell(0.5, 0.85).unwrap();
        let w3_dark = s3.cell(0.5, 0.85).unwrap();
        for (a, b) in w0_dark.iter().zip(w3_dark) {
            assert!((a - b).abs() < 1e-6, "black target cell should stay luma-only: {a} vs {b}");
        }

        // Bright grey subject cell in near band (u = 0.5, v = 0.15): chroma_weight=0.3 must reduce RGB spread.
        let w0_lit = s0.cell(0.5, 0.15).unwrap();
        let w3_lit = s3.cell(0.5, 0.15).unwrap();
        let spread = |w: &[f32]| -> f64 {
            let ro = preset.outer_radius_mm;
            views
                .iter()
                .take(5)
                .map(|v| {
                    let (gt, gr, _) = ViewGeom::from_view(v).g_at(0.5, 0.15, ro);
                    let mut rgb = [0.0f64; 3];
                    for (j, &wj) in w.iter().enumerate() {
                        let p = resp.at(j, gt, gr);
                        for ch in 0..3 {
                            rgb[ch] += wj as f64 * p[ch] as f64;
                        }
                    }
                    let mx = rgb[0].max(rgb[1]).max(rgb[2]);
                    let mn = rgb[0].min(rgb[1]).min(rgb[2]);
                    (mx - mn) / mx.max(1e-6)
                })
                .sum::<f64>()
                / 5.0
        };
        assert!(
            spread(w3_lit) < spread(w0_lit) - 0.1,
            "bright neutral subject should be significantly less saturated with chroma_weight=0.3: {:.3} vs {:.3}",
            spread(w3_lit),
            spread(w0_lit)
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod look {
    use super::*;

    /// `HOLO_OUT=dir cargo test -p disc-core --release look -- --ignored`: renders two
    /// views of a two-picture design, for checking by eye.
    #[test]
    #[ignore]
    fn render_two_views() {
        let out = std::path::PathBuf::from(std::env::var("HOLO_OUT").unwrap());
        let (a, b) = (out.join("a.png"), out.join("b.png"));
        let ring = |x: u32, y: u32| ((x as f64 - 128.0).hypot(y as f64 - 128.0) / 20.0) as u32;
        image::RgbaImage::from_fn(256, 256, |x, _| image::Rgba(if (x / 32) % 2 == 0 { [255, 255, 255, 255] } else { [0, 0, 0, 255] })).save(&a).unwrap();
        image::RgbaImage::from_fn(256, 256, |x, y| image::Rgba(if ring(x, y) % 2 == 0 { [255, 255, 255, 255] } else { [0, 0, 0, 255] })).save(&b).unwrap();
        let shared = shared_codebook(0.1333);
        let preset = crate::preset::reference_preset();
        let views = vec![ViewSpec::flash(a.to_string_lossy(), 10.0), ViewSpec::flash(b.to_string_lossy(), 22.0)];
        let input = DesignInput { views: views.clone(), chroma_weight: 0.0, exposure: 0.6, grid: 128 };
        let solved = solve(&input, &shared.1, &preset).unwrap();
        println!("error {}", solved.error);
        for deg in [10.0, 15.0, 22.0, 30.0, 40.0] {
            let v = ViewSpec { angle_deg: deg, ..views[0].clone() };
            let px = render(&solved, &shared.1, &preset, &v, 400, 1.0);
            image::RgbaImage::from_raw(400, 400, px).unwrap().save(out.join(format!("view-{deg}.png"))).unwrap();
        }
    }
}
