//! Light-field design: solve a 3D scene so that every head position sees exactly one image.
//!
//! Why ghosts appear: a disc cell scatters light with
//! `Y(h) = sum_j w_j A_j(h)`, where `h` is the head position and `A_j` the luminance of
//! codebook entry `j`. Under white light each entry is one spectral line, and a line only
//! covers about 1.2x in `g_t` (the width of V(lambda)), so across the head sweep the columns
//! `A_j(h)` span only a handful of independent shapes. Targets that vary faster than that in
//! `h` cannot be reproduced. A least-squares fit to N sample views then rings and mixes
//! neighbouring views, which shows up as doubled edges.
//!
//! Remedy (Zwicker et al., EGSR 2006, "display depth of field"): band-limit the target
//! light field to what the cell can show *before* solving. This module renders float
//! targets straight from the meshes, averages them over an eye kernel matched to the local
//! resolution (the number of independent views, [`independent_views`]), optionally scales
//! the disparity by disc position, and solves with the converged simplex QP from `design`.
//!
//! Everything here is plain Rust and Tauri-free.

use super::codebook::Codebook;
use super::design::{self, Responses};
use super::model3d;
use super::optics::{self, Spectrum};
use super::scene;
use rayon::prelude::*;

/// Lowest wavelength of the fine grid used for illuminant responses, nm.
const NM0: f64 = 400.0;
/// Step of that grid, nm.
const NM_DX: f64 = 2.0;
/// Number of grid points (400..=700 nm).
const NM_N: usize = 151;

/// Share of sync frames the drive writes itself (SYNC, header, EDC, parity): modelled as plain data.
pub const PLAIN_LEAK: f64 = 0.153;

/// Light source spectrum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Illuminant {
    /// Equal-energy white (a halogen bulb is close enough).
    White,
    /// Phone flash LED, assumed shape: 450 nm blue pump (sigma 10 nm) plus 565 nm phosphor
    /// (sigma 38/56 nm, 0.56 of the pump). Not measured.
    PhoneLed,
    /// Gaussian band: centre and full width at half maximum, nm (a green filter or LED).
    Band { center_nm: f64, fwhm_nm: f64 },
}

impl Illuminant {
    /// Relative power at `nm`.
    pub fn power(&self, nm: f64) -> f64 {
        match *self {
            Illuminant::White => 1.0,
            Illuminant::PhoneLed => {
                let blue = (-0.5 * ((nm - 450.0) / 10.0).powi(2)).exp();
                let sig = if nm < 565.0 { 38.0 } else { 56.0 };
                blue + 0.56 * (-0.5 * ((nm - 565.0) / sig).powi(2)).exp()
            }
            Illuminant::Band { center_nm, fwhm_nm } => {
                let s = fwhm_nm / 2.354_820_045;
                (-0.5 * ((nm - center_nm) / s).powi(2)).exp()
            }
        }
    }

    pub fn label(&self) -> String {
        match *self {
            Illuminant::White => "white".into(),
            Illuminant::PhoneLed => "phone LED".into(),
            Illuminant::Band { center_nm, fwhm_nm } => format!("{fwhm_nm:.0} nm @ {center_nm:.0}"),
        }
    }
}

