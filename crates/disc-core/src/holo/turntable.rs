//! A disc looked at from several positions: turned to each, it shows that position's objects.
//!
//! A position is an ordinary scene in front of one fixed lamp and eye, with the disc turned by
//! its own angle. Tracks run in circles, so depth works best near 12 and 6 o'clock of that
//! position, and those places lie elsewhere on the disc for every other one. Each position is
//! solved by itself; a cell of the disc belongs to one position only. Everything no object
//! covers is one dark sequence (`design::BACKGROUND_BYTE`).

use super::codebook::Codebook;
use super::design::{self, Responses, Solved};
#[cfg(test)]
use super::design::ViewSpec;
use super::ink;
use super::model3d::{self, Mesh, Position, Scene, SceneObject, Vertex};
use super::scene;
use crate::preset::DiscGeometryPreset;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Solved eye positions across the sweep.
pub const VIEWS: usize = 24;
/// Half the width of the head's sweep, mm.
pub const HEAD_HALF_SPAN_MM: f64 = 40.0;
/// The lamp, in the viewer's frame: right, up, towards the viewer.
pub const LAMP_MM: [f64; 3] = [150.0, 0.0, 400.0];
/// Heads outside the window that are asked for evenness only, mm.
const WIDE_MM: [f64; 4] = [60.0, 85.0, 120.0, 170.0];
/// Share of a sector the drive writes itself, which shows as plain data.
const PLAIN_LEAK: f64 = 0.153;
/// A cell is an object's when some view asks for this much light of it.
const LIT: f32 = 0.004;
/// Depth kept either side of an object's own, mm.
const THICKNESS_MM: f64 = 3.0;

type V3 = [f64; 3];

/// How the disc is meant to be lit and looked at, in the viewer's frame (x right, y up, z
/// towards the viewer, origin at the disc centre).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Options {
    pub distance_mm: f64,
    pub chroma_weight: f64,
    pub exposure: f64,
    pub grid: usize,
    /// How much further objects move with the eye than their depth asks for. At 1 the disc is a
    /// plain window, and a few millimetres of depth move too little to notice.
    pub parallax: f64,
    /// Weight of keeping a subject even from outside the window, against the window itself.
    pub wide: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options { distance_mm: design::DEFAULT_DISTANCE_MM, chroma_weight: 1.0, exposure: 0.6, grid: 512, parallax: 2.0, wide: 0.3 }
    }
}

/// Turn a disc-frame point clockwise, as seen, by `t` radians. The disc picture has y down.
pub fn turn(p: V3, t: f64) -> V3 {
    let (s, c) = t.sin_cos();
    [p[0] * c - p[1] * s, p[0] * s + p[1] * c, p[2]]
}

/// Eye and lamp, in the disc frame, for a disc turned so that `angle` is at the top.
pub fn eye_and_lamp(opts: &Options, lamp_mm: V3, head_x_mm: f64, angle: f64) -> (V3, V3) {
    let eye = [head_x_mm, 0.0, opts.distance_mm];
    (turn(eye, angle), turn(model3d::scene_to_disc(lamp_mm), angle))
}

/// As `eye_and_lamp`, the disc tilted left or right by `tilt` radians under a lamp and an eye
/// that stay where they are.
pub fn tilted_eye_and_lamp(opts: &Options, lamp_mm: V3, tilt: f64, angle: f64) -> (V3, V3) {
    let (s, c) = tilt.sin_cos();
    let roll = |p: V3| [p[0] * c - p[2] * s, p[1], p[0] * s + p[2] * c];
    (turn(roll([0.0, 0.0, opts.distance_mm]), angle), turn(roll(model3d::scene_to_disc(lamp_mm)), angle))
}

/// An object's mesh in the disc frame, for a position that has `angle` at the top.
///
/// Its depth is eased into `thickness_mm` either side of where the viewer mostly sees it, and
/// that place is put at the depth the object was given: what is near the disc's own surface is
/// sharp, and what is far from it smears.
pub fn place(mesh: &Mesh, obj: &SceneObject, angle: f64, thickness_mm: f64) -> Result<Mesh, String> {
    let mut m = model3d::place_scene_object(mesh, obj)?;
    settle(&mut m, obj.position_mm[2], thickness_mm);
    let tris = m.tris.into_iter().map(|tri| tri.map(|v| Vertex { pos: turn(v.pos, angle), normal: turn(v.normal, angle), ..v })).collect();
    Ok(Mesh { tris })
}

fn settle(m: &mut Mesh, at_mm: f64, thickness_mm: f64) {
    if thickness_mm <= 0.0 {
        return;
    }
    // The depth half of the surface seen from in front lies before, on a coarse grid.
    let Some((lo, hi)) = m.bounds() else { return };
    const N: usize = 96;
    let size = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-6) / N as f64;
    let mut front = vec![f64::NEG_INFINITY; N * N];
    for tri in &m.tris {
        // A triangle is smaller than a cell in any model worth easing: its corners will do,
        // and its middle for those that are not.
        let mid = [0, 1, 2].map(|c| (tri[0].pos[c] + tri[1].pos[c] + tri[2].pos[c]) / 3.0);
        for p in tri.iter().map(|v| v.pos).chain([mid]) {
            let (x, y) = ((((p[0] - lo[0]) / size) as usize).min(N - 1), (((p[1] - lo[1]) / size) as usize).min(N - 1));
            front[y * N + x] = front[y * N + x].max(p[2]);
        }
    }
    let mut seen: Vec<f64> = front.into_iter().filter(|z| z.is_finite()).collect();
    seen.sort_by(|a, b| a.total_cmp(b));
    let focus = seen[seen.len() / 2];
    for v in m.tris.iter_mut().flatten() {
        let d = v.pos[2] - focus;
        v.pos[2] = at_mm + thickness_mm * (d / thickness_mm).tanh();
    }
}

/// The scene's positions with their visible objects, leaving out empty ones.
fn grouped(scene: &Scene) -> Vec<(Position, Vec<&SceneObject>)> {
    let mut out: Vec<(Position, Vec<&SceneObject>)> = scene.positions.iter().map(|p| (p.clone(), vec![])).collect();
    if out.is_empty() {
        out.push((Position { id: String::new(), name: String::new(), phi_deg: 0.0, crop_deg: 30.0 }, vec![]));
    }
    for o in scene.objects.iter().filter(|o| o.visible) {
        let i = out.iter().position(|(p, _)| p.id == o.position_id).unwrap_or(0);
        out[i].1.push(o);
    }
    out.retain(|(_, o)| !o.is_empty());
    out
}

