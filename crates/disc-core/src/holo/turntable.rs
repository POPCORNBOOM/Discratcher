//! A disc looked at from several positions: turned to each, it shows that position's objects.
//!
//! A position is an ordinary scene in front of one fixed lamp and eye, with the disc turned by
//! its own angle. Tracks run in circles, so depth works best near 12 and 6 o'clock of that
//! position, and those places lie elsewhere on the disc for every other one. Each position is
//! solved by itself; a cell of the disc belongs to one position only. Everything no object
//! covers is one dark sequence (`design::BACKGROUND_BYTE`).

use super::codebook::Codebook;
use super::design::{self, Responses, Solved};
use super::flat;
#[cfg(test)]
use super::design::ViewSpec;
use super::ink;
use super::model3d::{self, Mesh, Position, Scene, SceneImage, SceneObject, Vertex};
use super::scene;
use crate::preset::DiscGeometryPreset;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Solved eye positions across the sweep (real parallax mode).
pub const VIEWS: usize = 24;
/// Solved disc-tilt poses across the sweep (transform animation mode).
pub const TRANSFORM_VIEWS: usize = 40;
/// The widest tilt either way allowed, degrees.
pub const MAX_TILT_DEG: f64 = 30.0;
/// Tilt either way used when the position does not choose one, degrees.
pub const DEFAULT_TRANSFORM_TILT_DEG: f64 = 3.5;
/// Fixed target exposure for transform animation (Winner A).
pub const TRANSFORM_EXPOSURE: f64 = 0.6;
/// Half the width of the head's sweep, mm.
pub const HEAD_HALF_SPAN_MM: f64 = 40.0;
/// Half the distance between the two eyes, mm.
pub const EYE_HALF_MM: f64 = 32.0;
/// Viewing poses solved across `[-EYE_HALF_MM, +EYE_HALF_MM]` for colour pictures, including `0`.
const COLOUR_VIEWS: usize = 5;
/// Colour weight for colour pictures, holding greys neutral even past local white headroom.
pub const PICTURE_CHROMA: f64 = 9.0;
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
    /// Weight of keeping a subject even from outside the window, against the window itself.
    pub wide: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options { distance_mm: design::DEFAULT_DISTANCE_MM, chroma_weight: 1.0, exposure: 0.6, grid: 512, wide: 0.3 }
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

/// Number of solver views for a position's calculation mode.
pub fn views_for_mode(mode: model3d::Mode) -> usize {
    match mode {
        model3d::Mode::Real => VIEWS,
        model3d::Mode::Transform => TRANSFORM_VIEWS,
    }
}

/// The disc's tilt, degrees, at animation parameter `t` in `[0, 1]`: from `-tilt_deg` to `+tilt_deg`.
pub fn transform_tilt_at(tilt_deg: f64, t: f64) -> f64 {
    tilt_deg * (2.0 * t - 1.0)
}

/// Eye and lamp in the disc frame for a transform-mode position at animation parameter `t`.
pub fn transform_pose(opts: &Options, lamp_mm: V3, tilt_deg: f64, t: f64, angle: f64) -> (V3, V3) {
    tilted_eye_and_lamp(opts, lamp_mm, transform_tilt_at(tilt_deg, t).to_radians(), angle)
}

fn transform_poses(opts: &Options, lamp_mm: V3, tilt_deg: f64, angle: f64) -> Vec<(V3, V3)> {
    let denom = (TRANSFORM_VIEWS - 1).max(1) as f64;
    (0..TRANSFORM_VIEWS).map(|i| transform_pose(opts, lamp_mm, tilt_deg, i as f64 / denom, angle)).collect()
}

/// A model's tilt either way in transform mode, degrees: its own, or the default.
pub fn tilt_of(o: &SceneObject) -> f64 {
    if o.tilt_deg > 0.0 {
        o.tilt_deg.min(MAX_TILT_DEG)
    } else {
        DEFAULT_TRANSFORM_TILT_DEG
    }
}

/// How hard a mode holds views white when the position does not say: real parallax as the
/// scene's options ask, transform animation not at all.
pub fn default_neutral(mode: model3d::Mode, opts: &Options) -> f64 {
    match mode {
        model3d::Mode::Real => opts.chroma_weight,
        model3d::Mode::Transform => 0.0,
    }
}

/// The options a position's models of `mode` are solved with: its own neutral colour, and for
/// transform animation no evenness outside the sweep and a fixed exposure.
fn position_options(pos: &Position, mode: model3d::Mode, opts: &Options) -> Options {
    let chroma_weight = pos.neutral.unwrap_or_else(|| default_neutral(mode, opts)).max(0.0);
    match mode {
        model3d::Mode::Real => Options { chroma_weight, ..opts.clone() },
        model3d::Mode::Transform => Options { chroma_weight, wide: 0.0, exposure: TRANSFORM_EXPOSURE, ..opts.clone() },
    }
}

/// Pick the codebook option (or plain data last) with the lowest peak rendered luminance over
/// `poses` across `crop`'s kept region on the written ring.
fn transform_background(crop: &flat::PositionCrop<'_>, poses: &[(V3, V3)], resp: &Responses, preset: &DiscGeometryPreset) -> (usize, f64) {
    let k = resp.options;
    let plain = k - 1;
    let luma = |p: [f32; 3]| 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64;
    let sample_grid = |grid: usize| -> Vec<V3> {
        let mut pts = Vec::new();
        for iy in 0..grid {
            let y = ((iy as f64 + 0.5) / grid as f64 - 0.5) * 2.0 * preset.outer_radius_mm;
            for ix in 0..grid {
                let x = ((ix as f64 + 0.5) / grid as f64 - 0.5) * 2.0 * preset.outer_radius_mm;
                if flat::on_ring(x, y, preset) && crop.kept(x, y) {
                    pts.push([x, y, 0.0]);
                }
            }
        }
        pts
    };
    let mut pts = sample_grid(28);
    if pts.is_empty() {
        pts = sample_grid(64);
    }
    if pts.is_empty() {
        return (0, 0.0);
    }

    let mut max_y = vec![0.0f64; k];
    for &(eye, lamp) in poses {
        let sc = scene::Scene { light_mm: lamp, camera_mm: eye, ..Default::default() };
        for &p in &pts {
            let (gt, gr) = scene::g_at(&sc, p[0], p[1]);
            let plain_y = luma(resp.at(plain, gt, gr));
            for j in 0..k {
                let y = (1.0 - PLAIN_LEAK) * luma(resp.at(j, gt, gr)) + PLAIN_LEAK * plain_y;
                if y > max_y[j] {
                    max_y[j] = y;
                }
            }
        }
    }

    let mut best = 0usize;
    let mut best_y = f64::INFINITY;
    for (j, &y) in max_y.iter().enumerate() {
        if y < best_y {
            best_y = y;
            best = j;
        }
    }
    (best, best_y)
}

fn place_normalized_with(norm: &Mesh, obj: &SceneObject, transform: Option<&[f64; 16]>, angle: f64, thickness_mm: f64) -> Mesh {
    let mut m = model3d::place_normalized_with(norm, obj, transform);
    let at_z = obj.position_mm[2] + transform.map_or(0.0, |t| t[11]);
    settle(&mut m, at_z, thickness_mm);
    let tris = m.tris.into_iter().map(|tri| tri.map(|v| Vertex { pos: turn(v.pos, angle), normal: turn(v.normal, angle), ..v })).collect();
    Mesh { tris }
}

/// An object's mesh in the disc frame, with an optional scene-space transform `T` applied about
/// the object's position before thickness easing (`settle`).
pub fn place_with(mesh: &Mesh, obj: &SceneObject, transform: Option<&[f64; 16]>, angle: f64, thickness_mm: f64) -> Result<Mesh, String> {
    let norm = model3d::normalize_mesh(mesh)?;
    Ok(place_normalized_with(&norm, obj, transform, angle, thickness_mm))
}

/// An object's mesh in the disc frame, for a position that has `angle` at the top.
///
/// Its depth is eased into `thickness_mm` either side of where the viewer mostly sees it, and
/// that place is put at the depth the object was given: what is near the disc's own surface is
/// sharp, and what is far from it smears.
pub fn place(mesh: &Mesh, obj: &SceneObject, angle: f64, thickness_mm: f64) -> Result<Mesh, String> {
    place_with(mesh, obj, None, angle, thickness_mm)
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

/// A model or a picture in a position's list.
#[derive(Clone, Copy)]
pub(crate) enum Item<'a> {
    Model(&'a SceneObject),
    Picture(&'a SceneImage),
}

/// The scene's positions, or one, not turned, when it has none.
pub(crate) fn positions(scene: &Scene) -> Vec<Position> {
    if !scene.positions.is_empty() {
        return scene.positions.clone();
    }
    vec![Position { id: String::new(), name: String::new(), phi_deg: 0.0, crop_deg: 30.0, crop_side: Default::default(), crop_offset_deg: 0.0, crops: None, neutral: None, order: vec![] }]
}

/// Position `at`'s models and pictures, top first: as its `order` lists them, then any it leaves
/// out. Those whose position is gone belong to the first.
pub(crate) fn items<'a>(scene: &'a Scene, positions: &[Position], at: usize) -> Vec<Item<'a>> {
    let pos = &positions[at];
    let mine = |id: &str| id == pos.id || (at == 0 && !positions.iter().any(|p| p.id == id));
    let models: Vec<&SceneObject> = scene.objects.iter().filter(|o| mine(&o.position_id)).collect();
    let pictures: Vec<&SceneImage> = scene.images.iter().filter(|i| mine(&i.position_id)).collect();
    let mut out: Vec<Item> = vec![];
    let id_of = |it: &Item| match it {
        Item::Model(o) => o.id.clone(),
        Item::Picture(p) => p.id.clone(),
    };
    let listed = pos.order.iter().filter_map(|id| {
        models.iter().find(|o| &o.id == id).map(|o| Item::Model(o)).or_else(|| pictures.iter().find(|p| &p.id == id).map(|p| Item::Picture(p)))
    });
    let rest = models.iter().map(|o| Item::Model(o)).chain(pictures.iter().map(|p| Item::Picture(p)));
    for it in listed.chain(rest) {
        if !out.iter().any(|x| id_of(x) == id_of(&it)) {
            out.push(it);
        }
    }
    out
}

/// The parameter that sets a model's sweep: its parallax boost, or its tilt.
fn shape_of(o: &SceneObject) -> f64 {
    match o.mode {
        model3d::Mode::Real => o.parallax,
        model3d::Mode::Transform => tilt_of(o),
    }
}

/// A position's visible models that are solved together: one mode, one parallax or tilt. Within
/// it the list order decides, view by view, which is in front.
struct Group<'a> {
    at: usize,
    mode: model3d::Mode,
    shape: f64,
    /// Top first.
    objects: Vec<&'a SceneObject>,
    /// Its top member's place in the position's list.
    rank: usize,
}

fn groups<'a>(scene: &'a Scene, positions: &[Position]) -> Vec<Group<'a>> {
    let mut out: Vec<Group> = vec![];
    for at in 0..positions.len() {
        for (rank, it) in items(scene, positions, at).into_iter().enumerate() {
            let Item::Model(o) = it else { continue };
            if !o.visible {
                continue;
            }
            let shape = shape_of(o);
            match out.iter_mut().find(|g| g.at == at && g.mode == o.mode && g.shape == shape) {
                Some(g) => g.objects.push(o),
                None => out.push(Group { at, mode: o.mode, shape, objects: vec![o], rank }),
            }
        }
    }
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
    /// The disc the pictures take.
    #[serde(skip)]
    pub flat: Arc<flat::FlatOwners>,
}

/// What each view of `objects` (top first) should show: in `mode`, with `shape` its parallax boost.
#[allow(clippy::too_many_arguments)]
fn position_targets(pos: &Position, mode: model3d::Mode, shape: f64, objects: &[&SceneObject], lamp_mm: V3, opts: &Options, preset: &DiscGeometryPreset) -> Result<Vec<Vec<f32>>, String> {
    position_targets_n(pos, mode, shape, objects, lamp_mm, opts, preset, views_for_mode(mode))
}

#[allow(clippy::too_many_arguments)]
fn position_targets_n(pos: &Position, mode: model3d::Mode, shape: f64, objects: &[&SceneObject], lamp_mm: V3, opts: &Options, preset: &DiscGeometryPreset, views: usize) -> Result<Vec<Vec<f32>>, String> {
    let angle = pos.phi_deg.to_radians();
    let n = opts.grid;
    let denom = (views - 1).max(1) as f64;
    // Load and normalize each mesh once, even in transform mode where placement varies per view.
    let normalized: Vec<Mesh> = objects
        .iter()
        .map(|o| model3d::normalize_mesh(&*model3d::load_mesh_or_builtin(&o.path)?))
        .collect::<Result<Vec<_>, String>>()?;
    // Higher in the list draws over lower in every view; depth decides only between equals.
    let ranks: Vec<u32> = (0..objects.len()).map(|i| (objects.len() - i) as u32).collect();
    match mode {
        model3d::Mode::Real => {
            let mesh: Vec<(Mesh, ink::Style)> = objects
                .iter()
                .zip(&normalized)
                .map(|(o, norm)| (place_normalized_with(norm, o, None, angle, THICKNESS_MM), o.style))
                .collect();
            let drawn: Vec<V3> = (0..views)
                .map(|i| eye_and_lamp(opts, lamp_mm, shape * HEAD_HALF_SPAN_MM * (2.0 * i as f64 / denom - 1.0), angle).0)
                .collect();
            Ok(drawn
                .par_iter()
                .map(|e| ink::draw(&mesh, &ranks, *e, angle, n, preset.inner_radius_mm, preset.outer_radius_mm))
                .collect())
        }
        model3d::Mode::Transform => Ok((0..views)
            .into_par_iter()
            .map(|i| {
                let t = i as f64 / denom;
                let mesh: Vec<(Mesh, ink::Style)> = objects
                    .iter()
                    .zip(&normalized)
                    .map(|(o, norm)| {
                        let m = o.motion.sample(t);
                        (place_normalized_with(norm, o, Some(&m), angle, THICKNESS_MM), o.style)
                    })
                    .collect();
                ink::draw_ortho(&mesh, &ranks, angle, n, preset.inner_radius_mm, preset.outer_radius_mm)
            })
            .collect()),
    }
}

