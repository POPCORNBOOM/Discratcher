//! Research harness measuring how well a colour picture can be reproduced by the
//! diffraction designer over a top-crop viewing wedge.
//!
//! Run with:
//! `cargo test -p disc-core --release color_study -- --ignored --nocapture`

use super::design::{shared_codebook, solve_simplex_qp, Responses};
use super::model3d::scene_to_disc;
use super::scene::{g_at, Scene};
use super::turntable::{eye_and_lamp, turn, Options, LAMP_MM};
use crate::preset::reference_preset;
use image::{Rgb, RgbImage};
use rayon::prelude::*;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const STEP_MM: f64 = 0.25; // 4 px per mm
const HALF_ANGLE_DEG: f64 = 15.0;
const RADIAL_MARGIN_MM: f64 = 0.5;
const DEFAULT_EXPOSURE: f64 = 0.6;
const DEFAULT_CHROMA_WEIGHT: f64 = 1.0;

struct PatchSpec {
    name: &'static str,
    srgb: [u8; 3],
}

const PATCHES: [PatchSpec; 24] = [
    PatchSpec { name: "Dark Skin", srgb: [115, 82, 68] },
    PatchSpec { name: "Light Skin", srgb: [194, 150, 130] },
    PatchSpec { name: "Blue Sky", srgb: [98, 122, 157] },
    PatchSpec { name: "Foliage", srgb: [87, 108, 67] },
    PatchSpec { name: "Blue Flower", srgb: [133, 128, 177] },
    PatchSpec { name: "Bluish Green", srgb: [103, 189, 170] },
    PatchSpec { name: "Orange", srgb: [214, 126, 44] },
    PatchSpec { name: "Purplish Blue", srgb: [80, 91, 166] },
    PatchSpec { name: "Moderate Red", srgb: [193, 90, 99] },
    PatchSpec { name: "Purple", srgb: [94, 60, 108] },
    PatchSpec { name: "Yellow Green", srgb: [157, 188, 64] },
    PatchSpec { name: "Orange Yellow", srgb: [224, 163, 46] },
    PatchSpec { name: "Blue", srgb: [56, 61, 150] },
    PatchSpec { name: "Green", srgb: [70, 148, 73] },
    PatchSpec { name: "Red", srgb: [175, 54, 60] },
    PatchSpec { name: "Yellow", srgb: [231, 199, 31] },
    PatchSpec { name: "Magenta", srgb: [187, 86, 149] },
    PatchSpec { name: "Cyan", srgb: [8, 133, 161] },
    PatchSpec { name: "White 9.5", srgb: [243, 243, 242] },
    PatchSpec { name: "Neutral 8", srgb: [200, 200, 200] },
    PatchSpec { name: "Neutral 6.5", srgb: [160, 160, 160] },
    PatchSpec { name: "Neutral 5", srgb: [122, 122, 121] },
    PatchSpec { name: "Neutral 3.5", srgb: [85, 85, 85] },
    PatchSpec { name: "Black 2", srgb: [52, 52, 52] },
];

#[derive(Clone, Copy)]
enum GeomModel {
    True,
    Parallel,
}

#[derive(Clone, Copy)]
struct SamplePoint {
    px: u32,
    py: u32,
    x: f64,
    y: f64,
    patch: usize,
}

struct WedgeGrid {
    width: u32,
    height: u32,
    points: Vec<SamplePoint>,
}