/// One position's share of the disc: the cells it lights in any view, and their weights.
pub struct Patch {
    cells: Vec<u32>,
    weights: Vec<f32>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Solution {
    #[serde(skip)]
    pub solved: Arc<Solved>,
    /// Positions that cover some of the same disc as another.
    pub overlapping: Vec<String>,
}

fn solve_position(angle: f64, objects: &[&SceneObject], lamp_mm: V3, opts: &Options, resp: &Responses, preset: &DiscGeometryPreset) -> Result<Patch, String> {
    let mesh = objects
        .iter()
        .map(|o| Ok((place(&*model3d::load_mesh_or_builtin(&o.path)?, o, angle, THICKNESS_MM)?, o.style)))
        .collect::<Result<Vec<_>, String>>()?;
    let n = opts.grid;
    let k = resp.options;
    let plain = k - 1;
    let poses: Vec<(V3, V3)> = (0..VIEWS)
        .map(|i| eye_and_lamp(opts, lamp_mm, HEAD_HALF_SPAN_MM * (2.0 * i as f64 / (VIEWS - 1) as f64 - 1.0), angle))
        .collect();
    let drawn: Vec<V3> = (0..VIEWS)
        .map(|i| eye_and_lamp(opts, lamp_mm, opts.parallax * HEAD_HALF_SPAN_MM * (2.0 * i as f64 / (VIEWS - 1) as f64 - 1.0), angle).0)
        .collect();
    let sharp: Vec<Vec<f32>> = drawn
        .par_iter()
        .map(|e| ink::draw(&mesh, *e, angle, n, preset.inner_radius_mm, preset.outer_radius_mm))
        .collect();
    // The disc cannot tell neighbouring views apart: blended, a point far from its surface is
    // one soft image rather than a row of copies.
    const BLEND: [f32; 5] = [0.1, 0.23, 0.34, 0.23, 0.1];
    let target = |v: usize, c: usize| -> f32 {
        let (mut s, mut w) = (0.0, 0.0);
        for (d, wd) in BLEND.iter().enumerate() {
            let i = v as isize + d as isize - 2;
            if i >= 0 && (i as usize) < VIEWS {
                s += wd * sharp[i as usize][c];
                w += wd;
            }
        }
        s / w
    };
    let lit: Vec<u32> = (0..n * n).filter(|&c| sharp.iter().any(|v| v[c] > LIT)).map(|c| c as u32).collect();
    let luma = |p: [f32; 3]| 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64;
    let colour = opts.chroma_weight.max(0.0).sqrt();
    let weights: Vec<f32> = lit
        .par_iter()
        .flat_map_iter(|&c| {
            let c = c as usize;
            let x = (((c % n) as f64 + 0.5) / n as f64 - 0.5) * 2.0 * preset.outer_radius_mm;
            let y = (((c / n) as f64 + 0.5) / n as f64 - 0.5) * 2.0 * preset.outer_radius_mm;
            let (mut g, mut h) = (vec![0.0f64; k * k], vec![0.0f64; k]);
            let mut row = vec![0.0f64; k];
            let mut add = |row: &[f64], b: f64| {
                for p in 0..k {
                    h[p] += row[p] * b;
                    let (a, g) = (row[p], &mut g[p * k + p..(p + 1) * k]);
                    for (g, r) in g.iter_mut().zip(&row[p..]) {
                        *g += a * r;
                    }
                }
            };
            for (v, (eye, lamp)) in poses.iter().enumerate() {
                let (gt, gr) = scene::g_at(&scene::Scene { light_mm: *lamp, camera_mm: *eye, ..Default::default() }, x, y);
                let cols: Vec<[f32; 3]> = (0..k).map(|j| resp.at(j, gt, gr)).collect();
                let t = target(v, c) as f64 * opts.exposure;
                for j in 0..k {
                    row[j] = (1.0 - PLAIN_LEAK) * luma(cols[j]);
                }
                add(&row, (t - PLAIN_LEAK * luma(cols[plain])).max(0.0));
                // Black has no colour to match. Elsewhere each channel is asked to equal the
                // brightness, so that a cell too dim to reach its target is still neutral.
                if colour > 0.0 && t > 1e-6 {
                    let grey = luma(cols[plain]);
                    for ch in 0..3 {
                        for j in 0..k {
                            row[j] = colour * (1.0 - PLAIN_LEAK) * (cols[j][ch] as f64 - luma(cols[j]));
                        }
                        add(&row, -colour * PLAIN_LEAK * (cols[plain][ch] as f64 - grey));
                    }
                }
            }
            if opts.wide > 0.0 {
                let (first, last) = (target(0, c) as f64 * opts.exposure, target(VIEWS - 1, c) as f64 * opts.exposure);
                for (head, t) in WIDE_MM.iter().flat_map(|&h| [(-h, first), (h, last)]) {
                    let (eye, lamp) = eye_and_lamp(opts, lamp_mm, head, angle);
                    let (gt, gr) = scene::g_at(&scene::Scene { light_mm: lamp, camera_mm: eye, ..Default::default() }, x, y);
                    let cols: Vec<[f32; 3]> = (0..k).map(|j| resp.at(j, gt, gr)).collect();
                    // Plain data is even from everywhere: be no brighter or dimmer than it.
                    let t = t.min(luma(cols[plain]));
                    for j in 0..k {
                        row[j] = opts.wide * (1.0 - PLAIN_LEAK) * luma(cols[j]);
                    }
                    add(&row, opts.wide * (t - PLAIN_LEAK * luma(cols[plain])).max(0.0));
                    if colour > 0.0 && t > 1e-6 {
                        for ch in 0..3 {
                            for j in 0..k {
                                row[j] = opts.wide * colour * (1.0 - PLAIN_LEAK) * (cols[j][ch] as f64 - luma(cols[j]));
                            }
                            add(&row, -opts.wide * colour * PLAIN_LEAK * (cols[plain][ch] as f64 - luma(cols[plain])));
                        }
                    }
                }
            }
            for p in 0..k {
                for q in 0..p {
                    g[p * k + q] = g[q * k + p];
                }
            }
            design::solve_simplex_qp(&g, &h, k).into_iter().map(|w| w as f32)
        })
        .collect();
    Ok(Patch { cells: lit, weights })
}

/// Solve every position and lay the patches out on the disc. Positions already solved, and not
/// changed since, are taken from the last few.
pub fn solve(scene: &Scene, opts: &Options, cb: &Codebook, resp: &Responses, preset: &DiscGeometryPreset) -> Result<Solution, String> {
    const KEEP: usize = 16;
    static CACHE: Mutex<Vec<(String, Arc<Patch>)>> = Mutex::new(Vec::new());
    let opts = Options { grid: opts.grid.clamp(64, 1024), ..opts.clone() };
    let lamp = scene.lamp_mm.unwrap_or(LAMP_MM);
    let groups = grouped(scene);
    if groups.is_empty() {
        return Err("场景中没有可见模型".into());
    }
    let (n, k) = (opts.grid, resp.options);
    let bg = cb.entries.iter().position(|e| e.bytes.iter().all(|&b| b == design::BACKGROUND_BYTE)).ok_or("码本里没有背景序列")?;
    let mut patches = Vec::with_capacity(groups.len());
    for (pos, objects) in &groups {
        // Names and ids change nothing that is burned.
        let same: Vec<_> = objects
            .iter()
            .map(|o| {
                let modified = std::fs::metadata(&o.path).and_then(|m| m.modified()).ok();
                (SceneObject { id: String::new(), name: String::new(), position_id: String::new(), ..(*o).clone() }, modified)
            })
            .collect();
        let key = serde_json::to_string(&(pos.phi_deg, &same, lamp, &opts, preset)).map_err(|e| e.to_string())?;
        let held = CACHE.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|c| c.0 == key).map(|c| c.1.clone());
        let patch = match held {
            Some(p) => p,
            None => {
                let p = Arc::new(solve_position(pos.phi_deg.to_radians(), objects, lamp, &opts, resp, preset)?);
                let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
                if cache.len() >= KEEP {
                    cache.remove(0);
                }
                cache.push((key, p.clone()));
                p
            }
        };
        patches.push(patch);
    }