/// Responses of `cb` + plain data under `illum`, in the layout `design::render` expects.
/// Luminance is scaled to the brightest of the first `ref_entries` entries plus plain data,
/// as `design::responses_with_ref_peak` does for white light.
pub fn responses_for(cb: &Codebook, ref_entries: usize, illum: Illuminant) -> Responses {
    let bit_um = cb.bit_um;
    let (source_blur, sigma_r) = (design::DEFAULT_SOURCE_BLUR, design::DEFAULT_SIGMA_R);
    let (gt_bins, gr_bins, g_max) = (400usize, 64usize, 2.0f64);
    let lams: Vec<f64> = (0..NM_N).map(|i| (NM0 + i as f64 * NM_DX) * 1e-3).collect();
    let mut cmf: Vec<[f64; 3]> = (0..NM_N).map(|i| {
        let nm = NM0 + i as f64 * NM_DX;
        let c = optics::cie_xyz(nm);
        let p = illum.power(nm);
        [c[0] * p, c[1] * p, c[2] * p]
    }).collect();
    let y_sum: f64 = cmf.iter().map(|c| c[1]).sum();
    for c in cmf.iter_mut() {
        for v in c.iter_mut() {
            *v /= y_sum;
        }
    }
    let mut specs: Vec<Spectrum> = cb.entries.iter().map(|e| design::entry_spectrum(&e.bytes, bit_um)).collect();
    specs.push(design::plain_spectrum(bit_um));
    let inv_2s2 = 1.0 / (2.0 * sigma_r * sigma_r);
    let env: Vec<Vec<f64>> = (0..gr_bins)
        .map(|ir| {
            let gr = ir as f64 / (gr_bins - 1) as f64 * g_max;
            lams.iter().map(|&lam| (-(gr / lam).powi(2) * inv_2s2).exp()).collect()
        })
        .collect();
    let raw: Vec<Vec<[f64; 3]>> = specs
        .par_iter()
        .map(|s| {
            let s = s.blurred(source_blur);
            let mut table = Vec::with_capacity(gt_bins * gr_bins);
            let mut p_lam = vec![0.0f64; NM_N];
            for it in 0..gt_bins {
                let gt = it as f64 / (gt_bins - 1) as f64 * g_max;
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
        .map(|v| v.into_iter().map(|c| [(c[0] / peak).max(0.0) as f32, (c[1] / peak).max(0.0) as f32, (c[2] / peak).max(0.0) as f32]).collect())
        .collect();
    Responses { options: specs.len(), gt_bins, gr_bins, rgb }
}

/// Rec. 709 luminance of a linear RGB response.
#[inline]
pub fn luma(p: [f32; 3]) -> f64 {
    0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64
}

/// How the viewing geometry changes along the sweep that the views are spread over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sweep {
    /// Eye moves sideways by `+-half_span_mm`, disc and lamp fixed.
    Sway { half_span_mm: f64 },
    /// Eye and lamp fixed, disc rolled about its vertical (12-6 o'clock) axis by `+-half_deg`.
    Roll { half_deg: f64 },
}

impl Sweep {
    /// Sweep coordinate of sample `i` of `n` (mm or degrees).
    pub fn coord(&self, i: usize, n: usize) -> f64 {
        let half = match *self {
            Sweep::Sway { half_span_mm } => half_span_mm,
            Sweep::Roll { half_deg } => half_deg,
        };
        if n <= 1 { 0.0 } else { -half + 2.0 * half * i as f64 / (n - 1) as f64 }
    }

    pub fn half(&self) -> f64 {
        match *self {
            Sweep::Sway { half_span_mm } => half_span_mm,
            Sweep::Roll { half_deg } => half_deg,
        }
    }
}

/// Viewing setup in the disc frame (x right, y toward the viewer's side = picture bottom, z up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub lamp_mm: [f64; 3],
    pub distance_mm: f64,
    pub tilt_deg: f64,
}

impl Default for Pose {
    fn default() -> Self {
        Pose { lamp_mm: design::HELD_FLASHLIGHT_MM, distance_mm: design::DEFAULT_DISTANCE_MM, tilt_deg: 20.0 }
    }
}

impl Pose {
    /// Eye and lamp in the disc frame for sweep coordinate `s` (mm or degrees).
    pub fn at(&self, sweep: Sweep, s: f64) -> ([f64; 3], [f64; 3]) {
        let a = self.tilt_deg.to_radians();
        let eye = [0.0, self.distance_mm * a.sin(), self.distance_mm * a.cos()];
        match sweep {
            Sweep::Sway { .. } => ([eye[0] + s, eye[1], eye[2]], self.lamp_mm),
            Sweep::Roll { .. } => {
                // Disc rolled by `s` about y: express eye and lamp in the rolled disc frame.
                let (sn, cs) = s.to_radians().sin_cos();
                let rot = |p: [f64; 3]| [p[0] * cs - p[2] * sn, p[1], p[0] * sn + p[2] * cs];
                (rot(eye), rot(self.lamp_mm))
            }
        }
    }

    /// `(g_t, g_r)` at disc point `(x, y)` for sweep coordinate `s`.
    pub fn g(&self, sweep: Sweep, s: f64, x: f64, y: f64) -> (f64, f64) {
        let (eye, lamp) = self.at(sweep, s);
        scene::g_at(&scene::Scene { light_mm: lamp, camera_mm: eye, ..Default::default() }, x, y)
    }
}

/// Luminance matrix `A[row * options + j]` of every option at `rows` sweep samples, for disc point `(x, y)`.
pub fn response_matrix(resp: &Responses, pose: &Pose, sweep: Sweep, rows: usize, x: f64, y: f64) -> Vec<f64> {
    let k = resp.options;
    let mut a = Vec::with_capacity(rows * k);
    for r in 0..rows {
        let (gt, gr) = pose.g(sweep, sweep.coord(r, rows), x, y);
        for j in 0..k {
            a.push(luma(resp.at(j, gt, gr)));
        }
    }
    a
}

/// Singular values (descending) of a `rows x cols` row-major matrix, via cyclic Jacobi on `A^T A`.
pub fn singular_values(a: &[f64], rows: usize, cols: usize) -> Vec<f64> {
    let mut g = vec![0.0f64; cols * cols];
    for r in 0..rows {
        let row = &a[r * cols..(r + 1) * cols];
        for i in 0..cols {
            for j in i..cols {
                g[i * cols + j] += row[i] * row[j];
            }
        }
    }
    for i in 0..cols {
        for j in 0..i {
            g[i * cols + j] = g[j * cols + i];
        }
    }
    for _ in 0..60 {
        let mut off = 0.0;
        for i in 0..cols {
            for j in 0..i {
                off += g[i * cols + j] * g[i * cols + j];
            }
        }
        if off < 1e-24 {
            break;
        }
        for p in 0..cols {
            for q in p + 1..cols {
                let apq = g[p * cols + q];
                if apq.abs() < 1e-300 {
                    continue;
                }
                let theta = (g[q * cols + q] - g[p * cols + p]) / (2.0 * apq);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..cols {
                    let (gkp, gkq) = (g[k * cols + p], g[k * cols + q]);
                    g[k * cols + p] = c * gkp - s * gkq;
                    g[k * cols + q] = s * gkp + c * gkq;
                }
                for k in 0..cols {
                    let (gpk, gqk) = (g[p * cols + k], g[q * cols + k]);
                    g[p * cols + k] = c * gpk - s * gqk;
                    g[q * cols + k] = s * gpk + c * gqk;
                }
            }
        }
    }
    let mut ev: Vec<f64> = (0..cols).map(|i| g[i * cols + i].max(0.0).sqrt()).collect();
    ev.sort_by(|a, b| b.total_cmp(a));
    ev
}

/// What the singular values of one cell's response matrix say about independent views.
#[derive(Clone, Debug)]
pub struct ViewCount {
    /// Affine (simplex) pattern strengths: centred columns scaled by `1 - PLAIN_LEAK`, descending,
    /// as per-row RMS luminance (reference peak = 1).
    pub strengths: Vec<f64>,
    /// Patterns above 3 %, 5 % and 10 % of the strongest.
    pub n_rel: [usize; 3],
    /// Patterns whose RMS luminance is at least 0.01 of the reference peak (absolute floor).
    pub n_abs: usize,
    /// Range of `g_t` the sweep covers at this point.
    pub gt_range: (f64, f64),
}

/// Independent views at disc point `(x, y)` along `sweep`, sampled at `rows` positions.
///
/// The weights of a cell live on the simplex, so what matters is the affine span of the
/// columns: they are centred over the options first. The 15.3 % plain leakage adds a fixed
/// offset (no extra patterns) and scales the controllable part by `0.847`.
pub fn independent_views(resp: &Responses, pose: &Pose, sweep: Sweep, rows: usize, x: f64, y: f64) -> ViewCount {
    let k = resp.options;
    let mut a = response_matrix(resp, pose, sweep, rows, x, y);
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for r in 0..rows {
        let (gt, _) = pose.g(sweep, sweep.coord(r, rows), x, y);
        lo = lo.min(gt);
        hi = hi.max(gt);
    }
    for r in 0..rows {
        let row = &mut a[r * k..(r + 1) * k];
        let mean = row.iter().sum::<f64>() / k as f64;
        for v in row.iter_mut() {
            *v = (*v - mean) * (1.0 - PLAIN_LEAK);
        }
    }
    let sv = singular_values(&a, rows, k);
    let rms: Vec<f64> = sv.iter().map(|s| s / (rows as f64).sqrt()).collect();
    let top = rms.first().copied().unwrap_or(0.0).max(1e-12);
    let n_rel = [0.03, 0.05, 0.10].map(|e| rms.iter().filter(|&&s| s >= e * top).count());
    let n_abs = rms.iter().filter(|&&s| s >= 0.01).count();
    ViewCount { strengths: rms, n_rel, n_abs, gt_range: (lo, hi) }
}

// ---------------------------------------------------------------------------------------
// Float targets straight from the meshes
// ---------------------------------------------------------------------------------------

type V3 = [f64; 3];

fn unit3(a: V3) -> V3 {
    let n = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-12);
    [a[0] / n, a[1] / n, a[2] / n]
}
fn dot3(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// One rendered view on the cell grid: luminance (Lambert + ambient, linear, radial mask applied)
/// and the height `z` (mm above the disc plane) of the visible surface, 0 where empty.
struct Raster {
    lum: Vec<f32>,
    z: Vec<f32>,
}

/// Software-rasterise placed meshes (disc frame) seen from `eye`, `n x n` cells over the disc's
/// bounding square, 2x2 supersampled, perspective-correct depth. Same shading as
/// `model3d::render_scene_target` but kept in float and without 8-bit sRGB round trips.
fn raster(meshes: &[model3d::Mesh], eye: V3, n: usize, r_in: f64, r_out: f64) -> Raster {
    let ss = 2usize;
    let w = n * ss;
    let mut zbuf = vec![0.0f64; w * w];
    let mut lbuf = vec![0.0f32; w * w];
    let mut hbuf = vec![0.0f32; w * w];
    let light = unit3(model3d::scene_to_disc(model3d::KEY_LIGHT_RAW));
    let inv_two_ro = 1.0 / (2.0 * r_out);
    let wf = w as f64;
    for mesh in meshes {
        for tri in &mesh.tris {
            let Some((x0, y0, dz0)) = model3d::project_point(tri[0].pos, eye) else { continue };
            let Some((x1, y1, dz1)) = model3d::project_point(tri[1].pos, eye) else { continue };
            let Some((x2, y2, dz2)) = model3d::project_point(tri[2].pos, eye) else { continue };
            let area = (x1 - x0) * (y2 - y0) - (y1 - y0) * (x2 - x0);
            if area.abs() < 1e-12 {
                continue;
            }
            let inv_area = 1.0 / area;
            let (min_x, max_x) = (x0.min(x1).min(x2), x0.max(x1).max(x2));
            let (min_y, max_y) = (y0.min(y1).min(y2), y0.max(y1).max(y2));
            if max_x < -r_out || min_x > r_out || max_y < -r_out || min_y > r_out {
                continue;
            }
            let cl = |v: f64| (v.clamp(0.0, wf - 1.0)) as usize;
            let (ix0, ix1) = (cl(((min_x * inv_two_ro + 0.5) * wf).floor()), cl(((max_x * inv_two_ro + 0.5) * wf).ceil()));
            let (iy0, iy1) = (cl(((min_y * inv_two_ro + 0.5) * wf).floor()), cl(((max_y * inv_two_ro + 0.5) * wf).ceil()));
            let (iw0, iw1, iw2) = (1.0 / dz0, 1.0 / dz1, 1.0 / dz2);
            let nw = |k: usize, iw: f64| [tri[k].normal[0] * iw, tri[k].normal[1] * iw, tri[k].normal[2] * iw];
            let pw = |k: usize, iw: f64| [tri[k].pos[0] * iw, tri[k].pos[1] * iw, tri[k].pos[2] * iw];
            let (n0w, n1w, n2w) = (nw(0, iw0), nw(1, iw1), nw(2, iw2));
            let (p0w, p1w, p2w) = (pw(0, iw0), pw(1, iw1), pw(2, iw2));
            for sy in iy0..=iy1 {
                let ym = ((sy as f64 + 0.5) / wf - 0.5) * 2.0 * r_out;
                for sx in ix0..=ix1 {
                    let xm = ((sx as f64 + 0.5) / wf - 0.5) * 2.0 * r_out;
                    let b0 = ((x1 - xm) * (y2 - ym) - (y1 - ym) * (x2 - xm)) * inv_area;
                    let b1 = ((x2 - xm) * (y0 - ym) - (y2 - ym) * (x0 - xm)) * inv_area;
                    let b2 = 1.0 - b0 - b1;
                    if b0 < -1e-9 || b1 < -1e-9 || b2 < -1e-9 {
                        continue;
                    }
                    let inv_w = b0 * iw0 + b1 * iw1 + b2 * iw2;
                    let idx = sy * w + sx;
                    if inv_w <= zbuf[idx] {
                        continue;
                    }
                    zbuf[idx] = inv_w;
                    let inv_iw = 1.0 / inv_w;
                    let p_hit = [
                        (b0 * p0w[0] + b1 * p1w[0] + b2 * p2w[0]) * inv_iw,
                        (b0 * p0w[1] + b1 * p1w[1] + b2 * p2w[1]) * inv_iw,
                        (b0 * p0w[2] + b1 * p1w[2] + b2 * p2w[2]) * inv_iw,
                    ];
                    let n_raw = [
                        (b0 * n0w[0] + b1 * n1w[0] + b2 * n2w[0]) * inv_iw,
                        (b0 * n0w[1] + b1 * n1w[1] + b2 * n2w[1]) * inv_iw,
                        (b0 * n0w[2] + b1 * n1w[2] + b2 * n2w[2]) * inv_iw,
                    ];
                    let mut nn = unit3(n_raw);
                    if dot3(nn, [eye[0] - p_hit[0], eye[1] - p_hit[1], eye[2] - p_hit[2]]) < 0.0 {
                        nn = [-nn[0], -nn[1], -nn[2]];
                    }
                    lbuf[idx] = (model3d::AMBIENT + model3d::DIFFUSE * dot3(nn, light).max(0.0)) as f32;
                    hbuf[idx] = p_hit[2] as f32;
                }
            }
        }
    }
    let mut lum = vec![0.0f32; n * n];
    let mut zz = vec![0.0f32; n * n];
    for cy in 0..n {
        for cx in 0..n {
            let xm = ((cx as f64 + 0.5) / n as f64 - 0.5) * 2.0 * r_out;
            let ym = ((cy as f64 + 0.5) / n as f64 - 0.5) * 2.0 * r_out;
            let mask = model3d::radial_mask(xm, ym, r_in, r_out) as f32;
            if mask <= 0.0 {
                continue;
            }
            let (mut l, mut zsum, mut cnt) = (0.0f32, 0.0f32, 0.0f32);
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let i = (cy * ss + dy) * w + cx * ss + dx;
                l += lbuf[i];
                if lbuf[i] > 0.0 {
                    zsum += hbuf[i];
                    cnt += 1.0;
                }
            }
            lum[cy * n + cx] = 0.25 * l * mask;
            zz[cy * n + cx] = if cnt > 0.0 { zsum / cnt } else { 0.0 };
        }
    }
    Raster { lum, z: zz }
}