fn srgb_to_linear(v: u8) -> f64 {
    let c = v as f64 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb_u8(v: f64) -> u8 {
    let l = v.clamp(0.0, 1.0);
    let s = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

fn luma_f32(p: [f32; 3]) -> f64 {
    0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64
}

fn luma_f64(p: [f64; 3]) -> f64 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

/// Standard IEC 61966-2-1 linear sRGB -> CIE XYZ (D65).
fn linear_srgb_to_xyz(rgb: [f64; 3]) -> [f64; 3] {
    [
        0.4124564 * rgb[0] + 0.3575761 * rgb[1] + 0.1804375 * rgb[2],
        0.2126729 * rgb[0] + 0.7151522 * rgb[1] + 0.0721750 * rgb[2],
        0.0193339 * rgb[0] + 0.1191920 * rgb[1] + 0.9503041 * rgb[2],
    ]
}

/// Standard CIE XYZ (D65) -> linear sRGB (inverse of `linear_srgb_to_xyz`).
fn xyz_to_linear_srgb_d65(xyz: [f64; 3]) -> [f64; 3] {
    [
        3.2404542 * xyz[0] - 1.5371385 * xyz[1] - 0.4985314 * xyz[2],
        -0.9692660 * xyz[0] + 1.8760108 * xyz[1] + 0.0415560 * xyz[2],
        0.0556434 * xyz[0] - 0.2040259 * xyz[1] + 1.0572252 * xyz[2],
    ]
}

fn lab_f(t: f64) -> f64 {
    const DELTA: f64 = 6.0 / 29.0;
    const DELTA3: f64 = DELTA * DELTA * DELTA;
    const FACTOR: f64 = 1.0 / (3.0 * DELTA * DELTA);
    const OFFSET: f64 = 4.0 / 29.0;
    let t = t.max(0.0);
    if t > DELTA3 {
        t.cbrt()
    } else {
        FACTOR * t + OFFSET
    }
}

fn lab_f_inv(t: f64) -> f64 {
    const DELTA: f64 = 6.0 / 29.0;
    if t > DELTA {
        t * t * t
    } else {
        3.0 * DELTA * DELTA * (t - 4.0 / 29.0)
    }
}

fn linear_srgb_to_lab(rgb: [f64; 3], exposure: f64) -> [f64; 3] {
    let xyz = linear_srgb_to_xyz(rgb);
    let white = linear_srgb_to_xyz([exposure, exposure, exposure]);
    let fx = lab_f(xyz[0] / white[0]);
    let fy = lab_f(xyz[1] / white[1]);
    let fz = lab_f(xyz[2] / white[2]);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn lab_to_linear_srgb(lab: [f64; 3], exposure: f64) -> [f64; 3] {
    let white = linear_srgb_to_xyz([exposure, exposure, exposure]);
    let fy = (lab[0] + 16.0) / 116.0;
    let fx = fy + lab[1] / 500.0;
    let fz = fy - lab[2] / 200.0;
    let xyz = [white[0] * lab_f_inv(fx), white[1] * lab_f_inv(fy), white[2] * lab_f_inv(fz)];
    xyz_to_linear_srgb_d65(xyz)
}

fn delta_e76(ach: [f64; 3], tgt: [f64; 3]) -> f64 {
    let dl = ach[0] - tgt[0];
    let da = ach[1] - tgt[1];
    let db = ach[2] - tgt[2];
    (dl * dl + da * da + db * db).sqrt()
}

fn pose_scene(delta_deg: f64, head_x_mm: f64) -> Scene {
    let (eye, lamp) = eye_and_lamp(&Options::default(), LAMP_MM, head_x_mm, delta_deg.to_radians());
    Scene { light_mm: lamp, camera_mm: eye, ..Default::default() }
}

fn general_pose_scene(lamp_scene_mm: [f64; 3], hx: f64, hy: f64, delta_deg: f64) -> Scene {
    let delta = delta_deg.to_radians();
    let eye = turn([hx, hy, Options::default().distance_mm], delta);
    let lamp = turn(scene_to_disc(lamp_scene_mm), delta);
    Scene { light_mm: lamp, camera_mm: eye, ..Default::default() }
}

fn unit3(a: [f64; 3]) -> [f64; 3] {
    let n = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    [a[0] / n, a[1] / n, a[2] / n]
}

/// Parallel-track assumption: every point uses the 12 o'clock tangent (1, 0) and radial (0, -1).
fn g_at_parallel(sc: &Scene, x: f64, y: f64) -> (f64, f64) {
    let l = unit3([sc.light_mm[0] - x, sc.light_mm[1] - y, sc.light_mm[2]]);
    let v = unit3([sc.camera_mm[0] - x, sc.camera_mm[1] - y, sc.camera_mm[2]]);
    let gx = l[0] + v[0];
    let gy = l[1] + v[1];
    (gx, -gy)
}

fn g_with_model(model: GeomModel, sc: &Scene, x: f64, y: f64) -> (f64, f64) {
    match model {
        GeomModel::True => g_at(sc, x, y),
        GeomModel::Parallel => g_at_parallel(sc, x, y),
    }
}

fn build_wedge(r_min: f64, r_max: f64) -> WedgeGrid {
    let half_rad = HALF_ANGLE_DEG.to_radians();
    let x_max = r_max * half_rad.sin();
    let y_min = -r_max;
    let y_max = -r_min * half_rad.cos();

    let half_w = (x_max / STEP_MM).ceil() as isize;
    let iy_min = (y_min / STEP_MM).floor() as isize;
    let iy_max = (y_max / STEP_MM).ceil() as isize;

    let width = (2 * half_w + 1) as u32;
    let height = (iy_max - iy_min + 1) as u32;

    let mut points = Vec::new();
    for py in 0..height {
        let y = (iy_min + py as isize) as f64 * STEP_MM;
        for px in 0..width {
            let x = (px as isize - half_w) as f64 * STEP_MM;
            let r = x.hypot(y);
            if r < r_min || r > r_max {
                continue;
            }
            // 12 o'clock is (0, -r). Angle increases left (-15 deg) to right (+15 deg).
            let theta = x.atan2(-y);
            if theta.abs() > half_rad {
                continue;
            }
            let col = (((theta + half_rad) / (2.0 * half_rad)) * 6.0).floor().clamp(0.0, 5.0) as usize;
            let row = (((r_max - r) / (r_max - r_min)) * 4.0).floor().clamp(0.0, 3.0) as usize;
            points.push(SamplePoint { px, py, x, y, patch: row * 6 + col });
        }
    }
    WedgeGrid { width, height, points }
}

/// Build a wedge centred at `axis_deg` clockwise from 12 o'clock in the y-down disc frame
/// (0 deg = 12 o'clock (0, -1), 90 deg = 3 o'clock (1, 0), 270 deg = 9 o'clock (-1, 0)).
fn build_wedge_at(r_min: f64, r_max: f64, axis_deg: f64) -> WedgeGrid {
    let half_rad = HALF_ANGLE_DEG.to_radians();
    let axis_rad = axis_deg.to_radians();
    let (sa, ca) = axis_rad.sin_cos();
    // Unit axis vector (sa, -ca) and clockwise perpendicular (ca, sa).
    let (mut x_min, mut x_max) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut y_min, mut y_max) = (f64::INFINITY, f64::NEG_INFINITY);
    const BOUND_SAMPLES: usize = 240;
    for i in 0..=BOUND_SAMPLES {
        let rel = -half_rad + 2.0 * half_rad * i as f64 / BOUND_SAMPLES as f64;
        let ang = axis_rad + rel;
        let (s, c) = ang.sin_cos();
        for &r in &[r_min, r_max] {
            let x = r * s;
            let y = -r * c;
            x_min = x_min.min(x);
            x_max = x_max.max(x);
            y_min = y_min.min(y);
            y_max = y_max.max(y);
        }
    }

    let ix_min = (x_min / STEP_MM).floor() as isize;
    let ix_max = (x_max / STEP_MM).ceil() as isize;
    let iy_min = (y_min / STEP_MM).floor() as isize;
    let iy_max = (y_max / STEP_MM).ceil() as isize;

    let width = (ix_max - ix_min + 1) as u32;
    let height = (iy_max - iy_min + 1) as u32;

    let mut points = Vec::new();
    for py in 0..height {
        let y = (iy_min + py as isize) as f64 * STEP_MM;
        for px in 0..width {
            let x = (ix_min + px as isize) as f64 * STEP_MM;
            let r = x.hypot(y);
            if r < r_min || r > r_max {
                continue;
            }
            let proj_axis = x * sa - y * ca;
            let proj_perp = x * ca + y * sa;
            let rel = proj_perp.atan2(proj_axis);
            if rel.abs() > half_rad {
                continue;
            }
            let col = (((rel + half_rad) / (2.0 * half_rad)) * 6.0).floor().clamp(0.0, 5.0) as usize;
            let row = (((r_max - r) / (r_max - r_min)) * 4.0).floor().clamp(0.0, 3.0) as usize;
            points.push(SamplePoint { px, py, x, y, patch: row * 6 + col });
        }
    }
    WedgeGrid { width, height, points }
}

fn patch_target_linear(patch: usize, exposure: f64) -> [f64; 3] {
    let s = PATCHES[patch].srgb;
    [srgb_to_linear(s[0]) * exposure, srgb_to_linear(s[1]) * exposure, srgb_to_linear(s[2]) * exposure]
}

/// Solve a single point over one or more `(gt, gr)` views using the exact normal-equation
/// formulation from `holo::design::solve`.
fn solve_point(
    resp: &Responses,
    views_g: &[(f64, f64)],
    target_lin: [f64; 3],
    chroma_weight: f64,
) -> Vec<f64> {
    let k = resp.options;
    let c = chroma_weight.max(0.0).sqrt();
    let tg_y = luma_f64(target_lin);

    let mut gram = vec![0.0f64; k * k];
    let mut rhs = vec![0.0f64; k];
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
    };

    for &(gt, gr) in views_g {
        for j in 0..k {
            cols[j] = resp.at(j, gt, gr);
            row[j] = luma_f32(cols[j]);
        }
        add_row(&row, tg_y);
        if c > 0.0 && tg_y > 1e-6 {
            for ch in 0..3 {
                for j in 0..k {
                    row[j] = c * cols[j][ch] as f64;
                }
                add_row(&row, c * target_lin[ch]);
            }
        }
    }

    solve_simplex_qp(&gram, &rhs, k)
}

fn solve_wedge(
    wedge: &WedgeGrid,
    resp: &Responses,
    scenes: &[Scene],
    model: GeomModel,
    exposure: f64,
    chroma_weight: f64,
) -> Vec<Vec<f64>> {
    wedge
        .points
        .par_iter()
        .map(|pt| {
            let views_g: Vec<(f64, f64)> = scenes.iter().map(|sc| g_with_model(model, sc, pt.x, pt.y)).collect();
            let target_lin = patch_target_linear(pt.patch, exposure);
            solve_point(resp, &views_g, target_lin, chroma_weight)
        })
        .collect()
}

fn achieved_rgb(resp: &Responses, weights: &[f64], gt: f64, gr: f64) -> [f64; 3] {
    let mut out = [0.0f64; 3];
    for (j, &w) in weights.iter().enumerate() {
        if w == 0.0 {
            continue;
        }
        let c = resp.at(j, gt, gr);
        out[0] += w * c[0] as f64;
        out[1] += w * c[1] as f64;
        out[2] += w * c[2] as f64;
    }
    out
}

struct EvalStats {
    median_de: f64,
    p90_de: f64,
    mean_de: f64,
    mean_dl: f64,
    mean_dc: f64,
    patch_mean_de: [f64; 24],
    patch_mean_dl: [f64; 24],
    patch_mean_dc: [f64; 24],
    patch_counts: [usize; 24],
    achieved: Vec<[f64; 3]>,
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let pos = q.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    let t = pos - lo as f64;
    sorted[lo] * (1.0 - t) + sorted[hi] * t
}

/// Evaluate weights using the true geometry model T at `eval_scene`.
fn evaluate_wedge(
    wedge: &WedgeGrid,
    resp: &Responses,
    weights: &[Vec<f64>],
    eval_scene: &Scene,
    exposure: f64,
) -> EvalStats {
    let target_labs: [[f64; 3]; 24] = std::array::from_fn(|p| linear_srgb_to_lab(patch_target_linear(p, exposure), exposure));

    let per_point: Vec<([f64; 3], f64, f64, f64, usize)> = wedge
        .points
        .par_iter()
        .zip(weights.par_iter())
        .map(|(pt, w)| {
            let (gt, gr) = g_at(eval_scene, pt.x, pt.y);
            let ach = achieved_rgb(resp, w, gt, gr);
            let ach_lab = linear_srgb_to_lab(ach, exposure);
            let tgt_lab = target_labs[pt.patch];
            let de = delta_e76(ach_lab, tgt_lab);
            let dl = ach_lab[0] - tgt_lab[0];
            let c_ach = ach_lab[1].hypot(ach_lab[2]);
            let c_tgt = tgt_lab[1].hypot(tgt_lab[2]);
            let dc = c_ach - c_tgt;
            (ach, de, dl, dc, pt.patch)
        })
        .collect();

    let n = per_point.len() as f64;
    let mut des = Vec::with_capacity(per_point.len());
    let mut achieved = Vec::with_capacity(per_point.len());
    let (mut sum_de, mut sum_dl, mut sum_dc) = (0.0, 0.0, 0.0);
    let mut p_de = [0.0f64; 24];
    let mut p_dl = [0.0f64; 24];
    let mut p_dc = [0.0f64; 24];
    let mut p_cnt = [0usize; 24];

    for (ach, de, dl, dc, patch) in per_point {
        achieved.push(ach);
        des.push(de);
        sum_de += de;
        sum_dl += dl;
        sum_dc += dc;
        p_de[patch] += de;
        p_dl[patch] += dl;
        p_dc[patch] += dc;
        p_cnt[patch] += 1;
    }

    des.sort_by(|a, b| a.total_cmp(b));
    let mut patch_mean_de = [0.0; 24];
    let mut patch_mean_dl = [0.0; 24];
    let mut patch_mean_dc = [0.0; 24];
    for p in 0..24 {
        if p_cnt[p] > 0 {
            let c = p_cnt[p] as f64;
            patch_mean_de[p] = p_de[p] / c;
            patch_mean_dl[p] = p_dl[p] / c;
            patch_mean_dc[p] = p_dc[p] / c;
        }
    }

    EvalStats {
        median_de: percentile(&des, 0.50),
        p90_de: percentile(&des, 0.90),
        mean_de: sum_de / n,
        mean_dl: sum_dl / n,
        mean_dc: sum_dc / n,
        patch_mean_de,
        patch_mean_dl,
        patch_mean_dc,
        patch_counts: p_cnt,
        achieved,
    }
}

fn render_target(wedge: &WedgeGrid, exposure: f64) -> RgbImage {
    let mut img = RgbImage::from_pixel(wedge.width, wedge.height, Rgb([0, 0, 0]));
    for pt in &wedge.points {
        let lin = patch_target_linear(pt.patch, exposure);
        img.put_pixel(
            pt.px,
            pt.py,
            Rgb([
                linear_to_srgb_u8(lin[0] / exposure),
                linear_to_srgb_u8(lin[1] / exposure),
                linear_to_srgb_u8(lin[2] / exposure),
            ]),
        );
    }
    img
}

fn render_achieved(wedge: &WedgeGrid, achieved: &[[f64; 3]], exposure: f64) -> RgbImage {
    let mut img = RgbImage::from_pixel(wedge.width, wedge.height, Rgb([0, 0, 0]));
    for (pt, lin) in wedge.points.iter().zip(achieved.iter()) {
        img.put_pixel(
            pt.px,
            pt.py,
            Rgb([
                linear_to_srgb_u8(lin[0] / exposure),
                linear_to_srgb_u8(lin[1] / exposure),
                linear_to_srgb_u8(lin[2] / exposure),
            ]),
        );
    }
    img
}

fn build_contact_sheet(panels: &[&RgbImage], gap: u32) -> RgbImage {
    let w = panels[0].width();
    let h = panels[0].height();
    let total_w = w * panels.len() as u32 + gap * (panels.len().saturating_sub(1) as u32);
    let mut sheet = RgbImage::from_pixel(total_w, h, Rgb([48, 48, 48]));
    for (idx, panel) in panels.iter().enumerate() {
        let x0 = idx as u32 * (w + gap);
        for y in 0..h {
            for x in 0..w {
                sheet.put_pixel(x0 + x, y, *panel.get_pixel(x, y));
            }
        }
    }
    sheet
}

fn out_dir() -> PathBuf {
    if let Ok(p) = std::env::var("COLOR_STUDY_OUT") {
        PathBuf::from(p)
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/color-study")
    }
}

#[test]
#[ignore]
fn color_study() {
    let out = out_dir();
    std::fs::create_dir_all(&out).expect("create output directory");

    let preset = reference_preset();
    let bit_um = preset.channel_bit_length_nm * 1e-3;
    let inner = preset.inner_radius_mm;
    let outer = preset.outer_radius_mm;
    let r_min = inner + RADIAL_MARGIN_MM;
    let r_max = outer - RADIAL_MARGIN_MM;
    let r_mid = 0.5 * (inner + outer);

    let shared = shared_codebook(bit_um);
    let (cb, resp) = (&shared.0, &shared.1);

    let design_sc = pose_scene(0.0, 0.0);

    // Assert that parallel assumption P matches true model T identically on the 12 o'clock axis (x = 0).
    for &r in &[r_min, inner + 3.0, r_mid, outer - 3.0, r_max] {
        let (gt_t, gr_t) = g_at(&design_sc, 0.0, -r);
        let (gt_p, gr_p) = g_at_parallel(&design_sc, 0.0, -r);
        assert!(
            (gt_t - gt_p).abs() < 1e-12 && (gr_t - gr_p).abs() < 1e-12,
            "P must match T at x=0, r={r}: T=({gt_t}, {gr_t}) vs P=({gt_p}, {gr_p})"
        );
    }

    let wedge = build_wedge(r_min, r_max);
    let mut md = String::new();

    writeln!(md, "# Diffraction Colour Reproduction Study").unwrap();
    writeln!(md).unwrap();
    writeln!(
        md,
        "- Preset: `{}` (DVD, bit = {:.4} um, inner = {:.3} mm, outer = {:.3} mm)",
        preset.id, bit_um, inner, outer
    )
    .unwrap();
    writeln!(
        md,
        "- Codebook: {} entries + 1 plain = {} options",
        cb.entries.len(),
        resp.options
    )
    .unwrap();
    writeln!(
        md,
        "- Wedge: r in [{:.3}, {:.3}] mm, angle in [-{:.1}, +{:.1}] deg from 12 o'clock (0, -r)",
        r_min, r_max, HALF_ANGLE_DEG, HALF_ANGLE_DEG
    )
    .unwrap();
    writeln!(
        md,
        "- Sampling grid: {:.2} mm step (4 px/mm), {} sample points in {}x{} px bounding box",
        STEP_MM,
        wedge.points.len(),
        wedge.width,
        wedge.height
    )
    .unwrap();
    writeln!(
        md,
        "- Design pose: delta = 0 deg, head_x = 0 mm, eye = {:?} mm, lamp = {:?} mm",
        design_sc.camera_mm, design_sc.light_mm
    )
    .unwrap();
    writeln!(
        md,
        "- Colour metric: linear sRGB -> XYZ (D65) -> CIELAB with ref white = XYZ(exposure, exposure, exposure); dE76, dL = L_ach - L_tgt, dC = C_ach - C_tgt (negative dC = chroma loss)."
    )
    .unwrap();
    writeln!(md).unwrap();

    // Render target.png
    let target_img = render_target(&wedge, DEFAULT_EXPOSURE);
    target_img.save(out.join("target.png")).unwrap();

    // =========================================================================
    // E1: Geometry
    // =========================================================================
    writeln!(md, "## E1: Geometry at Design Pose (delta = 0 deg, head_x = 0 mm)").unwrap();
    writeln!(md).unwrap();
    writeln!(
        md,
        "| r (mm) | angle (deg) | gt (T) | gr (T) | gt (P) | gr (P) | gt_T - gt_P | lam_T(0.469) nm | lam_T(1.25) nm |"
    )
    .unwrap();
    writeln!(md, "|---:|---:|---:|---:|---:|---:|---:|---:|---:|").unwrap();
    for &r in &[inner + 3.0, r_mid, outer - 3.0] {
        for &ang in &[-15.0_f64, -7.5, 0.0, 7.5, 15.0] {
            let th = ang.to_radians();
            let x = r * th.sin();
            let y = -r * th.cos();
            let (gt_t, gr_t) = g_at(&design_sc, x, y);
            let (gt_p, gr_p) = g_at_parallel(&design_sc, x, y);
            let lam_lo = gt_t.abs() / 0.469 * 1000.0;
            let lam_hi = gt_t.abs() / 1.25 * 1000.0;
            writeln!(
                md,
                "| {:.2} | {:+.1} | {:.4} | {:.4} | {:.4} | {:.4} | {:+.4} | {:.1} | {:.1} |",
                r,
                ang,
                gt_t,
                gr_t,
                gt_p,
                gr_p,
                gt_t - gt_p,
                lam_lo,
                lam_hi
            )
            .unwrap();
        }
    }
    writeln!(md).unwrap();

    // =========================================================================
    // E2: Design pose (solve with T, evaluate with T)
    // =========================================================================
    let e2_weights = solve_wedge(
        &wedge,
        resp,
        &[design_sc.clone()],
        GeomModel::True,
        DEFAULT_EXPOSURE,
        DEFAULT_CHROMA_WEIGHT,
    );
    let e2_eval = evaluate_wedge(&wedge, resp, &e2_weights, &design_sc, DEFAULT_EXPOSURE);
    let e2_img = render_achieved(&wedge, &e2_eval.achieved, DEFAULT_EXPOSURE);
    e2_img.save(out.join("e2_true.png")).unwrap();

    writeln!(
        md,
        "## E2: Design Pose Solve (Model T, chroma_weight = {:.1}, exposure = {:.2})",
        DEFAULT_CHROMA_WEIGHT, DEFAULT_EXPOSURE
    )
    .unwrap();
    writeln!(md).unwrap();
    writeln!(
        md,
        "Overall: median dE = **{:.2}**, p90 dE = **{:.2}**, mean dE = {:.2}, mean dL = {:+.2}, mean dC = {:+.2}",
        e2_eval.median_de, e2_eval.p90_de, e2_eval.mean_de, e2_eval.mean_dl, e2_eval.mean_dc
    )
    .unwrap();
    writeln!(md).unwrap();
    writeln!(md, "| # | Patch | sRGB | pts | mean dE | mean dL | mean dC |").unwrap();
    writeln!(md, "|---:|---|---|---:|---:|---:|---:|").unwrap();
    for p in 0..24 {
        let s = PATCHES[p].srgb;
        writeln!(
            md,
            "| {} | {} | ({},{},{}) | {} | {:.2} | {:+.2} | {:+.2} |",
            p + 1,
            PATCHES[p].name,
            s[0],
            s[1],
            s[2],
            e2_eval.patch_counts[p],
            e2_eval.patch_mean_de[p],
            e2_eval.patch_mean_dl[p],
            e2_eval.patch_mean_dc[p]
        )
        .unwrap();
    }
    writeln!(md).unwrap();

    // =========================================================================
    // E3: Parallel assumption (solve with P, evaluate with T)
    // =========================================================================
    let e3_weights = solve_wedge(
        &wedge,
        resp,
        &[design_sc.clone()],
        GeomModel::Parallel,
        DEFAULT_EXPOSURE,
        DEFAULT_CHROMA_WEIGHT,
    );
    let e3_eval = evaluate_wedge(&wedge, resp, &e3_weights, &design_sc, DEFAULT_EXPOSURE);
    let e3_img = render_achieved(&wedge, &e3_eval.achieved, DEFAULT_EXPOSURE);
    e3_img.save(out.join("e3_parallel.png")).unwrap();

    writeln!(
        md,
        "## E3: Parallel Assumption Solve (Model P -> Evaluated with T, chroma_weight = {:.1}, exposure = {:.2})",
        DEFAULT_CHROMA_WEIGHT, DEFAULT_EXPOSURE
    )
    .unwrap();
    writeln!(md).unwrap();
    writeln!(
        md,
        "Overall: median dE = **{:.2}**, p90 dE = **{:.2}**, mean dE = {:.2}, mean dL = {:+.2}, mean dC = {:+.2}",
        e3_eval.median_de, e3_eval.p90_de, e3_eval.mean_de, e3_eval.mean_dl, e3_eval.mean_dc
    )
    .unwrap();
    writeln!(md).unwrap();
    writeln!(md, "| # | Patch | sRGB | pts | mean dE | mean dL | mean dC |").unwrap();
    writeln!(md, "|---:|---|---|---:|---:|---:|---:|").unwrap();
    for p in 0..24 {
        let s = PATCHES[p].srgb;
        writeln!(
            md,
            "| {} | {} | ({},{},{}) | {} | {:.2} | {:+.2} | {:+.2} |",
            p + 1,
            PATCHES[p].name,
            s[0],
            s[1],
            s[2],
            e3_eval.patch_counts[p],
            e3_eval.patch_mean_de[p],
            e3_eval.patch_mean_dl[p],
            e3_eval.patch_mean_dc[p]
        )
        .unwrap();
    }
    writeln!(md).unwrap();

    // =========================================================================
    // E4 (Robustness of E2) & E5 (Multi-view solve)
    // =========================================================================
    let mv_scenes: Vec<Scene> = [-3.0, 0.0, 3.0]
        .iter()
        .flat_map(|&d| [-40.0, 0.0, 40.0].iter().map(move |&h| pose_scene(d, h)))
        .collect();
    let e5_weights = solve_wedge(
        &wedge,
        resp,
        &mv_scenes,
        GeomModel::True,
        DEFAULT_EXPOSURE,
        DEFAULT_CHROMA_WEIGHT,
    );
    let e5_design = evaluate_wedge(&wedge, resp, &e5_weights, &design_sc, DEFAULT_EXPOSURE);
    let e5_img = render_achieved(&wedge, &e5_design.achieved, DEFAULT_EXPOSURE);
    e5_img.save(out.join("e5_multiview.png")).unwrap();

    let eval_poses: Vec<(&str, f64, f64, Option<&str>)> = vec![
        ("design (0 deg, 0 mm)", 0.0, 0.0, None),
        ("delta -10 deg", -10.0, 0.0, None),
        ("delta -5 deg", -5.0, 0.0, Some("e4_delta_m5.png")),
        ("delta -2 deg", -2.0, 0.0, None),
        ("delta -1 deg", -1.0, 0.0, None),
        ("delta +1 deg", 1.0, 0.0, None),
        ("delta +2 deg", 2.0, 0.0, None),
        ("delta +5 deg", 5.0, 0.0, Some("e4_delta_p5.png")),
        ("delta +10 deg", 10.0, 0.0, None),
        ("head_x -80 mm", 0.0, -80.0, None),
        ("head_x -40 mm", 0.0, -40.0, Some("e4_head_m40.png")),
        ("head_x -20 mm", 0.0, -20.0, None),
        ("head_x +20 mm", 0.0, 20.0, None),
        ("head_x +40 mm", 0.0, 40.0, Some("e4_head_p40.png")),
        ("head_x +80 mm", 0.0, 80.0, None),
    ];

    let mut e4_delta_m5_img: Option<RgbImage> = None;
    let mut e4_delta_p5_img: Option<RgbImage> = None;

    writeln!(
        md,
        "## E4 & E5: Pose Robustness (E2 Single-Pose vs E5 9-View Solve: delta {{-3,0,3}} deg x head {{-40,0,40}} mm)"
    )
    .unwrap();
    writeln!(md).unwrap();
    writeln!(
        md,
        "| Pose | E4 med dE | E4 p90 dE | E4 mean dL | E4 mean dC | E5 med dE | E5 p90 dE | E5 mean dL | E5 mean dC |"
    )
    .unwrap();
    writeln!(md, "|---|---:|---:|---:|---:|---:|---:|---:|---:|").unwrap();

    for (label, delta_deg, head_x, png_name) in &eval_poses {
        let sc = pose_scene(*delta_deg, *head_x);
        let r4 = evaluate_wedge(&wedge, resp, &e2_weights, &sc, DEFAULT_EXPOSURE);
        let r5 = evaluate_wedge(&wedge, resp, &e5_weights, &sc, DEFAULT_EXPOSURE);
        if let Some(name) = png_name {
            let img = render_achieved(&wedge, &r4.achieved, DEFAULT_EXPOSURE);
            img.save(out.join(name)).unwrap();
            if *name == "e4_delta_m5.png" {
                e4_delta_m5_img = Some(img);
            } else if *name == "e4_delta_p5.png" {
                e4_delta_p5_img = Some(img);
            }
        }
        writeln!(
            md,
            "| {} | {:.2} | {:.2} | {:+.2} | {:+.2} | {:.2} | {:.2} | {:+.2} | {:+.2} |",
            label,
            r4.median_de,
            r4.p90_de,
            r4.mean_dl,
            r4.mean_dc,
            r5.median_de,
            r5.p90_de,
            r5.mean_dl,
            r5.mean_dc
        )
        .unwrap();
    }
    writeln!(md).unwrap();

    // Contact sheet: target, E2, E3, E4 delta -5, E4 delta +5, E5 at design pose
    let contact = build_contact_sheet(
        &[
            &target_img,
            &e2_img,
            &e3_img,
            e4_delta_m5_img.as_ref().unwrap(),
            e4_delta_p5_img.as_ref().unwrap(),
            &e5_img,
        ],
        6,
    );
    contact.save(out.join("contact.png")).unwrap();

    // =========================================================================
    // E6: Exposure x chroma_weight sweep (single design-pose solve with T)
    // =========================================================================
    writeln!(md, "## E6: Exposure x chroma_weight Sweep (Design Pose, Model T)").unwrap();
    writeln!(md).unwrap();
    writeln!(
        md,
        "| exposure | chroma_weight | median dE | p90 dE | mean dL | mean dC | patches mean dE < 10 |"
    )
    .unwrap();
    writeln!(md, "|---:|---:|---:|---:|---:|---:|---:|").unwrap();
    for &exp in &[0.3, 0.45, 0.6, 0.8, 1.0] {
        for &cw in &[0.3, 1.0, 3.0] {
            let w = solve_wedge(&wedge, resp, &[design_sc.clone()], GeomModel::True, exp, cw);
            let ev = evaluate_wedge(&wedge, resp, &w, &design_sc, exp);
            let good = ev.patch_mean_de.iter().filter(|&&d| d < 10.0).count();
            writeln!(
                md,
                "| {:.2} | {:.1} | {:.2} | {:.2} | {:+.2} | {:+.2} | {}/24 |",
                exp, cw, ev.median_de, ev.p90_de, ev.mean_dl, ev.mean_dc, good
            )
            .unwrap();
        }
    }
    writeln!(md).unwrap();

    // =========================================================================
    // E7: Gamut at L* = 50, exposure = 0.6 (design pose, model T)
    // =========================================================================
    let gamut_pts = [
        ("axis (0 deg, r_mid)", 0.0_f64),
        ("-15 deg (r_mid)", -15.0_f64),
        ("+15 deg (r_mid)", 15.0_f64),
    ];

    writeln!(
        md,
        "## E7: Achievable Chroma C* at L* = 50 (dE < 5, exposure = {:.2}, chroma_weight = {:.1}, design pose)",
        DEFAULT_EXPOSURE, DEFAULT_CHROMA_WEIGHT
    )
    .unwrap();
    writeln!(md).unwrap();
    writeln!(
        md,
        "| Hue (deg) | sRGB max C* | axis (0 deg) | -15 deg | +15 deg |"
    )
    .unwrap();
    writeln!(md, "|---:|---:|---:|---:|---:|").unwrap();

    for h_step in 0..12 {
        let hue_deg = (h_step * 30) as f64;
        let hue_rad = hue_deg.to_radians();

        let mut srgb_max_c: Option<u32> = None;
        for c_int in (0..=100u32).step_by(5) {
            let c_val = c_int as f64;
            let lab = [50.0, c_val * hue_rad.cos(), c_val * hue_rad.sin()];
            let rgb = lab_to_linear_srgb(lab, DEFAULT_EXPOSURE);
            if rgb.iter().all(|&v| v >= -1e-9 && v <= DEFAULT_EXPOSURE + 1e-9) {
                srgb_max_c = Some(c_int);
            }
        }

        let mut pt_max = [None::<u32>; 3];
        for (pi, &(_, ang_deg)) in gamut_pts.iter().enumerate() {
            let th = ang_deg.to_radians();
            let x = r_mid * th.sin();
            let y = -r_mid * th.cos();
            let g = g_at(&design_sc, x, y);
            for c_int in (0..=100u32).step_by(5) {
                let c_val = c_int as f64;
                let tgt_lab = [50.0, c_val * hue_rad.cos(), c_val * hue_rad.sin()];
                let tgt_rgb = lab_to_linear_srgb(tgt_lab, DEFAULT_EXPOSURE);
                let w = solve_point(resp, &[g], tgt_rgb, DEFAULT_CHROMA_WEIGHT);
                let ach = achieved_rgb(resp, &w, g.0, g.1);
                let ach_lab = linear_srgb_to_lab(ach, DEFAULT_EXPOSURE);
                if delta_e76(ach_lab, tgt_lab) < 5.0 {
                    pt_max[pi] = Some(c_int);
                }
            }
        }

        let fmt_c = |v: Option<u32>| v.map_or_else(|| "none".to_string(), |c| c.to_string());
        writeln!(
            md,
            "| {:3.0} | {} | {} | {} | {} |",
            hue_deg,
            fmt_c(srgb_max_c),
            fmt_c(pt_max[0]),
            fmt_c(pt_max[1]),
            fmt_c(pt_max[2])
        )
        .unwrap();
    }
    writeln!(md).unwrap();

    // =========================================================================
    // E8: Tangent-orientation hypothesis (vertical lamp + 3/9 o'clock wedges)
    // =========================================================================
    writeln!(
        md,
        "## E8: Horizontal Head Motion vs Track Tangent Orientation (G0 vs G1 vs G2)"
    )
    .unwrap();
    writeln!(md).unwrap();
    writeln!(
        md,
        "- Pose generalisation: eye = turn([hx, hy, 350], delta) in disc frame (+x right, +y down), lamp = turn(scene_to_disc(lamp_scene), delta)."
    )
    .unwrap();

    // Check G1 and G2 |gt| band at default lamp height 400 mm; test 300/500 mm if needed.
    let g0_mean_abs_gt: f64 = {
        let mut s = 0.0;
        let mut n = 0usize;
        for &r in &[inner + 3.0, r_mid, outer - 3.0] {
            for &ang in &[-15.0_f64, -7.5, 0.0, 7.5, 15.0] {
                let th = ang.to_radians();
                let (gt, _) = g_at(&design_sc, r * th.sin(), -r * th.cos());
                s += gt.abs();
                n += 1;
            }
        }
        s / n as f64
    };

    let sample_gt_stats = |lamp_scene: [f64; 3], axis_deg: f64| -> (f64, f64, f64) {
        let sc = general_pose_scene(lamp_scene, 0.0, 0.0, 0.0);
        let axis_rad = axis_deg.to_radians();
        let (mut sum, mut lo, mut hi, mut n) = (0.0, f64::INFINITY, 0.0_f64, 0usize);
        for &r in &[inner + 3.0, r_mid, outer - 3.0] {
            for &ang in &[-15.0_f64, -7.5, 0.0, 7.5, 15.0] {
                let th = axis_rad + ang.to_radians();
                let (gt, _) = g_at(&sc, r * th.sin(), -r * th.cos());
                let a = gt.abs();
                sum += a;
                lo = lo.min(a);
                hi = hi.max(a);
                n += 1;
            }
        }
        (sum / n as f64, lo, hi)
    };

    let mut g1_lamp_scene = [0.0, 150.0, 400.0];
    let (g1_mean, g1_lo, g1_hi) = sample_gt_stats(g1_lamp_scene, 90.0);
    if !(0.30..=0.40).contains(&g1_mean) {
        writeln!(
            md,
            "- G1 default lamp [0, 150, 400] had mean |gt| = {:.4} (outside 0.30..0.40); testing lamp z in {{300, 400, 500}} mm against G0 mean |gt| = {:.4}:",
            g1_mean, g0_mean_abs_gt
        )
        .unwrap();
        let mut best_z = 400.0;
        let mut best_diff = (g1_mean - g0_mean_abs_gt).abs();
        for &z in &[300.0, 400.0, 500.0] {
            let (m, lo, hi) = sample_gt_stats([0.0, 150.0, z], 90.0);
            writeln!(
                md,
                "  - z = {:.0} mm: mean |gt| = {:.4} (range {:.4}..{:.4})",
                z, m, lo, hi
            )
            .unwrap();
            let d = (m - g0_mean_abs_gt).abs();
            if d < best_diff {
                best_diff = d;
                best_z = z;
            }
        }
        g1_lamp_scene = [0.0, 150.0, best_z];
        writeln!(md, "- Selected G1/G2 lamp height z = {:.0} mm.", best_z).unwrap();
    } else {
        writeln!(
            md,
            "- G1/G2 lamp [0, 150, 400] (scene frame; disc frame [0, -150, 400]) gives mean |gt| = {:.4} (range {:.4}..{:.4}), matching G0's mean |gt| = {:.4} (within 0.30..0.40, so z = 400 mm is used).",
            g1_mean, g1_lo, g1_hi, g0_mean_abs_gt
        )
        .unwrap();
    }
    writeln!(md).unwrap();

    struct E8Geom {
        label: &'static str,
        lamp_scene: [f64; 3],
        axis_deg: f64,
        eyes_png: &'static str,
    }

    let geoms = [
        E8Geom { label: "G0 (12 o'clock)", lamp_scene: LAMP_MM, axis_deg: 0.0, eyes_png: "e8_g0_eyes.png" },
        E8Geom { label: "G1 (3 o'clock)", lamp_scene: g1_lamp_scene, axis_deg: 90.0, eyes_png: "e8_g1_eyes.png" },
        E8Geom { label: "G2 (9 o'clock)", lamp_scene: g1_lamp_scene, axis_deg: 270.0, eyes_png: "e8_g2_eyes.png" },
    ];

    // E8.1: Geometry tables for G1 and G2
    for g in &geoms[1..] {
        let sc = general_pose_scene(g.lamp_scene, 0.0, 0.0, 0.0);
        let axis_rad = g.axis_deg.to_radians();
        writeln!(
            md,
            "### E8.1 Geometry Table: {} (lamp scene {:?}, disc {:?}, axis {:.0} deg)",
            g.label, g.lamp_scene, sc.light_mm, g.axis_deg
        )
        .unwrap();
        writeln!(md).unwrap();
        writeln!(
            md,
            "| r (mm) | rel angle (deg) | gt (T) | gr (T) | lam_T(0.469) nm | lam_T(1.25) nm |"
        )
        .unwrap();
        writeln!(md, "|---:|---:|---:|---:|---:|---:|").unwrap();
        for &r in &[inner + 3.0, r_mid, outer - 3.0] {
            for &ang in &[-15.0_f64, -7.5, 0.0, 7.5, 15.0] {
                let th = axis_rad + ang.to_radians();
                let x = r * th.sin();
                let y = -r * th.cos();
                let (gt_t, gr_t) = g_at(&sc, x, y);
                let lam_lo = gt_t.abs() / 0.469 * 1000.0;
                let lam_hi = gt_t.abs() / 1.25 * 1000.0;
                writeln!(
                    md,
                    "| {:.2} | {:+.1} | {:.4} | {:.4} | {:.1} | {:.1} |",
                    r, ang, gt_t, gr_t, lam_lo, lam_hi
                )
                .unwrap();
            }
        }
        writeln!(md).unwrap();
    }

    // Build wedges and solve centre (single-pose) and binocular (2-view) for G0, G1, G2.
    let e8_wedges: Vec<WedgeGrid> = geoms.iter().map(|g| build_wedge_at(r_min, r_max, g.axis_deg)).collect();
    let e8_centre_weights: Vec<Vec<Vec<f64>>> = geoms
        .iter()
        .zip(&e8_wedges)
        .map(|(g, w)| {
            let sc = general_pose_scene(g.lamp_scene, 0.0, 0.0, 0.0);
            solve_wedge(w, resp, &[sc], GeomModel::True, DEFAULT_EXPOSURE, DEFAULT_CHROMA_WEIGHT)
        })
        .collect();
    let e8_bino_weights: Vec<Vec<Vec<f64>>> = geoms
        .iter()
        .zip(&e8_wedges)
        .map(|(g, w)| {
            let sc_l = general_pose_scene(g.lamp_scene, -32.0, 0.0, 0.0);
            let sc_r = general_pose_scene(g.lamp_scene, 32.0, 0.0, 0.0);
            solve_wedge(w, resp, &[sc_l, sc_r], GeomModel::True, DEFAULT_EXPOSURE, DEFAULT_CHROMA_WEIGHT)
        })
        .collect();

    // E8.2: Pose robustness comparison table across G0, G1, G2
    let e8_poses: Vec<(&str, f64, f64, f64)> = vec![
        ("design (0, 0, 0 deg)", 0.0, 0.0, 0.0),
        ("hx -80 mm", -80.0, 0.0, 0.0),
        ("hx -40 mm", -40.0, 0.0, 0.0),
        ("hx -32 mm (L eye)", -32.0, 0.0, 0.0),
        ("hx -20 mm", -20.0, 0.0, 0.0),
        ("hx +20 mm", 20.0, 0.0, 0.0),
        ("hx +32 mm (R eye)", 32.0, 0.0, 0.0),
        ("hx +40 mm", 40.0, 0.0, 0.0),
        ("hx +80 mm", 80.0, 0.0, 0.0),
        ("hy -80 mm", 0.0, -80.0, 0.0),
        ("hy -40 mm", 0.0, -40.0, 0.0),
        ("hy -32 mm", 0.0, -32.0, 0.0),
        ("hy -20 mm", 0.0, -20.0, 0.0),
        ("hy +20 mm", 0.0, 20.0, 0.0),
        ("hy +32 mm", 0.0, 32.0, 0.0),
        ("hy +40 mm", 0.0, 40.0, 0.0),
        ("hy +80 mm", 0.0, 80.0, 0.0),
        ("delta -5 deg", 0.0, 0.0, -5.0),
        ("delta -2 deg", 0.0, 0.0, -2.0),
        ("delta +2 deg", 0.0, 0.0, 2.0),
        ("delta +5 deg", 0.0, 0.0, 5.0),
    ];

    writeln!(
        md,
        "### E8.2 Single-Pose Robustness: G0 (12 o'clock) vs G1 (3 o'clock) vs G2 (9 o'clock)"
    )
    .unwrap();
    writeln!(md).unwrap();
    writeln!(
        md,
        "| Pose | G0 med dE | G0 p90 dE | G1 med dE | G1 p90 dE | G2 med dE | G2 p90 dE |"
    )
    .unwrap();
    writeln!(md, "|---|---:|---:|---:|---:|---:|---:|").unwrap();
    for &(label, hx, hy, delta_deg) in &e8_poses {
        let mut row_vals = [(0.0, 0.0); 3];
        for gi in 0..3 {
            let sc = general_pose_scene(geoms[gi].lamp_scene, hx, hy, delta_deg);
            let ev = evaluate_wedge(&e8_wedges[gi], resp, &e8_centre_weights[gi], &sc, DEFAULT_EXPOSURE);
            row_vals[gi] = (ev.median_de, ev.p90_de);
        }
        writeln!(
            md,
            "| {} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} |",
            label,
            row_vals[0].0,
            row_vals[0].1,
            row_vals[1].0,
            row_vals[1].1,
            row_vals[2].0,
            row_vals[2].1
        )
        .unwrap();
    }
    writeln!(md).unwrap();

    // E8.3: Binocular evaluation (eyes at hx = -32 and +32 mm, hy = 0) + PNGs
    writeln!(
        md,
        "### E8.3 Binocular Evaluation (Left Eye hx = -32 mm, Right Eye hx = +32 mm, hy = 0)"
    )
    .unwrap();
    writeln!(md).unwrap();
    writeln!(
        md,
        "| Geometry | Solve | View | median dE | p90 dE | mean dL | mean dC |"
    )
    .unwrap();
    writeln!(md, "|---|---|---|---:|---:|---:|---:|").unwrap();

    for gi in 0..3 {
        let g = &geoms[gi];
        let w = &e8_wedges[gi];
        let sc_l = general_pose_scene(g.lamp_scene, -32.0, 0.0, 0.0);
        let sc_c = general_pose_scene(g.lamp_scene, 0.0, 0.0, 0.0);
        let sc_r = general_pose_scene(g.lamp_scene, 32.0, 0.0, 0.0);

        let ev_c_l = evaluate_wedge(w, resp, &e8_centre_weights[gi], &sc_l, DEFAULT_EXPOSURE);
        let ev_c_r = evaluate_wedge(w, resp, &e8_centre_weights[gi], &sc_r, DEFAULT_EXPOSURE);

        let img_l = render_achieved(w, &ev_c_l.achieved, DEFAULT_EXPOSURE);
        let img_r = render_achieved(w, &ev_c_r.achieved, DEFAULT_EXPOSURE);
        let pair = build_contact_sheet(&[&img_l, &img_r], 6);
        pair.save(out.join(g.eyes_png)).unwrap();

        let ev_b_l = evaluate_wedge(w, resp, &e8_bino_weights[gi], &sc_l, DEFAULT_EXPOSURE);
        let ev_b_c = evaluate_wedge(w, resp, &e8_bino_weights[gi], &sc_c, DEFAULT_EXPOSURE);
        let ev_b_r = evaluate_wedge(w, resp, &e8_bino_weights[gi], &sc_r, DEFAULT_EXPOSURE);

        for (solve_name, view_name, ev) in [
            ("(a) centre", "left (-32 mm)", &ev_c_l),
            ("(a) centre", "right (+32 mm)", &ev_c_r),
            ("(b) 2-view eyes", "left (-32 mm)", &ev_b_l),
            ("(b) 2-view eyes", "centre (0 mm)", &ev_b_c),
            ("(b) 2-view eyes", "right (+32 mm)", &ev_b_r),
        ] {
            writeln!(
                md,
                "| {} | {} | {} | {:.2} | {:.2} | {:+.2} | {:+.2} |",
                g.label, solve_name, view_name, ev.median_de, ev.p90_de, ev.mean_dl, ev.mean_dc
            )
            .unwrap();
        }
    }
    writeln!(md).unwrap();

    // =========================================================================
    // E9: Single diagonal lamp serving both 3D (12 o'clock) and flat colour (3 o'clock)
    // =========================================================================
    writeln!(
        md,
        "## E9: Shared Diagonal Lamp for 3D (12 o'clock) + Flat Colour (3 o'clock)"
    )
    .unwrap();
    writeln!(md).unwrap();

    const G0_TARGET_GT: f64 = 0.3435;
    let w12 = &e8_wedges[0];
    let w3 = &e8_wedges[1];

    let mean_abs_gt_wedge = |wedge: &WedgeGrid, sc: &Scene| -> f64 {
        let sum: f64 = wedge.points.iter().map(|pt| g_at(sc, pt.x, pt.y).0.abs()).sum();
        sum / wedge.points.len() as f64
    };
    let mean_abs_gr_wedge = |wedge: &WedgeGrid, sc: &Scene| -> f64 {
        let sum: f64 = wedge.points.iter().map(|pt| g_at(sc, pt.x, pt.y).1.abs()).sum();
        sum / wedge.points.len() as f64
    };

    // E9.1: Lamp search [a, a, 400] for a in 100..=320 step 10, plus best a for z = 300, 500
    writeln!(
        md,
        "### E9.1 Diagonal Lamp Search: lamp_scene = [a, a, 400] (target |gt| = {:.4})",
        G0_TARGET_GT
    )
    .unwrap();
    writeln!(md).unwrap();
    writeln!(
        md,
        "| a (mm) | mean |gt| (12 o'clock) | mean |gt| (3 o'clock) | mean |gr| (12 o'clock) | mean |gr| (3 o'clock) | err vs 0.3435 |"
    )
    .unwrap();
    writeln!(md, "|---:|---:|---:|---:|---:|---:|").unwrap();

    let search_z = |z: f64, log_table: Option<&mut String>| -> (f64, f64, f64, f64) {
        let mut best_a = 100.0;
        let mut best_m12 = 0.0;
        let mut best_m3 = 0.0;
        let mut best_err = f64::INFINITY;
        let mut out_md = log_table;
        for a_int in (100..=320).step_by(10) {
            let a = a_int as f64;
            let sc = general_pose_scene([a, a, z], 0.0, 0.0, 0.0);
            let m12 = mean_abs_gt_wedge(w12, &sc);
            let m3 = mean_abs_gt_wedge(w3, &sc);
            let err = ((m12 - G0_TARGET_GT).abs() + (m3 - G0_TARGET_GT).abs()) * 0.5;
            if let Some(ref mut s) = out_md {
                let gr12 = mean_abs_gr_wedge(w12, &sc);
                let gr3 = mean_abs_gr_wedge(w3, &sc);
                writeln!(
                    s,
                    "| {:3} | {:.4} | {:.4} | {:.4} | {:.4} | {:.4} |",
                    a_int, m12, m3, gr12, gr3, err
                )
                .unwrap();
            }
            if err < best_err {
                best_err = err;
                best_a = a;
                best_m12 = m12;
                best_m3 = m3;
            }
        }
        (best_a, best_m12, best_m3, best_err)
    };

    let (a400, m12_400, m3_400, err_400) = search_z(400.0, Some(&mut md));
    let (a300, m12_300, m3_300, err_300) = search_z(300.0, None);
    let (a500, m12_500, m3_500, err_500) = search_z(500.0, None);

    writeln!(md).unwrap();
    writeln!(md, "Best `a` per lamp height `z` (a in 100..=320 step 10):").unwrap();
    writeln!(md).unwrap();
    writeln!(
        md,
        "| z (mm) | best a (mm) | mean |gt| (12 o'clock) | mean |gt| (3 o'clock) | mean err vs 0.3435 |"
    )
    .unwrap();
    writeln!(md, "|---:|---:|---:|---:|---:|").unwrap();
    for &(z, a, m12, m3, err) in &[
        (300.0, a300, m12_300, m3_300, err_300),
        (400.0, a400, m12_400, m3_400, err_400),
        (500.0, a500, m12_500, m3_500, err_500),
    ] {
        writeln!(md, "| {:.0} | {:.0} | {:.4} | {:.4} | {:.4} |", z, a, m12, m3, err).unwrap();
    }
    writeln!(md).unwrap();

    // Use z = 400 a* unless another z gets both means clearly closer (by > 0.005).
    let (chosen_a, chosen_z) = if err_300 + 0.005 < err_400 && err_300 <= err_500 {
        (a300, 300.0)
    } else if err_500 + 0.005 < err_400 {
        (a500, 500.0)
    } else {
        (a400, 400.0)
    };
    let diag_lamp_scene = [chosen_a, chosen_a, chosen_z];
    let diag_design_sc = general_pose_scene(diag_lamp_scene, 0.0, 0.0, 0.0);
    writeln!(
        md,
        "Chosen diagonal lamp: `a* = {:.0} mm, z = {:.0} mm` -> scene `{:?}`, disc `{:?}`.",
        chosen_a, chosen_z, diag_lamp_scene, diag_design_sc.light_mm
    )
    .unwrap();
    writeln!(md).unwrap();

    // E9.2: Brightness headroom and design-pose dE
    let mean_headroom = |wedge: &WedgeGrid, sc: &Scene| -> f64 {
        let k = resp.options;
        let sum: f64 = wedge
            .points
            .par_iter()
            .map(|pt| {
                let (gt, gr) = g_at(sc, pt.x, pt.y);
                (0..k).map(|j| luma_f32(resp.at(j, gt, gr))).fold(0.0_f64, f64::max)
            })
            .sum();
        sum / wedge.points.len() as f64
    };

    let diag12_centre_w = solve_wedge(
        w12,
        resp,
        &[diag_design_sc.clone()],
        GeomModel::True,
        DEFAULT_EXPOSURE,
        DEFAULT_CHROMA_WEIGHT,
    );
    let diag3_centre_w = solve_wedge(
        w3,
        resp,
        &[diag_design_sc.clone()],
        GeomModel::True,
        DEFAULT_EXPOSURE,
        DEFAULT_CHROMA_WEIGHT,
    );

    let g0_12_ev = evaluate_wedge(w12, resp, &e8_centre_weights[0], &design_sc, DEFAULT_EXPOSURE);
    let diag12_ev = evaluate_wedge(w12, resp, &diag12_centre_w, &diag_design_sc, DEFAULT_EXPOSURE);
    let diag3_ev = evaluate_wedge(w3, resp, &diag3_centre_w, &diag_design_sc, DEFAULT_EXPOSURE);

    writeln!(
        md,
        "### E9.2 Brightness Headroom & Design-Pose Accuracy (chroma_weight = 1.0, exposure = 0.60)"
    )
    .unwrap();
    writeln!(md).unwrap();
    writeln!(
        md,
        "| Wedge / Lamp | mean |gt| | mean |gr| | mean max_j luma | median dE | p90 dE | mean dL | mean dC |"
    )
    .unwrap();
    writeln!(md, "|---|---:|---:|---:|---:|---:|---:|---:|").unwrap();
    for (name, wg, sc, ev) in [
        ("G0 12 o'clock", w12, &design_sc, &g0_12_ev),
        ("a* 12 o'clock", w12, &diag_design_sc, &diag12_ev),
        ("a* 3 o'clock", w3, &diag_design_sc, &diag3_ev),
    ] {
        writeln!(
            md,
            "| {} | {:.4} | {:.4} | {:.4} | {:.2} | {:.2} | {:+.2} | {:+.2} |",
            name,
            mean_abs_gt_wedge(wg, sc),
            mean_abs_gr_wedge(wg, sc),
            mean_headroom(wg, sc),
            ev.median_de,
            ev.p90_de,
            ev.mean_dl,
            ev.mean_dc
        )
        .unwrap();
    }
    writeln!(md).unwrap();

    // E9.3: 3D suitability (mean |gt(left) - gt(right)| between hx = -32 and +32 mm)
    let mean_eye_gt_diff = |wedge: &WedgeGrid, lamp_scene: [f64; 3]| -> (f64, f64, f64) {
        let sc_l = general_pose_scene(lamp_scene, -32.0, 0.0, 0.0);
        let sc_r = general_pose_scene(lamp_scene, 32.0, 0.0, 0.0);
        let diffs: Vec<f64> = wedge
            .points
            .iter()
            .map(|pt| (g_at(&sc_l, pt.x, pt.y).0 - g_at(&sc_r, pt.x, pt.y).0).abs())
            .collect();
        let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
        let lo = diffs.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = diffs.iter().copied().fold(0.0_f64, f64::max);
        (mean, lo, hi)
    };

    writeln!(
        md,
        "### E9.3 3D Parallax Suitability: |gt(hx = -32 mm) - gt(hx = +32 mm)|"
    )
    .unwrap();
    writeln!(md).unwrap();
    writeln!(md, "| Wedge / Lamp | mean |delta gt| | min | max |").unwrap();
    writeln!(md, "|---|---:|---:|---:|").unwrap();
    for (name, wg, lamp) in [
        ("G0 12 o'clock", w12, LAMP_MM),
        ("a* 12 o'clock", w12, diag_lamp_scene),
        ("a* 3 o'clock", w3, diag_lamp_scene),
    ] {
        let (mean, lo, hi) = mean_eye_gt_diff(wg, lamp);
        writeln!(md, "| {} | {:.4} | {:.4} | {:.4} |", name, mean, lo, hi).unwrap();
    }
    writeln!(md).unwrap();

    // E9.4: Flat colour at a* 3 o'clock vs G1 (from E8)
    let diag3_sc_l = general_pose_scene(diag_lamp_scene, -32.0, 0.0, 0.0);
    let diag3_sc_r = general_pose_scene(diag_lamp_scene, 32.0, 0.0, 0.0);
    let diag3_bino_w = solve_wedge(
        w3,
        resp,
        &[diag3_sc_l.clone(), diag3_sc_r.clone()],
        GeomModel::True,
        DEFAULT_EXPOSURE,
        DEFAULT_CHROMA_WEIGHT,
    );

    // Save e9_eyes.png: a* 3 o'clock, 2-view solve, left / centre / right side by side
    let diag3_bino_l = evaluate_wedge(w3, resp, &diag3_bino_w, &diag3_sc_l, DEFAULT_EXPOSURE);
    let diag3_bino_c = evaluate_wedge(w3, resp, &diag3_bino_w, &diag_design_sc, DEFAULT_EXPOSURE);
    let diag3_bino_r = evaluate_wedge(w3, resp, &diag3_bino_w, &diag3_sc_r, DEFAULT_EXPOSURE);
    let e9_img_l = render_achieved(w3, &diag3_bino_l.achieved, DEFAULT_EXPOSURE);
    let e9_img_c = render_achieved(w3, &diag3_bino_c.achieved, DEFAULT_EXPOSURE);
    let e9_img_r = render_achieved(w3, &diag3_bino_r.achieved, DEFAULT_EXPOSURE);
    build_contact_sheet(&[&e9_img_l, &e9_img_c, &e9_img_r], 6)
        .save(out.join("e9_eyes.png"))
        .unwrap();

    writeln!(
        md,
        "### E9.4 Flat Colour at a* 3 o'clock vs G1 (3 o'clock, E8)"
    )
    .unwrap();
    writeln!(md).unwrap();
    writeln!(
        md,
        "| Solve / View | a* med dE | a* p90 dE | a* mean dC | G1 med dE | G1 p90 dE | G1 mean dC |"
    )
    .unwrap();
    writeln!(md, "|---|---:|---:|---:|---:|---:|---:|").unwrap();

    let e9_comp_rows: Vec<(&str, bool, f64, f64, f64)> = vec![
        ("(a) centre @ left (-32 mm)", false, -32.0, 0.0, 0.0),
        ("(a) centre @ right (+32 mm)", false, 32.0, 0.0, 0.0),
        ("(b) 2-view @ left (-32 mm)", true, -32.0, 0.0, 0.0),
        ("(b) 2-view @ centre (0 mm)", true, 0.0, 0.0, 0.0),
        ("(b) 2-view @ right (+32 mm)", true, 32.0, 0.0, 0.0),
        ("(a) centre @ delta -5 deg", false, 0.0, 0.0, -5.0),
        ("(a) centre @ delta -2 deg", false, 0.0, 0.0, -2.0),
        ("(a) centre @ delta +2 deg", false, 0.0, 0.0, 2.0),
        ("(a) centre @ delta +5 deg", false, 0.0, 0.0, 5.0),
        ("(a) centre @ hy -20 mm", false, 0.0, -20.0, 0.0),
        ("(a) centre @ hy +20 mm", false, 0.0, 20.0, 0.0),
    ];

    for &(label, is_bino, hx, hy, delta_deg) in &e9_comp_rows {
        let sc_a = general_pose_scene(diag_lamp_scene, hx, hy, delta_deg);
        let sc_g1 = general_pose_scene(g1_lamp_scene, hx, hy, delta_deg);
        let w_a = if is_bino { &diag3_bino_w } else { &diag3_centre_w };
        let w_g1 = if is_bino { &e8_bino_weights[1] } else { &e8_centre_weights[1] };
        let ev_a = evaluate_wedge(w3, resp, w_a, &sc_a, DEFAULT_EXPOSURE);
        let ev_g1 = evaluate_wedge(w3, resp, w_g1, &sc_g1, DEFAULT_EXPOSURE);
        writeln!(
            md,
            "| {} | {:.2} | {:.2} | {:+.2} | {:.2} | {:.2} | {:+.2} |",
            label,
            ev_a.median_de,
            ev_a.p90_de,
            ev_a.mean_dc,
            ev_g1.median_de,
            ev_g1.p90_de,
            ev_g1.mean_dc
        )
        .unwrap();
    }
    writeln!(md).unwrap();

    // Verify no U+00B7 slipped into summary.md
    assert!(!md.contains('\u{00B7}'), "summary.md must not contain U+00B7");

    let summary_path = out.join("summary.md");
    std::fs::write(&summary_path, &md).expect("write summary.md");

    println!("\n=== COLOR STUDY OUTPUT: {} ===\n{}", out.display(), md);
}