fn solve_patch(
    poses: &[(V3, V3)],
    wide_poses: &[(V3, V3, usize)],
    sharp: &[Vec<f32>],
    blend: bool,
    opts: &Options,
    resp: &Responses,
    preset: &DiscGeometryPreset,
) -> Patch {
    let views = poses.len();
    assert_eq!(sharp.len(), views);
    let n = opts.grid;
    let k = resp.options;
    let plain = k - 1;
    // The disc cannot tell neighbouring views apart: blended, a point far from its surface is
    // one soft image rather than a row of copies.
    const BLEND: [f32; 5] = [0.1, 0.23, 0.34, 0.23, 0.1];
    let target = |v: usize, c: usize| -> f32 {
        if !blend {
            return sharp[v][c];
        }
        let (mut s, mut w) = (0.0, 0.0);
        for (d, wd) in BLEND.iter().enumerate() {
            let i = v as isize + d as isize - 2;
            if i >= 0 && (i as usize) < views {
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
            if opts.wide > 0.0 && !wide_poses.is_empty() {
                for &(eye, lamp, ref_v) in wide_poses {
                    let t = target(ref_v, c) as f64 * opts.exposure;
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
    Patch { cells: lit, weights }
}

/// Solve `objects` (top first) of one position together, in `mode` with `shape` its parallax
/// boost or tilt.
#[allow(clippy::too_many_arguments)]
fn solve_position(pos: &Position, mode: model3d::Mode, shape: f64, objects: &[&SceneObject], lamp_mm: V3, opts: &Options, resp: &Responses, preset: &DiscGeometryPreset) -> Result<Patch, String> {
    let angle = pos.phi_deg.to_radians();
    match mode {
        model3d::Mode::Real => {
            let poses: Vec<(V3, V3)> = (0..VIEWS)
                .map(|i| eye_and_lamp(opts, lamp_mm, HEAD_HALF_SPAN_MM * (2.0 * i as f64 / (VIEWS - 1) as f64 - 1.0), angle))
                .collect();
            let wide_poses: Vec<(V3, V3, usize)> = if opts.wide > 0.0 {
                WIDE_MM
                    .iter()
                    .flat_map(|&h| [(-h, 0usize), (h, VIEWS - 1)])
                    .map(|(head, rv)| {
                        let (eye, lamp) = eye_and_lamp(opts, lamp_mm, head, angle);
                        (eye, lamp, rv)
                    })
                    .collect()
            } else {
                vec![]
            };
            let sharp = position_targets(pos, mode, shape, objects, lamp_mm, opts, preset)?;
            Ok(solve_patch(&poses, &wide_poses, &sharp, true, opts, resp, preset))
        }
        model3d::Mode::Transform => {
            let poses = transform_poses(opts, lamp_mm, shape, angle);
            let sharp = position_targets(pos, mode, shape, objects, lamp_mm, opts, preset)?;
            Ok(solve_patch(&poses, &[], &sharp, false, opts, resp, preset))
        }
    }
}

#[inline]
fn srgb_to_linear(v: f32) -> f64 {
    let c = (v as f64 / 255.0).clamp(0.0, 1.0);
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Solve one disc cell's codebook weights for a colour picture across the viewing poses, its
/// hues weighted `chroma` against its brightness.
fn solve_colour_cell(x: f64, y: f64, rgb: [f32; 3], chroma: f64, poses: &[(V3, V3)], opts: &Options, resp: &Responses) -> Vec<f32> {
    let k = resp.options;
    let plain = k - 1;
    let luma = |p: [f32; 3]| 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64;
    let target_lin = [srgb_to_linear(rgb[0]), srgb_to_linear(rgb[1]), srgb_to_linear(rgb[2])];
    let target_luma = 0.2126 * target_lin[0] + 0.7152 * target_lin[1] + 0.0722 * target_lin[2];
    let t = target_luma * opts.exposure;
    let colour = chroma.max(0.0).sqrt();

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

    for &(eye, lamp) in poses {
        let (gt, gr) = scene::g_at(&scene::Scene { light_mm: lamp, camera_mm: eye, ..Default::default() }, x, y);
        let cols: Vec<[f32; 3]> = (0..k).map(|j| resp.at(j, gt, gr)).collect();
        let plain_y = luma(cols[plain]);
        for j in 0..k {
            row[j] = (1.0 - PLAIN_LEAK) * luma(cols[j]);
        }
        add(&row, (t - PLAIN_LEAK * plain_y).max(0.0));
        if colour > 0.0 && t > 1e-6 {
            for ch in 0..3 {
                for j in 0..k {
                    row[j] = colour * (1.0 - PLAIN_LEAK) * (cols[j][ch] as f64 - luma(cols[j]));
                }
                let target_chroma = (target_lin[ch] - target_luma) * opts.exposure;
                add(&row, colour * (target_chroma - PLAIN_LEAK * (cols[plain][ch] as f64 - plain_y)));
            }
        }
    }

    for p in 0..k {
        for q in 0..p {
            g[p * k + q] = g[q * k + p];
        }
    }
    design::solve_simplex_qp(&g, &h, k).into_iter().map(|w| w as f32).collect()
}

/// Solve every position and lay the patches out on the disc. Groups already solved, and not
/// changed since, are taken from the last few.
///
/// Where two things want the same cell, the earlier position wins, and within a position the one
/// higher in its list; a group of models stands where its top member does.
pub fn solve(scene: &Scene, opts: &Options, cb: &Codebook, resp: &Responses, preset: &DiscGeometryPreset) -> Result<Solution, String> {
    const KEEP: usize = 16;
    static CACHE: Mutex<Vec<(String, Arc<Patch>)>> = Mutex::new(Vec::new());
    let opts = Options { grid: opts.grid.clamp(64, 1024), ..opts.clone() };
    let lamp = scene.lamp_mm.unwrap_or(LAMP_MM);
    let positions = positions(scene);
    let pos_crops: Vec<flat::PositionCrop> = positions
        .iter()
        .map(|p| flat::PositionCrop::load(p, preset.outer_radius_mm, Some(1024)))
        .collect::<Result<_, _>>()?;
    let groups = groups(scene, &positions);
    // Pictures are looked at the solve grid's size.
    let pictures = flat::pictures(scene, &positions, Some(1024))?;
    if groups.is_empty() && pictures.is_empty() {
        return Err("场景中没有可见模型".into());
    }
    let (n, k) = (opts.grid, resp.options);
    let bg = cb.entries.iter().position(|e| e.bytes.iter().all(|&b| b == design::BACKGROUND_BYTE)).ok_or("码本里没有背景序列")?;
    let mut patches = Vec::with_capacity(groups.len());
    for g in &groups {
        let pos = &positions[g.at];
        // Names, ids and places in other lists change nothing that is burned.
        let same: Vec<_> = g
            .objects
            .iter()
            .map(|o| {
                let modified = std::fs::metadata(&o.path).and_then(|m| m.modified()).ok();
                (SceneObject { id: String::new(), name: String::new(), position_id: String::new(), ..(*o).clone() }, modified)
            })
            .collect();
        let p_opts = position_options(pos, g.mode, &opts);
        let key = serde_json::to_string(&(pos.phi_deg, g.mode, g.shape, &same, lamp, &p_opts, preset)).map_err(|e| e.to_string())?;
        let held = CACHE.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|c| c.0 == key).map(|c| c.1.clone());
        let patch = match held {
            Some(p) => p,
            None => {
                let p = Arc::new(solve_position(pos, g.mode, g.shape, &g.objects, lamp, &p_opts, resp, preset)?);
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
    let at_mm = |c: usize| {
        let x = (((c % n) as f64 + 0.5) / n as f64 - 0.5) * 2.0 * preset.outer_radius_mm;
        let y = (((c / n) as f64 + 0.5) / n as f64 - 0.5) * 2.0 * preset.outer_radius_mm;
        (x, y)
    };
    let kept = |at: usize, x: f64, y: f64| pos_crops[at].kept(x, y);
    enum Claim {
        Group(usize),
        Picture(usize),
    }
    let mut claims: Vec<(usize, usize, Claim)> = groups.iter().enumerate().map(|(i, g)| (g.at, g.rank, Claim::Group(i))).collect();
    claims.extend(pictures.iter().enumerate().map(|(i, p)| (p.at, p.rank, Claim::Picture(i))));
    claims.sort_by_key(|c| (c.0, c.1));
    let mut owner = vec![usize::MAX; n * n];
    let has_flat = pictures.iter().any(|p| !p.colour);
    let mut flat_owner = vec![0u8; if has_flat { n * n } else { 0 }];
    let mut clash = vec![false; positions.len()];
    let mut take = |at: usize, c: usize| -> bool {
        if owner[c] == usize::MAX {
            owner[c] = at;
            return true;
        }
        if owner[c] != at {
            clash[at] = true;
            clash[owner[c]] = true;
        }
        false
    };
    let across = 2.0 * preset.outer_radius_mm;
    for (_, _, claim) in &claims {
        match *claim {
            Claim::Group(gi) => {
                let at = groups[gi].at;
                let p = &patches[gi];
                for (j, &c) in p.cells.iter().enumerate() {
                    let c = c as usize;
                    let (x, y) = at_mm(c);
                    if kept(at, x, y) && take(at, c) {
                        weights[c * k..(c + 1) * k].copy_from_slice(&p.weights[j * k..(j + 1) * k]);
                    }
                }
            }
            Claim::Picture(pi) => {
                let pic = &pictures[pi];
                if pic.colour {
                    let mut cells: Vec<(usize, f64, f64, [f32; 3])> = Vec::new();
                    for c in 0..n * n {
                        let (x, y) = at_mm(c);
                        if !flat::on_ring(x, y, preset) || !kept(pic.at, x, y) {
                            continue;
                        }
                        if let Some(rgb) = pic.colour_covered(x / across, y / across) {
                            if take(pic.at, c) {
                                cells.push((c, x, y, rgb));
                            }
                        }
                    }
                    let angle = positions[pic.at].phi_deg.to_radians();
                    let poses: Vec<_> = (0..COLOUR_VIEWS)
                        .map(|i| eye_and_lamp(&opts, lamp, EYE_HALF_MM * (2.0 * i as f64 / (COLOUR_VIEWS - 1) as f64 - 1.0), angle))
                        .collect();
                    let chroma = pic.chroma.unwrap_or(PICTURE_CHROMA);
                    let solved_cells: Vec<(usize, Vec<f32>)> = cells
                        .par_iter()
                        .map(|&(c, x, y, rgb)| (c, solve_colour_cell(x, y, rgb, chroma, &poses, &opts, resp)))
                        .collect();
                    for (c, w) in solved_cells {
                        weights[c * k..(c + 1) * k].copy_from_slice(&w);
                    }
                } else {
                    for c in 0..n * n {
                        let (x, y) = at_mm(c);
                        if flat::on_ring(x, y, preset) && kept(pic.at, x, y) && pic.covered(x / across, y / across).is_some() && take(pic.at, c) {
                            flat_owner[c] = pi as u8 + 1;
                        }
                    }
                }
            }
        }
    }
    // Unlit cells inside the kept sectors of a position with animated models use the darkest
    // option over its top animation's sweep (0xA7 glows at high g_t); others keep 0xA7.
    for (at, pos) in positions.iter().enumerate() {
        let Some(g) = groups.iter().filter(|g| g.at == at && g.mode == model3d::Mode::Transform).min_by_key(|g| g.rank) else {
            continue;
        };
        let poses = transform_poses(&position_options(pos, g.mode, &opts), lamp, g.shape, pos.phi_deg.to_radians());
        let (t_bg, _) = transform_background(&pos_crops[at], &poses, resp, preset);
        if t_bg == bg {
            continue;
        }
        for c in 0..n * n {
            let (x, y) = at_mm(c);
            if owner[c] == usize::MAX && kept(at, x, y) {
                let cell = &mut weights[c * k..(c + 1) * k];
                cell.fill(0.0);
                cell[t_bg] = 1.0;
            }
        }
    }
    let overlapping = positions.iter().zip(&clash).filter(|(_, &c)| c).map(|(p, _)| p.id.clone()).collect();
    let flat_pictures = if has_flat { pictures.iter().map(|p| (p.id.clone(), p.phi_rad)).collect() } else { vec![] };
    let flat = Arc::new(flat::FlatOwners { grid: n, owner: flat_owner, pictures: flat_pictures });
    Ok(Solution { solved: Arc::new(Solved { grid: n, options: k, weights, error: 0.0 }), overlapping, flat })
}

/// The solved weights for drawing on the graphics card: `[grid u32][options u32]`, then
/// `ceil(options / 4)` layers of `grid * grid` RGBA bytes, one byte per option.
/// After them one more layer: what `flat::Flat::preview_layer` gives, or nothing where it is
/// empty (no flat positions).
pub fn weight_layers(s: &Solved, flat: &[u8]) -> Vec<u8> {
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
    if flat.len() == n * n * 4 {
        out.extend_from_slice(flat);
    } else {
        out.resize(out.len() + n * n * 4, 0);
    }
    out
}

/// What every option looks like, for drawing on the graphics card: `[options u32][gt u32][gr u32]
/// [groove u32][carve levels u32]` and `[g max f32][groove spread f32]`, the responses as RGB
/// floats ordered option, gt, gr, then the carve page's bytes at each level
/// (`design::carve_looks`) the same way, then the groove's rainbow by `|g_r|` at `g_t = 0`.
pub fn response_tables(resp: &Responses, carve: &[Vec<[f32; 3]>], pitch_um: f64) -> Vec<u8> {
    const GROOVE: usize = 512;
    let mut out = Vec::new();
    for v in [resp.options, resp.gt_bins, resp.gr_bins, GROOVE, carve.len()] {
        out.extend_from_slice(&(v as u32).to_le_bytes());
    }
    for v in [design::G_MAX as f32, design::GROOVE_SOURCE as f32] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let mut put = |p: [f32; 3]| p.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
    resp.rgb.iter().flatten().for_each(|p| put(*p));
    carve.iter().flatten().for_each(|p| put(*p));
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
            position_mm: [x, y, 6.0], rotation_deg: [0.0; 3], scale_mm: [22.0; 3], position_id: position.into(), style: ink::Style::default(),
            motion: model3d::Motion::default(),
            mode: Default::default(),
            parallax: 2.0,
            tilt_deg: 0.0,
        }
    }

    fn pos(id: &str, phi_deg: f64, crop_deg: f64) -> Position {
        Position {
            id: id.into(),
            name: String::new(),
            phi_deg,
            crop_deg,
            crop_side: Default::default(),
            crop_offset_deg: 0.0,
            crops: None,
            neutral: None,
            order: vec![],
        }
    }

    fn scene(phis: &[f64], objects: Vec<SceneObject>) -> Scene {
        let positions = phis.iter().enumerate().map(|(i, &phi_deg)| pos(&i.to_string(), phi_deg, 30.0)).collect();
        Scene { version: model3d::SCENE_VERSION, objects, lamp_mm: None, positions, images: vec![] }
    }

    #[test]
    fn overlapping_models_of_one_mode_are_solved_together_in_list_order() {
        let a = star("0", 0.0, 41.0);
        let b = SceneObject { id: "b".into(), position_mm: [4.0, 44.0, 6.0], ..star("0", 0.0, 41.0) };
        let mut sc = scene(&[0.0], vec![a.clone(), b.clone()]);
        sc.positions[0].order = vec!["b".into(), a.id.clone()];
        let ps = positions(&sc);
        let gs = groups(&sc, &ps);
        assert_eq!(gs.len(), 1, "one joint solve, so they still share cells view by view");
        assert_eq!(gs[0].objects.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), ["b", a.id.as_str()]);
        // A different mode, or a different parallax, is a solve of its own.
        let anim = SceneObject { id: "c".into(), mode: model3d::Mode::Transform, position_mm: [0.0, -41.0, 6.0], ..star("0", 0.0, -41.0) };
        let wider = SceneObject { id: "d".into(), parallax: 4.0, ..star("0", 30.0, -41.0) };
        let mixed = scene(&[0.0], vec![a, b, anim, wider]);
        assert_eq!(groups(&mixed, &positions(&mixed)).len(), 3);
        let shared = design::shared_codebook(0.1333);
        let opts = Options { grid: 96, ..Default::default() };
        let sol = solve(&mixed, &opts, &shared.0, &shared.1, &reference_preset()).unwrap();
        assert!(sol.overlapping.is_empty(), "things in one position do not clash: {:?}", sol.overlapping);
    }

    #[test]
    fn the_higher_of_a_picture_and_a_model_takes_the_disc_they_share() {
        let dir = std::env::temp_dir().join(format!("holo-order-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.png");
        image::GrayImage::from_pixel(16, 16, image::Luma([40])).save(&path).unwrap();
        let preset = reference_preset();
        let across = 2.0 * preset.outer_radius_mm;
        let model = star("0", 0.0, 41.0);
        let picture = SceneImage {
            id: "pic".into(), name: "p".into(), width: 16, height: 16, position_id: "0".into(), colour: false, chroma: None,
            layer: crate::artwork::Layer { path: path.to_string_lossy().into(), y: -41.0 / across, size: 30.0 / across, ..crate::artwork::Layer::plain() },
        };
        let shared = design::shared_codebook(0.1333);
        let opts = Options { grid: 128, ..Default::default() };
        let owner_at_star = |order: Vec<String>| {
            let mut sc = scene(&[0.0], vec![model.clone()]);
            sc.images = vec![picture.clone()];
            sc.positions[0].order = order;
            let sol = solve(&sc, &opts, &shared.0, &shared.1, &preset).unwrap();
            assert!(sol.overlapping.is_empty());
            sol.flat.at(0.0, -41.0 / across)
        };
        assert_eq!(owner_at_star(vec!["pic".into(), model.id.clone()]), Some(0), "picture on top");
        assert_eq!(owner_at_star(vec![model.id.clone(), "pic".into()]), None, "model on top");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn colour_picture_solves_weights_into_matching_hues_without_flat_owner() {
        let dir = std::env::temp_dir().join(format!("holo-colour-pic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rb.png");
        let img = image::RgbImage::from_fn(16, 16, |x, _| {
            if x < 8 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) }
        });
        img.save(&path).unwrap();
        let preset = reference_preset();
        let across = 2.0 * preset.outer_radius_mm;
        let make_pic = |colour: bool| SceneImage {
            id: "pic".into(),
            name: "rb".into(),
            width: 16,
            height: 16,
            position_id: "0".into(),
            colour,
            chroma: None,
            layer: crate::artwork::Layer {
                path: path.to_string_lossy().into(),
                x: 41.0 / across,
                y: 0.0,
                size: 28.0 / across,
                ..crate::artwork::Layer::plain()
            },
        };
        let shared = design::shared_codebook(0.1333);
        let (cb, resp) = (&shared.0, &shared.1);
        let opts = Options { grid: 128, ..Default::default() };
        let k = resp.options;
        let plain = k - 1;
        let eval_pt = |sol: &Solution, lamp_mm: [f64; 3], hx: f64, x: f64, y: f64| -> [f64; 3] {
            let (eye, lamp) = eye_and_lamp(&opts, lamp_mm, hx, 0.0);
            let sc_view = scene::Scene { light_mm: lamp, camera_mm: eye, ..Default::default() };
            let w = sol.solved.cell(x / across + 0.5, y / across + 0.5).unwrap();
            let (gt, gr) = scene::g_at(&sc_view, x, y);
            let pl = resp.at(plain, gt, gr);
            let mut rgb = [0.0f64; 3];
            for j in 0..k {
                let c = resp.at(j, gt, gr);
                for ch in 0..3 {
                    rgb[ch] += w[j] as f64 * c[ch] as f64;
                }
            }
            for ch in 0..3 {
                rgb[ch] = (1.0 - PLAIN_LEAK) * rgb[ch] + PLAIN_LEAK * pl[ch] as f64;
            }
            rgb
        };

        // (a) Colour picture at 3 o'clock under Top light: no flat owner cells; left, centre, and
        // right poses all render red-dominant on the red half and blue-dominant on the blue half.
        let top_lamp = [0.0, 150.0, 400.0];
        let sc_colour = Scene {
            version: model3d::SCENE_VERSION,
            objects: vec![],
            lamp_mm: Some(top_lamp),
            positions: vec![pos("0", 0.0, 90.0)],
            images: vec![make_pic(true)],
        };
        let sol_colour = solve(&sc_colour, &opts, cb, resp, &preset).unwrap();
        assert!(sol_colour.flat.owner.iter().all(|&o| o == 0), "colour picture must not claim flat_owner");
        for &hx in &[-EYE_HALF_MM, 0.0, EYE_HALF_MM] {
            let red = eval_pt(&sol_colour, top_lamp, hx, 34.0, 0.0);
            let blue = eval_pt(&sol_colour, top_lamp, hx, 48.0, 0.0);
            assert!(red[0] > red[1] && red[0] > red[2], "hx={hx}: red half should be red-dominant: {red:?}");
            assert!(blue[2] > blue[0] && blue[2] > blue[1], "hx={hx}: blue half should be blue-dominant: {blue:?}");
        }

        // (b) 4-quadrant RGBW picture at 3 o'clock under Top-right light: primaries stay dominant
        // and white stays neutral across left (-32 mm), centre (0 mm), and right (+32 mm) poses.
        let rgbw_path = dir.join("rgbw.png");
        let rgbw = image::RgbImage::from_fn(16, 16, |x, y| match (x < 8, y < 8) {
            (true, true) => image::Rgb([255, 0, 0]),
            (false, true) => image::Rgb([0, 220, 0]),
            (true, false) => image::Rgb([0, 0, 255]),
            (false, false) => image::Rgb([240, 240, 240]),
        });
        rgbw.save(&rgbw_path).unwrap();
        let top_right = [150.0, 150.0, 400.0];
        let sc_rgbw = Scene {
            version: model3d::SCENE_VERSION,
            objects: vec![],
            lamp_mm: Some(top_right),
            positions: vec![pos("0", 0.0, 90.0)],
            images: vec![SceneImage {
                id: "rgbw".into(),
                name: "rgbw".into(),
                width: 16,
                height: 16,
                position_id: "0".into(),
                colour: true,
                chroma: None,
                layer: crate::artwork::Layer {
                    path: rgbw_path.to_string_lossy().into(),
                    x: 41.0 / across,
                    y: 0.0,
                    size: 28.0 / across,
                    ..crate::artwork::Layer::plain()
                },
            }],
        };
        let sol_rgbw = solve(&sc_rgbw, &opts, cb, resp, &preset).unwrap();
        for &hx in &[-EYE_HALF_MM, 0.0, EYE_HALF_MM] {
            let r = eval_pt(&sol_rgbw, top_right, hx, 34.0, -7.0);
            let g = eval_pt(&sol_rgbw, top_right, hx, 48.0, -7.0);
            let b = eval_pt(&sol_rgbw, top_right, hx, 34.0, 7.0);
            let w = eval_pt(&sol_rgbw, top_right, hx, 48.0, 7.0);
            assert!(r[0] > r[1] && r[0] > r[2], "hx={hx}: red quadrant not red-dominant: {r:?}");
            assert!(g[1] > g[0] && g[1] > g[2], "hx={hx}: green quadrant not green-dominant: {g:?}");
            assert!(b[2] > b[0] && b[2] > b[1], "hx={hx}: blue quadrant not blue-dominant: {b:?}");
            let mean = (w[0] + w[1] + w[2]) / 3.0;
            let spread = (w[0].max(w[1]).max(w[2]) - w[0].min(w[1]).min(w[2])) / mean.max(1e-9);
            assert!(spread < 0.25, "hx={hx}: white quadrant not neutral (spread={spread:.3}): {w:?}");
        }

        // (c) Mono picture: flat owner set, weights untouched (background everywhere).
        let sc_mono = Scene {
            version: model3d::SCENE_VERSION,
            objects: vec![],
            lamp_mm: None,
            positions: vec![pos("0", 0.0, 90.0)],
            images: vec![make_pic(false)],
        };
        let sol_mono = solve(&sc_mono, &opts, cb, resp, &preset).unwrap();
        assert_eq!(sol_mono.flat.at(34.0 / across, 0.0), Some(0));
        assert_eq!(sol_mono.flat.at(48.0 / across, 0.0), Some(0));
        let bg = cb.entries.iter().position(|e| e.bytes.iter().all(|&b| b == design::BACKGROUND_BYTE)).unwrap();
        for cell in sol_mono.solved.weights.chunks_exact(k) {
            for (j, &w) in cell.iter().enumerate() {
                let want = if j == bg { 1.0 } else { 0.0 };
                assert_eq!(w, want, "mono picture must leave weights as background");
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
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
        let o = SceneObject { position_mm: [0.0, 41.0, -2.0], scale_mm: [30.0; 3], ..star("0", 0.0, 41.0) };
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
            let obj = SceneObject { scale_mm: [28.0; 3], ..star("0", 22.0, 34.0) };
            let make = |crop_deg: f64| Scene {
                version: model3d::SCENE_VERSION,
                images: vec![],
                objects: vec![obj.clone()],
                lamp_mm: None,
                positions: vec![pos("0", phi_deg, crop_deg)],
            };
            let sol30 = solve(&make(30.0), &opts, &shared.0, &shared.1, &preset).unwrap();
            let sol90 = solve(&make(90.0), &opts, &shared.0, &shared.1, &preset).unwrap();

            // 90° equals the raw uncropped patch written over background.
            let raw = solve_position(&pos("0", phi_deg, 90.0), model3d::Mode::Real, obj.parallax, &[&obj], LAMP_MM, &opts, &shared.1, &preset).unwrap();
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
            images: vec![],
            objects: vec![
                SceneObject { scale_mm: [24.0; 3], ..star("0", 29.0, 29.0) },
                SceneObject { scale_mm: [24.0; 3], ..star("1", -29.0, 29.0) },
            ],
            lamp_mm: None,
            positions: vec![
                pos("0", 0.0, crop_deg),
                pos("1", 90.0, crop_deg),
            ],
        };
        assert_eq!(solve(&both(90.0), &opts, &shared.0, &shared.1, &preset).unwrap().overlapping, ["0", "1"]);
        assert!(solve(&both(30.0), &opts, &shared.0, &shared.1, &preset).unwrap().overlapping.is_empty());

        // Circle crop in `Position.crops` restricts overlapping objects just like sector crops.
        let circled = Scene {
            version: model3d::SCENE_VERSION,
            images: vec![],
            objects: vec![star("0", 0.0, 41.0), star("1", 0.0, 41.0)],
            lamp_mm: None,
            positions: vec![
                Position {
                    crops: Some(vec![model3d::Crop::Circle {
                        id: "c0".into(),
                        name: String::new(),
                        visible: true,
                        x: 0.0,
                        y: -0.35,
                        size: 0.25,
                        stretch_x: 1.0,
                        stretch_y: 1.0,
                        rotation_deg: 0.0,
                    }]),
                    ..pos("0", 0.0, 90.0)
                },
                Position {
                    crops: Some(vec![model3d::Crop::Circle {
                        id: "c1".into(),
                        name: String::new(),
                        visible: true,
                        x: 0.0,
                        y: 0.35,
                        size: 0.25,
                        stretch_x: 1.0,
                        stretch_y: 1.0,
                        rotation_deg: 0.0,
                    }]),
                    ..pos("1", 90.0, 90.0)
                },
            ],
        };
        assert!(solve(&circled, &opts, &shared.0, &shared.1, &preset).unwrap().overlapping.is_empty());
    }

    #[test]
    fn transform_mode_identity_motion_gives_constant_targets_and_ignores_distance() {
        let preset = reference_preset();
        let opts = Options { grid: 96, ..Default::default() };
        let obj = SceneObject { position_mm: [0.0, 41.0, 0.0], ..star("0", 0.0, 41.0) };
        let p_transform = pos("0", 0.0, 30.0);
        let anim = model3d::Mode::Transform;
        let t_targets = position_targets(&p_transform, anim, DEFAULT_TRANSFORM_TILT_DEG, &[&obj], LAMP_MM, &opts, &preset).unwrap();
        assert_eq!(t_targets.len(), TRANSFORM_VIEWS);
        for v in 0..TRANSFORM_VIEWS {
            assert_eq!(t_targets[v], t_targets[0], "view {v} should match view 0 for identity motion");
        }
        // Viewing distance must not affect orthographic transform targets.
        let near = position_targets(&p_transform, anim, DEFAULT_TRANSFORM_TILT_DEG, &[&obj], LAMP_MM, &Options { distance_mm: 180.0, ..opts.clone() }, &preset).unwrap();
        let far = position_targets(&p_transform, anim, DEFAULT_TRANSFORM_TILT_DEG, &[&obj], LAMP_MM, &Options { distance_mm: 600.0, ..opts.clone() }, &preset).unwrap();
        assert_eq!(near, far, "transform targets must be independent of viewing distance");

        // Non-identity motion must shift targets between t=0 and t=1.
        let mut moved = obj.clone();
        moved.motion = model3d::Motion {
            preset: "slideX".into(),
            matrix: vec![],
            samples: vec![
                [1.0, 0.0, 0.0, -10.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
                [1.0, 0.0, 0.0, 10.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
            ],
            vars: vec![],
        };
        let anim_targets = position_targets(&p_transform, anim, DEFAULT_TRANSFORM_TILT_DEG, &[&moved], LAMP_MM, &opts, &preset).unwrap();
        assert_ne!(anim_targets[0], anim_targets[TRANSFORM_VIEWS - 1], "sliding motion should change target across views");
    }

    #[test]
    fn transform_mode_spinning_star_separates_start_and_end_frames() {
        let shared = design::shared_codebook(0.1333);
        let (cb, resp) = (&shared.0, &shared.1);
        let preset = reference_preset();
        let grid = 192usize;
        let samples: Vec<[f64; 16]> = (0..=64)
            .map(|i| {
                // A half turn looks much the same at both ends, so the star also slides across.
                let t = i as f64 / 64.0;
                let (s, c) = (t * 180.0).to_radians().sin_cos();
                let x = 16.0 * (2.0 * t - 1.0);
                [c, 0.0, s, x, 0.0, 1.0, 0.0, 0.0, -s, 0.0, c, 0.0, 0.0, 0.0, 0.0, 1.0]
            })
            .collect();
        let obj = SceneObject {
            id: "star".into(),
            path: model3d::BUILTIN_SAMPLE_PATH.into(),
            name: "star".into(),
            visible: true,
            position_mm: [0.0, 41.0, 0.0],
            rotation_deg: [0.0; 3],
            scale_mm: [30.0; 3],
            position_id: "0".into(),
            style: ink::Style::default(),
            motion: model3d::Motion { preset: "spinY".into(), matrix: vec![], samples, vars: vec![] },
            mode: model3d::Mode::Transform,
            parallax: 2.0,
            tilt_deg: 0.0,
        };
        let p0 = pos("0", 0.0, 30.0);
        let lamp = [225.0, 0.0, 400.0];
        let sc = Scene {
            version: model3d::SCENE_VERSION,
            images: vec![],
            objects: vec![obj.clone()],
            lamp_mm: Some(lamp),
            positions: vec![p0.clone()],
        };
        let opts = Options { grid, chroma_weight: 1.0, exposure: 1.0, wide: 0.3, ..Default::default() };
        let sol = solve(&sc, &opts, cb, resp, &preset).unwrap();
        let tilt = tilt_of(&obj);

        let targets = position_targets(&p0, obj.mode, tilt, &[&obj], lamp, &opts, &preset).unwrap();
        let region: Vec<usize> = (0..grid * grid).filter(|&c| targets.iter().any(|t| t[c] > LIT)).collect();
        let pearson = |a: &[f64], b: &[f32]| -> f64 {
            let n = region.len() as f64;
            let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for &c in &region {
                let (x, y) = (a[c], b[c] as f64);
                sa += x; sb += y; saa += x * x; sbb += y * y; sab += x * y;
            }
            let d = ((saa - sa * sa / n).max(0.0) * (sbb - sb * sb / n).max(0.0)).sqrt();
            if d < 1e-12 { 0.0 } else { (sab - sa * sb / n) / d }
        };
        let render_at = |t: f64| -> Vec<f64> {
            let (e, l) = transform_pose(&opts, lamp, tilt, t, 0.0);
            super::super::lightfield::render_float(&sol.solved, resp, &preset, e, l, grid, true)
                .into_iter()
                .map(super::super::lightfield::luma)
                .collect()
        };
        let r0 = render_at(0.0);
        let r_mid = render_at(0.5);
        let r1 = render_at(1.0);
        let mid_v = TRANSFORM_VIEWS / 2;
        let (c00, c01, c0m) = (pearson(&r0, &targets[0]), pearson(&r0, &targets[TRANSFORM_VIEWS - 1]), pearson(&r0, &targets[mid_v]));
        let (c11, c10, c1m) = (pearson(&r1, &targets[TRANSFORM_VIEWS - 1]), pearson(&r1, &targets[0]), pearson(&r1, &targets[mid_v]));
        let (cmm, cm0, cm1) = (pearson(&r_mid, &targets[mid_v]), pearson(&r_mid, &targets[0]), pearson(&r_mid, &targets[TRANSFORM_VIEWS - 1]));

        // Unlit cell inside the kept sector uses the darkest transform background, darker than 0xA7.
        let bg_a7 = cb.entries.iter().position(|e| e.bytes.iter().all(|&b| b == design::BACKGROUND_BYTE)).unwrap();
        let poses = transform_poses(&opts, lamp, tilt, 0.0);
        let crop0 = flat::PositionCrop::load(&p0, preset.outer_radius_mm, None).unwrap();
        let (t_bg, t_bg_max) = transform_background(&crop0, &poses, resp, &preset);
        let mut a7_max = 0.0f64;
        let ro = preset.outer_radius_mm;
        for &(eye, lamp) in &poses {
            let s = scene::Scene { light_mm: lamp, camera_mm: eye, ..Default::default() };
            let (gt, gr) = scene::g_at(&s, 0.0, -41.0);
            let y = (1.0 - PLAIN_LEAK) * super::super::lightfield::luma(resp.at(bg_a7, gt, gr))
                + PLAIN_LEAK * super::super::lightfield::luma(resp.at(resp.options - 1, gt, gr));
            a7_max = a7_max.max(y);
        }
        println!(
            "transform_star: t=0(own={c00:.3}, t1={c01:.3}, mid={c0m:.3}) t=0.5(own={cmm:.3}, t0={cm0:.3}, t1={cm1:.3}) t=1(own={c11:.3}, t0={c10:.3}, mid={c1m:.3}) | bg_opt={t_bg} (plain={}) max_y={t_bg_max:.4} vs 0xA7(opt {bg_a7}) max_y={a7_max:.4}",
            t_bg == cb.entries.len(),
        );
        assert!(c00 > c01, "t=0 render must correlate better with t=0 target than t=1: own={c00:.3} other={c01:.3}");
        assert!(c11 > c10, "t=1 render must correlate better with t=1 target than t=0: own={c11:.3} other={c10:.3}");
        assert!(c00 > c0m + 0.15, "t=0 (face-on) must clearly beat t=0.5 (edge-on): own={c00:.3} mid={c0m:.3}");
        assert!(cmm > cm0.max(cm1) + 0.15, "t=0.5 (edge-on) must clearly beat face-on endpoints: own={cmm:.3} t0={cm0:.3} t1={cm1:.3}");
        assert!(c11 > c1m + 0.15, "t=1 (face-on) must clearly beat t=0.5 (edge-on): own={c11:.3} mid={c1m:.3}");
        assert!(t_bg_max < a7_max, "transform background opt {t_bg} ({t_bg_max:.3}) must be darker than 0xA7 ({a7_max:.3})");
        // Check an unlit cell at 6 o'clock (0, +41 mm), inside ±30° crop sector.
        let unlit_u = 0.5;
        let unlit_v = 0.5 + 41.0 / (2.0 * ro);
        let w = sol.solved.cell(unlit_u, unlit_v).unwrap();
        assert_eq!(w[t_bg], 1.0, "unlit cell inside transform crop sector must use opt {t_bg}");
        // Check an unlit cell at 3 o'clock (+41, 0 mm), outside ±30° crop sector, keeps 0xA7.
        let out_w = sol.solved.cell(0.5 + 41.0 / (2.0 * ro), 0.5).unwrap();
        assert_eq!(out_w[bg_a7], 1.0, "unlit cell outside transform crop sector must keep 0xA7");
    }

    #[test]
    fn scene_json_without_mode_motion_parallax_deserializes_with_defaults() {
        let json = r#"{
            "version": 1,
            "positions": [{"id": "p0", "name": "", "phiDeg": 15.0}],
            "objects": [{
                "id": "o0",
                "path": "builtin:star",
                "name": "star",
                "positionMm": [0, 41, 6],
                "rotationDeg": [0, 0, 0],
                "scaleMm": 22,
                "positionId": "p0"
            }]
        }"#;
        let s: Scene = serde_json::from_str(json).unwrap();
        assert_eq!(s.positions.len(), 1);
        assert_eq!(s.objects[0].mode, model3d::Mode::Real);
        assert_eq!(s.objects[0].parallax, 2.0);
        assert_eq!(s.positions[0].crop_deg, 30.0);
        assert_eq!(s.objects[0].motion.sample(0.5), model3d::IDENTITY_MAT4);
        assert!(s.objects[0].motion.samples.is_empty());
    }

    /// Three positions, the last of two models, as seen turning the disc.
    #[test]
    #[ignore]
    fn look_positions() {
        let girl = |p: &str, x: f64, y: f64, rot: [f64; 3], size: f64| SceneObject { path: std::env::var("MODEL").unwrap(), rotation_deg: rot, scale_mm: [size; 3], position_mm: [x, y, 0.0], ..star(p, x, y) };
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
        let girl = |p: &str, size: f64| SceneObject { path: path.clone(), scale_mm: [size; 3], position_mm: [0.0, 41.0, 0.0], ..star(p, 0.0, 41.0) };
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
                let opts = Options { grid: 384, chroma_weight: chroma, ..Default::default() };
                let mut sc = scene(&[0.0], vec![o.clone()]);
                sc.objects[0].parallax = gain;
                let sol = solve(&sc, &opts, &shared.0, &shared.1, &preset).unwrap();
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
        std::fs::write(format!("{out}/weights.bin"), weight_layers(&s.solved, &[])).unwrap();
        std::fs::write(format!("{out}/tables.bin"), response_tables(&shared.1, &[], preset.base_pitch_um)).unwrap();
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
            let opts = Options { grid: 256, chroma_weight: chroma, ..Default::default() };
            let mut sc = scene(&[0.0], vec![o.clone()]);
            sc.objects[0].parallax = parallax;
            let sol = solve(&sc, &opts, &shared.0, &shared.1, &preset).unwrap();
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
            let o = SceneObject { path: path.into(), scale_mm: [34.0; 3], position_mm: [0.0, 41.0, 0.0], ..star("0", 0.0, 41.0) };
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
                let opts = Options::default();
                let drawn = ink::draw(&mesh, &[], [0.0, 0.0, 350.0], 0.0, n, preset.inner_radius_mm, preset.outer_radius_mm);
                put(&|i| [enc(drawn[i] as f64); 3], 0, row as u32);
                let t = std::time::Instant::now();
                let mut sc = scene(&[0.0], vec![o.clone()]);
                sc.objects[0].parallax = parallax;
                let sol = solve(&sc, &opts, &shared.0, &shared.1, &preset).unwrap();
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
            position_mm: [0.0, 41.0, 0.0], rotation_deg: [0.0, phi, 0.0], scale_mm: [scale; 3], position_id: k.to_string(), style: ink::Style::default(),
            motion: model3d::Motion::default(),
            mode: Default::default(),
            parallax: 2.0,
            tilt_deg: 0.0,
        }).collect();
        let opts = Options { grid: 384, ..Default::default() };
        let mut sc = scene(&phis, objects);
        for o in &mut sc.objects {
            o.parallax = parallax;
        }
        let t = std::time::Instant::now();
        let sol = solve(&sc, &opts, &shared.0, &shared.1, &preset).unwrap();
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
        let o = SceneObject { path: std::env::var("MODEL").unwrap(), scale_mm: [45.0; 3], position_mm: [0.0, 41.0, 0.0], ..star("0", 0.0, 41.0) };
        let opts = Options::default();
        let sol = solve(&scene(&[0.0], vec![o]), &opts, &shared.0, &shared.1, &preset).unwrap();
        std::fs::write(format!("{out}/weights.bin"), weight_layers(&sol.solved, &[])).unwrap();
        std::fs::write(format!("{out}/tables.bin"), response_tables(&shared.1, &[], preset.base_pitch_um)).unwrap();
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
            let o = SceneObject { path: path.clone(), scale_mm: [scale; 3], position_mm: [0.0, 36.0, 0.0], rotation_deg: [0.0, k as f64 * span / frames as f64, 0.0], ..star("0", 0.0, 36.0) };
            let m = [(place(&mesh, &o, psi.to_radians(), 1.0).unwrap(), ink::Style::default())];
            let (e, l) = eye_and_lamp(&opts, lamp, 0.0, psi.to_radians());
            let drawn = ink::draw(&m, &[], e, psi.to_radians(), grid, preset.inner_radius_mm, preset.outer_radius_mm);
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

#[cfg(test)]
mod frames_study {
    use super::*;
    use crate::holo::{codebook, design, lightfield, model3d, scene};
    use crate::preset::reference_preset;
    use crate::rng::SplitMix64;

    const GRID: usize = 256;
    const OBJ_Y_SCENE: f64 = 41.0;
    const OBJ_Y_DISC: f64 = -41.0;

    fn out_dir() -> String {
        let dir = std::env::var("HOLO_OUT").unwrap_or_else(|_| std::env::temp_dir().join("frames-study").to_string_lossy().into());
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sway_poses(opts: &Options, lamp_mm: V3, views: usize, half_span_mm: f64) -> Vec<(V3, V3)> {
        let denom = (views - 1).max(1) as f64;
        (0..views)
            .map(|i| eye_and_lamp(opts, lamp_mm, half_span_mm * (2.0 * i as f64 / denom - 1.0), 0.0))
            .collect()
    }

    fn sway_wide_poses(opts: &Options, lamp_mm: V3, views: usize) -> Vec<(V3, V3, usize)> {
        WIDE_MM
            .iter()
            .flat_map(|&h| [(-h, 0usize), (h, views - 1)])
            .map(|(head, rv)| {
                let (e, l) = eye_and_lamp(opts, lamp_mm, head, 0.0);
                (e, l, rv)
            })
            .collect()
    }

    fn tilt_poses(opts: &Options, lamp_mm: V3, views: usize, start_deg: f64, end_deg: f64) -> Vec<(V3, V3)> {
        let denom = (views - 1).max(1) as f64;
        (0..views)
            .map(|i| {
                let deg = start_deg + (end_deg - start_deg) * (i as f64 / denom);
                tilted_eye_and_lamp(opts, lamp_mm, deg.to_radians(), 0.0)
            })
            .collect()
    }

    fn tilt_wide_poses(opts: &Options, lamp_mm: V3, views: usize, start_deg: f64, end_deg: f64) -> Vec<(V3, V3, usize)> {
        let step = (end_deg - start_deg).signum();
        let mut out = Vec::new();
        for &d in &[3.0f64, 6.0, 10.0, 15.0] {
            let (e0, l0) = tilted_eye_and_lamp(opts, lamp_mm, (start_deg - step * d).to_radians(), 0.0);
            let (e1, l1) = tilted_eye_and_lamp(opts, lamp_mm, (end_deg + step * d).to_radians(), 0.0);
            if e0[2] > 5.0 && l0[2] > 5.0 {
                out.push((e0, l0, 0));
            }
            if e1[2] > 5.0 && l1[2] > 5.0 {
                out.push((e1, l1, views - 1));
            }
        }
        out
    }

    fn gt_stats(poses: &[(V3, V3)], x: f64, y: f64) -> (f64, f64, bool, f64) {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        let mut min_lamp_z = f64::INFINITY;
        for &(eye, lamp) in poses {
            let (gt, _) = scene::g_at(&scene::Scene { light_mm: lamp, camera_mm: eye, ..Default::default() }, x, y);
            lo = lo.min(gt);
            hi = hi.max(gt);
            min_lamp_z = min_lamp_z.min(lamp[2]).min(eye[2]);
        }
        let crosses_zero = lo < 0.0 && hi > 0.0;
        (lo, hi, crosses_zero, min_lamp_z)
    }

    fn svd_independent_views(poses: &[(V3, V3)], resp: &Responses, x: f64, y: f64) -> (usize, usize, Vec<f64>) {
        let rows = poses.len();
        let k = resp.options;
        let mut a = Vec::with_capacity(rows * k);
        for &(eye, lamp) in poses {
            let (gt, gr) = scene::g_at(&scene::Scene { light_mm: lamp, camera_mm: eye, ..Default::default() }, x, y);
            let mut row: Vec<f64> = (0..k).map(|j| lightfield::luma(resp.at(j, gt, gr))).collect();
            let mean = row.iter().sum::<f64>() / k as f64;
            for v in &mut row {
                *v = (*v - mean) * (1.0 - PLAIN_LEAK);
            }
            a.extend(row);
        }
        let sv = lightfield::singular_values(&a, rows, k);
        let rms: Vec<f64> = sv.iter().map(|s| s / (rows as f64).sqrt()).collect();
        let top = rms.first().copied().unwrap_or(0.0).max(1e-12);
        let n5 = rms.iter().filter(|&&s| s >= 0.05 * top).count();
        let n3 = rms.iter().filter(|&&s| s >= 0.03 * top).count();
        (n5, n3, rms)
    }

    /// Correlated target: builtin star at [0, 41, 0], scale 30 mm, spinning 180 deg around Y.
    fn make_star_targets(views: usize, grid: usize, preset: &DiscGeometryPreset, flat_bw: bool) -> Vec<Vec<f32>> {
        let samples: Vec<[f64; 16]> = (0..=64)
            .map(|i| {
                let rad = (i as f64 / 64.0 * 180.0).to_radians();
                let (s, c) = rad.sin_cos();
                [
                    c, 0.0, s, 0.0,
                    0.0, 1.0, 0.0, 0.0,
                    -s, 0.0, c, 0.0,
                    0.0, 0.0, 0.0, 1.0,
                ]
            })
            .collect();
        let style = if flat_bw {
            ink::Style { lines: 0.0, shading: 0.0, texture: 0.0 }
        } else {
            ink::Style::default()
        };
        let obj = SceneObject {
            id: "star".into(),
            path: model3d::BUILTIN_SAMPLE_PATH.into(),
            name: "star".into(),
            visible: true,
            position_mm: [0.0, OBJ_Y_SCENE, 0.0],
            rotation_deg: [0.0, 0.0, 0.0],
            scale_mm: [30.0; 3],
            position_id: "0".into(),
            style,
            motion: model3d::Motion { preset: "spinY".into(), matrix: vec![], samples, vars: vec![] },
            mode: model3d::Mode::Transform,
            parallax: 2.0,
            tilt_deg: 0.0,
        };
        let pos = Position {
            id: "0".into(),
            name: String::new(),
            phi_deg: 0.0,
            crop_deg: 90.0,
            crop_side: Default::default(),
            crop_offset_deg: 0.0,
            crops: None,
            neutral: None,
            order: vec![],
        };
        let opts = Options { grid, ..Default::default() };
        position_targets_n(&pos, obj.mode, tilt_of(&obj), &[&obj], LAMP_MM, &opts, preset, views).unwrap()
    }

    /// Uncorrelated target A: `views` low-correlation 5x5 binary block glyphs in a 28x28 mm box at (0, -41).
    /// Every frame lights exactly 6 of 25 tiles; any two frames share <= 2 tiles (pairwise corr <= 0.123).
    fn make_uncorrelated_glyphs(views: usize, grid: usize, preset: &DiscGeometryPreset) -> Vec<Vec<f32>> {
        let mut rng = SplitMix64(0x1234_5678_9ABC_DEF0);
        let mut patterns: Vec<u32> = Vec::with_capacity(views);
        for max_overlap in [2u32, 3, 4] {
            let mut tries = 0usize;
            while patterns.len() < views && tries < 200_000 {
                tries += 1;
                let mut mask = 0u32;
                while mask.count_ones() < 6 {
                    let bit = (rng.next_u64() % 25) as u32;
                    mask |= 1u32 << bit;
                }
                if patterns.iter().all(|&p| (p & mask).count_ones() <= max_overlap) {
                    patterns.push(mask);
                }
            }
            if patterns.len() >= views {
                break;
            }
        }
        let ro = preset.outer_radius_mm;
        let half = 14.0f64;
        patterns
            .iter()
            .map(|&mask| {
                let mut img = vec![0.0f32; grid * grid];
                for cy in 0..grid {
                    let y = ((cy as f64 + 0.5) / grid as f64 - 0.5) * 2.0 * ro;
                    let ty = (y - (OBJ_Y_DISC - half)) / (2.0 * half);
                    if !(0.0..1.0).contains(&ty) {
                        continue;
                    }
                    let gy = (ty * 5.0).floor() as usize;
                    for cx in 0..grid {
                        let x = ((cx as f64 + 0.5) / grid as f64 - 0.5) * 2.0 * ro;
                        let tx = (x + half) / (2.0 * half);
                        if !(0.0..1.0).contains(&tx) {
                            continue;
                        }
                        let gx = (tx * 5.0).floor() as usize;
                        let bit = (gy.min(4) * 5 + gx.min(4)) as u32;
                        if (mask >> bit) & 1 != 0 {
                            let m = model3d::radial_mask(x, y, preset.inner_radius_mm, ro) as f32;
                            img[cy * grid + cx] = m;
                        }
                    }
                }
                img
            })
            .collect()
    }

    /// 24 recognizable bold 7x7 shapes & digits/letters, ordered so neighbours are dissimilar.
    const SHAPES_7X7: [&[&str; 7]; 24] = [
        // 0: big '+'
        &["..###..", "..###..", "#######", "#######", "#######", "..###..", "..###.."],
        // 1: '0' / hollow box
        &["#######", "#.....#", "#.....#", "#.....#", "#.....#", "#.....#", "#######"],
        // 2: '1'
        &["...##..", "..###..", ".####..", "...##..", "...##..", "...##..", ".######"],
        // 3: 'X'
        &["##...##", ".##.##.", "..###..", "...#...", "..###..", ".##.##.", "##...##"],
        // 4: top-left / bottom-right blocks
        &["####...", "####...", "####...", ".......", "...####", "...####", "...####"],
        // 5: '2'
        &[".#####.", "##...##", ".....##", "..####.", ".##....", "##.....", "#######"],
        // 6: vertical bars
        &["##...##", "##...##", "##...##", "##...##", "##...##", "##...##", "##...##"],
        // 7: '3'
        &[".#####.", ".....##", ".....##", "..####.", ".....##", ".....##", ".#####."],
        // 8: up triangle
        &["...#...", "..###..", "..###..", ".#####.", ".#####.", "#######", "#######"],
        // 9: '4'
        &["##...##", "##...##", "##...##", "#######", ".....##", ".....##", ".....##"],
        // 10: horizontal bars
        &["#######", "#######", ".......", ".......", ".......", "#######", "#######"],
        // 11: '7'
        &["#######", ".....##", "....##.", "...##..", "..##...", "..##...", "..##..."],
        // 12: diamond
        &["...#...", "..###..", ".#####.", "#######", ".#####.", "..###..", "...#..."],
        // 13: 'L'
        &["##.....", "##.....", "##.....", "##.....", "##.....", "#######", "#######"],
        // 14: bottom-left / top-right blocks
        &["...####", "...####", "...####", ".......", "####...", "####...", "####..."],
        // 15: 'T'
        &["#######", "#######", "..###..", "..###..", "..###..", "..###..", "..###.."],
        // 16: down triangle
        &["#######", "#######", ".#####.", ".#####.", "..###..", "..###..", "...#..."],
        // 17: 'A'
        &["..###..", ".##.##.", "##...##", "#######", "##...##", "##...##", "##...##"],
        // 18: 4 corners
        &["##...##", "##...##", ".......", ".......", ".......", "##...##", "##...##"],
        // 19: 'Z'
        &["#######", "....##.", "...##..", "..##...", ".##....", "##.....", "#######"],
        // 20: center square
        &[".......", ".#####.", ".#####.", ".#####.", ".#####.", ".#####.", "......."],
        // 21: 'U'
        &["##...##", "##...##", "##...##", "##...##", "##...##", "##...##", ".#####."],
        // 22: diagonal slash
        &[".....##", "....###", "...###.", "..###..", ".###...", "###....", "##....."],
        // 23: 'H'
        &["##...##", "##...##", "##...##", "#######", "##...##", "##...##", "##...##"],
    ];

    fn make_shapes_digits(views: usize, grid: usize, preset: &DiscGeometryPreset) -> Vec<Vec<f32>> {
        let ro = preset.outer_radius_mm;
        let half = 14.0f64;
        (0..views)
            .map(|k| {
                let pat = SHAPES_7X7[k % SHAPES_7X7.len()];
                let mut img = vec![0.0f32; grid * grid];
                for cy in 0..grid {
                    let y = ((cy as f64 + 0.5) / grid as f64 - 0.5) * 2.0 * ro;
                    let ty = (y - (OBJ_Y_DISC - half)) / (2.0 * half);
                    if !(0.0..1.0).contains(&ty) {
                        continue;
                    }
                    let gy = ((ty * 7.0).floor() as usize).min(6);
                    let row = pat[gy].as_bytes();
                    for cx in 0..grid {
                        let x = ((cx as f64 + 0.5) / grid as f64 - 0.5) * 2.0 * ro;
                        let tx = (x + half) / (2.0 * half);
                        if !(0.0..1.0).contains(&tx) {
                            continue;
                        }
                        let gx = ((tx * 7.0).floor() as usize).min(6);
                        if row[gx] == b'#' {
                            let m = model3d::radial_mask(x, y, preset.inner_radius_mm, ro) as f32;
                            img[cy * grid + cx] = m;
                        }
                    }
                }
                img
            })
            .collect()
    }

    fn make_flipbook(keyframes: &[Vec<f32>], views: usize) -> Vec<Vec<f32>> {
        let m = keyframes.len();
        (0..views).map(|v| keyframes[(v * m) / views].clone()).collect()
    }

    fn patch_to_solved(patch: &Patch, grid: usize, cb: &Codebook, resp: &Responses, bg_override: Option<usize>) -> Solved {
        let k = resp.options;
        let plain = k - 1;
        let bg = bg_override
            .or_else(|| cb.entries.iter().position(|e| e.bytes.iter().all(|&b| b == design::BACKGROUND_BYTE)))
            .unwrap_or_else(|| {
                (0..plain)
                    .min_by(|&a, &b| {
                        let ea: f64 = resp.rgb[a].iter().map(|&p| lightfield::luma(p).powi(2)).sum();
                        let eb: f64 = resp.rgb[b].iter().map(|&p| lightfield::luma(p).powi(2)).sum();
                        ea.total_cmp(&eb)
                    })
                    .unwrap_or(0)
            });
        let mut weights = vec![0.0f32; grid * grid * k];
        for cell in weights.chunks_exact_mut(k) {
            cell[bg] = 1.0;
        }
        for (j, &c) in patch.cells.iter().enumerate() {
            let c = c as usize;
            weights[c * k..(c + 1) * k].copy_from_slice(&patch.weights[j * k..(j + 1) * k]);
        }
        Solved { grid, options: k, weights, error: 0.0 }
    }

    fn pearson(a: &[f64], b: &[f32], region: &[usize]) -> f64 {
        let n = region.len() as f64;
        if n < 2.0 {
            return 0.0;
        }
        let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for &c in region {
            let (x, y) = (a[c], b[c] as f64);
            sa += x;
            sb += y;
            saa += x * x;
            sbb += y * y;
            sab += x * y;
        }
        let va = (saa - sa * sa / n).max(0.0);
        let vb = (sbb - sb * sb / n).max(0.0);
        let denom = (va * vb).sqrt();
        if denom < 1e-12 {
            0.0
        } else {
            (sab - sa * sb / n) / denom
        }
    }

    #[derive(Clone, Debug)]
    struct Metrics {
        mean_own: f64,
        mean_ghost: f64,
        mean_contrast: f64,
        distinct: usize,
        total: usize,
        gt_lo: f64,
        gt_hi: f64,
        crosses_zero: bool,
        max_bg_luma: f64,
    }

    fn eval_and_save(
        name: &str,
        solved: &Solved,
        resp: &Responses,
        preset: &DiscGeometryPreset,
        poses: &[(V3, V3)],
        targets: &[Vec<f32>],
        save_png: Option<&str>,
    ) -> Metrics {
        let views = poses.len();
        let grid = solved.grid;
        let region: Vec<usize> = (0..grid * grid).filter(|&c| targets.iter().any(|t| t[c] > LIT)).collect();
        let (gt_lo, gt_hi, crosses_zero, _) = gt_stats(poses, 0.0, OBJ_Y_DISC);

        // Background cell sample just outside the object at (18, -41) mm (never lit).
        let ro = preset.outer_radius_mm;
        let bg_cx = (((18.0 / (2.0 * ro) + 0.5) * grid as f64) as usize).min(grid - 1);
        let bg_cy = (((OBJ_Y_DISC / (2.0 * ro) + 0.5) * grid as f64) as usize).min(grid - 1);
        let bg_idx = bg_cy * grid + bg_cx;

        let renders: Vec<Vec<[f32; 3]>> = poses
            .iter()
            .map(|&(eye, lamp)| lightfield::render_float(solved, resp, preset, eye, lamp, grid, true))
            .collect();

        let mut own_sum = 0.0;
        let mut ghost_sum = 0.0;
        let mut contrast_sum = 0.0;
        let mut contrast_cnt = 0usize;
        let mut distinct = 0usize;
        let mut max_bg_luma = 0.0f64;

        for k in 0..views {
            let lum: Vec<f64> = renders[k].iter().map(|&p| lightfield::luma(p)).collect();
            max_bg_luma = max_bg_luma.max(lum[bg_idx]);
            let own = pearson(&lum, &targets[k], &region);
            let mut ghost = f64::NEG_INFINITY;
            for j in 0..views {
                if j.abs_diff(k) >= 3 {
                    ghost = ghost.max(pearson(&lum, &targets[j], &region));
                }
            }
            if !ghost.is_finite() {
                ghost = 0.0;
            }
            own_sum += own;
            ghost_sum += ghost;
            if own >= ghost + 0.1 {
                distinct += 1;
            }

            let (mut lit_s, mut lit_n, mut dark_s, mut dark_n) = (0.0, 0usize, 0.0, 0usize);
            for &c in &region {
                if targets[k][c] > 0.1 {
                    lit_s += lum[c];
                    lit_n += 1;
                } else {
                    dark_s += lum[c];
                    dark_n += 1;
                }
            }
            if lit_n > 0 && dark_n > 0 {
                let c_ratio = (lit_s / lit_n as f64) / (dark_s / dark_n as f64).max(1e-6);
                contrast_sum += c_ratio;
                contrast_cnt += 1;
            }
        }

        if let Some(filename) = save_png {
            let n_samp = views.min(10);
            let crop = 96usize;
            let cx0 = (grid / 2).saturating_sub(crop / 2);
            let cy_center = ((OBJ_Y_DISC / (2.0 * ro) + 0.5) * grid as f64).round() as usize;
            let cy0 = cy_center.saturating_sub(crop / 2).min(grid - crop);
            let gap = 4usize;
            let mut img = image::RgbImage::from_pixel((n_samp * crop) as u32, (crop * 2 + gap) as u32, image::Rgb([40, 40, 40]));
            let enc = |v: f64| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8;
            for s in 0..n_samp {
                let k = if n_samp <= 1 { 0 } else { (s * (views - 1) + (n_samp - 1) / 2) / (n_samp - 1) };
                for y in 0..crop {
                    for x in 0..crop {
                        let idx = (cy0 + y) * grid + (cx0 + x);
                        let tv = enc(targets[k][idx] as f64);
                        img.put_pixel((s * crop + x) as u32, y as u32, image::Rgb([tv, tv, tv]));
                        let rv = renders[k][idx];
                        // Fixed gain 1.5 so brightness & contrast stay comparable across configs.
                        let rgb = [enc(rv[0] as f64 * 1.5), enc(rv[1] as f64 * 1.5), enc(rv[2] as f64 * 1.5)];
                        img.put_pixel((s * crop + x) as u32, (crop + gap + y) as u32, image::Rgb(rgb));
                    }
                }
            }
            let path = format!("{}/{filename}", out_dir());
            img.save(&path).unwrap();
            eprintln!("  saved contact sheet: {path}");
        }

        let m = Metrics {
            mean_own: own_sum / views as f64,
            mean_ghost: ghost_sum / views as f64,
            mean_contrast: if contrast_cnt > 0 { contrast_sum / contrast_cnt as f64 } else { 0.0 },
            distinct,
            total: views,
            gt_lo,
            gt_hi,
            crosses_zero,
            max_bg_luma,
        };
        println!(
            "{name:<48} | gt [{:+.2},{:+.2}]{} | own {:+.3} | ghost {:+.3} | ctr {:5.2}x | distinct {:2}/{:2} | bg_max {:.3}",
            m.gt_lo,
            m.gt_hi,
            if m.crosses_zero { "X" } else { " " },
            m.mean_own,
            m.mean_ghost,
            m.mean_contrast,
            m.distinct,
            m.total,
            m.max_bg_luma,
        );
        m
    }

    fn ideal_metrics(name: &str, targets: &[Vec<f32>]) {
        let views = targets.len();
        let grid = (targets[0].len() as f64).sqrt() as usize;
        let region: Vec<usize> = (0..grid * grid).filter(|&c| targets.iter().any(|t| t[c] > LIT)).collect();
        let (mut ghost_sum, mut distinct) = (0.0, 0usize);
        for k in 0..views {
            let lum: Vec<f64> = targets[k].iter().map(|&v| v as f64).collect();
            let own = pearson(&lum, &targets[k], &region);
            let mut ghost = f64::NEG_INFINITY;
            for j in 0..views {
                if j.abs_diff(k) >= 3 {
                    ghost = ghost.max(pearson(&lum, &targets[j], &region));
                }
            }
            ghost_sum += ghost;
            if own >= ghost + 0.1 {
                distinct += 1;
            }
        }
        println!("IDEAL TARGET {name:<35} | region {:5} cells | own +1.000 | tgt_ghost {:+.3} | ideal_distinct {:2}/{:2}", region.len(), ghost_sum / views as f64, distinct, views);
    }

    /// Diagnostic: inspect codebook spectral coverage, g_t sweeps, SVD independent views, and ideal target bounds.
    #[test]
    #[ignore]
    fn exp0_physics_probe() {
        let shared = design::shared_codebook(0.1333);
        let preset = reference_preset();
        let (cb, resp) = (&shared.0, &shared.1);
        println!("=== Codebook entries ({} + 1 plain = {}) ===", cb.entries.len(), resp.options);
        for (j, e) in cb.entries.iter().enumerate() {
            let (mut pk_y, mut pk_gt) = (0.0f64, 0.0f64);
            let mut ys = Vec::new();
            for it in 0..200 {
                let gt = it as f64 / 199.0 * 1.5;
                let y = lightfield::luma(resp.at(j, gt, 0.0));
                ys.push((gt, y));
                if y > pk_y {
                    pk_y = y;
                    pk_gt = gt;
                }
            }
            let half = 0.5 * pk_y;
            let lo = ys.iter().find(|&&(_, y)| y >= half).map(|&(g, _)| g).unwrap_or(0.0);
            let hi = ys.iter().rev().find(|&&(_, y)| y >= half).map(|&(g, _)| g).unwrap_or(0.0);
            println!("  opt {:2} | f0={:.3} pur={:.2} | pk_gt={:.3} pk_Y={:.2} | FWHM [{:.3}..{:.3}] (w={:.3})", j, e.f0, e.purity, pk_gt, pk_y, lo, hi, hi - lo);
        }
        let plain = resp.options - 1;
        let pl_025 = lightfield::luma(resp.at(plain, 0.25, 0.0));
        let pl_050 = lightfield::luma(resp.at(plain, 0.50, 0.0));
        let pl_075 = lightfield::luma(resp.at(plain, 0.75, 0.0));
        let pl_100 = lightfield::luma(resp.at(plain, 1.00, 0.0));
        println!("  plain  | Y(0.25)={pl_025:.3} Y(0.50)={pl_050:.3} Y(0.75)={pl_075:.3} Y(1.00)={pl_100:.3}");

        println!("\n=== Sweep g_t ranges & SVD independent views at (0,-41) [12 o'clock] and (0,+41) [6 o'clock] ===");
        let opts = Options { grid: GRID, ..Default::default() };
        let configs: Vec<(&str, Vec<(V3, V3)>)> = vec![
            ("sway +-40mm lamp [150,0,400]", sway_poses(&opts, LAMP_MM, 40, 40.0)),
            ("tilt +-5  lamp [150,0,400]", tilt_poses(&opts, LAMP_MM, 40, -5.0, 5.0)),
            ("tilt +-10 lamp [150,0,400]", tilt_poses(&opts, LAMP_MM, 40, -10.0, 10.0)),
            ("tilt +-20 lamp [150,0,400]", tilt_poses(&opts, LAMP_MM, 40, -20.0, 20.0)),
            ("tilt +-30 lamp [150,0,400]", tilt_poses(&opts, LAMP_MM, 40, -30.0, 30.0)),
            ("tilt +-10 lamp [220,0,400]", tilt_poses(&opts, [220.0, 0.0, 400.0], 40, -10.0, 10.0)),
            ("tilt +-10 lamp [250,0,400]", tilt_poses(&opts, [250.0, 0.0, 400.0], 40, -10.0, 10.0)),
            ("tilt +-15 lamp [320,0,400]", tilt_poses(&opts, [320.0, 0.0, 400.0], 40, -15.0, 15.0)),
            ("tilt +-20 lamp [380,0,380]", tilt_poses(&opts, [380.0, 0.0, 380.0], 40, -20.0, 20.0)),
            ("tilt +-20 lamp [420,0,350]", tilt_poses(&opts, [420.0, 0.0, 350.0], 40, -20.0, 20.0)),
            ("tilt +-30 lamp [550,0,350]", tilt_poses(&opts, [550.0, 0.0, 350.0], 40, -30.0, 30.0)),
            ("tilt 0->30  lamp [0,0,400]", tilt_poses(&opts, [0.0, 0.0, 400.0], 40, 0.0, 30.0)),
            ("tilt 5->30  lamp [0,0,400]", tilt_poses(&opts, [0.0, 0.0, 400.0], 40, 5.0, 30.0)),
            ("tilt 6->24  lamp [0,0,400]", tilt_poses(&opts, [0.0, 0.0, 400.0], 40, 6.0, 24.0)),
            ("tilt 6->35  lamp [0,0,400]", tilt_poses(&opts, [0.0, 0.0, 400.0], 40, 6.0, 35.0)),
            ("tilt -25->+5 lamp [150,0,400]", tilt_poses(&opts, LAMP_MM, 40, -25.0, 5.0)),
            ("tilt -30->0  lamp [150,0,400]", tilt_poses(&opts, LAMP_MM, 40, -30.0, 0.0)),
        ];
        for (label, poses) in &configs {
            let (lo12, hi12, x12, minz) = gt_stats(poses, 0.0, -41.0);
            let (lo6, hi6, x6, _) = gt_stats(poses, 0.0, 41.0);
            let (n5, n3, _) = svd_independent_views(poses, resp, 0.0, -41.0);
            println!(
                "  {label:<32} | 12h [{lo12:+.2},{hi12:+.2}]{} | 6h [{lo6:+.2},{hi6:+.2}]{} | minZ={minz:5.0} | SVD n5={n5:2} n3={n3:2}",
                if x12 { "X" } else { " " },
                if x6 { "X" } else { " " },
            );
        }

        println!("\n=== Ideal target self-similarity ===");
        ideal_metrics("star-ink 24v", &make_star_targets(24, GRID, &preset, false));
        ideal_metrics("star-flat 24v", &make_star_targets(24, GRID, &preset, true));
        ideal_metrics("star-ink 40v", &make_star_targets(40, GRID, &preset, false));
        ideal_metrics("star-ink 60v", &make_star_targets(60, GRID, &preset, false));
        ideal_metrics("glyphs 24v", &make_uncorrelated_glyphs(24, GRID, &preset));
        ideal_metrics("glyphs 40v", &make_uncorrelated_glyphs(40, GRID, &preset));
        ideal_metrics("glyphs 60v", &make_uncorrelated_glyphs(60, GRID, &preset));
        ideal_metrics("shapes/digits 24v", &make_shapes_digits(24, GRID, &preset));
    }

    /// Experiments 1, 2, 3 (and wide=0 on sway): baseline vs BLEND off vs chroma 0 vs combined on sway.
    #[test]
    #[ignore]
    fn exp1_to_3_ablation() {
        let shared = design::shared_codebook(0.1333);
        let preset = reference_preset();
        let (cb, resp) = (&shared.0, &shared.1);
        let base_opts = Options { grid: GRID, chroma_weight: 1.0, exposure: 0.6, wide: 0.3, ..Default::default() };
        let poses = sway_poses(&base_opts, LAMP_MM, 24, HEAD_HALF_SPAN_MM);
        let wide_p = sway_wide_poses(&base_opts, LAMP_MM, 24);

        let datasets: Vec<(&str, Vec<Vec<f32>>)> = vec![
            ("star", make_star_targets(24, GRID, &preset, false)),
            ("glyphs", make_uncorrelated_glyphs(24, GRID, &preset)),
            ("shapes", make_shapes_digits(24, GRID, &preset)),
        ];

        println!("=== Experiments 1-3: Single-factor ablation on baseline sway (+-40mm, 24v) ===");
        for (dname, targets) in &datasets {
            println!("\n--- Dataset: {dname} ---");
            let runs: Vec<(&str, bool, f64, f64, Option<String>)> = vec![
                ("1. baseline (BLEND=on, chr=1, wide=0.3)", true, 1.0, 0.3, Some(format!("exp1_baseline_{dname}.png"))),
                ("2. BLEND=off (chr=1, wide=0.3)", false, 1.0, 0.3, Some(format!("exp2_noblend_{dname}.png"))),
                ("3a. chr=0 (BLEND=on, wide=0.3)", true, 0.0, 0.3, None),
                ("3b. BLEND=off + chr=0 (wide=0.3)", false, 0.0, 0.3, Some(format!("exp3_noblend_chr0_{dname}.png"))),
                ("3c. BLEND=off + chr=0 + wide=0", false, 0.0, 0.0, None),
            ];
            for (label, blend, chroma, wide, png) in runs {
                let opts = Options { chroma_weight: chroma, wide, ..base_opts.clone() };
                let wp = if wide > 0.0 { &wide_p[..] } else { &[] };
                let patch = solve_patch(&poses, wp, targets, blend, &opts, resp, &preset);
                let solved = patch_to_solved(&patch, GRID, cb, resp, None);
                eval_and_save(label, &solved, resp, &preset, &poses, targets, png.as_deref());
            }
        }
    }

    /// Experiment 4: Viewing motion = disc tilt (symmetric & one-sided) across lamp positions.
    #[test]
    #[ignore]
    fn exp4_tilt_and_lamp() {
        let shared = design::shared_codebook(0.1333);
        let preset = reference_preset();
        let (cb, resp) = (&shared.0, &shared.1);
        let opts_clean = Options { grid: GRID, chroma_weight: 0.0, exposure: 0.6, wide: 0.0, ..Default::default() };
        let opts_base = Options { grid: GRID, chroma_weight: 1.0, exposure: 0.6, wide: 0.3, ..Default::default() };

        let datasets: Vec<(&str, Vec<Vec<f32>>)> = vec![
            ("star", make_star_targets(24, GRID, &preset, false)),
            ("glyphs", make_uncorrelated_glyphs(24, GRID, &preset)),
            ("shapes", make_shapes_digits(24, GRID, &preset)),
        ];

        // (label, start_deg, end_deg, lamp, blend, opts, png_tag)
        let configs: Vec<(&str, f64, f64, V3, bool, &Options, Option<&str>)> = vec![
            // Isolated tilt with baseline settings (BLEND=on, chr=1, wide=0.3)
            ("4a. tilt +-10 base-opts lamp[150,0,400]", -10.0, 10.0, LAMP_MM, true, &opts_base, None),
            ("4b. tilt +-20 base-opts lamp[420,0,350]", -20.0, 20.0, [420.0, 0.0, 350.0], true, &opts_base, None),
            // Symmetric tilt with default lamp [150,0,400] (clean: BLEND=off, chr=0, wide=0)
            ("4c. tilt +-5  lamp[150,0,400]", -5.0, 5.0, LAMP_MM, false, &opts_clean, None),
            ("4d. tilt +-10 lamp[150,0,400]", -10.0, 10.0, LAMP_MM, false, &opts_clean, Some("exp4_tilt10_lamp150")),
            ("4e. tilt +-20 lamp[150,0,400] (crosses 0)", -20.0, 20.0, LAMP_MM, false, &opts_clean, Some("exp4_tilt20_lamp150_cross0")),
            ("4f. tilt +-30 lamp[150,0,400] (crosses 0)", -30.0, 30.0, LAMP_MM, false, &opts_clean, None),
            // Symmetric tilt with offset lamps keeping g_t > 0
            ("4g. tilt +-10 lamp[220,0,400]", -10.0, 10.0, [220.0, 0.0, 400.0], false, &opts_clean, Some("exp4_tilt10_lamp220")),
            ("4h. tilt +-10 lamp[250,0,400]", -10.0, 10.0, [250.0, 0.0, 400.0], false, &opts_clean, None),
            ("4i. tilt +-15 lamp[320,0,400]", -15.0, 15.0, [320.0, 0.0, 400.0], false, &opts_clean, Some("exp4_tilt15_lamp320")),
            ("4j. tilt +-20 lamp[380,0,380]", -20.0, 20.0, [380.0, 0.0, 380.0], false, &opts_clean, None),
            ("4k. tilt +-20 lamp[420,0,350]", -20.0, 20.0, [420.0, 0.0, 350.0], false, &opts_clean, Some("exp4_tilt20_lamp420")),
            ("4l. tilt +-30 lamp[550,0,350]", -30.0, 30.0, [550.0, 0.0, 350.0], false, &opts_clean, Some("exp4_tilt30_lamp550")),
            // One-sided sweeps
            ("4m. tilt 0->30  lamp[0,0,400]", 0.0, 30.0, [0.0, 0.0, 400.0], false, &opts_clean, Some("exp4_tilt0_30_lamp0")),
            ("4n. tilt 5->30  lamp[0,0,400]", 5.0, 30.0, [0.0, 0.0, 400.0], false, &opts_clean, Some("exp4_tilt5_30_lamp0")),
            ("4o. tilt 6->24  lamp[0,0,400]", 6.0, 24.0, [0.0, 0.0, 400.0], false, &opts_clean, Some("exp4_tilt6_24_lamp0")),
            ("4p. tilt 6->35  lamp[0,0,400]", 6.0, 35.0, [0.0, 0.0, 400.0], false, &opts_clean, None),
            ("4q. tilt -25->+5 lamp[150,0,400]", -25.0, 5.0, LAMP_MM, false, &opts_clean, None),
            ("4r. tilt -30->0  lamp[150,0,400]", -30.0, 0.0, LAMP_MM, false, &opts_clean, None),
        ];

        println!("=== Experiment 4: Disc tilt + lamp positions (24 views, grid 256) ===");
        for (dname, targets) in &datasets {
            println!("\n--- Dataset: {dname} ---");
            for &(label, s_deg, e_deg, lamp, blend, opts, png_tag) in &configs {
                let poses = tilt_poses(opts, lamp, 24, s_deg, e_deg);
                let wp = if opts.wide > 0.0 { sway_wide_poses(opts, lamp, 24) } else { vec![] };
                let patch = solve_patch(&poses, &wp, targets, blend, opts, resp, &preset);
                let solved = patch_to_solved(&patch, GRID, cb, resp, None);
                let png = png_tag.map(|t| format!("{t}_{dname}.png"));
                eval_and_save(label, &solved, resp, &preset, &poses, targets, png.as_deref());
            }
        }
    }

    /// Experiment 5: Number of views 24 vs 40 vs 60 (continuous star, N-distinct glyphs, and M-keyframe flipbook).
    #[test]
    #[ignore]
    fn exp5_views() {
        let shared = design::shared_codebook(0.1333);
        let preset = reference_preset();
        let (cb, resp) = (&shared.0, &shared.1);
        let opts = Options { grid: GRID, chroma_weight: 0.0, exposure: 0.6, wide: 0.0, ..Default::default() };

        // Test on two top one-sided & symmetric tilt motions covering the full usable g_t band:
        // (A) 6°->24° overhead lamp [0,0,400] (g_t ~ [0.21, 0.81])
        // (B) ±15° lamp [320,0,400] (g_t ~ [0.14, 1.04])
        // (C) ±20° lamp [420,0,350] (g_t ~ [0.18, 1.24])
        let motions: Vec<(&str, f64, f64, V3)> = vec![
            ("tilt6_24_lamp0", 6.0, 24.0, [0.0, 0.0, 400.0]),
            ("tilt15_lamp320", -15.0, 15.0, [320.0, 0.0, 400.0]),
            ("tilt20_lamp420", -20.0, 20.0, [420.0, 0.0, 350.0]),
        ];

        println!("=== Experiment 5: View count 24 vs 40 vs 60 ===");
        for (mname, s_deg, e_deg, lamp) in &motions {
            println!("\n--- Motion: {mname} ({s_deg}..{e_deg} deg, lamp {lamp:?}) ---");
            for &views in &[24usize, 40, 60] {
                let poses = tilt_poses(&opts, *lamp, views, *s_deg, *e_deg);
                let star_t = make_star_targets(views, GRID, &preset, false);
                let patch = solve_patch(&poses, &[], &star_t, false, &opts, resp, &preset);
                let solved = patch_to_solved(&patch, GRID, cb, resp, None);
                let png = if *mname == "tilt15_lamp320" { Some(format!("exp5_{mname}_{views}v_star.png")) } else { None };
                eval_and_save(&format!("star   {views:2}v ({mname})"), &solved, resp, &preset, &poses, &star_t, png.as_deref());

                let glyphs_t = make_uncorrelated_glyphs(views, GRID, &preset);
                let patch = solve_patch(&poses, &[], &glyphs_t, false, &opts, resp, &preset);
                let solved = patch_to_solved(&patch, GRID, cb, resp, None);
                let png = if *mname == "tilt15_lamp320" { Some(format!("exp5_{mname}_{views}v_glyphs.png")) } else { None };
                eval_and_save(&format!("glyphs {views:2}v ({mname})"), &solved, resp, &preset, &poses, &glyphs_t, png.as_deref());
            }
        }

        // Flipbook capacity test: M distinct bold shapes/glyphs mapped onto views = 24, 40, 60
        // Evaluate at the M keyframe centers (one pose per keyframe) to see how many discrete frames are cleanly separable!
        println!("\n--- Flipbook capacity: M discrete keyframes (glyphs) on tilt 6->30 lamp [0,0,400] ---");
        let keyframes_all = make_uncorrelated_glyphs(24, GRID, &preset);
        for &m in &[6usize, 8, 10, 12, 16, 20, 24] {
            let kf = &keyframes_all[..m];
            for &views in &[m, 40, 60] {
                if views < m {
                    continue;
                }
                let poses = tilt_poses(&opts, [0.0, 0.0, 400.0], views, 6.0, 30.0);
                let targets = make_flipbook(kf, views);
                let patch = solve_patch(&poses, &[], &targets, false, &opts, resp, &preset);
                let solved = patch_to_solved(&patch, GRID, cb, resp, None);
                // Evaluate at the M keyframe centre poses, with |j-k| >= 1 (adjacent frames must be distinct!).
                let kf_poses: Vec<(V3, V3)> = (0..m)
                    .map(|k| {
                        let deg = 6.0 + (30.0 - 6.0) * ((k as f64 + 0.5) / m as f64);
                        tilted_eye_and_lamp(&opts, [0.0, 0.0, 400.0], deg.to_radians(), 0.0)
                    })
                    .collect();
                let region: Vec<usize> = (0..GRID * GRID).filter(|&c| kf.iter().any(|t| t[c] > LIT)).collect();
                let (mut own_s, mut gh_adj_s, mut gh_3_s, mut ctr_s, mut dist_adj, mut dist_3) = (0.0, 0.0, 0.0, 0.0, 0usize, 0usize);
                for k in 0..m {
                    let px = lightfield::render_float(&solved, resp, &preset, kf_poses[k].0, kf_poses[k].1, GRID, true);
                    let lum: Vec<f64> = px.iter().map(|&p| lightfield::luma(p)).collect();
                    let own = pearson(&lum, &kf[k], &region);
                    let gh_adj = (0..m).filter(|&j| j != k).map(|j| pearson(&lum, &kf[j], &region)).fold(f64::NEG_INFINITY, f64::max);
                    let gh_3 = (0..m).filter(|&j| j.abs_diff(k) >= 3).map(|j| pearson(&lum, &kf[j], &region)).fold(f64::NEG_INFINITY, f64::max);
                    own_s += own;
                    gh_adj_s += gh_adj;
                    if gh_3.is_finite() { gh_3_s += gh_3; }
                    if own >= gh_adj + 0.1 { dist_adj += 1; }
                    if !gh_3.is_finite() || own >= gh_3 + 0.1 { dist_3 += 1; }
                    let (mut ls, mut ln, mut ds, mut dn) = (0.0, 0, 0.0, 0);
                    for &c in &region {
                        if kf[k][c] > 0.1 { ls += lum[c]; ln += 1; } else { ds += lum[c]; dn += 1; }
                    }
                    ctr_s += (ls / ln as f64) / (ds / dn as f64).max(1e-6);
                }
                println!(
                    "  flipbook M={m:2} solved@{views:2}v | own {:+.3} | max_other(j!=k) {:+.3} (dist {:2}/{:2}) | ghost(|j-k|>=3) {:+.3} (dist {:2}/{:2}) | ctr {:4.2}x",
                    own_s / m as f64,
                    gh_adj_s / m as f64,
                    dist_adj,
                    m,
                    gh_3_s / m as f64,
                    dist_3,
                    m,
                    ctr_s / m as f64,
                );
            }
        }
    }

    /// Build a custom codebook and compute its Responses normalized to the EXACT same physical
    /// luminance peak as `shared_codebook(bit_um)` (by prepending the swatch codebook as reference
    /// entries, then slicing them off when `keep_swatch` is false).
    fn build_range_codebook(
        n: usize,
        f_low: f64,
        f_high: f64,
        bit_um: f64,
        iters: usize,
        seed: u64,
        keep_swatch: bool,
        with_periodic: bool,
    ) -> (Codebook, Responses) {
        let swatch = design::swatch_codebook(bit_um);
        let n_swatch = swatch.entries.len();
        let centres: Vec<f64> = (0..n)
            .map(|k| f_low * (f_high / f_low).powf(k as f64 / (n - 1).max(1) as f64))
            .collect();
        let sa_entries: Vec<codebook::Entry> = centres
            .into_par_iter()
            .enumerate()
            .map(|(k, f0)| codebook::search(f0, bit_um, iters, seed ^ (k as u64).wrapping_mul(0x9E37_79B9)))
            .collect();
        let mut all_entries = swatch.entries.clone();
        all_entries.extend(sa_entries);
        if with_periodic {
            all_entries.extend(codebook::periodic_entries(bit_um));
        }
        let full_cb = Codebook { bit_um, entries: all_entries };
        let full_resp = design::responses_with_ref_peak(&full_cb, n_swatch, bit_um, design::DEFAULT_SOURCE_BLUR, design::DEFAULT_SIGMA_R);
        if keep_swatch {
            return (full_cb, full_resp);
        }
        // Strip the reference swatch entries while keeping identical luminance normalization + plain last.
        let entries = full_cb.entries[n_swatch..].to_vec();
        let rgb = full_resp.rgb[n_swatch..].to_vec();
        let options = rgb.len();
        (
            Codebook { bit_um, entries },
            Responses { options, gt_bins: full_resp.gt_bins, gr_bins: full_resp.gr_bins, rgb },
        )
    }

    /// Experiment 6: Codebook size & frequency range (all normalized to the identical swatch luminance scale).
    #[test]
    #[ignore]
    fn exp6_codebook() {
        let shared = design::shared_codebook(0.1333);
        let preset = reference_preset();
        let opts = Options { grid: GRID, chroma_weight: 0.0, exposure: 0.6, wide: 0.0, ..Default::default() };
        let poses = tilt_poses(&opts, [0.0, 0.0, 400.0], 24, 6.5, 22.5);
        let poses_sym = tilt_poses(&opts, [225.0, 0.0, 400.0], 24, -8.5, 8.5);

        let t0 = std::time::Instant::now();
        let per_only = build_range_codebook(0, 0.42, 1.25, 0.1333, 0, 0, false, true);
        let sa24 = build_range_codebook(24, 0.42, 1.25, 0.1333, 15_000, 0x5EED_0024, false, false);
        let sa24_ext = build_range_codebook(24, 0.42, 1.25, 0.1333, 15_000, 0x5EED_0024, false, true);
        let sa36 = build_range_codebook(36, 0.42, 1.25, 0.1333, 15_000, 0x5EED_0036, false, false);
        let sa36_ext = build_range_codebook(36, 0.42, 1.25, 0.1333, 15_000, 0x5EED_0036, false, true);
        let wide36_ext = build_range_codebook(36, 0.25, 1.60, 0.1333, 15_000, 0x5EED_0099, false, true);
        println!("Built custom codebooks (15k SA iters, matched scale) in {:?}", t0.elapsed());

        let books: Vec<(&str, &Codebook, &Responses)> = vec![
            ("shared (12 SA 0.42..1.12 + 15 per = 28)", &shared.0, &shared.1),
            ("periodic-only (15 per = 16)", &per_only.0, &per_only.1),
            ("SA-24 (0.42..1.25 = 25)", &sa24.0, &sa24.1),
            ("SA-24 + 15 per (40)", &sa24_ext.0, &sa24_ext.1),
            ("SA-36 (0.42..1.25 = 37)", &sa36.0, &sa36.1),
            ("SA-36 + 15 per (52)", &sa36_ext.0, &sa36_ext.1),
            ("SA-36 wide (0.25..1.60) + 15 per (52)", &wide36_ext.0, &wide36_ext.1),
        ];

        for (bname, cb, resp) in &books {
            let avg_pur = cb.entries.iter().map(|e| e.purity).sum::<f64>() / cb.entries.len() as f64;
            let (n5, n3, _) = svd_independent_views(&poses, resp, 0.0, OBJ_Y_DISC);
            println!("  codebook {bname:<42} | avg_purity={avg_pur:.3} | SVD n5={n5:2} n3={n3:2}");
        }

        let datasets: Vec<(&str, Vec<Vec<f32>>)> = vec![
            ("star", make_star_targets(24, GRID, &preset, false)),
            ("glyphs", make_uncorrelated_glyphs(24, GRID, &preset)),
        ];

        println!("\n=== Experiment 6: Codebook comparison on tilt 6.5->22.5 lamp [0,0,400] (24v, matched scale) ===");
        for (dname, targets) in &datasets {
            println!("\n--- Dataset: {dname} ---");
            for (bname, cb, resp) in &books {
                let patch = solve_patch(&poses, &[], targets, false, &opts, resp, &preset);
                let solved = patch_to_solved(&patch, GRID, cb, resp, None);
                let png = if bname.starts_with("SA-36 +") || bname.starts_with("SA-36 wide") {
                    Some(format!("exp6_{}_{dname}.png", if bname.contains("wide") { "wide36" } else { "sa36ext" }))
                } else {
                    None
                };
                eval_and_save(bname, &solved, resp, &preset, &poses, targets, png.as_deref());
            }
        }

        println!("\n=== Experiment 6b: Codebook comparison on symmetric tilt +-8.5 lamp [225,0,400] (24v, glyphs) ===");
        let glyphs = &datasets[1].1;
        for (bname, cb, resp) in &books {
            let patch = solve_patch(&poses_sym, &[], glyphs, false, &opts, resp, &preset);
            let solved = patch_to_solved(&patch, GRID, cb, resp, None);
            eval_and_save(bname, &solved, resp, &preset, &poses_sym, glyphs, None);
        }
    }

    /// Experiment 7: wide 0 vs 0.3, exposure 0.4/0.6/0.8/1.0, background entry, and combined winners.
    #[test]
    #[ignore]
    fn exp7_wide_exposure_and_combined() {
        let shared = design::shared_codebook(0.1333);
        let preset = reference_preset();
        let (cb, resp) = (&shared.0, &shared.1);
        let base_tilt_opts = Options { grid: GRID, chroma_weight: 0.0, exposure: 0.6, wide: 0.0, ..Default::default() };

        let poses_sym = tilt_poses(&base_tilt_opts, [320.0, 0.0, 400.0], 24, -15.0, 15.0);
        let wide_sway = sway_wide_poses(&base_tilt_opts, [320.0, 0.0, 400.0], 24);
        let wide_tilt = tilt_wide_poses(&base_tilt_opts, [320.0, 0.0, 400.0], 24, -15.0, 15.0);

        let datasets: Vec<(&str, Vec<Vec<f32>>)> = vec![
            ("star", make_star_targets(24, GRID, &preset, false)),
            ("glyphs", make_uncorrelated_glyphs(24, GRID, &preset)),
            ("shapes", make_shapes_digits(24, GRID, &preset)),
        ];

        println!("=== Experiment 7: wide & exposure on tilt +-15 lamp [320,0,400] (24v) ===");
        for (dname, targets) in &datasets {
            println!("\n--- Dataset: {dname} ---");
            for (label, wide, use_tilt_wide, exp, png) in [
                ("7a. wide=0.0, exp=0.6", 0.0, false, 0.6, None),
                ("7b. wide=0.3 (legacy sway-wide), exp=0.6", 0.3, false, 0.6, None),
                ("7c. wide=0.3 (tilt-wide), exp=0.6", 0.3, true, 0.6, None),
                ("7d. wide=0.0, exp=0.4", 0.0, false, 0.4, None),
                ("7e. wide=0.0, exp=0.8", 0.0, false, 0.8, Some(format!("exp7_exp08_{dname}.png"))),
                ("7f. wide=0.0, exp=1.0", 0.0, false, 1.0, None),
            ] {
                let opts = Options { wide, exposure: exp, ..base_tilt_opts.clone() };
                let wp: &[(V3, V3, usize)] = if wide == 0.0 {
                    &[]
                } else if use_tilt_wide {
                    &wide_tilt
                } else {
                    &wide_sway
                };
                let patch = solve_patch(&poses_sym, wp, targets, false, &opts, resp, &preset);
                let solved = patch_to_solved(&patch, GRID, cb, resp, None);
                eval_and_save(label, &solved, resp, &preset, &poses_sym, targets, png.as_deref());
            }
        }

        // Background byte check: 0xA7 vs 0x83 on high-g_t sweep (tilt 10->30, lamp [0,0,400], g_t in [0.35, 1.0])
        println!("\n=== Background entry check: 0xA7 vs 0x83 on tilt 10->30 lamp [0,0,400] (g_t >= 0.35) ===");
        let idx_83 = cb.entries.iter().position(|e| e.bytes.iter().all(|&b| b == 0x83));
        let poses_hi = tilt_poses(&base_tilt_opts, [0.0, 0.0, 400.0], 24, 10.0, 30.0);
        let star_t = &datasets[0].1;
        let patch = solve_patch(&poses_hi, &[], star_t, false, &base_tilt_opts, resp, &preset);
        let sol_a7 = patch_to_solved(&patch, GRID, cb, resp, None);
        let sol_83 = patch_to_solved(&patch, GRID, cb, resp, idx_83);
        eval_and_save("bg=0xA7 (current BACKGROUND_BYTE)", &sol_a7, resp, &preset, &poses_hi, star_t, Some("exp7_bg_0xA7_star.png"));
        eval_and_save("bg=0x83 (dark for |g_t|>=0.35)", &sol_83, resp, &preset, &poses_hi, star_t, Some("exp7_bg_0x83_star.png"));
    }

    /// Bonus physics diagnostic: how much does illuminant bandwidth (White vs PhoneLed vs narrow-band LED) limit separable frames?
    #[test]
    #[ignore]
    fn exp8_illuminant_bound() {
        let shared = design::shared_codebook(0.1333);
        let preset = reference_preset();
        let cb = &shared.0;
        let opts = Options { grid: GRID, chroma_weight: 0.0, exposure: 0.6, wide: 0.0, ..Default::default() };
        let poses = tilt_poses(&opts, [0.0, 0.0, 400.0], 24, 6.0, 30.0);
        let glyphs = make_uncorrelated_glyphs(24, GRID, &preset);
        let shapes = make_shapes_digits(24, GRID, &preset);

        println!("=== Experiment 8 (Bonus): Illuminant bandwidth vs separable frames (tilt 6->30, lamp [0,0,400], 24v) ===");
        for (iname, illum) in [
            ("White (400-700nm)", lightfield::Illuminant::White),
            ("PhoneLed", lightfield::Illuminant::PhoneLed),
            ("Green LED (532nm, 25nm FWHM)", lightfield::Illuminant::Band { center_nm: 532.0, fwhm_nm: 25.0 }),
            ("Narrow LED/Laser (532nm, 5nm FWHM)", lightfield::Illuminant::Band { center_nm: 532.0, fwhm_nm: 5.0 }),
        ] {
            let resp = lightfield::responses_for(cb, 12, illum);
            let (n5, n3, _) = svd_independent_views(&poses, &resp, 0.0, OBJ_Y_DISC);
            println!("  [{iname}] SVD n5={n5} n3={n3}");
            let patch = solve_patch(&poses, &[], &glyphs, false, &opts, &resp, &preset);
            let solved = patch_to_solved(&patch, GRID, cb, &resp, None);
            eval_and_save(&format!("  glyphs ({iname})"), &solved, &resp, &preset, &poses, &glyphs, Some(&format!("exp8_{}_glyphs.png", iname.split_whitespace().next().unwrap().to_lowercase())));

            let patch_s = solve_patch(&poses, &[], &shapes, false, &opts, &resp, &preset);
            let solved_s = patch_to_solved(&patch_s, GRID, cb, &resp, None);
            eval_and_save(&format!("  shapes ({iname})"), &solved_s, &resp, &preset, &poses, &shapes, Some(&format!("exp8_{}_shapes.png", iname.split_whitespace().next().unwrap().to_lowercase())));
        }
    }

    /// Refined sweeps tightly matched to the active codebook g_t band [0.22..0.75],
    /// chroma_weight curve, wide/exposure on winning sweep, log-g_t spacing, and flipbook contact sheets.
    #[test]
    #[ignore]
    fn exp9_refined_winners() {
        let shared = design::shared_codebook(0.1333);
        let preset = reference_preset();
        let (cb, resp) = (&shared.0, &shared.1);
        let opts = Options { grid: GRID, chroma_weight: 0.0, exposure: 0.6, wide: 0.0, ..Default::default() };

        let star = make_star_targets(24, GRID, &preset, false);
        let glyphs = make_uncorrelated_glyphs(24, GRID, &preset);
        let shapes = make_shapes_digits(24, GRID, &preset);

        // Log-g_t spaced tilt poses for overhead lamp [0,0,400] covering |g_t| in [0.23..0.74]
        let log_poses_overhead = |views: usize, gt_lo: f64, gt_hi: f64| -> Vec<(V3, V3)> {
            let denom = (views - 1).max(1) as f64;
            (0..views)
                .map(|i| {
                    let gt_want = gt_lo * (gt_hi / gt_lo).powf(i as f64 / denom);
                    let deg = (gt_want / 2.0).clamp(-0.99, 0.99).asin().to_degrees();
                    tilted_eye_and_lamp(&opts, [0.0, 0.0, 400.0], deg.to_radians(), 0.0)
                })
                .collect()
        };

        println!("=== Experiment 9a: Active-band matched tilt sweeps (24v, BLEND=off, chr=0, wide=0, exp=0.6) ===");
        let sweeps: Vec<(&str, Vec<(V3, V3)>, Option<&str>)> = vec![
            ("tilt 7->21   lamp[0,0,400] (linear)", tilt_poses(&opts, [0.0, 0.0, 400.0], 24, 7.0, 21.0), Some("exp9_tilt7_21_lamp0")),
            ("tilt 6.5->22.5 lamp[0,0,400] (linear)", tilt_poses(&opts, [0.0, 0.0, 400.0], 24, 6.5, 22.5), Some("exp9_tilt65_225_lamp0")),
            ("tilt 6.5->22.5 lamp[0,0,400] (log-gt)", log_poses_overhead(24, 0.23, 0.76), Some("exp9_loggt_lamp0")),
            ("tilt +-8     lamp[220,0,400] (sym)", tilt_poses(&opts, [220.0, 0.0, 400.0], 24, -8.0, 8.0), None),
            ("tilt +-8.5   lamp[225,0,400] (sym)", tilt_poses(&opts, [225.0, 0.0, 400.0], 24, -8.5, 8.5), Some("exp9_tilt85_lamp225")),
            ("tilt +-10    lamp[235,0,400] (sym)", tilt_poses(&opts, [235.0, 0.0, 400.0], 24, -10.0, 10.0), None),
            ("tilt +-10    lamp[220,0,400] (sym)", tilt_poses(&opts, [220.0, 0.0, 400.0], 24, -10.0, 10.0), None),
            ("tilt -18->+3 lamp[150,0,400] (cur lamp)", tilt_poses(&opts, LAMP_MM, 24, -18.0, 3.0), Some("exp9_tilt_m18_p3_lamp150")),
        ];
        for (dname, targets) in [("star", &star), ("glyphs", &glyphs), ("shapes", &shapes)] {
            println!("\n--- Dataset: {dname} ---");
            for (label, poses, png_tag) in &sweeps {
                let patch = solve_patch(poses, &[], targets, false, &opts, resp, &preset);
                let solved = patch_to_solved(&patch, GRID, cb, resp, None);
                let png = png_tag.map(|t| format!("{t}_{dname}.png"));
                eval_and_save(label, &solved, resp, &preset, poses, targets, png.as_deref());
            }
        }

        // Chroma weight curve on winning symmetric & one-sided sweeps
        println!("\n=== Experiment 9b: chroma_weight sweep on tilt 6.5->22.5 lamp[0,0,400] (24v) ===");
        let win_poses = tilt_poses(&opts, [0.0, 0.0, 400.0], 24, 6.5, 22.5);
        for (dname, targets) in [("star", &star), ("glyphs", &glyphs), ("shapes", &shapes)] {
            println!("\n--- Dataset: {dname} ---");
            for &cw in &[0.0, 0.05, 0.1, 0.3, 1.0] {
                let o = Options { chroma_weight: cw, ..opts.clone() };
                let patch = solve_patch(&win_poses, &[], targets, false, &o, resp, &preset);
                let solved = patch_to_solved(&patch, GRID, cb, resp, None);
                eval_and_save(&format!("chroma_weight = {cw:.2}"), &solved, resp, &preset, &win_poses, targets, None);
            }
        }

        // wide & exposure on winning sweep (tilt 6.5->22.5 lamp [0,0,400])
        println!("\n=== Experiment 9c: wide & exposure on tilt 6.5->22.5 lamp[0,0,400] (24v) ===");
        let win_wide_tilt = tilt_wide_poses(&opts, [0.0, 0.0, 400.0], 24, 6.5, 22.5);
        let win_wide_sway = sway_wide_poses(&opts, [0.0, 0.0, 400.0], 24);
        for (dname, targets) in [("star", &star), ("glyphs", &glyphs), ("shapes", &shapes)] {
            println!("\n--- Dataset: {dname} ---");
            for (label, wide, use_tilt_w, exp) in [
                ("wide=0.0, exp=0.4", 0.0, false, 0.4),
                ("wide=0.0, exp=0.6", 0.0, false, 0.6),
                ("wide=0.0, exp=0.8", 0.0, false, 0.8),
                ("wide=0.0, exp=1.0", 0.0, false, 1.0),
                ("wide=0.3 (tilt-wide), exp=0.6", 0.3, true, 0.6),
                ("wide=0.3 (sway-wide), exp=0.6", 0.3, false, 0.6),
            ] {
                let o = Options { wide, exposure: exp, ..opts.clone() };
                let wp: &[(V3, V3, usize)] = if wide == 0.0 { &[] } else if use_tilt_w { &win_wide_tilt } else { &win_wide_sway };
                let patch = solve_patch(&win_poses, wp, targets, false, &o, resp, &preset);
                let solved = patch_to_solved(&patch, GRID, cb, resp, None);
                eval_and_save(label, &solved, resp, &preset, &win_poses, targets, None);
            }
        }

        // Clean flipbook capacity on active-band tilt 6.5->22.5 (40v oversampled, linear vs log-gt)
        println!("\n=== Experiment 9d: Flipbook capacity on active-band tilt 6.5->22.5 (40v oversampled) ===");
        for (dname, all_kf) in [("glyphs", &glyphs), ("shapes", &shapes)] {
            println!("\n--- Flipbook dataset: {dname} ---");
            for &m in &[6usize, 8, 10, 12, 16, 20, 24] {
                let kf = &all_kf[..m];
                for &(spacing, use_log) in &[("lin", false), ("log", true)] {
                    let poses = if use_log { log_poses_overhead(40, 0.23, 0.76) } else { tilt_poses(&opts, [0.0, 0.0, 400.0], 40, 6.5, 22.5) };
                    let targets = make_flipbook(kf, 40);
                    let patch = solve_patch(&poses, &[], &targets, false, &opts, resp, &preset);
                    let solved = patch_to_solved(&patch, GRID, cb, resp, None);
                    let kf_poses: Vec<(V3, V3)> = (0..m)
                        .map(|k| {
                            let frac = (k as f64 + 0.5) / m as f64;
                            let deg = if use_log {
                                let gt = 0.23 * (0.76 / 0.23f64).powf(frac);
                                (gt / 2.0).asin().to_degrees()
                            } else {
                                6.5 + (22.5 - 6.5) * frac
                            };
                            tilted_eye_and_lamp(&opts, [0.0, 0.0, 400.0], deg.to_radians(), 0.0)
                        })
                        .collect();
                    let region: Vec<usize> = (0..GRID * GRID).filter(|&c| kf.iter().any(|t| t[c] > LIT)).collect();
                    let (mut own_s, mut gh_adj_s, mut gh_3_s, mut ctr_s, mut dist_adj, mut dist_3) = (0.0, 0.0, 0.0, 0.0, 0usize, 0usize);
                    let mut renders = Vec::with_capacity(m);
                    for k in 0..m {
                        let px = lightfield::render_float(&solved, resp, &preset, kf_poses[k].0, kf_poses[k].1, GRID, true);
                        let lum: Vec<f64> = px.iter().map(|&p| lightfield::luma(p)).collect();
                        let own = pearson(&lum, &kf[k], &region);
                        let gh_adj = (0..m).filter(|&j| j != k).map(|j| pearson(&lum, &kf[j], &region)).fold(f64::NEG_INFINITY, f64::max);
                        let gh_3 = (0..m).filter(|&j| j.abs_diff(k) >= 3).map(|j| pearson(&lum, &kf[j], &region)).fold(f64::NEG_INFINITY, f64::max);
                        own_s += own;
                        gh_adj_s += gh_adj;
                        if gh_3.is_finite() { gh_3_s += gh_3; }
                        if own >= gh_adj + 0.1 { dist_adj += 1; }
                        if !gh_3.is_finite() || own >= gh_3 + 0.1 { dist_3 += 1; }
                        let (mut ls, mut ln, mut ds, mut dn) = (0.0, 0, 0.0, 0);
                        for &c in &region {
                            if kf[k][c] > 0.1 { ls += lum[c]; ln += 1; } else { ds += lum[c]; dn += 1; }
                        }
                        ctr_s += (ls / ln as f64) / (ds / dn as f64).max(1e-6);
                        renders.push(px);
                    }
                    println!(
                        "  flipbook {dname:<6} M={m:2} ({spacing}) @40v | own {:+.3} | max_other(j!=k) {:+.3} (dist {:2}/{:2}) | ghost(|j-k|>=3) {:+.3} (dist {:2}/{:2}) | ctr {:4.2}x",
                        own_s / m as f64,
                        gh_adj_s / m as f64,
                        dist_adj,
                        m,
                        gh_3_s / m as f64,
                        dist_3,
                        m,
                        ctr_s / m as f64,
                    );
                    // Save visual contact sheets for M=8 and M=12 shapes (log spacing)
                    if dname == "shapes" && use_log && (m == 8 || m == 12) {
                        let crop = 96usize;
                        let ro = preset.outer_radius_mm;
                        let cx0 = (GRID / 2).saturating_sub(crop / 2);
                        let cy_center = ((OBJ_Y_DISC / (2.0 * ro) + 0.5) * GRID as f64).round() as usize;
                        let cy0 = cy_center.saturating_sub(crop / 2).min(GRID - crop);
                        let gap = 4usize;
                        let mut img = image::RgbImage::from_pixel((m * crop) as u32, (crop * 2 + gap) as u32, image::Rgb([40, 40, 40]));
                        let enc = |v: f64| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8;
                        for k in 0..m {
                            for y in 0..crop {
                                for x in 0..crop {
                                    let idx = (cy0 + y) * GRID + (cx0 + x);
                                    let tv = enc(kf[k][idx] as f64);
                                    img.put_pixel((k * crop + x) as u32, y as u32, image::Rgb([tv, tv, tv]));
                                    let rv = renders[k][idx];
                                    let rgb = [enc(rv[0] as f64 * 1.5), enc(rv[1] as f64 * 1.5), enc(rv[2] as f64 * 1.5)];
                                    img.put_pixel((k * crop + x) as u32, (crop + gap + y) as u32, image::Rgb(rgb));
                                }
                            }
                        }
                        let path = format!("{}/exp9_flipbook_M{m}_log_shapes.png", out_dir());
                        img.save(&path).unwrap();
                        eprintln!("  saved contact sheet: {path}");
                    }
                }
            }
        }
    }
}