// ---------------------------------------------------------------------------------------
// Usability map: how many independent views each part of the disc has
// ---------------------------------------------------------------------------------------

/// Everything the light-field solver and the editor need to know about where a scene can live.
#[derive(Clone, Debug)]
pub struct Options {
    /// Number of solved views (least-squares rows), spread evenly over the sweep.
    pub views: usize,
    /// Cells across the disc diameter.
    pub grid: usize,
    /// Head sway half-span, mm.
    pub head_half_span_mm: f64,
    pub pose: Pose,
    pub illuminant: Illuminant,
    pub prefilter: Prefilter,
    /// Scale the parallax by disc position (full at 6/12 o'clock, flat near 3/9).
    pub disparity_scaling: bool,
    /// Draw cells where `g_t` crosses zero or the sweep has fewer than ~3 independent views dark.
    pub dark_unusable: bool,
    pub tolerance: Tolerance,
    pub exposure: Exposure,
    /// Account for the 15.3 % of frames the drive writes itself (plain data) as a fixed offset.
    pub plain_leak: bool,
    /// 0 = brightness only, 1 = colour counts as much as brightness (as in `design`).
    pub chroma_weight: f64,
}

/// How the target light field is band-limited before solving.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Prefilter {
    /// Targets are the sharp renders at the solved eye positions (what `design` does today).
    None,
    /// Average renders over a Gaussian eye kernel `sigma = kappa * dh(cell)`, `dh` = sweep / independent views.
    EyeKernel { kappa: f64 },
    /// Render sharp, then blur horizontally by the parallax of the visible depth over that same kernel
    /// (Zwicker's depth-dependent prefilter): `sigma_px = kappa * dh(cell) * |z| / (E_z - z)`.
    DepthBlur { kappa: f64 },
}

