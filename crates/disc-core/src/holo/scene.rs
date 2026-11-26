//! A disc on a table, a small lamp and a camera nearby: what the photo shows.
//!
//! The far-field model in `design` puts light and eye infinitely far away, so the
//! along-track frequency only depends on azimuth. A hand-held phone is 20-50 cm from a
//! 120 mm disc, and the angles change by many degrees across it, which is as much as a
//! sequence's whole blue-to-red spread. Here every surface point gets its own directions.
//!
//! At a point P with unit vectors l (to the lamp) and v (to the eye), light leaves with
//! in-plane wave vector change g = (l + v) projected onto the disc, in units of 1/lambda.
//! Split into the track direction (g_t) and the radial one (g_r):
//!
//! - marks: power S(g_t / lambda) from the ring's spectrum, times a radial envelope
//!   exp(-(g_r / lambda)^2 / 2 sigma_r^2). Tracks add incoherently, so radially each mark
//!   row scatters like one narrow strip; sigma_r is set by the mark width and is fitted.
//! - groove: orders m = +-1 at g_r = m lambda / pitch, g_t = 0 (the plain disc rainbow).
//! - mirror: g = 0, the lamp's own reflection.
//!
//! Every term is blurred by the lamp's angular size.

use super::optics::{self, Spectrum, NM_COUNT, NM_FIRST, NM_STEP};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// Lamp, camera and how to expose the picture. Millimetres, disc centre at the origin, z up.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Scene {
    pub light_mm: [f64; 3],
    pub camera_mm: [f64; 3],
    /// Point the camera is aimed at (on or near the disc).
    pub aim_mm: [f64; 3],
    /// Field of view across the picture's width, degrees.
    pub hfov_deg: f64,
    /// Radial envelope of mark scattering, per µm (1 sigma).
    pub sigma_r: f64,
    /// Lamp's angular size as a blur on g (unitless, 1 sigma).
    pub source: f64,
    pub marks_gain: f64,
    pub groove_gain: f64,
    pub mirror_gain: f64,
    pub exposure: f64,
}

impl Default for Scene {
    fn default() -> Self {
        Scene {
            light_mm: [0.0, 0.0, 1000.0],
            camera_mm: [0.0, -200.0, 200.0],
            aim_mm: [0.0; 3],
            hfov_deg: 53.0,
            sigma_r: 1.0,
            source: 0.01,
            marks_gain: 1.0,
            groove_gain: 1.0,
            mirror_gain: 1.0,
            exposure: 1.0,
        }
    }
}

/// An annulus of the disc filled with one kind of marks.
pub struct Zone {
    pub r_in_mm: f64,
    pub r_out_mm: f64,
    pub spec: Spectrum,
}

/// The disc being photographed.
pub struct Disc {
    /// Data area: grooves (and marks) exist only between these radii.
    pub r_data_mm: (f64, f64),
    pub pitch_um: f64,
    pub zones: Vec<Zone>,
    /// Marks everywhere else in the data area.
    pub background: Spectrum,
}

type V3 = [f64; 3];
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn unit(a: V3) -> V3 {
    let n = dot(a, a).sqrt();
    [a[0] / n, a[1] / n, a[2] / n]
}

/// (g_t, g_r) at disc point (x, y), mm: the in-plane change of direction, along the
/// track and along the radius.
pub fn g_at(scene: &Scene, x: f64, y: f64) -> (f64, f64) {
    let p = [x, y, 0.0];
    let l = unit(sub(scene.light_mm, p));
    let v = unit(sub(scene.camera_mm, p));
    let g = [l[0] + v[0], l[1] + v[1]];
    let r = x.hypot(y);
    let (c, s) = (x / r, y / r);
    (-g[0] * s + g[1] * c, g[0] * c + g[1] * s)
}

/// Linear RGB reaching the eye from disc point (x, y), mm.
pub fn shade(scene: &Scene, disc: &Disc, cmf: &[[f64; 3]], x: f64, y: f64) -> [f64; 3] {
    let r = x.hypot(y);
    if r < disc.r_data_mm.0 || r > disc.r_data_mm.1 {
        return [0.0; 3];
    }
    let (gt, gr) = g_at(scene, x, y);
    let spec = disc.zones.iter().find(|z| (z.r_in_mm..z.r_out_mm).contains(&r)).map_or(&disc.background, |z| &z.spec);
    let s2 = 2.0 * scene.source * scene.source;
    let mirror = scene.mirror_gain * (-(gt * gt + gr * gr) / s2).exp();
    let mut out = [mirror; 3];
    for (i, w) in cmf.iter().enumerate().take(NM_COUNT) {
        let lam = (NM_FIRST + i as f64 * NM_STEP) * 1e-3;
        let fr = gr / lam;
        let marks = scene.marks_gain * spec.at(gt / lam) as f64 * (-fr * fr / (2.0 * scene.sigma_r * scene.sigma_r)).exp();
        let d = gr.abs() - lam / disc.pitch_um;
        let groove = scene.groove_gain * (-(gt * gt + d * d) / s2).exp();
        let p = marks + groove;
        for k in 0..3 {
            out[k] += p * w[k];
        }
    }
    let rgb = optics::xyz_to_linear_srgb(out);
    [rgb[0] * scene.exposure, rgb[1] * scene.exposure, rgb[2] * scene.exposure]
}

