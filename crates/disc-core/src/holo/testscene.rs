//! Synthetic 4-sphere 3D parallax test scene ("Fixed Sideways Flashlight, 20° tilt, ±80 mm head sway, N=8").
//!
//! Four shaded spheres at different depths (`+25`, `0`, `+12`, `-20` mm) placed in the near
//! (`y < 0`) and far (`y > 0`) bands along `x = 0`, where the circular DVD tracks run
//! left-right so horizontal head motion (and disc roll) steers `g_t` directly while pitch
//! stays in `g_r`. Viewed with a fixed point light (`[250, 0, 400]` mm — phone flashlight
//! propped ~25 cm to the viewer's right and ~40 cm above the disc, aligned in `y`), eye at
//! 350 mm and 20° tilt on the `-y` side, moving horizontally left-right by ±80 mm across 8 views.

use super::design::{self, ViewSpec};
use crate::preset::DiscGeometryPreset;
use image::{Rgba, RgbaImage};
use std::path::Path;

/// Number of views in the default 3D parallax test scene.
pub const DEFAULT_VIEWS: usize = 8;
/// Nominal eye distance from the disc centre (mm).
pub const DEFAULT_DISTANCE_MM: f64 = design::DEFAULT_DISTANCE_MM;
/// Fixed eye elevation tilt from the disc normal (degrees).
pub const DEFAULT_TILT_DEG: f64 = 20.0;
/// Horizontal head-motion half-span (±H, mm).
pub const DEFAULT_HEAD_HALF_SPAN_MM: f64 = design::DEFAULT_HEAD_HALF_SPAN_MM;
/// Fixed lamp position for the default 3D test scene (mm): phone flashlight propped
/// ~25 cm to the viewer's right and ~40 cm above the disc (`y = 0`).
pub const DEFAULT_TEST3D_LAMP_MM: [f64; 3] = design::HELD_FLASHLIGHT_MM;
/// Edge length of each generated full-disc target PNG (pixels).
pub const DEFAULT_TARGET_SIZE: u32 = 512;

/// A bright sphere in 3D space above/below the disc plane (mm, disc centre at origin).
#[derive(Clone, Copy, Debug)]
pub struct Sphere3D {
    pub name: &'static str,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub radius: f64,
}

impl Sphere3D {
    /// Orthographic projection onto the disc plane `z = 0` along the eye direction of `view`.
    pub fn project(&self, view: &ViewSpec) -> (f64, f64) {
        let e = view.eye_pos();
        (self.x - self.z * e[0] / e[2], self.y - self.z * e[1] / e[2])
    }
}

/// Four spheres placed inside the DVD data ring (`r ∈ [24.2, 57.9]` mm) in the near (`y < 0`)
/// and far (`y > 0`) bands along `x = 0`, where tracks run left-right so horizontal head motion
/// modulates tangential `g_t` directly:
/// - `near(+25mm)` projects to `py = -48.5 mm`, `px ∈ [-6.1, +6.1] mm` (outer near band)
/// - `surf(0mm)`   projects to `py = -32.5 mm`, `px = 0.0 mm` (inner near band, static ref)
/// - `mid(+12mm)`  projects to `py = +32.5 mm`, `px ∈ [-2.9, +2.9] mm` (inner far band)
/// - `deep(-20mm)` projects to `py = +48.5 mm`, `px ∈ [-4.9, +4.9] mm` (outer far band, moves opposite)
pub const SCENE_SPHERES: [Sphere3D; 4] = [
    Sphere3D { name: "near(+25mm)", x: 0.0, y: -57.6, z: 25.0, radius: 5.5 },
    Sphere3D { name: "surf(0mm)", x: 0.0, y: -32.5, z: 0.0, radius: 5.5 },
    Sphere3D { name: "mid(+12mm)", x: 0.0, y: 28.1, z: 12.0, radius: 5.5 },
    Sphere3D { name: "deep(-20mm)", x: 0.0, y: 55.8, z: -20.0, radius: 5.5 },
];