/// Pose errors to stay robust against, applied as extra least-squares rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tolerance {
    /// Lamp x and z each +-this many mm.
    pub lamp_mm: f64,
    /// Eye tilt +-this many degrees.
    pub tilt_deg: f64,
    /// Total weight of the perturbed rows relative to the nominal rows (0 = off).
    pub weight: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Exposure {
    Fixed(f64),
    /// Largest scale at which 75 % of the lit target samples are within reach of the codebook.
    Auto,
}

impl Default for Options {
    fn default() -> Self {
        Options::recommended(24)
    }
}

impl Options {
    /// What `design::solve` does today: sharp targets, nominal pose only, no offset, exposure 0.6.
    pub fn baseline(views: usize, grid: usize) -> Self {
        Options {
            views,
            grid,
            head_half_span_mm: design::DEFAULT_HEAD_HALF_SPAN_MM,
            pose: Pose::default(),
            illuminant: Illuminant::White,
            prefilter: Prefilter::None,
            disparity_scaling: false,
            dark_unusable: false,
            tolerance: Tolerance { lamp_mm: 0.0, tilt_deg: 0.0, weight: 0.0 },
            exposure: Exposure::Fixed(0.6),
            plain_leak: false,
            chroma_weight: 0.3,
        }
    }

    /// Recommended defaults.
    pub fn recommended(views: usize) -> Self {
        Options {
            prefilter: Prefilter::EyeKernel { kappa: 0.5 },
            disparity_scaling: true,
            dark_unusable: true,
            tolerance: Tolerance { lamp_mm: 30.0, tilt_deg: 5.0, weight: 0.5 },
            exposure: Exposure::Auto,
            plain_leak: true,
            ..Options::baseline(views, 256)
        }
    }
}

/// Per-cell map over the disc (square `n x n` grid, row-major, picture y down).
#[derive(Clone, Debug)]
pub struct UsabilityMap {
    pub n: usize,
    /// Outer radius of the disc the map covers, mm.
    pub outer_radius_mm: f64,
    /// Independent views along the sweep (5 % floor), 0 outside the data ring.
    pub n_eff: Vec<f32>,
    /// Head distance between independent views, mm (`sweep / n_eff`).
    pub delta_h_mm: Vec<f32>,
    /// `g_t` range over the sweep.
    pub gt_lo: Vec<f32>,
    pub gt_hi: Vec<f32>,
    /// Parallax scale 0..1 (1 where the sweep covers as much `g_t` as at the best cell).
    pub scale: Vec<f32>,
    /// 0..1: usable at all (enough independent views, `g_t` stays clear of zero).
    pub usable: Vec<f32>,
}

#[derive(Clone, Copy, Debug)]
pub enum Field {
    NEff,
    DeltaH,
    Scale,
    Usable,
}

impl UsabilityMap {
    fn data(&self, f: Field) -> &[f32] {
        match f {
            Field::NEff => &self.n_eff,
            Field::DeltaH => &self.delta_h_mm,
            Field::Scale => &self.scale,
            Field::Usable => &self.usable,
        }
    }