fn encode(l: f64) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let s = if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round() as u8
}

/// Camera basis: forward, right, up, and the half-width of the image plane at unit depth.
fn camera(scene: &Scene) -> (V3, V3, V3, f64) {
    let fwd = unit(sub(scene.aim_mm, scene.camera_mm));
    let right = unit(cross(fwd, [0.0, 0.0, 1.0]));
    let up = cross(right, fwd);
    (fwd, right, up, (scene.hfov_deg.to_radians() / 2.0).tan())
}

/// Where disc point (x, y) lands in a `w` x `h` picture, pixels from the top left.
pub fn project(scene: &Scene, w: usize, h: usize, x: f64, y: f64) -> Option<(f64, f64)> {
    let (fwd, right, up, half) = camera(scene);
    let d = sub([x, y, 0.0], scene.camera_mm);
    let z = dot(d, fwd);
    if z <= 0.0 {
        return None;
    }
    let (sx, sy) = (dot(d, right) / z / half, dot(d, up) / z / half * w as f64 / h as f64);
    Some(((sx + 1.0) / 2.0 * w as f64, (1.0 - sy) / 2.0 * h as f64))
}

/// Photo of the disc through a pinhole camera, `w` x `h`, sRGB RGBA. Rays that miss the
/// data area are left transparent.
pub fn render(scene: &Scene, disc: &Disc, w: usize, h: usize) -> Vec<u8> {
    let cmf = optics::cmf_table();
    let (fwd, right, up, half) = camera(scene);
    let rows: Vec<Vec<u8>> = (0..h)
        .into_par_iter()
        .map(|j| {
            let mut row = Vec::with_capacity(w * 4);
            for i in 0..w {
                let sx = (2.0 * (i as f64 + 0.5) / w as f64 - 1.0) * half;
                let sy = (1.0 - 2.0 * (j as f64 + 0.5) / h as f64) * half * h as f64 / w as f64;
                let ray = [fwd[0] + sx * right[0] + sy * up[0], fwd[1] + sx * right[1] + sy * up[1], fwd[2] + sx * right[2] + sy * up[2]];
                if ray[2] >= 0.0 {
                    row.extend([0, 0, 0, 0]);
                    continue;
                }
                let t = -scene.camera_mm[2] / ray[2];
                let (x, y) = (scene.camera_mm[0] + t * ray[0], scene.camera_mm[1] + t * ray[1]);
                let r = x.hypot(y);
                if r < disc.r_data_mm.0 || r > disc.r_data_mm.1 {
                    row.extend([0, 0, 0, 0]);
                    continue;
                }
                let c = shade(scene, disc, &cmf, x, y);
                row.extend([encode(c[0]), encode(c[1]), encode(c[2]), 255]);
            }
            row
        })
        .collect();
    rows.concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_disc() -> Disc {
        // Flat, except near zero where no run has any power (see `optics::F_MIN`).
        let df = optics::F_MAX / optics::BINS as f64;
        let spec = Spectrum { df, s: (0..=optics::BINS).map(|k| if k as f64 * df < optics::F_MIN { 0.0 } else { 1.0 }).collect() };
        Disc { r_data_mm: (22.0, 58.0), pitch_um: 0.74, zones: vec![], background: spec }
    }

    /// Far away, the scene must reduce to the far-field rule of `design`:
    /// g_t = (sin eye + sin light) |sin(dir - track angle)|.
    #[test]
    fn far_field_limit_matches_design() {
        let (a, d) = (35f64.to_radians(), 20f64.to_radians());
        let far = 1e9;
        let cam = [far * a.sin() * d.cos(), far * a.sin() * d.sin(), far * a.cos()];
        let scene = Scene { light_mm: [0.0, 0.0, far], camera_mm: cam, ..Default::default() };
        for t in [0.0f64, 0.7, 1.9, 3.3, 5.0] {
            let (gt, _) = g_at(&scene, 40.0 * t.cos(), 40.0 * t.sin());
            assert!((gt.abs() - a.sin() * (d - t).sin().abs()).abs() < 1e-6, "t {t}: {gt}");
        }
    }

    /// With the lamp straight above and the camera far away on -y, the marks on the
    /// left and right (tracks along the view plane) must outshine those at top and bottom
    /// (tracks across it), and the groove rainbow must sit at top and bottom.
    #[test]
    fn marks_left_right_groove_top_bottom() {
        let scene = Scene { light_mm: [0.0, 0.0, 1e6], camera_mm: [0.0, -0.5e6, 0.5e6], groove_gain: 0.0, mirror_gain: 0.0, ..Default::default() };
        let disc = flat_disc();
        let cmf = optics::cmf_table();
        let y = |c: [f64; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let side = y(shade(&scene, &disc, &cmf, 40.0, 0.0));
        let top = y(shade(&scene, &disc, &cmf, 0.0, 40.0));
        assert!(side > 10.0 * top.max(1e-9), "side {side} top {top}");
    }
}

#[cfg(test)]
mod look {
    use super::*;
    use crate::holo::{design, efm, swatch};

    fn naive_spectrum(bytes: &[u8], bit_um: f64) -> Spectrum {
        // The run repeated through main data without regard to frames: inside a frame it
        // starts at some other offset. Take one rotation, encoded from the SYNC state.
        let n = 2 * 91;
        let run: Vec<u8> = (0..n).map(|i| bytes[(i + 37) % bytes.len()]).collect();
        let e = efm::encode_after_sync(&run);
        optics::spectrum(&efm::transitions(&e.codes), n as u32 * 16, bit_um, 0.0)
    }

    /// `SWATCH=layout.json HOLO_OUT=out.png SCENE='{json}' W=1080 H=1440 cargo test -p
    /// disc-core --release scene::look -- --ignored`: the calibration disc as photographed.
    #[test]
    #[ignore]
    fn render_swatch() {
        let lay: swatch::SwatchLayout = serde_json::from_slice(&std::fs::read(std::env::var("SWATCH").unwrap()).unwrap()).unwrap();
        let scene: Scene = serde_json::from_str(&std::env::var("SCENE").unwrap()).unwrap();
        let w: usize = std::env::var("W").map_or(1080, |s| s.parse().unwrap());
        let h: usize = std::env::var("H").map_or(1440, |s| s.parse().unwrap());
        let bit_um = lay.codebook.bit_um;
        let blur = scene.source * 0.5;
        // ONLY=k keeps ring k alone, the rest plain data: which ring makes which feature.
        let only: Option<usize> = std::env::var("ONLY").ok().and_then(|s| s.parse().ok());
        let zones = lay
            .rings
            .iter()
            .enumerate()
            .filter(|(k, _)| only.is_none_or(|o| o == *k))
            .map(|(_, r)| {
                let spec = match &r.fill {
                    swatch::Fill::Constant { byte } => naive_spectrum(&[*byte], bit_um),
                    swatch::Fill::Naive { bytes } => naive_spectrum(bytes, bit_um),
                    swatch::Fill::Aligned { bytes } => design::entry_spectrum(bytes, bit_um),
                };
                Zone { r_in_mm: r.r_in_mm, r_out_mm: r.r_out_mm, spec: spec.blurred(blur) }
            })
            .collect();
        let disc = Disc { r_data_mm: (22.0, 58.5), pitch_um: 0.74, zones, background: design::plain_spectrum(bit_um).blurred(blur) };
        let out = std::env::var("HOLO_OUT").unwrap();
        let px = render(&scene, &disc, w, h);
        image::RgbaImage::from_raw(w as u32, h as u32, px).unwrap().save(&out).unwrap();
        // Disc unrolled: azimuth across (0 = +x, counter-clockwise), radius down from the
        // inner edge; the photo (if given) on top, the model below, ring edges marked.
        let Ok(photo) = std::env::var("PHOTO") else { return };
        let photo = image::open(photo).unwrap().to_rgb8();
        let (pw, ph) = (photo.width() as usize, photo.height() as usize);
        let (cols, r0, r1, step) = (720u32, 22.0, 58.5, 0.1);
        let rows = ((r1 - r0) / step) as u32;
        let cmf = optics::cmf_table();
        let edge = |r: f64| lay.rings.iter().any(|z| (r - z.r_in_mm).abs() < step / 2.0 || (r - z.r_out_mm).abs() < step / 2.0);
        let un = image::RgbImage::from_fn(cols, rows * 2 + 4, |c, k| {
            let th = c as f64 / cols as f64 * std::f64::consts::TAU;
            let (top, kk) = if k < rows { (true, k) } else if k >= rows + 4 { (false, k - rows - 4) } else { return image::Rgb([255, 255, 255]) };
            let r = r0 + kk as f64 * step;
            if edge(r) && c % 8 < 2 {
                return image::Rgb([255, 255, 255]);
            }
            let (x, y) = (r * th.cos(), r * th.sin());
            if top {
                match project(&scene, pw, ph, x, y) {
                    Some((u, v)) if u >= 0.0 && v >= 0.0 && (u as usize) < pw && (v as usize) < ph => *photo.get_pixel(u as u32, v as u32),
                    _ => image::Rgb([0, 0, 0]),
                }
            } else {
                let c = shade(&scene, &disc, &cmf, x, y);
                image::Rgb([encode(c[0]), encode(c[1]), encode(c[2])])
            }
        });
        un.save(out.replace(".png", "-unrolled.png")).unwrap();
    }
}