/// Evaluate a slice of synthetic 3D spheres brightness in `[0, 1]` at disc point `(x_mm, y_mm)`.
pub fn eval_spheres_3d(spheres: &[Sphere3D], view: &ViewSpec, x_mm: f64, y_mm: f64) -> f64 {
    let mut val = 0.0f64;
    for s in spheres {
        let (px, py) = s.project(view);
        let d2 = (x_mm - px).powi(2) + (y_mm - py).powi(2);
        let r2 = s.radius * s.radius;
        if d2 < r2 {
            let dome = (1.0 - d2 / r2).sqrt();
            val = val.max(0.35 + 0.65 * dome);
        }
    }
    val
}

/// Evaluate the default 4-sphere synthetic 3D scene brightness in `[0, 1]` at `(x_mm, y_mm)`.
pub fn eval_scene_3d(view: &ViewSpec, x_mm: f64, y_mm: f64) -> f64 {
    eval_spheres_3d(&SCENE_SPHERES, view, x_mm, y_mm)
}

/// Render the 4-sphere scene for `view` as a full-disc square RGBA image, matching the
/// picture convention expected by `design::load_target`.
pub fn render_target(view: &ViewSpec, size: u32, outer_radius_mm: f64) -> RgbaImage {
    RgbaImage::from_fn(size, size, |x, y| {
        let u = (x as f64 + 0.5) / size as f64;
        let v = (y as f64 + 0.5) / size as f64;
        let xm = (u - 0.5) * 2.0 * outer_radius_mm;
        let ym = (v - 0.5) * 2.0 * outer_radius_mm;
        let g = (eval_scene_3d(view, xm, ym).powf(1.0 / 2.2) * 255.0).round().clamp(0.0, 255.0) as u8;
        Rgba([g, g, g, 255])
    })
}

/// Build `n` flash-mode views at fixed `tilt_deg` spread evenly across an open azimuth arc
/// of `span_deg` centred on `-90°`.
pub fn azimuth_arc_views(n: usize, tilt_deg: f64, span_deg: f64) -> Vec<ViewSpec> {
    let start = -90.0 - span_deg * 0.5;
    (0..n)
        .map(|i| {
            let dir = if n <= 1 { -90.0 } else { start + span_deg * i as f64 / (n - 1) as f64 };
            ViewSpec { dir_deg: dir, ..ViewSpec::flash("", tilt_deg) }
        })
        .collect()
}

/// Build `n` fixed-lamp views for a head moving horizontally by `±half_span_mm` at
/// elevation tilt `tilt_deg` and distance `distance_mm`, looking along `dir = -90°`.
pub fn head_motion_views(
    n: usize,
    distance_mm: f64,
    tilt_deg: f64,
    half_span_mm: f64,
    light_mm: [f64; 3],
) -> Vec<ViewSpec> {
    ViewSpec::head_motion_views(n, distance_mm, tilt_deg, -90.0, half_span_mm, light_mm)
}

/// Generate `(ViewSpec, RgbaImage)` pairs for `n` fixed-lamp horizontal head-motion views.
/// Each `ViewSpec` has an empty `image_path` until saved to disk.
pub fn generate_head_motion_views(
    n: usize,
    distance_mm: f64,
    tilt_deg: f64,
    half_span_mm: f64,
    light_mm: [f64; 3],
    size: u32,
    outer_radius_mm: f64,
) -> Vec<(ViewSpec, RgbaImage)> {
    head_motion_views(n, distance_mm, tilt_deg, half_span_mm, light_mm)
        .into_iter()
        .map(|v| {
            let img = render_target(&v, size, outer_radius_mm);
            (v, img)
        })
        .collect()
}