    /// Bilinear sample at picture coordinates `(u, v)` in 0..1.
    pub fn sample(&self, f: Field, u: f64, v: f64) -> f32 {
        let d = self.data(f);
        let (fx, fy) = ((u * self.n as f64 - 0.5).clamp(0.0, self.n as f64 - 1.0), (v * self.n as f64 - 0.5).clamp(0.0, self.n as f64 - 1.0));
        let (ix, iy) = ((fx as usize).min(self.n - 2), (fy as usize).min(self.n - 2));
        let (tx, ty) = ((fx - ix as f64) as f32, (fy - iy as f64) as f32);
        let at = |x: usize, y: usize| d[y * self.n + x];
        let top = at(ix, iy) * (1.0 - tx) + at(ix + 1, iy) * tx;
        let bot = at(ix, iy + 1) * (1.0 - tx) + at(ix + 1, iy + 1) * tx;
        top * (1.0 - ty) + bot * ty
    }

    /// Largest `|z|` (mm) at which a feature of `detail_mm` stays one image at picture point
    /// `(u, v)`: the disparity between adjacent independent views, `|z| * dh / E_z`, must not
    /// exceed the detail (Zwicker et al. 2006, display depth of field).
    pub fn max_depth_mm(&self, pose: &Pose, detail_mm: f64, u: f64, v: f64) -> f64 {
        let dh = self.sample(Field::DeltaH, u, v) as f64;
        if dh <= 0.0 {
            return 0.0;
        }
        let ez = pose.distance_mm * pose.tilt_deg.to_radians().cos();
        // |z| * dh / (E_z - |z|) = detail  =>  |z| = detail * E_z / (dh + detail)
        detail_mm * ez / (dh + detail_mm)
    }
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Responses for `options.illuminant` (white reuses the designer's shared set).
pub fn responses_for_options(options: &Options, preset: &crate::preset::DiscGeometryPreset) -> (std::sync::Arc<(Codebook, Responses)>, Responses) {
    let shared = design::shared_codebook(preset.channel_bit_length_nm * 1e-3);
    let resp = if options.illuminant == Illuminant::White { shared.1.clone() } else { responses_for(&shared.0, 12, options.illuminant) };
    (shared, resp)
}

/// Where on the disc a scene can live, from singular values of the response matrix.
pub fn usability_map(preset: &crate::preset::DiscGeometryPreset, options: &Options) -> UsabilityMap {
    let (_, resp) = responses_for_options(options, preset);
    usability_map_with(&resp, preset, options)
}

pub fn usability_map_with(resp: &Responses, preset: &crate::preset::DiscGeometryPreset, options: &Options) -> UsabilityMap {
    let n = 64usize;
    let ro = preset.outer_radius_mm;
    let sweep = Sweep::Sway { half_span_mm: options.head_half_span_mm };
    let span = 2.0 * options.head_half_span_mm;
    let rows = ((span / 4.0).ceil() as usize + 1).max(21);
    let cells: Vec<(f32, f32, f32, f32)> = (0..n * n)
        .into_par_iter()
        .map(|i| {
            let (u, v) = (((i % n) as f64 + 0.5) / n as f64, ((i / n) as f64 + 0.5) / n as f64);
            let (x, y) = ((u - 0.5) * 2.0 * ro, (v - 0.5) * 2.0 * ro);
            let r = x.hypot(y);
            if r < preset.inner_radius_mm || r > ro {
                return (0.0, 0.0, 0.0, 0.0);
            }
            let vc = independent_views(resp, &options.pose, sweep, rows, x, y);
            let mut min_abs = f64::INFINITY;
            for k in 0..rows {
                min_abs = min_abs.min(options.pose.g(sweep, sweep.coord(k, rows), x, y).0.abs());
            }
            let neff = vc.n_rel[1] as f64;
            let usable = smoothstep(2.5, 4.5, neff) * smoothstep(0.10, 0.20, min_abs);
            (neff as f32, vc.gt_range.0 as f32, vc.gt_range.1 as f32, usable as f32)
        })
        .collect();
    let spans: Vec<f64> = cells.iter().filter(|c| c.0 > 0.0).map(|c| (c.2 - c.1) as f64).collect();
    let mut sorted = spans.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let ref_span = sorted.get(sorted.len() * 95 / 100).copied().unwrap_or(0.46).max(0.05);
    let mut map = UsabilityMap {
        n,
        outer_radius_mm: ro,
        n_eff: vec![0.0; n * n],
        delta_h_mm: vec![0.0; n * n],
        gt_lo: vec![0.0; n * n],
        gt_hi: vec![0.0; n * n],
        scale: vec![0.0; n * n],
        usable: vec![0.0; n * n],
    };
    for (i, c) in cells.iter().enumerate() {
        if c.0 <= 0.0 {
            continue;
        }
        map.n_eff[i] = c.0;
        map.delta_h_mm[i] = (span / (c.0 as f64).max(1.0)) as f32;
        map.gt_lo[i] = c.1;
        map.gt_hi[i] = c.2;
        map.scale[i] = smoothstep(0.25, 0.9, (c.2 - c.1) as f64 / ref_span) as f32;
        map.usable[i] = c.3;
    }
    map
}

// ---------------------------------------------------------------------------------------
// Solver
// ---------------------------------------------------------------------------------------

/// What `solve_meshes` returns beyond the weights.
pub struct LightfieldResult {
    pub solved: design::Solved,
    /// Responses to hand to `design::render` for these weights.
    pub responses: Responses,
    /// Exposure scale actually used.
    pub exposure: f64,
    pub usability: UsabilityMap,
    /// Solved view positions (head x, mm).
    pub heads_mm: Vec<f64>,
    pub targets_secs: f64,
    pub solve_secs: f64,
}

/// Head positions of `n` views over `+-half`.
pub fn view_heads(n: usize, half: f64) -> Vec<f64> {
    (0..n).map(|i| if n <= 1 { 0.0 } else { -half + 2.0 * half * i as f64 / (n - 1) as f64 }).collect()
}

/// Fine renders over the sweep plus everything per cell the target generator needs.
pub struct Prep {
    pub n: usize,
    ro: f64,
    ez: f64,
    fine_h: Vec<f64>,
    fine: Vec<Raster>,
    /// Kernel sigma in mm of head position, per cell.
    sigma: Vec<f32>,
    scale: Vec<f32>,
    usable: Vec<f32>,
    pub umap: UsabilityMap,
    prefilter: Prefilter,
    dark_unusable: bool,
}

const FINE_STEP_MM: f64 = 4.0;

impl Prep {
    pub fn new(meshes: &[model3d::Mesh], opts: &Options, preset: &crate::preset::DiscGeometryPreset, umap: UsabilityMap) -> Prep {
        let n = opts.grid.clamp(16, 1024);
        let ro = preset.outer_radius_mm;
        let half = opts.head_half_span_mm + 8.0;
        let m = ((2.0 * half / FINE_STEP_MM).round() as usize).max(2) + 1;
        let fine_h = view_heads(m, half);
        let a = opts.pose.tilt_deg.to_radians();
        let eye0 = [0.0, opts.pose.distance_mm * a.sin(), opts.pose.distance_mm * a.cos()];
        let fine: Vec<Raster> = fine_h
            .par_iter()
            .map(|&h| raster(meshes, [eye0[0] + h, eye0[1], eye0[2]], n, preset.inner_radius_mm, ro))
            .collect();
        let kappa = match opts.prefilter {
            Prefilter::None => 0.0,
            Prefilter::EyeKernel { kappa } | Prefilter::DepthBlur { kappa } => kappa,
        };
        let mut sigma = vec![0.0f32; n * n];
        let mut scale = vec![1.0f32; n * n];
        let mut usable = vec![1.0f32; n * n];
        for i in 0..n * n {
            let (u, v) = (((i % n) as f64 + 0.5) / n as f64, ((i / n) as f64 + 0.5) / n as f64);
            sigma[i] = (kappa * umap.sample(Field::DeltaH, u, v) as f64) as f32;
            if opts.disparity_scaling {
                scale[i] = umap.sample(Field::Scale, u, v);
            }
            if opts.dark_unusable {
                usable[i] = umap.sample(Field::Usable, u, v);
            }
        }
        Prep { n, ro, ez: eye0[2], fine_h, fine, sigma, scale, usable, umap, prefilter: opts.prefilter, dark_unusable: opts.dark_unusable }
    }

    /// Fine-render index pair and weight for head position `h` (linear interpolation), clamped.
    fn lerp_idx(&self, h: f64) -> (usize, usize, f32) {
        let m = self.fine_h.len();
        let t = ((h - self.fine_h[0]) / (self.fine_h[m - 1] - self.fine_h[0]) * (m - 1) as f64).clamp(0.0, (m - 1) as f64);
        let i0 = (t as usize).min(m - 2);
        (i0, i0 + 1, (t - i0 as f64) as f32)
    }

    /// Raw (pre-exposure) target luminance at head positions `heads`, `[view][cell]`.
    /// With `Prefilter::None` and no disparity scaling this is the sharp render at `h`.
    pub fn targets(&self, heads: &[f64]) -> Vec<Vec<f32>> {
        let n = self.n;
        heads
            .par_iter()
            .map(|&h| {
                let mut out = vec![0.0f32; n * n];
                match self.prefilter {
                    Prefilter::DepthBlur { .. } => {
                        // Sharp image at the scaled eye, then scatter each lit pixel along x by the
                        // parallax of its own depth over the kernel.
                        let cell_mm = 2.0 * self.ro / n as f64;
                        let mut sharp = vec![0.0f32; n * n];
                        let mut zs = vec![0.0f32; n * n];
                        for c in 0..n * n {
                            let (i0, i1, t) = self.lerp_idx(h * self.scale[c] as f64);
                            sharp[c] = self.fine[i0].lum[c] * (1.0 - t) + self.fine[i1].lum[c] * t;
                            zs[c] = if t < 0.5 { self.fine[i0].z[c] } else { self.fine[i1].z[c] };
                        }
                        for row in 0..n {
                            for col in 0..n {
                                let c = row * n + col;
                                let l = sharp[c];
                                if l <= 0.0 {
                                    continue;
                                }
                                let z = zs[c] as f64;
                                let s_mm = self.sigma[c] as f64 * z.abs() / (self.ez - z).max(50.0);
                                let s_px = s_mm / cell_mm;
                                if s_px < 0.3 {
                                    out[c] += l;
                                    continue;
                                }
                                let r = (3.0 * s_px).ceil() as isize;
                                let mut wsum = 0.0f64;
                                let wts: Vec<f64> = (-r..=r).map(|d| (-0.5 * (d as f64 / s_px).powi(2)).exp()).collect();
                                for w in &wts {
                                    wsum += w;
                                }
                                for (d, w) in (-r..=r).zip(&wts) {
                                    let cc = col as isize + d;
                                    if cc >= 0 && (cc as usize) < n {
                                        out[row * n + cc as usize] += l * (*w / wsum) as f32;
                                    }
                                }
                            }
                        }
                        for c in 0..n * n {
                            out[c] = out[c].min(1.2);
                        }
                    }
                    _ => {
                        for c in 0..n * n {
                            let he = h * self.scale[c] as f64;
                            let s = self.sigma[c] as f64;
                            if s < 2.0 {
                                let (i0, i1, t) = self.lerp_idx(he);
                                out[c] = self.fine[i0].lum[c] * (1.0 - t) + self.fine[i1].lum[c] * t;
                                continue;
                            }
                            let (mut acc, mut wsum) = (0.0f64, 0.0f64);
                            let lo = ((he - 3.0 * s - self.fine_h[0]) / FINE_STEP_MM).floor().max(0.0) as usize;
                            let hi = (((he + 3.0 * s - self.fine_h[0]) / FINE_STEP_MM).ceil() as usize).min(self.fine_h.len() - 1);
                            for mi in lo..=hi {
                                let d = (self.fine_h[mi] - he) / s;
                                let w = (-0.5 * d * d).exp();
                                acc += w * self.fine[mi].lum[c] as f64;
                                wsum += w;
                            }
                            out[c] = (acc / wsum.max(1e-12)) as f32;
                        }
                    }
                }
                if self.dark_unusable {
                    for c in 0..n * n {
                        out[c] *= self.usable[c];
                    }
                }
                out
            })
            .collect()
    }

    /// The sharp, unfiltered render at head position `h` (nearest fine render, linear), for reference.
    pub fn sharp(&self, h: f64) -> Vec<f32> {
        let (i0, i1, t) = self.lerp_idx(h);
        (0..self.n * self.n).map(|c| self.fine[i0].lum[c] * (1.0 - t) + self.fine[i1].lum[c] * t).collect()
    }
}

/// Place a `model3d::Scene` into disc-frame meshes.
pub fn place_scene(scene: &model3d::Scene) -> Result<Vec<model3d::Mesh>, String> {
    if scene.version != model3d::SCENE_VERSION {
        return Err(format!("不支持的场景版本：{}", scene.version));
    }
    let mut cache: std::collections::HashMap<String, std::sync::Arc<model3d::Mesh>> = Default::default();
    let mut placed = Vec::new();
    for obj in &scene.objects {
        if !obj.visible {
            continue;
        }
        if !cache.contains_key(&obj.path) {
            cache.insert(obj.path.clone(), model3d::load_mesh_or_builtin(&obj.path)?);
        }
        placed.push(model3d::place_scene_object(&cache[&obj.path], obj)?);
    }
    if placed.is_empty() {
        return Err("场景中没有可见模型".into());
    }
    Ok(placed)
}

/// Solve a scene for `design::render` / `design::build_iso`. Use [`solve_meshes`] to also get
/// the exposure, responses and usability map.
pub fn solve_scene(scene: &model3d::Scene, options: &Options, preset: &crate::preset::DiscGeometryPreset) -> Result<design::Solved, String> {
    let meshes = place_scene(scene)?;
    Ok(solve_meshes(&meshes, options, preset)?.solved)
}

/// Eye/lamp for solved view `h` under pose perturbation scenario `s` (0 = nominal).
fn scenario_pose(base: &Pose, tol: &Tolerance, s: usize) -> Pose {
    let mut p = *base;
    match s {
        1 => p.lamp_mm[0] += tol.lamp_mm,
        2 => p.lamp_mm[0] -= tol.lamp_mm,
        3 => p.lamp_mm[2] += tol.lamp_mm,
        4 => p.lamp_mm[2] -= tol.lamp_mm,
        5 => p.tilt_deg += tol.tilt_deg,
        6 => p.tilt_deg -= tol.tilt_deg,
        _ => {}
    }
    p
}

pub fn solve_meshes(meshes: &[model3d::Mesh], opts: &Options, preset: &crate::preset::DiscGeometryPreset) -> Result<LightfieldResult, String> {
    if opts.views == 0 {
        return Err("至少需要一个视角".into());
    }
    let t0 = std::time::Instant::now();
    let (_, resp) = responses_for_options(opts, preset);
    let umap = usability_map_with(&resp, preset, opts);
    let prep = Prep::new(meshes, opts, preset, umap.clone());
    let heads = view_heads(opts.views, opts.head_half_span_mm);
    let raw = prep.targets(&heads);
    let n = prep.n;
    let k = resp.options;
    let plain = k - 1;
    let ro = preset.outer_radius_mm;
    let (r_in, r_out) = (preset.inner_radius_mm, ro);
    let gain = if opts.plain_leak { 1.0 - PLAIN_LEAK } else { 1.0 };
    let leak = if opts.plain_leak { PLAIN_LEAK } else { 0.0 };
    let sweep = Sweep::Sway { half_span_mm: opts.head_half_span_mm };
    let cell_xy = |i: usize| -> (f64, f64) { ((((i % n) as f64 + 0.5) / n as f64 - 0.5) * 2.0 * ro, (((i / n) as f64 + 0.5) / n as f64 - 0.5) * 2.0 * ro) };

    // Exposure.
    let exposure = match opts.exposure {
        Exposure::Fixed(e) => e,
        Exposure::Auto => {
            let mut ratios: Vec<f64> = Vec::new();
            for i in (0..n * n).step_by(3) {
                let (x, y) = cell_xy(i);
                let r = x.hypot(y);
                if r < r_in || r > r_out {
                    continue;
                }
                for (vi, &h) in heads.iter().enumerate().step_by((opts.views / 8).max(1)) {
                    let l = raw[vi][i] as f64;
                    if l < 0.08 {
                        continue;
                    }
                    let (gt, gr) = opts.pose.g(sweep, h, x, y);
                    let ceil = (0..k).map(|j| luma(resp.at(j, gt, gr))).fold(0.0, f64::max);
                    let floor = leak * luma(resp.at(plain, gt, gr));
                    ratios.push((gain * ceil + floor) / l);
                }
            }
            ratios.sort_by(|a, b| a.total_cmp(b));
            let q = ratios.get(ratios.len() / 4).copied().unwrap_or(0.6);
            q.clamp(0.2, 1.0)
        }
    };
    let targets_secs = t0.elapsed().as_secs_f64();

    // Rows: nominal views plus perturbed copies (luminance only).
    let tol_on = opts.tolerance.weight > 0.0 && (opts.tolerance.lamp_mm > 0.0 || opts.tolerance.tilt_deg > 0.0);
    let pert_stride = (opts.views / 12).max(1);
    let n_pert_views = (0..opts.views).step_by(pert_stride).count();
    let pert_w = if tol_on { (opts.tolerance.weight * opts.views as f64 / (6.0 * n_pert_views as f64)).sqrt() } else { 0.0 };
    let pert_poses: Vec<Pose> = (1..=6).map(|s| scenario_pose(&opts.pose, &opts.tolerance, s)).collect();
    let c_w = opts.chroma_weight.max(0.0).sqrt();
    let luma_f64 = |p: [f32; 3]| luma(p);

    let dark_default: usize = (0..plain)
        .min_by(|&a, &b| {
            let ea: f64 = resp.rgb[a].iter().map(|&p| luma_f64(p).powi(2)).sum();
            let eb: f64 = resp.rgb[b].iter().map(|&p| luma_f64(p).powi(2)).sum();
            ea.total_cmp(&eb)
        })
        .unwrap_or(0);

    let t1 = std::time::Instant::now();
    let cells: Vec<(Vec<f32>, f64, f64)> = (0..n * n)
        .into_par_iter()
        .map(|i| {
            let (x, y) = cell_xy(i);
            let r = x.hypot(y);
            let mut w = vec![0.0f32; k];
            if r < r_in || r > r_out {
                // Outside the ring: darkest entry over the sweep.
                let mut best = (dark_default, f64::INFINITY);
                let (xc, yc) = if r < 1e-6 { (r_in, 0.0) } else { (x / r * r.clamp(r_in, r_out), y / r * r.clamp(r_in, r_out)) };
                for j in 0..plain {
                    let mut e = 0.0;
                    for &h in &heads {
                        let (gt, gr) = opts.pose.g(sweep, h, xc, yc);
                        e += luma(resp.at(j, gt, gr)).powi(2);
                    }
                    if e < best.1 {
                        best = (j, e);
                    }
                }
                w[best.0] = 1.0;
                return (w, 0.0, 0.0);
            }
            let mut gram = vec![0.0f64; k * k];
            let mut rhs = vec![0.0f64; k];
            let (mut b_sq, mut n_rows) = (0.0f64, 0usize);
            let mut row = vec![0.0f64; k];
            let mut add = |row: &[f64], b: f64, gram: &mut [f64], rhs: &mut [f64]| {
                for a in 0..k {
                    let ra = row[a];
                    if ra == 0.0 {
                        continue;
                    }
                    rhs[a] += ra * b;
                    let g = &mut gram[a * k..(a + 1) * k];
                    for c in a..k {
                        g[c] += ra * row[c];
                    }
                }
                b_sq += b * b;
                n_rows += 1;
            };
            for (vi, &h) in heads.iter().enumerate() {
                let t = (raw[vi][i] as f64 * exposure).max(0.0);
                let (gt, gr) = opts.pose.g(sweep, h, x, y);
                let cols: Vec<[f32; 3]> = (0..k).map(|j| resp.at(j, gt, gr)).collect();
                let pl = cols[plain];
                let t_eff = (t - leak * luma(pl)).max(0.0);
                for j in 0..k {
                    row[j] = gain * luma(cols[j]);
                }
                add(&row, t_eff, &mut gram, &mut rhs);
                if c_w > 0.0 && t > 1e-6 {
                    for ch in 0..3 {
                        for j in 0..k {
                            row[j] = c_w * gain * cols[j][ch] as f64;
                        }
                        add(&row, c_w * (t - leak * pl[ch] as f64).max(0.0), &mut gram, &mut rhs);
                    }
                }
                if tol_on && vi % pert_stride == 0 {
                    for p in &pert_poses {
                        let (gt, gr) = p.g(sweep, h, x, y);
                        let pp = luma(resp.at(plain, gt, gr));
                        let t_p = (t - leak * pp).max(0.0);
                        for j in 0..k {
                            row[j] = pert_w * gain * luma(resp.at(j, gt, gr));
                        }
                        add(&row, pert_w * t_p, &mut gram, &mut rhs);
                    }
                }
            }
            for a in 0..k {
                for c in 0..a {
                    gram[a * k + c] = gram[c * k + a];
                }
            }
            if n_rows == 0 {
                w[dark_default] = 1.0;
                return (w, 0.0, 0.0);
            }
            let w64 = design::solve_simplex_qp(&gram, &rhs, k);
            let mut err = b_sq;
            for j in 0..k {
                let mut gw = 0.0f64;
                for m in 0..k {
                    gw += gram[j * k + m] * w64[m];
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
    let solve_secs = t1.elapsed().as_secs_f64();
    Ok(LightfieldResult {
        solved: design::Solved { grid: n, options: k, weights, error: err / rows.max(1.0) },
        responses: resp,
        exposure,
        usability: umap,
        heads_mm: heads,
        targets_secs,
        solve_secs,
    })
}

/// Linear RGB of a solved disc seen from `eye` with the lamp at `lamp` (disc frame), `size` square,
/// without the groove. `leak` mixes in the 15.3 % plain data. Outside the data ring: zeros.
pub fn render_float(solved: &design::Solved, resp: &Responses, preset: &crate::preset::DiscGeometryPreset, eye: V3, lamp: V3, size: usize, leak: bool) -> Vec<[f32; 3]> {
    let sc = scene::Scene { light_mm: lamp, camera_mm: eye, ..Default::default() };
    let ro = preset.outer_radius_mm;
    let plain = resp.options - 1;
    (0..size * size)
        .into_par_iter()
        .map(|i| {
            let (u, v) = (((i % size) as f64 + 0.5) / size as f64, ((i / size) as f64 + 0.5) / size as f64);
            let (x, y) = ((u - 0.5) * 2.0 * ro, (v - 0.5) * 2.0 * ro);
            let r = x.hypot(y);
            if r < preset.inner_radius_mm || r > ro {
                return [0.0; 3];
            }
            let (gt, gr) = scene::g_at(&sc, x, y);
            let mut c = [0.0f32; 3];
            if let Some(w) = solved.cell(u, v) {
                for (k, &wk) in w.iter().enumerate() {
                    if wk > 0.0 {
                        let p = resp.at(k, gt, gr);
                        for ch in 0..3 {
                            c[ch] += wk * p[ch];
                        }
                    }
                }
            }
            if leak {
                let p = resp.at(plain, gt, gr);
                for ch in 0..3 {
                    c[ch] = (1.0 - PLAIN_LEAK as f32) * c[ch] + PLAIN_LEAK as f32 * p[ch];
                }
            }
            c
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> std::sync::Arc<(Codebook, Responses)> {
        design::shared_codebook(0.1333)
    }

    #[test]
    fn white_matches_design_responses() {
        let sh = shared();
        let mine = responses_for(&sh.0, 12, Illuminant::White);
        let (mut worst, mut sum, mut n) = (0.0f64, 0.0, 0usize);
        for j in [0usize, 5, 11, 20, sh.1.options - 1] {
            for gt in [0.3, 0.45, 0.6, 0.7] {
                let (a, b) = (luma(sh.1.at(j, gt, 0.1)), luma(mine.at(j, gt, 0.1)));
                worst = worst.max((a - b).abs());
                sum += a;
                n += 1;
            }
        }
        println!("white vs design: worst abs diff {worst:.4}, mean level {:.3}", sum / n as f64);
        assert!(worst < 0.06, "{worst}");
    }
}