    let mut weights = vec![0.0f32; n * n * k];
    for cell in weights.chunks_exact_mut(k) {
        cell[bg] = 1.0;
    }
    // A cell two positions want goes to the one with the nearer object.
    let nearest = |g: usize, c: usize| -> f64 {
        let x = (((c % n) as f64 + 0.5) / n as f64 - 0.5) * 2.0 * preset.outer_radius_mm;
        let y = (((c / n) as f64 + 0.5) / n as f64 - 0.5) * 2.0 * preset.outer_radius_mm;
        let at = model3d::scene_to_disc(turn([x, y, 0.0], -groups[g].0.phi_deg.to_radians()));
        groups[g].1.iter().map(|o| (o.position_mm[0] - at[0]).hypot(o.position_mm[1] - at[1])).fold(f64::INFINITY, f64::min)
    };
    let mut owner = vec![usize::MAX; n * n];
    let mut clash = vec![false; groups.len()];
    for (i, p) in patches.iter().enumerate() {
        let crop_deg = groups[i].0.crop_deg;
        let crop_rad = crop_deg.to_radians();
        let phi_rad = groups[i].0.phi_deg.to_radians();
        for (j, &c) in p.cells.iter().enumerate() {
            let c = c as usize;
            if crop_deg < 90.0 {
                let x = (((c % n) as f64 + 0.5) / n as f64 - 0.5) * 2.0 * preset.outer_radius_mm;
                let y = (((c / n) as f64 + 0.5) / n as f64 - 0.5) * 2.0 * preset.outer_radius_mm;
                let at = model3d::scene_to_disc(turn([x, y, 0.0], -phi_rad));
                if at[0].abs().atan2(at[1].abs()) > crop_rad {
                    continue;
                }
            }
            if owner[c] != usize::MAX {
                clash[i] = true;
                clash[owner[c]] = true;
                if nearest(owner[c], c) <= nearest(i, c) {
                    continue;
                }
            }
            owner[c] = i;
            weights[c * k..(c + 1) * k].copy_from_slice(&p.weights[j * k..(j + 1) * k]);
        }
    }
    let overlapping = groups.iter().zip(&clash).filter(|(_, &c)| c).map(|(g, _)| g.0.id.clone()).collect();
    Ok(Solution { solved: Arc::new(Solved { grid: n, options: k, weights, error: 0.0 }), overlapping })
}

/// The solved weights for drawing on the graphics card: `[grid u32][options u32]`, then
/// `ceil(options / 4)` layers of `grid * grid` RGBA bytes, one byte per option.
pub fn weight_layers(s: &Solved) -> Vec<u8> {
    let (n, k) = (s.grid, s.options);
    let layers = k.div_ceil(4);
    let mut out = Vec::with_capacity(8 + layers * n * n * 4);
    out.extend_from_slice(&(n as u32).to_le_bytes());
    out.extend_from_slice(&(k as u32).to_le_bytes());
    for l in 0..layers {
        for cell in s.weights.chunks_exact(k) {
            for c in 0..4 {
                out.push(cell.get(l * 4 + c).map_or(0, |w| (w * 255.0).round().clamp(0.0, 255.0) as u8));
            }
        }
    }
    out
}