/// Write `n` fixed-lamp head-motion target PNGs (`view-<k>.png`) into `dir` and return the
/// matching `ViewSpec` list.
pub fn write_head_motion_views(
    dir: &Path,
    n: usize,
    distance_mm: f64,
    tilt_deg: f64,
    half_span_mm: f64,
    light_mm: [f64; 3],
    size: u32,
    outer_radius_mm: f64,
) -> Result<Vec<ViewSpec>, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut views = Vec::with_capacity(n);
    for (k, (mut view, img)) in generate_head_motion_views(n, distance_mm, tilt_deg, half_span_mm, light_mm, size, outer_radius_mm)
        .into_iter()
        .enumerate()
    {
        let path = dir.join(format!("view-{k}.png"));
        img.save(&path).map_err(|e| format!("无法写入测试图 {}：{e}", path.display()))?;
        view.image_path = path.to_string_lossy().into_owned();
        views.push(view);
    }
    Ok(views)
}

/// Write the default 3D parallax test scene (fixed flashlight at `[250, 0, 400]` mm,
/// 20° tilt, 350 mm distance, ±80 mm horizontal head motion, 8 views) PNGs into `dir`
/// as `view-<k>.png` and return the matching `ViewSpec` list.
pub fn write_test3d_views(dir: &Path, preset: &DiscGeometryPreset) -> Result<Vec<ViewSpec>, String> {
    write_head_motion_views(
        dir,
        DEFAULT_VIEWS,
        DEFAULT_DISTANCE_MM,
        DEFAULT_TILT_DEG,
        DEFAULT_HEAD_HALF_SPAN_MM,
        DEFAULT_TEST3D_LAMP_MM,
        DEFAULT_TARGET_SIZE,
        preset.outer_radius_mm,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holo::design::{self, DesignInput, LightMode};
    use crate::holo::scene;
    use crate::preset::reference_preset;
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    #[test]
    fn test3d_views_match_head_motion_study() {
        let preset = reference_preset();
        let dir = std::env::temp_dir().join(format!("holo-test3d-head-{}", std::process::id()));
        let views = write_test3d_views(&dir, &preset).unwrap();
        assert_eq!(views.len(), 8);
        let expected_y = -DEFAULT_DISTANCE_MM * DEFAULT_TILT_DEG.to_radians().sin();
        let expected_z = DEFAULT_DISTANCE_MM * DEFAULT_TILT_DEG.to_radians().cos();
        for (k, v) in views.iter().enumerate() {
            assert_eq!(v.light_mode, LightMode::Lamp);
            assert_eq!(v.light_mm, Some(DEFAULT_TEST3D_LAMP_MM));
            let eye = v.eye_mm.expect("head-motion view must carry exact eye_mm");
            let expected_x = -DEFAULT_HEAD_HALF_SPAN_MM + 2.0 * DEFAULT_HEAD_HALF_SPAN_MM * k as f64 / 7.0;
            assert!((eye[0] - expected_x).abs() < 1e-12, "view {k} eye_x: {} vs {expected_x}", eye[0]);
            assert!((eye[1] - expected_y).abs() < 1e-12, "view {k} eye_y: {} vs {expected_y}", eye[1]);
            assert!((eye[2] - expected_z).abs() < 1e-12, "view {k} eye_z: {} vs {expected_z}", eye[2]);
            let p = Path::new(&v.image_path);
            assert!(p.ends_with(format!("view-{k}.png")));
            let img = image::open(p).unwrap().to_rgba8();
            assert_eq!(img.dimensions(), (DEFAULT_TARGET_SIZE, DEFAULT_TARGET_SIZE));
            let bright = img.pixels().filter(|px| px[0] > 128 && px[3] == 255).count();
            assert!(bright > 100, "view {k} should contain bright sphere pixels, got {bright}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test3d_spheres_stay_in_data_ring_and_single_sign_gt() {
        let preset = reference_preset();
        let views = head_motion_views(
            DEFAULT_VIEWS,
            DEFAULT_DISTANCE_MM,
            DEFAULT_TILT_DEG,
            DEFAULT_HEAD_HALF_SPAN_MM,
            DEFAULT_TEST3D_LAMP_MM,
        );
        for (k, v) in views.iter().enumerate() {
            let sc = v.scene();
            let pts: Vec<(f64, f64, f64)> = SCENE_SPHERES
                .iter()
                .map(|s| {
                    let (px, py) = s.project(v);
                    (px, py, s.radius)
                })
                .collect();
            for (i, &(px, py, rad)) in pts.iter().enumerate() {
                let r = px.hypot(py);
                assert!(
                    r - rad > preset.inner_radius_mm && r + rad < preset.outer_radius_mm,
                    "view {k} sphere {} protrudes outside data ring: r={r:.2}±{rad:.1} vs [{:.2}, {:.2}]",
                    SCENE_SPHERES[i].name,
                    preset.inner_radius_mm,
                    preset.outer_radius_mm
                );
                let (gt, _) = scene::g_at(&sc, px, py);
                let expected_sign = if py < 0.0 { 1.0 } else { -1.0 };
                assert!(
                    gt * expected_sign > 0.25,
                    "view {k} sphere {} g_t={gt:.3} must stay single-sign and inside codebook band",
                    SCENE_SPHERES[i].name
                );
                for j in (i + 1)..pts.len() {
                    let dist = (px - pts[j].0).hypot(py - pts[j].1);
                    assert!(
                        dist > rad + pts[j].2 + 1.0,
                        "view {k} spheres {} and {} overlap: dist={dist:.2}",
                        SCENE_SPHERES[i].name,
                        SCENE_SPHERES[j].name
                    );
                }
            }
        }
    }

    #[test]
    fn test3d_solves_and_renders() {
        let preset = reference_preset();
        let dir = std::env::temp_dir().join(format!("holo-test3d-solve-{}", std::process::id()));
        let views = write_head_motion_views(
            &dir,
            DEFAULT_VIEWS,
            DEFAULT_DISTANCE_MM,
            DEFAULT_TILT_DEG,
            DEFAULT_HEAD_HALF_SPAN_MM,
            DEFAULT_TEST3D_LAMP_MM,
            128,
            preset.outer_radius_mm,
        )
        .unwrap();
        let shared = design::shared_codebook(preset.channel_bit_length_nm * 1e-3);
        let (_, resp) = &*shared;

        let input = DesignInput { views: views.clone(), chroma_weight: 0.0, exposure: 0.6, grid: 32 };
        let solved = design::solve(&input, resp, &preset).unwrap();
        assert!(solved.error.is_finite() && solved.error >= 0.0);
        assert_eq!(solved.grid, 32);

        let r0 = design::render(&solved, resp, &preset, &views[0], 64, 1.0);
        let r7 = design::render(&solved, resp, &preset, &views[7], 64, 1.0);
        assert_eq!(r0.len(), 64 * 64 * 4);
        assert_eq!(r7.len(), 64 * 64 * 4);
        assert_ne!(r0, r7, "leftmost and rightmost head-motion views must render differently");
        assert!(r0.chunks_exact(4).any(|px| px[3] == 255 && (px[0] > 0 || px[1] > 0 || px[2] > 0)));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test3d_roll_changes_views_more_than_pitch() {
        let preset = reference_preset();
        let dir = std::env::temp_dir().join(format!("holo-test3d-rollpitch-{}", std::process::id()));
        let views = write_head_motion_views(
            &dir,
            DEFAULT_VIEWS,
            DEFAULT_DISTANCE_MM,
            DEFAULT_TILT_DEG,
            DEFAULT_HEAD_HALF_SPAN_MM,
            DEFAULT_TEST3D_LAMP_MM,
            128,
            preset.outer_radius_mm,
        )
        .unwrap();
        let shared = design::shared_codebook(preset.channel_bit_length_nm * 1e-3);
        let (_, resp) = &*shared;

        let solved = design::solve(
            &DesignInput { views: views.clone(), chroma_weight: 0.0, exposure: 0.6, grid: 48 },
            resp,
            &preset,
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        let rot_y = |p: [f64; 3], deg: f64| -> [f64; 3] {
            let a = deg.to_radians();
            [p[0] * a.cos() + p[2] * a.sin(), p[1], -p[0] * a.sin() + p[2] * a.cos()]
        };
        let rot_x = |p: [f64; 3], deg: f64| -> [f64; 3] {
            let a = deg.to_radians();
            [p[0], p[1] * a.cos() - p[2] * a.sin(), p[1] * a.sin() + p[2] * a.cos()]
        };

        let size = 96usize;
        let ro = preset.outer_radius_mm;
        let mut roi = Vec::new();
        for y in 0..size {
            for x in 0..size {
                let xm = ((x as f64 + 0.5) / size as f64 - 0.5) * 2.0 * ro;
                let ym = ((y as f64 + 0.5) / size as f64 - 0.5) * 2.0 * ro;
                if views.iter().any(|v| eval_scene_3d(v, xm, ym) > 0.0) {
                    roi.push(y * size + x);
                }
            }
        }
        let luma_roi = |rgba: &[u8]| -> Vec<f64> {
            roi.iter()
                .map(|&idx| {
                    let o = idx * 4;
                    let lin = |c: u8| (c as f64 / 255.0).powf(2.2);
                    0.2126 * lin(rgba[o]) + 0.7152 * lin(rgba[o + 1]) + 0.0722 * lin(rgba[o + 2])
                })
                .collect()
        };
        let pearson = |a: &[f64], b: &[f64]| -> f64 {
            let n = a.len() as f64;
            let ma = a.iter().sum::<f64>() / n;
            let mb = b.iter().sum::<f64>() / n;
            let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
            for (&x, &y) in a.iter().zip(b) {
                let (dx, dy) = (x - ma, y - mb);
                num += dx * dy;
                da += dx * dx;
                db += dy * dy;
            }
            if da <= 1e-18 || db <= 1e-18 { 0.0 } else { num / (da * db).sqrt() }
        };

        let eye0 = design::head_eye_mm(DEFAULT_DISTANCE_MM, DEFAULT_TILT_DEG, -90.0, 0.0);
        let lamp0 = DEFAULT_TEST3D_LAMP_MM;
        let angles = [-10.0f64, -5.0, 0.0, 5.0, 10.0];
        let roll_frames: Vec<Vec<f64>> = angles
            .iter()
            .map(|&a| {
                let v = ViewSpec::lamp("", rot_y(eye0, a), rot_y(lamp0, a));
                luma_roi(&design::render(&solved, resp, &preset, &v, size, 0.0))
            })
            .collect();
        let pitch_frames: Vec<Vec<f64>> = angles
            .iter()
            .map(|&a| {
                let v = ViewSpec::lamp("", rot_x(eye0, a), rot_x(lamp0, a));
                luma_roi(&design::render(&solved, resp, &preset, &v, size, 0.0))
            })
            .collect();

        let mean_dissim = |frames: &[Vec<f64>]| -> f64 {
            let (mut sum, mut cnt) = (0.0, 0usize);
            for i in 0..frames.len() {
                for j in (i + 1)..frames.len() {
                    sum += 1.0 - pearson(&frames[i], &frames[j]);
                    cnt += 1;
                }
            }
            sum / cnt as f64
        };
        let roll_change = mean_dissim(&roll_frames);
        let pitch_change = mean_dissim(&pitch_frames);
        let roll_5 = 1.0 - pearson(&roll_frames[1], &roll_frames[3]);
        let pitch_5 = 1.0 - pearson(&pitch_frames[1], &pitch_frames[3]);
        let roll_10 = 1.0 - pearson(&roll_frames[0], &roll_frames[4]);
        let pitch_10 = 1.0 - pearson(&pitch_frames[0], &pitch_frames[4]);
        assert!(
            roll_change > 2.5 * pitch_change,
            "disc ROLL ±10° sweep must change views far more than PITCH ±10°: roll={roll_change:.3}, pitch={pitch_change:.3}"
        );
        assert!(
            roll_5 > 3.0 * pitch_5,
            "disc ROLL ±5° must differ far more than PITCH ±5°: roll={roll_5:.3}, pitch={pitch_5:.3}"
        );
        assert!(
            roll_10 > 1.5 * pitch_10,
            "disc ROLL -10° vs +10° must differ more than PITCH -10° vs +10°: roll={roll_10:.3}, pitch={pitch_10:.3}"
        );
    }

    #[test]
    #[ignore]
    fn bench_test3d_build_iso() {
        let preset = reference_preset();
        let bit_um = preset.channel_bit_length_nm * 1e-3;
        let dir = std::env::temp_dir().join(format!("holo-test3d-bench-{}", std::process::id()));
        let t0 = Instant::now();
        let views = write_test3d_views(&dir, &preset).unwrap();
        let dt_targets = t0.elapsed();

        let t1 = Instant::now();
        let shared = design::shared_codebook(bit_um);
        let (cb, resp) = &*shared;
        let dt_cb = t1.elapsed();

        let t2 = Instant::now();
        let input = DesignInput { views, chroma_weight: 0.0, exposure: 0.6, grid: 256 };
        let solved = design::solve(&input, resp, &preset).unwrap();
        let dt_solve = t2.elapsed();

        let iso_path = dir.join("test3d.iso");
        let cancel = AtomicBool::new(false);
        let t3 = Instant::now();
        let summary = design::build_iso(&preset, cb, &solved, None, &iso_path.to_string_lossy(), &cancel, &|_| {});
        let dt_iso = t3.elapsed();

        let _ = std::fs::remove_dir_all(&dir);
        let summary = summary.unwrap();

        println!("targets (8x512 PNG): {:.3}s", dt_targets.as_secs_f64());
        println!("codebook + responses: {:.3}s", dt_cb.as_secs_f64());
        println!("solve (grid=256, 8 views, chroma=0.0): {:.3}s (mse={:.6})", dt_solve.as_secs_f64(), solved.error);
        println!("build_iso ({} sectors): {:.3}s", summary.total_sectors, dt_iso.as_secs_f64());
        println!("option usage shares (0..{} codebook, {} plain):", summary.usage.len() - 1, summary.usage.len() - 1);
        for (i, &u) in summary.usage.iter().enumerate() {
            let label = if i + 1 == summary.usage.len() { "plain".to_string() } else { format!("cb[{i:02}]") };
            println!("  {label}: {:6.2}%", u * 100.0);
        }
    }
}

#[cfg(test)]
mod look {
    use super::*;
    use crate::holo::design::{self, DesignInput};
    use crate::preset::reference_preset;

    /// `HOLO_OUT=dir cargo test -p disc-core --release testscene::look -- --ignored`: the
    /// burned test disc as the model sees it while the viewer's head moves left-right.
    #[test]
    #[ignore]
    fn swaying_past_the_test_disc() {
        let out = std::path::PathBuf::from(std::env::var("HOLO_OUT").unwrap());
        let preset = reference_preset();
        let views = write_test3d_views(&out.join("targets"), &preset).unwrap();
        let shared = design::shared_codebook(preset.channel_bit_length_nm * 1e-3);
        let input = DesignInput { views: views.clone(), chroma_weight: 0.0, exposure: 0.6, grid: 256 };
        let solved = design::solve(&input, &shared.1, &preset).unwrap();
        for k in 0..=8 {
            let h = -DEFAULT_HEAD_HALF_SPAN_MM + 2.0 * DEFAULT_HEAD_HALF_SPAN_MM * k as f64 / 8.0;
            let eye = design::head_eye_mm(DEFAULT_DISTANCE_MM, DEFAULT_TILT_DEG, -90.0, h);
            let v = ViewSpec::lamp("", eye, DEFAULT_TEST3D_LAMP_MM);
            let px = design::render(&solved, &shared.1, &preset, &v, 400, 1.0);
            image::RgbaImage::from_raw(400, 400, px).unwrap().save(out.join(format!("sway-{k}.png"))).unwrap();
        }
    }
}