/// What every option looks like, for drawing on the graphics card: `[options u32][gt u32][gr u32]
/// [groove u32]` and `[g max f32][groove spread f32]`, the responses as RGB floats ordered
/// option, gt, gr, then the groove's rainbow by `|g_r|` at `g_t = 0`.
pub fn response_tables(resp: &Responses, pitch_um: f64) -> Vec<u8> {
    const GROOVE: usize = 512;
    let mut out = Vec::new();
    for v in [resp.options, resp.gt_bins, resp.gr_bins, GROOVE] {
        out.extend_from_slice(&(v as u32).to_le_bytes());
    }
    for v in [design::G_MAX as f32, design::GROOVE_SOURCE as f32] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let mut put = |p: [f32; 3]| p.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
    resp.rgb.iter().flatten().for_each(|p| put(*p));
    (0..GROOVE).for_each(|i| put(design::groove_rainbow(i as f64 / (GROOVE - 1) as f64 * design::G_MAX, pitch_um)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::reference_preset;

    fn star(position: &str, x: f64, y: f64) -> SceneObject {
        SceneObject {
            id: format!("{position}{x},{y}"), path: model3d::BUILTIN_SAMPLE_PATH.into(), name: "s".into(), visible: true,
            position_mm: [x, y, 6.0], rotation_deg: [0.0; 3], scale_mm: 22.0, position_id: position.into(), style: ink::Style::default(),
        }
    }

    fn scene(phis: &[f64], objects: Vec<SceneObject>) -> Scene {
        let positions = phis.iter().enumerate().map(|(i, &phi_deg)| Position { id: i.to_string(), name: String::new(), phi_deg, crop_deg: 30.0 }).collect();
        Scene { version: model3d::SCENE_VERSION, objects, lamp_mm: None, positions }
    }

    fn run(scene: &Scene, grid: usize) -> Solution {
        let shared = design::shared_codebook(0.1333);
        solve(scene, &Options { grid, ..Default::default() }, &shared.0, &shared.1, &reference_preset()).unwrap()
    }

    fn look(scene: &Scene, angle: f64, head: f64) -> Vec<[f32; 3]> {
        let shared = design::shared_codebook(0.1333);
        let (e, l) = eye_and_lamp(&Options::default(), LAMP_MM, head, angle);
        super::super::lightfield::render_float(&run(scene, 128).solved, &shared.1, &reference_preset(), e, l, 128, false)
    }

    /// Mean luminance, and its centre along x, of a box round the top of the disc as the viewer
    /// has it: the disc is turned so that `angle` is at the top.
    fn at_top(px: &[[f32; 3]], angle: f64) -> (f64, f64, f64) {
        let ro = reference_preset().outer_radius_mm;
        let (mut s, mut sx, mut w, mut n) = (0.0, 0.0, 0.0, 0.0);
        for iy in 0..64 {
            for ix in 0..64 {
                let (x, y) = (-14.0 + 28.0 * ix as f64 / 63.0, -54.0 + 26.0 * iy as f64 / 63.0);
                let q = turn([x, y, 0.0], angle);
                let (u, v) = (((q[0] / (2.0 * ro) + 0.5) * 128.0) as usize, ((q[1] / (2.0 * ro) + 0.5) * 128.0) as usize);
                let p = px[v.min(127) * 128 + u.min(127)];
                let l = 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64;
                s += l;
                n += 1.0;
                let lit = (l - 0.12).max(0.0);
                sx += lit * x;
                w += lit;
            }
        }
        (s / n, sx / w.max(1e-9), w)
    }

    #[test]
    fn a_position_looks_the_same_whatever_its_angle_once_turned_to() {
        let top = at_top(&look(&scene(&[0.0], vec![star("0", 0.0, 41.0)]), 0.0, 0.0), 0.0);
        assert!(top.2 > 1.0, "lit at the top: {top:?}");
        for deg in [90.0f64, 137.0, 180.0, 300.0] {
            let m = at_top(&look(&scene(&[deg], vec![star("0", 0.0, 41.0)]), deg.to_radians(), 0.0), deg.to_radians());
            assert!((m.0 / top.0 - 1.0).abs() < 0.08, "{deg}: {m:?} against {top:?}");
            assert!((m.1 - top.1).abs() < 0.8, "{deg}: {m:?} against {top:?}");
        }
    }

    #[test]
    fn tilting_moves_the_eye_across_the_disc_and_keeps_the_lamp_to_the_right_of_it() {
        let o = Options::default();
        let (e, l) = tilted_eye_and_lamp(&o, LAMP_MM, 0.0, 0.0);
        assert_eq!((e, l), eye_and_lamp(&o, LAMP_MM, 0.0, 0.0));
        let (e, l) = tilted_eye_and_lamp(&o, LAMP_MM, 4f64.to_radians(), 0.0);
        assert!(e[0] < -20.0 && l[0] < LAMP_MM[0] && l[0] > e[0], "{e:?} {l:?}");
    }

    #[test]
    fn every_position_shows_its_own_objects_when_turned_to() {
        let s = scene(&[0.0, 120.0, 240.0], ["0", "1", "2"].map(|p| star(p, 0.0, 41.0)).to_vec());
        let seen = [0.0f64, 120.0, 240.0].map(|d| at_top(&look(&s, d.to_radians(), 0.0), d.to_radians()).0);
        assert!(seen.iter().all(|v| (v / seen[0] - 1.0).abs() < 0.08), "{seen:?}");
        // Turned to somewhere between two, the top of the disc is dark.
        let between = at_top(&look(&s, 60f64.to_radians(), 0.0), 60f64.to_radians()).0;
        assert!(between < 0.35 * seen[0], "{between} against {seen:?}");
    }

    #[test]
    fn a_turned_position_is_upright_and_not_mirrored() {
        let v = |pos| Vertex { pos, normal: [0.0, 0.0, 1.0], albedo: 1.0 };
        let m = Mesh { tris: vec![[v([0.0, 1.0, 0.0]), v([1.0, 0.0, 0.0]), v([-1.0, -1.0, 0.0])]] };
        for deg in [0.0f64, 90.0, 200.0] {
            let a = deg.to_radians();
            let p = place(&m, &star("0", 0.0, 41.0), a, 0.0).unwrap();
            let (up, right) = (turn(p.tris[0][0].pos, -a), turn(p.tris[0][1].pos, -a));
            // The picture's y runs down: up is more negative.
            assert!(up[1] < -50.0 && up[0].abs() < 1e-6, "{deg}: {up:?}");
            assert!(right[0] > 5.0 && (right[1] + 41.0).abs() < 1e-6, "{deg}: {right:?}");
        }
    }

    #[test]
    fn depth_is_eased_round_where_the_object_is_put() {
        let o = SceneObject { position_mm: [0.0, 41.0, -2.0], scale_mm: 30.0, ..star("0", 0.0, 41.0) };
        let mesh = model3d::load_mesh_or_builtin(&o.path).unwrap();
        let z = |t: f64| -> Vec<f64> { place(&mesh, &o, 0.0, t).unwrap().tris.iter().flatten().map(|v| v.pos[2]).collect() };
        let (whole, eased) = (z(0.0), z(3.0));
        assert!(whole.iter().any(|v| (v + 2.0).abs() > 8.0), "deep to begin with");
        assert!(eased.iter().all(|v| (v + 2.0).abs() <= 3.0));
        // What was in front still is.
        for (a, b) in whole.iter().zip(&eased) { for (c, d) in whole.iter().zip(&eased) { assert!(a <= c || b >= d); } }
    }

    #[test]
    fn positions_that_want_the_same_disc_are_reported() {
        // Objects of one position hide each other and never clash.
        assert!(run(&scene(&[0.0], vec![star("0", 0.0, 41.0), star("0", 6.0, 41.0)]), 96).overlapping.is_empty());
        assert!(run(&scene(&[0.0, 120.0], vec![star("0", 0.0, 41.0), star("1", 0.0, 41.0)]), 96).overlapping.is_empty());
        assert_eq!(run(&scene(&[0.0, 10.0], vec![star("0", 0.0, 41.0), star("1", 0.0, 41.0)]), 96).overlapping, ["0", "1"]);
    }

    #[test]
    fn a_scene_without_positions_is_one_that_is_not_turned() {
        let old: Scene = serde_json::from_str(r#"{"version":1,"objects":[{"id":"a","path":"builtin:star","name":"s","positionMm":[0,41,6],"rotationDeg":[0,0,0],"scaleMm":22}]}"#).unwrap();
        assert_eq!(old.objects[0].style, ink::Style::default());
        assert_eq!(run(&old, 96).solved.weights, run(&scene(&[0.0], vec![star("0", 0.0, 41.0)]), 96).solved.weights);
    }

    #[test]
    fn position_crop_keeps_only_sectors_and_90_is_uncropped() {
        let shared = design::shared_codebook(0.1333);
        let preset = reference_preset();
        let bg = shared.0.entries.iter().position(|e| e.bytes.iter().all(|&b| b == design::BACKGROUND_BYTE)).unwrap();
        let grid = 96usize;
        let opts = Options { grid, ..Default::default() };
        let k = shared.1.options;
        let is_bg = |cell: &[f32]| cell.iter().enumerate().all(|(i, &w)| if i == bg { w == 1.0 } else { w == 0.0 });

        for phi_deg in [0.0f64, 55.0] {
            // Centred near 33 deg off 12 o'clock so part lies inside ±30° and part outside.
            let obj = SceneObject { scale_mm: 28.0, ..star("0", 22.0, 34.0) };
            let make = |crop_deg: f64| Scene {
                version: model3d::SCENE_VERSION,
                objects: vec![obj.clone()],
                lamp_mm: None,
                positions: vec![Position { id: "0".into(), name: String::new(), phi_deg, crop_deg }],
            };
            let sol30 = solve(&make(30.0), &opts, &shared.0, &shared.1, &preset).unwrap();
            let sol90 = solve(&make(90.0), &opts, &shared.0, &shared.1, &preset).unwrap();

            // 90° equals the raw uncropped patch written over background.
            let raw = solve_position(phi_deg.to_radians(), &[&obj], LAMP_MM, &opts, &shared.1, &preset).unwrap();
            let mut uncropped = vec![0.0f32; grid * grid * k];
            for cell in uncropped.chunks_exact_mut(k) {
                cell[bg] = 1.0;
            }
            for (j, &c) in raw.cells.iter().enumerate() {
                let c = c as usize;
                uncropped[c * k..(c + 1) * k].copy_from_slice(&raw.weights[j * k..(j + 1) * k]);
            }
            assert_eq!(sol90.solved.weights, uncropped, "phi={phi_deg}: crop 90 must equal uncropped");

            let phi_rad = phi_deg.to_radians();
            let crop_rad = 30.0f64.to_radians();
            let (mut inside_lit, mut outside_lit_in_90) = (0usize, 0usize);
            for c in 0..grid * grid {
                let x = (((c % grid) as f64 + 0.5) / grid as f64 - 0.5) * 2.0 * preset.outer_radius_mm;
                let y = (((c / grid) as f64 + 0.5) / grid as f64 - 0.5) * 2.0 * preset.outer_radius_mm;
                let at = model3d::scene_to_disc(turn([x, y, 0.0], -phi_rad));
                let outside = at[0].abs().atan2(at[1].abs()) > crop_rad;
                let cell30 = &sol30.solved.weights[c * k..(c + 1) * k];
                let cell90 = &sol90.solved.weights[c * k..(c + 1) * k];
                if outside {
                    assert!(is_bg(cell30), "phi={phi_deg}: outside cell {c} must stay background");
                    if !is_bg(cell90) {
                        outside_lit_in_90 += 1;
                    }
                } else if !is_bg(cell30) {
                    inside_lit += 1;
                }
            }
            assert!(inside_lit > 0, "phi={phi_deg}: crop 30 should keep cells inside ±30°");
            assert!(outside_lit_in_90 > 0, "phi={phi_deg}: object should extend past ±30° when uncropped");
        }

        // Overlap detection only sees cropped cells.
        let both = |crop_deg: f64| Scene {
            version: model3d::SCENE_VERSION,
            objects: vec![
                SceneObject { scale_mm: 24.0, ..star("0", 29.0, 29.0) },
                SceneObject { scale_mm: 24.0, ..star("1", -29.0, 29.0) },
            ],
            lamp_mm: None,
            positions: vec![
                Position { id: "0".into(), name: String::new(), phi_deg: 0.0, crop_deg },
                Position { id: "1".into(), name: String::new(), phi_deg: 90.0, crop_deg },
            ],
        };
        assert_eq!(solve(&both(90.0), &opts, &shared.0, &shared.1, &preset).unwrap().overlapping, ["0", "1"]);
        assert!(solve(&both(30.0), &opts, &shared.0, &shared.1, &preset).unwrap().overlapping.is_empty());
    }

    /// Three positions, the last of two models, as seen turning the disc.
    #[test]
    #[ignore]
    fn look_positions() {
        let girl = |p: &str, x: f64, y: f64, rot: [f64; 3], size: f64| SceneObject { path: std::env::var("MODEL").unwrap(), rotation_deg: rot, scale_mm: size, position_mm: [x, y, 0.0], ..star(p, x, y) };
        let s = scene(&[0.0, 120.0, 240.0], vec![girl("0", 0.0, 41.0, [0.0; 3], 32.0), star("1", 0.0, 41.0), girl("2", -10.0, 41.0, [0.0, 40.0, 0.0], 28.0), star("2", 10.0, 41.0)]);
        let shared = design::shared_codebook(0.1333);
        let t0 = std::time::Instant::now();
        let solved = run(&s, 384);
        println!("{:?} clash {:?}", t0.elapsed(), solved.overlapping);
        let n = 300usize;
        let mut out = image::RgbaImage::new((n * 6) as u32, n as u32);
        for (i, (deg, head)) in [(0.0f64, -30.0), (0.0, 30.0), (60.0, 0.0), (120.0, 0.0), (240.0, -30.0), (240.0, 30.0)].into_iter().enumerate() {
            let a = deg.to_radians();
            let (e, l) = eye_and_lamp(&Options::default(), LAMP_MM, head, a);
            let px = design::render(&solved.solved, &shared.1, &reference_preset(), &ViewSpec::lamp("", e, l), n, 1.0);
            for j in 0..n * n {
                let q = turn([(j % n) as f64 + 0.5 - n as f64 / 2.0, (j / n) as f64 + 0.5 - n as f64 / 2.0, 0.0], a);
                let (u, v) = ((q[0] + n as f64 / 2.0) as usize, (q[1] + n as f64 / 2.0) as usize);
                let o = (v.min(n - 1) * n + u.min(n - 1)) * 4;
                out.put_pixel((i * n + j % n) as u32, (j / n) as u32, image::Rgba([px[o], px[o + 1], px[o + 2], px[o + 3]]));
            }
        }
        out.save(std::env::var("HOLO_OUT").unwrap()).unwrap();
    }

    /// Where the time goes, for a scene of three positions.
    #[test]
    #[ignore]
    fn time_solve() {
        let path = std::env::var("MODEL").unwrap();
        let _ = model3d::load_mesh_or_builtin(&path).unwrap();
        let shared = design::shared_codebook(0.1333);
        let girl = |p: &str, size: f64| SceneObject { path: path.clone(), scale_mm: size, position_mm: [0.0, 41.0, 0.0], ..star(p, 0.0, 41.0) };
        for grid in [128usize, 192, 256, 384, 512] {
            for n in [1usize, 3] {
                let s = scene(&[0.0, 90.0, 225.0][..n], ["0", "1", "2"][..n].iter().map(|p| girl(p, 32.0 + grid as f64 * 1e-4)).collect());
                let t = std::time::Instant::now();
                let sol = run(&s, grid);
                let solved = t.elapsed();
                let t = std::time::Instant::now();
                let (e, l) = eye_and_lamp(&Options::default(), LAMP_MM, 0.0, 0.0);
                design::render(&sol.solved, &shared.1, &reference_preset(), &ViewSpec::lamp("", e, l), 576, 1.0);
                let most: usize = sol.solved.weights.chunks_exact(sol.solved.options).map(|w| w.iter().filter(|&&v| v > 1e-3).count()).max().unwrap();
                println!("grid {grid} positions {n}: solve {solved:?} render {:?} most weights in a cell {most}", t.elapsed());
            }
        }
    }

    /// How much of the parallax asked for a disc delivers. The shift is found by sliding the
    /// middle picture to match, per row, so uneven colour and background do not pass for motion.
    #[test]
    #[ignore]
    fn measure_parallax() {
        let shared = design::shared_codebook(0.1333);
        let preset = reference_preset();
        let n = 512usize;
        let mm = 2.0 * preset.outer_radius_mm / n as f64;
        for (chroma, gain) in [(0.3, 1.0), (0.0, 1.0), (0.3, 1.5), (0.3, 2.0), (0.1, 2.0), (0.3, 3.0)] {
            let mut line = String::new();
            for z in [-12.0f64, -6.0, 6.0, 12.0] {
                let o = SceneObject { position_mm: [0.0, 41.0, z], ..star("0", 0.0, 41.0) };
                let opts = Options { grid: 384, chroma_weight: chroma, parallax: gain, ..Default::default() };
                let sol = solve(&scene(&[0.0], vec![o.clone()]), &opts, &shared.0, &shared.1, &preset).unwrap();
                let view = |h: f64| -> Vec<f64> {
                    let (e, l) = eye_and_lamp(&opts, LAMP_MM, h, 0.0);
                    super::super::lightfield::render_float(&sol.solved, &shared.1, &preset, e, l, n, false).iter().map(|p| (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]) as f64).collect()
                };
                // What a window would show, drawn sharp from the same place.
                let mesh = [place(&model3d::load_mesh_or_builtin(&o.path).unwrap(), &o, 0.0, 0.0).unwrap()];
                let ideal = |h: f64| -> Vec<f64> {
                    let (e, l) = eye_and_lamp(&opts, LAMP_MM, h, 0.0);
                    model3d::render_scene_light(&mesh, &ViewSpec::lamp("", e, l), 0.0, n, preset.inner_radius_mm, preset.outer_radius_mm).iter().map(|&v| v as f64).collect()
                };
                // Where along x the sharp star, seen from the middle, best matches the picture.
                let mid = ideal(0.0);
                let find = |p: &[f64]| -> (f64, f64) {
                    let (mut best, mut at) = (f64::NEG_INFINITY, 0.0);
                    for d in -60i32..=60 {
                        let (mut ab, mut a, mut b, mut aa, mut bb, mut c) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
                        for y in 20..140usize { for x in 180..332usize {
                            let (q, m) = (p[y * n + x], mid[y * n + (x as i32 - d) as usize]);
                            ab += q * m; a += q; b += m; aa += q * q; bb += m * m; c += 1.0;
                        } }
                        let r = (ab - a * b / c) / ((aa - a * a / c) * (bb - b * b / c)).sqrt().max(1e-9);
                        if r > best { best = r; at = d as f64 * mm; }
                    }
                    (at, best)
                };
                let (lo, hi, il, ih) = (find(&view(-32.0)), find(&view(32.0)), find(&ideal(-32.0)), find(&ideal(32.0)));
                line += &format!("  z{z:+}: {:+.2}/{:+.2} ({:.0}%) match {:.2}", hi.0 - lo.0, ih.0 - il.0, 100.0 * (hi.0 - lo.0) / (ih.0 - il.0), (lo.1 + hi.1) / 2.0);
            }
            println!("chroma {chroma} gain {gain}:{line}");
        }
    }

    /// Writes what the graphics card is given and what `design::render` makes of the same scene.
    #[test]
    #[ignore]
    fn dump_for_the_graphics_card() {
        let out = std::env::var("HOLO_OUT").unwrap();
        let shared = design::shared_codebook(0.1333);
        let preset = reference_preset();
        let s = run(&scene(&[0.0, 120.0], vec![star("0", 0.0, 41.0), star("1", 0.0, 41.0)]), 256);
        std::fs::write(format!("{out}/weights.bin"), weight_layers(&s.solved)).unwrap();
        std::fs::write(format!("{out}/tables.bin"), response_tables(&shared.1, preset.base_pitch_um)).unwrap();
        for (i, (tilt, angle)) in [(0.0f64, 0.0f64), (3.0, 0.0), (-3.0, 120.0)].into_iter().enumerate() {
            let (e, l) = tilted_eye_and_lamp(&Options::default(), LAMP_MM, tilt.to_radians(), angle.to_radians());
            let px = design::render(&s.solved, &shared.1, &preset, &ViewSpec::lamp("", e, l), 512, 1.0);
            std::fs::write(format!("{out}/ref{i}.bin"), px).unwrap();
        }
    }

    /// How white the subject is, by what is asked for and where it is seen from.
    #[test]
    #[ignore]
    fn measure_white() {
        let shared = design::shared_codebook(0.1333);
        let preset = reference_preset();
        let o = star("0", 0.0, 41.0);
        for (parallax, chroma) in [(2.0, 0.0), (2.0, 0.1), (2.0, 0.3), (2.0, 1.0), (2.0, 3.0), (1.0, 1.0)] {
            let opts = Options { grid: 256, parallax, chroma_weight: chroma, ..Default::default() };
            let sol = solve(&scene(&[0.0], vec![o.clone()]), &opts, &shared.0, &shared.1, &preset).unwrap();
            let mut line = String::new();
            for (name, h, lamp) in [("head -30", -30.0, LAMP_MM), ("head 0", 0.0, LAMP_MM), ("head 30", 30.0, LAMP_MM)] {
                let (e, l) = eye_and_lamp(&opts, lamp, h, 0.0);
                let px = super::super::lightfield::render_float(&sol.solved, &shared.1, &preset, e, l, 256, true);
                let mut c = [0.0f64; 3];
                let (mut sat, mut n) = (0.0, 0.0);
                for (i, p) in px.iter().enumerate() {
                    let x = ((i % 256) as f64 + 0.5 - 128.0) / 256.0 * 2.0 * preset.outer_radius_mm;
                    let y = ((i / 256) as f64 + 0.5 - 128.0) / 256.0 * 2.0 * preset.outer_radius_mm;
                    if x.hypot(y + 41.0) < 6.0 {
                        for k in 0..3 { c[k] += p[k] as f64; }
                        let m = p.iter().cloned().fold(0.0f32, f32::max);
                        sat += ((m - p.iter().cloned().fold(9.0f32, f32::min)) / m.max(1e-6)) as f64;
                        n += 1.0;
                    }
                }
                line += &format!("  {name}: rgb {:.2} {:.2} {:.2} sat {:.2}", c[0] / n, c[1] / n, c[2] / n, sat / n);
            }
            println!("parallax {parallax} chroma {chroma}:{line}");
        }
    }

    /// The drawing, and the disc it makes, against Lambert grey, from in and outside the window.
    #[test]
    #[ignore]
    fn look_ink() {
        let shared = design::shared_codebook(0.1333);
        let preset = reference_preset();
        let out_dir = std::env::var("HOLO_OUT").unwrap();
        let (n, crop) = (900usize, 300u32);
        for (mi, path) in std::env::var("MODELS").unwrap().split(':').enumerate() {
            let o = SceneObject { path: path.into(), scale_mm: 34.0, position_mm: [0.0, 41.0, 0.0], ..star("0", 0.0, 41.0) };
            let mut sheet = image::RgbaImage::new(crop * 7, crop * 5);
            let mut put = |px: &dyn Fn(usize) -> [u8; 3], col: u32, row: u32| {
                for y in 0..crop { for x in 0..crop {
                    let c = px(y as usize * n + x as usize + (n - crop as usize) / 2);
                    sheet.put_pixel(col * crop + x, row * crop + y, image::Rgba([c[0], c[1], c[2], 255]));
                } }
            };
            let enc = |v: f64| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8;
            let flat = ink::Style { lines: 0.0, shading: 0.75, texture: 0.0 };
            let _ = flat;
            for (row, (thickness_mm, parallax)) in [(0.0, 2.0), (6.0, 2.0), (3.0, 2.0), (3.0, 3.0), (1.5, 3.0)].into_iter().enumerate() {
                let style = ink::Style::default();
                let mesh = [(place(&model3d::load_mesh_or_builtin(path).unwrap(), &o, 0.0, thickness_mm).unwrap(), style)];
                let opts = Options { parallax, ..Default::default() };
                let drawn = ink::draw(&mesh, [0.0, 0.0, 350.0], 0.0, n, preset.inner_radius_mm, preset.outer_radius_mm);
                put(&|i| [enc(drawn[i] as f64); 3], 0, row as u32);
                let t = std::time::Instant::now();
                let sol = solve(&scene(&[0.0], vec![o.clone()]), &opts, &shared.0, &shared.1, &preset).unwrap();
                println!("{mi} row {row}: {:?}", t.elapsed());
                for (col, h) in [0.0, -35.0, 35.0, -90.0, 90.0].into_iter().enumerate() {
                    let (e, l) = eye_and_lamp(&opts, LAMP_MM, h, 0.0);
                    let px = super::super::lightfield::render_float(&sol.solved, &shared.1, &preset, e, l, n, true);
                    put(&|i| px[i].map(|v| enc(v as f64 * 1.6)), col as u32 + 1, row as u32);
                }
                let a = 9f64.to_radians();
                let e = [350.0 * a.sin(), 0.0, 350.0 * a.cos()];
                let px = super::super::lightfield::render_float(&sol.solved, &shared.1, &preset, e, [e[0] + 12.0, e[1] - 8.0, e[2]], n, true);
                put(&|i| px[i].map(|v| enc(v as f64 * 1.6)), 6, row as u32);
            }
            sheet.save(format!("{out_dir}/ink{mi}.png")).unwrap();
        }
    }
    /// A model that turns as the disc turns: N positions round the disc, each the model turned on
    /// by 360/N, seen across a sweep of disc angles at 1x, full disc, default lamp.
    #[test]
    #[ignore]
    fn look_turntable() {
        let shared = design::shared_codebook(0.1333);
        let preset = reference_preset();
        let out_dir = std::env::var("HOLO_OUT").unwrap();
        let path = std::env::var("MODEL").unwrap();
        let n_pos: usize = std::env::var("N").ok().and_then(|v| v.parse().ok()).unwrap_or(8);
        let scale: f64 = std::env::var("SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(45.0);
        let parallax: f64 = std::env::var("PARALLAX").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
        let phis: Vec<f64> = (0..n_pos).map(|k| k as f64 * 360.0 / n_pos as f64).collect();
        let objects = phis.iter().enumerate().map(|(k, &phi)| SceneObject {
            id: format!("m{k}"), path: path.clone(), name: "m".into(), visible: true,
            position_mm: [0.0, 41.0, 0.0], rotation_deg: [0.0, phi, 0.0], scale_mm: scale, position_id: k.to_string(), style: ink::Style::default(),
        }).collect();
        let opts = Options { grid: 384, parallax, ..Default::default() };
        let t = std::time::Instant::now();
        let sol = solve(&scene(&phis, objects), &opts, &shared.0, &shared.1, &preset).unwrap();
        println!("solve {:?}, overlapping {:?}", t.elapsed(), sol.overlapping);
        let n = 320usize;
        let sweep: Vec<f64> = (0..12).map(|i| i as f64 * 7.5).collect();
        let mut sheet = image::RgbaImage::new(n as u32 * 6, n as u32 * 2);
        let enc = |v: f64| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8;
        for (i, &psi) in sweep.iter().enumerate() {
            let (e, l) = eye_and_lamp(&opts, LAMP_MM, 0.0, psi.to_radians());
            let px = super::super::lightfield::render_float(&sol.solved, &shared.1, &preset, e, l, n, true);
            // Turn the picture back so the viewer's up is up.
            let (c, s) = (psi.to_radians().cos(), psi.to_radians().sin());
            for y in 0..n { for x in 0..n {
                let (dx, dy) = (x as f64 + 0.5 - n as f64 / 2.0, y as f64 + 0.5 - n as f64 / 2.0);
                let (sx, sy) = (c * dx + s * dy + n as f64 / 2.0, -s * dx + c * dy + n as f64 / 2.0);
                let col = if sx >= 0.0 && sy >= 0.0 && (sx as usize) < n && (sy as usize) < n {
                    px[sy as usize * n + sx as usize].map(|v| enc(v as f64))
                } else { [0, 0, 0] };
                sheet.put_pixel((i % 6) as u32 * n as u32 + x as u32, (i / 6) as u32 * n as u32 + y as u32, image::Rgba([col[0], col[1], col[2], 255]));
            } }
        }
        sheet.save(format!("{out_dir}/turntable-{n_pos}.png")).unwrap();
    }
    /// One model as the app would have it, for the graphics card and for `render_float`, to compare.
    #[test]
    #[ignore]
    fn dump_model_for_the_graphics_card() {
        let shared = design::shared_codebook(0.1333);
        let preset = reference_preset();
        let out = std::env::var("HOLO_OUT").unwrap();
        let o = SceneObject { path: std::env::var("MODEL").unwrap(), scale_mm: 45.0, position_mm: [0.0, 41.0, 0.0], ..star("0", 0.0, 41.0) };
        let opts = Options::default();
        let sol = solve(&scene(&[0.0], vec![o]), &opts, &shared.0, &shared.1, &preset).unwrap();
        std::fs::write(format!("{out}/weights.bin"), weight_layers(&sol.solved)).unwrap();
        std::fs::write(format!("{out}/tables.bin"), response_tables(&shared.1, preset.base_pitch_um)).unwrap();
        let (e, l) = eye_and_lamp(&opts, LAMP_MM, 0.0, 0.0);
        let n = 512;
        let px = super::super::lightfield::render_float(&sol.solved, &shared.1, &preset, e, l, n, true);
        let enc = |v: f32| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8;
        let img = image::RgbImage::from_fn(n as u32, n as u32, |x, y| image::Rgb(px[y as usize * n + x as usize].map(enc)));
        img.save(format!("{out}/cpu-1x.png")).unwrap();
        let px = design::render(&sol.solved, &shared.1, &preset, &ViewSpec::lamp("", e, l), n, 1.0);
        let img = image::RgbImage::from_fn(n as u32, n as u32, |x, y| { let i = (y as usize * n + x as usize) * 4; image::Rgb([px[i], px[i + 1], px[i + 2]]) });
        img.save(format!("{out}/design-render.png")).unwrap();
    }
    /// Frames by turn: frame k is the model turned by k * 360/F, seen with the disc turned by the
    /// same, the whole top half of the disc. Solved jointly, then swept in finer steps.
    #[test]
    #[ignore]
    fn look_frames() {
        let shared = design::shared_codebook(0.1333);
        let preset = reference_preset();
        let out = std::env::var("HOLO_OUT").unwrap();
        let path = std::env::var("MODEL").unwrap();
        let frames: usize = std::env::var("F").ok().and_then(|v| v.parse().ok()).unwrap_or(24);
        let span: f64 = std::env::var("SPAN").ok().and_then(|v| v.parse().ok()).unwrap_or(360.0);
        let scale: f64 = std::env::var("SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(60.0);
        let grid = 384;
        let opts = Options::default();
        let lamp: V3 = std::env::var("LAMP").ok().map(|v| { let p: Vec<f64> = v.split(',').map(|x| x.parse().unwrap()).collect(); [p[0], p[1], p[2]] }).unwrap_or(LAMP_MM);
        let mesh = model3d::load_mesh_or_builtin(&path).unwrap();
        let mut views = Vec::new();
        for k in 0..frames {
            let psi = k as f64 * 360.0 / frames as f64;
            let o = SceneObject { path: path.clone(), scale_mm: scale, position_mm: [0.0, 36.0, 0.0], rotation_deg: [0.0, k as f64 * span / frames as f64, 0.0], ..star("0", 0.0, 36.0) };
            let m = [(place(&mesh, &o, psi.to_radians(), 1.0).unwrap(), ink::Style::default())];
            let (e, l) = eye_and_lamp(&opts, lamp, 0.0, psi.to_radians());
            let drawn = ink::draw(&m, e, psi.to_radians(), grid, preset.inner_radius_mm, preset.outer_radius_mm);
            let img = image::GrayImage::from_fn(grid as u32, grid as u32, |x, y| image::Luma([(drawn[y as usize * grid + x as usize].clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8]));
            let f = format!("{out}/frame{k}.png");
            img.save(&f).unwrap();
            views.push(ViewSpec::lamp(f, e, l));
        }
        let t = std::time::Instant::now();
        let solved = design::solve(&design::DesignInput { views, chroma_weight: 1.0, exposure: 0.6, grid }, &shared.1, &preset).unwrap();
        println!("solve {:?}", t.elapsed());
        let n = 300usize;
        let step = 360.0 / frames as f64;
        let sweep: Vec<f64> = (0..12).map(|i| i as f64 * step / 2.0).collect();
        let mut sheet = image::RgbaImage::new(n as u32 * 6, n as u32 * 2);
        let enc = |v: f64| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8;
        for (i, &psi) in sweep.iter().enumerate() {
            let (e, l) = eye_and_lamp(&opts, lamp, 0.0, psi.to_radians());
            let px = super::super::lightfield::render_float(&solved, &shared.1, &preset, e, l, n, true);
            let (c, s) = (psi.to_radians().cos(), psi.to_radians().sin());
            for y in 0..n { for x in 0..n {
                let (dx, dy) = (x as f64 + 0.5 - n as f64 / 2.0, y as f64 + 0.5 - n as f64 / 2.0);
                let (sx, sy) = (c * dx - s * dy + n as f64 / 2.0, s * dx + c * dy + n as f64 / 2.0);
                let col = if sx >= 0.0 && sy >= 0.0 && (sx as usize) < n && (sy as usize) < n { px[sy as usize * n + sx as usize].map(|v| enc(v as f64)) } else { [0, 0, 0] };
                sheet.put_pixel((i % 6) as u32 * n as u32 + x as u32, (i / 6) as u32 * n as u32 + y as u32, image::Rgba([col[0], col[1], col[2], 255]));
            } }
        }
        sheet.save(format!("{out}/frames-{frames}-{}.png", std::env::var("LAMP").unwrap_or_default())).unwrap();
    }
}
