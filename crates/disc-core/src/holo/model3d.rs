//! 3D model (OBJ / glTF / GLB) → multi-view hologram target generator (PLAN stage 10).
//!
//! Physics constraint: on a DVD, tracks are concentric circles, so horizontal head sway
//! only modulates the tangential grating frequency `g_t` in the near (6 o'clock, `y < 0`)
//! and far (12 o'clock, `y > 0`) bands where tracks run roughly left-right. At 3 and 9
//! o'clock, tracks run vertically and horizontal head motion produces no parallax.
//!
//! Accordingly, this module:
//! 1. Normalizes any loaded OBJ / glTF / GLB mesh into a unit cube, rotates and scales it
//!    to `scale_mm`, and places its centre along the central eye ray to the chosen band
//!    centre `(0, ±r_mid, 0)` at depth `depth_mm` above/below the disc plane `z = 0`.
//! 2. Perspective-projects every triangle from each head-motion `ViewSpec::eye_pos()` onto
//!    the disc plane `z = 0` (generalizing `testscene::Sphere3D::project`) and software-
//!    rasterizes it with a perspective-correct `1/w` z-buffer, 2×2 supersampling, and
//!    grayscale Lambert + ambient shading.
//! 3. Applies `band_mask` so pixels inside the usable near/far horizontal-track window
//!    (`α = atan2(|x|, |y|) ≤ 25°`, `r ∈ [r_in + 2, r_out - 2]` mm) keep full strength,
//!    smoothly fading to zero by `α = 40°` and outside the DVD data ring.

use super::design::{self, ViewSpec};
use super::testscene::{
    DEFAULT_DISTANCE_MM, DEFAULT_HEAD_HALF_SPAN_MM, DEFAULT_TARGET_SIZE, DEFAULT_TEST3D_LAMP_MM,
    DEFAULT_TILT_DEG, DEFAULT_VIEWS,
};
use crate::preset::DiscGeometryPreset;
use base64::Engine as _;
use image::{imageops::FilterType, DynamicImage, ImageFormat, Rgba, RgbaImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Default model bounding-box longest edge (mm): fits cleanly inside one 33.7 mm radial band
/// with comfortable margin for perspective expansion and ±80 mm head sway.
pub const DEFAULT_SCALE_MM: f64 = 24.0;
/// Default depth offset of the model centre above the disc plane (mm): front floats ~18-20 mm
/// above the disc while the back anchors near z = 0 for strong, legible parallax.
pub const DEFAULT_DEPTH_MM: f64 = 8.0;
/// Angle off the 6/12 o'clock midline (`atan2(|x|, |y|)`) up to which the band mask is 1.0.
/// At 25°, `|x|/|y| ≈ 0.466` and horizontal sway coupling `cos(25°) ≈ 0.906`.
pub const BAND_FADE_START_DEG: f64 = 25.0;
/// Angle off the 6/12 o'clock midline at which the band mask reaches 0.0 (suppressing 3/9 o'clock).
pub const BAND_FADE_END_DEG: f64 = 40.0;
/// Radial guard inside the physical data-ring edges where the mask is 0.0 (mm).
pub const RADIAL_GUARD_MM: f64 = 0.5;
/// Radial feather width from `guard` to full weight 1.0 (mm).
pub const RADIAL_FEATHER_MM: f64 = 1.5;
/// Thumbnail edge length (pixels) returned for UI view-strip preview.
pub const THUMBNAIL_SIZE: u32 = 96;

pub(crate) const AMBIENT: f64 = 0.25;
pub(crate) const DIFFUSE: f64 = 0.75;
/// Directional key light roughly aligned with the sideways-above lamp, with slight +y tilt
/// so +X, +Y and +Z facets all receive distinct grayscale tones.
pub(crate) const KEY_LIGHT_RAW: [f64; 3] = [0.45, 0.25, 0.86];

type V3 = [f64; 3];

#[inline]
fn add3(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
fn sub3(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn scale3(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[inline]
fn dot3(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn cross3(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
fn norm3(a: V3) -> f64 {
    dot3(a, a).sqrt()
}

#[inline]
fn unit3(a: V3) -> Option<V3> {
    let n = norm3(a);
    if n > 1e-12 && n.is_finite() {
        Some([a[0] / n, a[1] / n, a[2] / n])
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Vertex {
    pub pos: V3,
    pub normal: V3,
    /// How light its surface is, linear; 1 where the model does not say.
    pub albedo: f32,
}

#[derive(Clone, Debug)]
pub struct Mesh {
    pub tris: Vec<[Vertex; 3]>,
}

impl Mesh {
    /// Axis-aligned bounding box `(min, max)` over all triangle vertices.
    pub fn bounds(&self) -> Option<(V3, V3)> {
        if self.tris.is_empty() {
            return None;
        }
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for tri in &self.tris {
            for v in tri {
                for c in 0..3 {
                    lo[c] = lo[c].min(v.pos[c]);
                    hi[c] = hi[c].max(v.pos[c]);
                }
            }
        }
        if lo.iter().all(|x| x.is_finite()) && hi.iter().all(|x| x.is_finite()) {
            Some((lo, hi))
        } else {
            None
        }
    }
}

/// Which horizontal-track band on the DVD holds the 3D model.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Band {
    /// Near band (6 o'clock, `y < 0`): smallest `|g_r|` under the sideways fixed lamp, brightest.
    #[default]
    Near,
    /// Far band (12 o'clock, `y > 0`).
    Far,
}

/// Placement and orientation parameters for importing a 3D model onto the DVD.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelParams {
    /// Normalized bounding-box longest edge in mm.
    #[serde(default = "default_scale_mm")]
    pub scale_mm: f64,
    /// Centre depth offset above (`+z`) or below (`-z`) the disc plane `z = 0`, in mm.
    #[serde(default = "default_depth_mm")]
    pub depth_mm: f64,
    /// Rotation around X axis (degrees), applied first.
    #[serde(default)]
    pub rotate_x_deg: f64,
    /// Rotation around Y axis (degrees), applied second.
    #[serde(default)]
    pub rotate_y_deg: f64,
    /// Rotation around Z axis (degrees), applied third.
    #[serde(default)]
    pub rotate_z_deg: f64,
    /// Target horizontal-track band (`near` or `far`).
    #[serde(default)]
    pub band: Band,
}

fn default_scale_mm() -> f64 {
    DEFAULT_SCALE_MM
}

fn default_depth_mm() -> f64 {
    DEFAULT_DEPTH_MM
}

impl Default for ModelParams {
    fn default() -> Self {
        Self {
            scale_mm: DEFAULT_SCALE_MM,
            depth_mm: DEFAULT_DEPTH_MM,
            rotate_x_deg: 0.0,
            rotate_y_deg: 0.0,
            rotate_z_deg: 0.0,
            band: Band::Near,
        }
    }
}

/// Output of `write_model_views`: head-motion `ViewSpec`s pointing to saved PNGs plus small
/// base64 thumbnails for the UI strip preview.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelViewsOutput {
    pub views: Vec<ViewSpec>,
    pub thumbnails: Vec<String>,
    pub triangles: usize,
}

#[inline]
fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Smooth window mask in `[0, 1]` restricting model brightness to the DVD data ring and the
/// near/far bands (6 & 12 o'clock) where circular tracks run roughly left-right.
///
/// - Radial: `0` outside `[r_in + 0.5, r_out - 0.5]` mm, smoothly ramping over `1.5 mm` to `1`
///   inside `[r_in + 2.0, r_out - 2.0]` mm.
/// - Azimuthal: `1` for `α = atan2(|x|, |y|) ≤ 25°` (`|x|/|y| ≤ 0.466`, `cos α ≥ 0.906`),
///   smoothly fading to `0` by `α = 40°` (`|x|/|y| ≥ 0.839`), masking out the 3 & 9 o'clock
///   dead zones where horizontal head sway does not steer `g_t`.
pub fn band_mask(x_mm: f64, y_mm: f64, inner_radius_mm: f64, outer_radius_mm: f64) -> f64 {
    let r = x_mm.hypot(y_mm);
    let w_in = smoothstep(inner_radius_mm + RADIAL_GUARD_MM, inner_radius_mm + RADIAL_GUARD_MM + RADIAL_FEATHER_MM, r);
    let w_out = smoothstep(outer_radius_mm - RADIAL_GUARD_MM, outer_radius_mm - RADIAL_GUARD_MM - RADIAL_FEATHER_MM, r);
    let alpha_deg = x_mm.abs().atan2(y_mm.abs()).to_degrees();
    let w_az = smoothstep(BAND_FADE_END_DEG, BAND_FADE_START_DEG, alpha_deg);
    w_in * w_out * w_az
}

/// Perspective projection of 3D point `p` (mm) from eye position `eye` (mm) onto the disc
/// plane `z = 0`, generalizing `testscene::Sphere3D::project`.
///
/// Returns `Some((proj_x_mm, proj_y_mm, dz))` where `dz = eye[2] - p[2] > 1.0` (smaller `dz`
/// is closer to the eye, and `1 / dz` interpolates linearly across projected screen space).
/// For any point on the disc plane `p[2] == 0.0`, `(proj_x, proj_y) == (p[0], p[1])` for all
/// eye positions.
#[inline]
pub fn project_point(p: V3, eye: V3) -> Option<(f64, f64, f64)> {
    let dz = eye[2] - p[2];
    if dz <= 1.0 || !dz.is_finite() {
        return None;
    }
    let t = eye[2] / dz;
    let px = eye[0] + (p[0] - eye[0]) * t;
    let py = eye[1] + (p[1] - eye[1]) * t;
    Some((px, py, dz))
}

fn rot_xyz(v: V3, rx_deg: f64, ry_deg: f64, rz_deg: f64) -> V3 {
    let (sx, cx) = rx_deg.to_radians().sin_cos();
    let (sy, cy) = ry_deg.to_radians().sin_cos();
    let (sz, cz) = rz_deg.to_radians().sin_cos();
    // Rx
    let y1 = v[1] * cx - v[2] * sx;
    let z1 = v[1] * sx + v[2] * cx;
    let x1 = v[0];
    // Ry
    let x2 = x1 * cy + z1 * sy;
    let z2 = -x1 * sy + z1 * cy;
    let y2 = y1;
    // Rz
    [x2 * cz - y2 * sz, x2 * sz + y2 * cz, z2]
}

/// Normalize `mesh` into a unit cube, apply Euler rotations and `scale_mm`, and position its
/// centre along the reference eye ray to the chosen horizontal-track band `(0, ±r_mid, 0)` at
/// depth `params.depth_mm`.
pub fn prepare_mesh(
    mesh: &Mesh,
    params: &ModelParams,
    preset: &DiscGeometryPreset,
    distance_mm: f64,
    tilt_deg: f64,
) -> Result<Mesh, String> {
    let (lo, hi) = mesh.bounds().ok_or_else(|| "模型不包含有效三角形".to_string())?;
    let center = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1]), 0.5 * (lo[2] + hi[2])];
    let extent = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]);
    if extent <= 1e-12 || !extent.is_finite() {
        return Err("模型尺寸为零，无法归一化".into());
    }
    let scale = params.scale_mm.clamp(2.0, 120.0);
    let depth = params.depth_mm.clamp(-80.0, 80.0);
    let r_mid = 0.5 * (preset.inner_radius_mm + preset.outer_radius_mm);
    let y_band = match params.band {
        Band::Near => -r_mid,
        Band::Far => r_mid,
    };
    let e0 = design::head_eye_mm(distance_mm, tilt_deg, -90.0, 0.0);
    let center_y = e0[1] + (y_band - e0[1]) * (e0[2] - depth) / e0[2];
    let world_center = [0.0, center_y, depth];

    let tris = mesh
        .tris
        .iter()
        .map(|tri| {
            let mut out = [Vertex { pos: [0.0; 3], normal: [0.0, 0.0, 1.0], albedo: 1.0 }; 3];
            for i in 0..3 {
                let norm_p = scale3(sub3(tri[i].pos, center), 1.0 / extent);
                let rot_p = rot_xyz(norm_p, params.rotate_x_deg, params.rotate_y_deg, params.rotate_z_deg);
                let rot_n = rot_xyz(tri[i].normal, params.rotate_x_deg, params.rotate_y_deg, params.rotate_z_deg);
                out[i] = Vertex {
                    pos: add3(scale3(rot_p, scale), world_center),
                    normal: unit3(rot_n).unwrap_or([0.0, 0.0, 1.0]),
                    albedo: tri[i].albedo,
                };
            }
            out
        })
        .collect();
    Ok(Mesh { tris })
}

#[inline]
fn encode_srgb(linear: f64) -> u8 {
    let l = linear.clamp(0.0, 1.0);
    let s = if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Sentinel path for the bundled 3D sample model ("试试示例").
pub const BUILTIN_SAMPLE_PATH: &str = "builtin:star";
/// Maximum accepted raw model file size for the scene editor.
pub const MAX_MODEL_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// A scene model is simplified to at most this many triangles. A disc cell is about 0.2 mm,
/// so on a model a few centimetres across more than this cannot be told apart.
pub const MAX_SCENE_TRIANGLES: usize = 150_000;
/// Current schema version for serialized 3D hologram scenes.
/// 2: modes moved from positions to models, and positions order their models and pictures.
pub const SCENE_VERSION: u32 = 2;

/// Bundled faceted five-pointed star OBJ (right-handed, +Y up, +Z toward viewer).
const BUILTIN_STAR_OBJ: &str = "\
v  0.0000  1.0000 -0.3000
v -0.2586  0.3560  0.0500
v -0.9511  0.3090 -0.3000
v -0.4185 -0.1360  0.0500
v -0.5878 -0.8090 -0.3000
v  0.0000 -0.4400  0.0500
v  0.5878 -0.8090 -0.3000
v  0.4185 -0.1360  0.0500
v  0.9511  0.3090 -0.3000
v  0.2586  0.3560  0.0500
v  0.0000  0.0000  1.0500
v  0.0000  0.0000 -0.4500
f 11 1 2
f 11 2 3
f 11 3 4
f 11 4 5
f 11 5 6
f 11 6 7
f 11 7 8
f 11 8 9
f 11 9 10
f 11 10 1
f 12 2 1
f 12 3 2
f 12 4 3
f 12 5 4
f 12 6 5
f 12 7 6
f 12 8 7
f 12 9 8
f 12 10 9
f 12 1 10
";

fn default_true() -> bool {
    true
}

fn default_crop_deg() -> f64 {
    30.0
}

fn default_one() -> f64 {
    1.0
}

fn default_sides() -> u32 {
    6
}

fn default_shape_size() -> f64 {
    0.35
}

fn default_mask_dim() -> u32 {
    1
}

pub const DEFAULT_PARALLAX: f64 = 2.0;

fn default_parallax() -> f64 {
    DEFAULT_PARALLAX
}

pub const IDENTITY_MAT4: [f64; 16] = [
    1.0, 0.0, 0.0, 0.0,
    0.0, 1.0, 0.0, 0.0,
    0.0, 0.0, 1.0, 0.0,
    0.0, 0.0, 0.0, 1.0,
];

/// How a viewing position's target views are generated across the viewing window.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Real parallax: fixed object geometry viewed from scaled head positions.
    #[default]
    Real,
    /// Transform animation: fixed centre eye, objects transformed by per-object T(t).
    Transform,
}

/// Per-object animation transform for transform mode.
///
/// Rust never parses `matrix`; it linearly interpolates `samples` (row-major 4x4 matrices at
/// evenly spaced `t` in `[0, 1]`). Empty `samples` means identity.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Motion {
    pub preset: String,
    pub matrix: Vec<String>,
    pub samples: Vec<[f64; 16]>,
    /// The numbers the matrix names as `{{name}}`, kept so a saved scene opens with them.
    pub vars: Vec<MotionVar>,
}

/// A named number in a motion's matrix.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MotionVar {
    pub name: String,
    pub value: f64,
}

impl Motion {
    /// Evaluate `T(t)` for `t` in `[0, 1]` by element-wise linear interpolation between samples.
    pub fn sample(&self, t: f64) -> [f64; 16] {
        match self.samples.len() {
            0 => IDENTITY_MAT4,
            1 => self.samples[0],
            n => {
                let tc = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
                let pos = tc * (n - 1) as f64;
                let i0 = (pos.floor() as usize).min(n - 2);
                let frac = pos - i0 as f64;
                let a = self.samples[i0];
                let b = self.samples[i0 + 1];
                std::array::from_fn(|k| a[k] + (b[k] - a[k]) * frac)
            }
        }
    }
}

/// One placed 3D object in the hologram scene editor (scene space: +x right, +y up, +z toward viewer, mm).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SceneObject {
    pub id: String,
    pub path: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub visible: bool,
    pub position_mm: [f64; 3],
    pub rotation_deg: [f64; 3],
    /// Size along each of its own axes, mm; a single number in older files is the same on all three.
    #[serde(deserialize_with = "one_or_three")]
    pub scale_mm: [f64; 3],
    /// The viewing position it belongs to; the first when empty or unknown.
    #[serde(default)]
    pub position_id: String,
    /// Per-object ink style (lines, shading, texture).
    #[serde(default)]
    pub style: super::ink::Style,
    /// Per-object transform animation used when the owning position is in `Mode::Transform`.
    #[serde(default)]
    pub motion: Motion,
    /// How its views are made.
    #[serde(default)]
    pub mode: Mode,
    /// Parallax boost in `Mode::Real`.
    #[serde(default = "default_parallax")]
    pub parallax: f64,
    /// How far the disc tilts either way in `Mode::Transform`, degrees; 0 takes the default.
    #[serde(default)]
    pub tilt_deg: f64,
}

/// Which ends of a position's crop axis keep their sector: the top, the bottom, or both.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CropSide {
    #[default]
    Both,
    Up,
    Down,
}

/// Which channel of a mask picture defines the kept region.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MaskChannel {
    #[default]
    Luma,
    Alpha,
}

/// One crop shape attached to a viewing position; visible crops unite to form its kept area.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Crop {
    #[serde(rename_all = "camelCase")]
    Sector {
        id: String,
        #[serde(default)]
        name: String,
        #[serde(default = "default_true")]
        visible: bool,
        #[serde(default = "default_crop_deg")]
        deg: f64,
        #[serde(default)]
        side: CropSide,
        #[serde(default)]
        offset_deg: f64,
    },
    #[serde(rename_all = "camelCase")]
    Circle {
        id: String,
        #[serde(default)]
        name: String,
        #[serde(default = "default_true")]
        visible: bool,
        #[serde(default)]
        x: f64,
        #[serde(default)]
        y: f64,
        #[serde(default = "default_shape_size")]
        size: f64,
        #[serde(default = "default_one")]
        stretch_x: f64,
        #[serde(default = "default_one")]
        stretch_y: f64,
        #[serde(default)]
        rotation_deg: f64,
    },
    #[serde(rename_all = "camelCase")]
    Polygon {
        id: String,
        #[serde(default)]
        name: String,
        #[serde(default = "default_true")]
        visible: bool,
        #[serde(default = "default_sides")]
        sides: u32,
        #[serde(default)]
        x: f64,
        #[serde(default)]
        y: f64,
        #[serde(default = "default_shape_size")]
        size: f64,
        #[serde(default = "default_one")]
        stretch_x: f64,
        #[serde(default = "default_one")]
        stretch_y: f64,
        #[serde(default)]
        rotation_deg: f64,
    },
    #[serde(rename_all = "camelCase")]
    Mask {
        id: String,
        #[serde(default)]
        name: String,
        #[serde(default = "default_true")]
        visible: bool,
        path: String,
        #[serde(default = "default_mask_dim")]
        width: u32,
        #[serde(default = "default_mask_dim")]
        height: u32,
        #[serde(default)]
        x: f64,
        #[serde(default)]
        y: f64,
        #[serde(default = "default_one")]
        size: f64,
        #[serde(default = "default_one")]
        stretch_x: f64,
        #[serde(default = "default_one")]
        stretch_y: f64,
        #[serde(default)]
        rotation_deg: f64,
        #[serde(default)]
        channel: MaskChannel,
        #[serde(default)]
        invert: bool,
    },
}

impl Crop {
    /// Whether this crop shape participates in masking.
    pub fn visible(&self) -> bool {
        match self {
            Crop::Sector { visible, .. } | Crop::Circle { visible, .. } | Crop::Polygon { visible, .. } | Crop::Mask { visible, .. } => *visible,
        }
    }

    /// The image path if this crop is a custom mask.
    pub fn mask_path(&self) -> Option<&str> {
        match self {
            Crop::Mask { path, .. } => Some(path.as_str()),
            _ => None,
        }
    }

    /// Mutable image path if this crop is a custom mask.
    pub fn mask_path_mut(&mut self) -> Option<&mut String> {
        match self {
            Crop::Mask { path, .. } => Some(path),
            _ => None,
        }
    }
}

/// One way the disc is turned to be looked at, with its own objects.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Position {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Which angle of the disc is at the top, clockwise in degrees.
    pub phi_deg: f64,
    /// Half-angle kept either side of the crop's axis, degrees; 90 or more with both sides kept
    /// means no crop (legacy field used when `crops` is absent).
    #[serde(default = "default_crop_deg")]
    pub crop_deg: f64,
    /// Which ends of the axis keep their sector (legacy field used when `crops` is absent).
    #[serde(default)]
    pub crop_side: CropSide,
    /// The axis turned from 12 o'clock, clockwise as seen, degrees (legacy field used when `crops` is absent).
    #[serde(default)]
    pub crop_offset_deg: f64,
    /// Multi-shape crops for this position; empty means no crop, non-empty takes union of visible items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crops: Option<Vec<Crop>>,
    /// How hard the solve holds every view white rather than letting it take the colour that is
    /// sharper; none takes each model's mode's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub neutral: Option<f64>,
    /// Solved for a disc that keeps turning: every cell as it passes `look_deg`, so the turn
    /// does not matter. Holds pictures only; its models are left out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub spin: bool,
    /// Where on the disc a spinning position is looked at, clockwise from 12 o'clock as seen,
    /// degrees; none takes `turntable::SPIN_LOOK_DEG`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look_deg: Option<f64>,
    /// Its models' and pictures' ids, top first: where two want the same disc, the higher wins.
    #[serde(default)]
    pub order: Vec<String>,
}

/// Versioned 3D hologram scene contract shared between the editor UI, project files, and renderer.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Scene {
    pub version: u32,
    pub objects: Vec<SceneObject>,
    /// Ways the disc is turned to be looked at; none means one, not turned.
    #[serde(default)]
    pub positions: Vec<Position>,
    /// Where the lamp stands, in scene space; the default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lamp_mm: Option<[f64; 3]>,
    /// The positions' pictures, carved flat.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<SceneImage>,
}

/// A picture on a flat position. Its layer is placed as on the carve page, in the position's own
/// frame: upright with the position at the top, a share of the diameter from the centre, y down.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SceneImage {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// The picture's own pixel size.
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    /// The viewing position it belongs to.
    #[serde(default)]
    pub position_id: String,
    /// Solve as a colour diffraction picture instead of grey carving.
    #[serde(default)]
    pub colour: bool,
    /// For a colour picture, how hard its hues are matched against its brightness; none takes
    /// `turntable::PICTURE_CHROMA`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chroma: Option<f64>,
    #[serde(flatten)]
    pub layer: crate::artwork::Layer,
}

/// Optional viewing / lamp parameters when composing a 3D `Scene` into hologram target views.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SceneViewOptions {
    pub light_mm: [f64; 3],
    pub distance_mm: f64,
    pub tilt_deg: f64,
    pub head_half_span_mm: f64,
    pub views: usize,
    pub target_size: u32,
}

impl Default for SceneViewOptions {
    fn default() -> Self {
        Self {
            light_mm: DEFAULT_TEST3D_LAMP_MM,
            distance_mm: DEFAULT_DISTANCE_MM,
            tilt_deg: DEFAULT_TILT_DEG,
            head_half_span_mm: DEFAULT_HEAD_HALF_SPAN_MM,
            views: DEFAULT_VIEWS,
            target_size: DEFAULT_TARGET_SIZE,
        }
    }
}

/// Convert a 3D point or direction vector from scene space into the disc picture frame.
///
/// The disc picture frame is y DOWN (`y = (v - 0.5) * 2 * outer_radius_mm`, whereas scene space
/// has +x right, +y up, +z toward viewer), so a naive copy mirrors the model vertically; the
/// 3D editor display must match what the burned disc shows. Every scene vertex, normal, eye,
/// lamp, and shading direction entering disc target rendering passes through this one function.
#[inline]
pub fn scene_to_disc(v: [f64; 3]) -> [f64; 3] {
    [v[0], -v[1], v[2]]
}

/// Smooth radial mask restricting brightness to `[r_in + guard, r_out - guard]` (mm).
#[inline]
pub fn radial_mask(x_mm: f64, y_mm: f64, inner_radius_mm: f64, outer_radius_mm: f64) -> f64 {
    let r = x_mm.hypot(y_mm);
    let w_in = smoothstep(inner_radius_mm + RADIAL_GUARD_MM, inner_radius_mm + RADIAL_GUARD_MM + RADIAL_FEATHER_MM, r);
    let w_out = smoothstep(outer_radius_mm - RADIAL_GUARD_MM, outer_radius_mm - RADIAL_GUARD_MM - RADIAL_FEATHER_MM, r);
    w_in * w_out
}

/// Normalize `mesh` so its bounding-box centre sits at `(0, 0, 0)` and its longest edge is `1.0`.
pub fn normalize_mesh(mesh: &Mesh) -> Result<Mesh, String> {
    let (lo, hi) = mesh.bounds().ok_or_else(|| "模型不包含有效三角形".to_string())?;
    let center = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1]), 0.5 * (lo[2] + hi[2])];
    let extent = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]);
    if extent <= 1e-12 || !extent.is_finite() {
        return Err("模型尺寸为零，无法归一化".into());
    }
    let inv = 1.0 / extent;
    let tris = mesh
        .tris
        .iter()
        .map(|tri| {
            let mut out = [Vertex { pos: [0.0; 3], normal: [0.0, 0.0, 1.0], albedo: 1.0 }; 3];
            for i in 0..3 {
                out[i] = Vertex {
                    pos: scale3(sub3(tri[i].pos, center), inv),
                    normal: unit3(tri[i].normal).unwrap_or([0.0, 0.0, 1.0]),
                    albedo: tri[i].albedo,
                };
            }
            out
        })
        .collect();
    Ok(Mesh { tris })
}

/// Load a scene model, simplified to `MAX_SCENE_TRIANGLES`, or the bundled sample for
/// `BUILTIN_SAMPLE_PATH`. Reading a model of millions of triangles takes seconds and the
/// preview asks for it again at every change, so the last few are kept.
pub fn load_mesh_or_builtin(path: &str) -> Result<Arc<Mesh>, String> {
    const KEEP: usize = 8;
    type Key = (String, u64, Option<std::time::SystemTime>);
    static CACHE: Mutex<Vec<(Key, Arc<Mesh>)>> = Mutex::new(Vec::new());

    if path == BUILTIN_SAMPLE_PATH {
        return load_obj_str(BUILTIN_STAR_OBJ).map(Arc::new);
    }
    let p = Path::new(path);
    let meta = std::fs::metadata(p).map_err(|e| format!("无法读取模型 {path}：{e}"))?;
    if meta.len() > MAX_MODEL_FILE_BYTES {
        return Err(format!(
            "模型文件过大（{:.0} MB，上限 {} MB）",
            meta.len() as f64 / (1024.0 * 1024.0),
            MAX_MODEL_FILE_BYTES / (1024 * 1024)
        ));
    }
    let key: Key = (path.to_string(), meta.len(), meta.modified().ok());
    if let Some((_, m)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| *k == key) {
        return Ok(m.clone());
    }
    let mesh = Arc::new(simplify_mesh(load_mesh(p)?, MAX_SCENE_TRIANGLES));
    let mut held = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    held.retain(|(k, _)| k.0 != key.0);
    if held.len() >= KEEP {
        held.remove(0);
    }
    held.push((key, mesh.clone()));
    Ok(mesh)
}

/// Merge the vertices of each cell of a grid over the model, on the finest grid that leaves
/// at most `max_tris` triangles. A mesh already under the limit is returned untouched.
pub fn simplify_mesh(mesh: Mesh, max_tris: usize) -> Mesh {
    if mesh.tris.len() <= max_tris {
        return mesh;
    }
    let Some((lo, hi)) = mesh.bounds() else { return mesh };
    let extent = (0..3).map(|c| hi[c] - lo[c]).fold(0.0f64, f64::max);
    if extent <= 1e-12 {
        return mesh;
    }
    let mut cells = 512.0f64;
    loop {
        let out = cluster_mesh(&mesh, lo, cells / extent);
        // Triangles on a surface go with the square of the resolution.
        if (out.tris.len() <= max_tris && !out.tris.is_empty()) || cells < 8.0 {
            return if out.tris.is_empty() { mesh } else { out };
        }
        let shrink = (max_tris as f64 / out.tris.len().max(1) as f64).sqrt() * 0.97;
        cells = (cells * shrink.clamp(0.3, 0.95)).floor();
    }
}

fn cluster_mesh(mesh: &Mesh, lo: V3, per_unit: f64) -> Mesh {
    use std::collections::{HashMap, HashSet};
    let mut ids: HashMap<[i32; 3], u32> = HashMap::new();
    let mut sums: Vec<(V3, V3, f64, f64)> = Vec::new();
    let mut corners: Vec<[u32; 3]> = Vec::with_capacity(mesh.tris.len());
    for tri in &mesh.tris {
        let mut c = [0u32; 3];
        for (slot, v) in c.iter_mut().zip(tri) {
            let cell = [0, 1, 2].map(|a| ((v.pos[a] - lo[a]) * per_unit) as i32);
            let id = *ids.entry(cell).or_insert_with(|| {
                sums.push(([0.0; 3], [0.0; 3], 0.0, 0.0));
                (sums.len() - 1) as u32
            });
            let s = &mut sums[id as usize];
            s.0 = add3(s.0, v.pos);
            s.1 = add3(s.1, v.normal);
            s.2 += 1.0;
            s.3 += v.albedo as f64;
            *slot = id;
        }
        corners.push(c);
    }
    let mut seen: HashSet<[u32; 3]> = HashSet::new();
    let mut tris = Vec::new();
    for c in corners {
        if c[0] == c[1] || c[1] == c[2] || c[0] == c[2] {
            continue;
        }
        let mut key = c;
        key.sort_unstable();
        if !seen.insert(key) {
            continue;
        }
        let p = c.map(|i| scale3(sums[i as usize].0, 1.0 / sums[i as usize].2));
        let Some(flat) = unit3(cross3(sub3(p[1], p[0]), sub3(p[2], p[0]))) else { continue };
        // Opposite sides of a thin part can share a cell and cancel: use the face then.
        let n = c.map(|i| {
            let s = &sums[i as usize];
            if norm3(s.1) > 0.3 * s.2 { unit3(s.1).unwrap_or(flat) } else { flat }
        });
        tris.push([0, 1, 2].map(|i| { let s = &sums[c[i] as usize]; Vertex { pos: p[i], normal: n[i], albedo: (s.3 / s.2) as f32 } }));
    }
    Mesh { tris }
}

/// Pack a normalized mesh into a compact little-endian binary payload for WebGL display:
/// `[vertex_count: u32][index_count: u32][positions: f32 * 3V][normals: f32 * 3V][indices: u32 * I]`.
pub fn mesh_to_display_bytes(mesh: &Mesh) -> Result<Vec<u8>, String> {
    let norm = normalize_mesh(mesh)?;
    let mut positions: Vec<f32> = Vec::with_capacity(norm.tris.len() * 3);
    let mut normals: Vec<f32> = Vec::with_capacity(norm.tris.len() * 3);
    let mut indices: Vec<u32> = Vec::with_capacity(norm.tris.len() * 3);
    let mut dedup: std::collections::HashMap<([u32; 3], [u32; 3]), u32> =
        std::collections::HashMap::with_capacity(norm.tris.len());

    for tri in &norm.tris {
        for v in tri {
            let p = [v.pos[0] as f32, v.pos[1] as f32, v.pos[2] as f32];
            let n = [v.normal[0] as f32, v.normal[1] as f32, v.normal[2] as f32];
            let key = (
                [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()],
                [n[0].to_bits(), n[1].to_bits(), n[2].to_bits()],
            );
            let idx = *dedup.entry(key).or_insert_with(|| {
                let id = (positions.len() / 3) as u32;
                positions.extend_from_slice(&p);
                normals.extend_from_slice(&n);
                id
            });
            indices.push(idx);
        }
    }

    let v_count = (positions.len() / 3) as u32;
    let i_count = indices.len() as u32;
    let total_bytes = 8 + positions.len() * 4 + normals.len() * 4 + indices.len() * 4;
    let mut out = Vec::with_capacity(total_bytes);
    out.extend_from_slice(&v_count.to_le_bytes());
    out.extend_from_slice(&i_count.to_le_bytes());
    for f in &positions {
        out.extend_from_slice(&f.to_le_bytes());
    }
    for f in &normals {
        out.extend_from_slice(&f.to_le_bytes());
    }
    for idx in &indices {
        out.extend_from_slice(&idx.to_le_bytes());
    }
    Ok(out)
}

/// Inverse-transpose of the upper-left 3x3 of a row-major 4x4 matrix, or the upper-left 3x3
/// itself if singular.
fn normal_matrix_3x3(m: &[f64; 16]) -> [[f64; 3]; 3] {
    let c0 = [m[0], m[4], m[8]];
    let c1 = [m[1], m[5], m[9]];
    let c2 = [m[2], m[6], m[10]];
    let u0 = cross3(c1, c2);
    let u1 = cross3(c2, c0);
    let u2 = cross3(c0, c1);
    let det = dot3(c0, u0);
    if det.abs() > 1e-12 && det.is_finite() {
        let inv = 1.0 / det;
        [
            [u0[0] * inv, u1[0] * inv, u2[0] * inv],
            [u0[1] * inv, u1[1] * inv, u2[1] * inv],
            [u0[2] * inv, u1[2] * inv, u2[2] * inv],
        ]
    } else {
        [
            [m[0], m[1], m[2]],
            [m[4], m[5], m[6]],
            [m[8], m[9], m[10]],
        ]
    }
}

/// A size that is one number for all three axes, or one for each.
fn one_or_three<'de, D: serde::Deserializer<'de>>(d: D) -> Result<[f64; 3], D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Size {
        One(f64),
        Three([f64; 3]),
    }
    Ok(match Size::deserialize(d)? {
        Size::One(v) => [v; 3],
        Size::Three(v) => v,
    })
}

/// Place an already-normalized mesh according to `obj` and optional row-major scene-space
/// transform `transform` applied about `obj.position_mm`:
/// `scene_p = position_mm + L * (R * (S * p)) + tr`, S scaling each of the model's own axes, then
/// convert into the disc picture frame.
pub(crate) fn place_normalized_with(norm: &Mesh, obj: &SceneObject, transform: Option<&[f64; 16]>) -> Mesh {
    let scale = obj.scale_mm.map(|v| v.clamp(1.0, 200.0));
    let [rx, ry, rz] = obj.rotation_deg;
    let pos = obj.position_mm;
    let n_mat = transform.map(normal_matrix_3x3);
    let tris = norm
        .tris
        .iter()
        .map(|tri| {
            let mut out = [Vertex { pos: [0.0; 3], normal: [0.0, 0.0, 1.0], albedo: 1.0 }; 3];
            for i in 0..3 {
                let p = tri[i].pos;
                let n = tri[i].normal;
                // Normals take the inverse of a stretch, so they stay square to the faces.
                let scaled = rot_xyz([p[0] * scale[0], p[1] * scale[1], p[2] * scale[2]], rx, ry, rz);
                let rot_n = rot_xyz([n[0] / scale[0], n[1] / scale[1], n[2] / scale[2]], rx, ry, rz);
                let scene_p = if let Some(m) = transform {
                    [
                        pos[0] + m[0] * scaled[0] + m[1] * scaled[1] + m[2] * scaled[2] + m[3],
                        pos[1] + m[4] * scaled[0] + m[5] * scaled[1] + m[6] * scaled[2] + m[7],
                        pos[2] + m[8] * scaled[0] + m[9] * scaled[1] + m[10] * scaled[2] + m[11],
                    ]
                } else {
                    add3(scaled, pos)
                };
                let scene_n = if let Some(nm) = n_mat {
                    unit3([dot3(nm[0], rot_n), dot3(nm[1], rot_n), dot3(nm[2], rot_n)]).unwrap_or([0.0, 0.0, 1.0])
                } else {
                    unit3(rot_n).unwrap_or([0.0, 0.0, 1.0])
                };
                out[i] = Vertex {
                    pos: scene_to_disc(scene_p),
                    normal: scene_to_disc(scene_n),
                    albedo: tri[i].albedo,
                };
            }
            out
        })
        .collect();
    Mesh { tris }
}

/// Place a raw mesh according to `obj` and an optional row-major scene-space 4x4 transform
/// applied about `obj.position_mm`, then convert into the disc picture frame via `scene_to_disc`.
pub fn place_scene_object_with(mesh: &Mesh, obj: &SceneObject, transform: Option<&[f64; 16]>) -> Result<Mesh, String> {
    let norm = normalize_mesh(mesh)?;
    Ok(place_normalized_with(&norm, obj, transform))
}

/// Place a raw mesh according to `obj` in scene space (normalize -> Rz*Ry*Rx -> scale -> translate),
/// then convert into the disc picture frame via `scene_to_disc`.
pub fn place_scene_object(mesh: &Mesh, obj: &SceneObject) -> Result<Mesh, String> {
    place_scene_object_with(mesh, obj, None)
}

/// Software-rasterize a slice of placed meshes (already in disc-frame mm) into one shared
/// perspective-correct `1/w` z-buffer with 2×2 supersampling and Lambert + ambient shading.
fn raster_light(
    meshes: &[Mesh],
    view: &ViewSpec,
    size: u32,
    inner_radius_mm: f64,
    outer_radius_mm: f64,
    key_light_disc: V3,
    use_band_mask: bool,
) -> Vec<f32> {
    let ss: usize = 2;
    let w = size as usize * ss;
    let h = w;
    let mut zbuf = vec![0.0f64; w * h];
    let mut lbuf = vec![0.0f32; w * h];

    let eye = view.eye_pos();
    let light_dir = unit3(key_light_disc).unwrap_or([0.0, 0.0, 1.0]);
    let inv_two_ro = 1.0 / (2.0 * outer_radius_mm);
    let wf = w as f64;

    for mesh in meshes {
        for tri in &mesh.tris {
            let Some((x0, y0, dz0)) = project_point(tri[0].pos, eye) else { continue };
            let Some((x1, y1, dz1)) = project_point(tri[1].pos, eye) else { continue };
            let Some((x2, y2, dz2)) = project_point(tri[2].pos, eye) else { continue };

            let area = (x1 - x0) * (y2 - y0) - (y1 - y0) * (x2 - x0);
            if area.abs() < 1e-12 {
                continue;
            }
            let inv_area = 1.0 / area;

            let min_x = x0.min(x1).min(x2);
            let max_x = x0.max(x1).max(x2);
            let min_y = y0.min(y1).min(y2);
            let max_y = y0.max(y1).max(y2);
            if max_x < -outer_radius_mm || min_x > outer_radius_mm || max_y < -outer_radius_mm || min_y > outer_radius_mm {
                continue;
            }

            let ix0 = (((min_x * inv_two_ro + 0.5) * wf).floor() as isize).clamp(0, w as isize - 1) as usize;
            let ix1 = (((max_x * inv_two_ro + 0.5) * wf).ceil() as isize).clamp(0, w as isize - 1) as usize;
            let iy0 = (((min_y * inv_two_ro + 0.5) * wf).floor() as isize).clamp(0, h as isize - 1) as usize;
            let iy1 = (((max_y * inv_two_ro + 0.5) * wf).ceil() as isize).clamp(0, h as isize - 1) as usize;

            let iw0 = 1.0 / dz0;
            let iw1 = 1.0 / dz1;
            let iw2 = 1.0 / dz2;
            let n0w = scale3(tri[0].normal, iw0);
            let n1w = scale3(tri[1].normal, iw1);
            let n2w = scale3(tri[2].normal, iw2);
            let p0w = scale3(tri[0].pos, iw0);
            let p1w = scale3(tri[1].pos, iw1);
            let p2w = scale3(tri[2].pos, iw2);

            for sy in iy0..=iy1 {
                let ym = ((sy as f64 + 0.5) / wf - 0.5) * 2.0 * outer_radius_mm;
                let row_off = sy * w;
                for sx in ix0..=ix1 {
                    let xm = ((sx as f64 + 0.5) / wf - 0.5) * 2.0 * outer_radius_mm;
                    let b0 = ((x1 - xm) * (y2 - ym) - (y1 - ym) * (x2 - xm)) * inv_area;
                    let b1 = ((x2 - xm) * (y0 - ym) - (y2 - ym) * (x0 - xm)) * inv_area;
                    let b2 = 1.0 - b0 - b1;
                    if b0 < -1e-9 || b1 < -1e-9 || b2 < -1e-9 {
                        continue;
                    }
                    let inv_w = b0 * iw0 + b1 * iw1 + b2 * iw2;
                    let idx = row_off + sx;
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
                    let mut n = unit3(n_raw).unwrap_or([0.0, 0.0, 1.0]);
                    // Double-sided shading so open shells and inconsistent-winding OBJs never turn black.
                    if dot3(n, sub3(eye, p_hit)) < 0.0 {
                        n = [-n[0], -n[1], -n[2]];
                    }
                    let ndotl = dot3(n, light_dir).max(0.0);
                    lbuf[idx] = (AMBIENT + DIFFUSE * ndotl) as f32;
                }
            }
        }
    }

    (0..(size * size) as usize)
        .map(|i| {
            let (x, y) = (i % size as usize, i / size as usize);
            let u = (x as f64 + 0.5) / size as f64;
            let v = (y as f64 + 0.5) / size as f64;
            let xm = (u - 0.5) * 2.0 * outer_radius_mm;
            let ym = (v - 0.5) * 2.0 * outer_radius_mm;
            let mask = if use_band_mask {
                band_mask(xm, ym, inner_radius_mm, outer_radius_mm)
            } else {
                radial_mask(xm, ym, inner_radius_mm, outer_radius_mm)
            };
            if mask <= 0.0 {
                return 0.0;
            }
            let (sx, sy) = (x * ss, y * ss);
            let sum = lbuf[sy * w + sx] as f64
                + lbuf[sy * w + sx + 1] as f64
                + lbuf[(sy + 1) * w + sx] as f64
                + lbuf[(sy + 1) * w + sx + 1] as f64;
            (0.25 * sum * mask) as f32
        })
        .collect()
}

fn render_meshes_inner(
    meshes: &[Mesh],
    view: &ViewSpec,
    size: u32,
    inner_radius_mm: f64,
    outer_radius_mm: f64,
    key_light_disc: V3,
    use_band_mask: bool,
) -> RgbaImage {
    let light = raster_light(meshes, view, size, inner_radius_mm, outer_radius_mm, key_light_disc, use_band_mask);
    RgbaImage::from_fn(size, size, |x, y| {
        let g = encode_srgb(light[(y * size + x) as usize] as f64);
        Rgba([g, g, g, 255])
    })
}

/// Brightness of placed meshes seen from `view`, `size` square, with the modelling light turned
/// clockwise by `angle` so that it stays with the viewer rather than with the disc.
pub fn render_scene_light(meshes: &[Mesh], view: &ViewSpec, angle: f64, size: usize, inner_radius_mm: f64, outer_radius_mm: f64) -> Vec<f32> {
    let key = super::turntable::turn(scene_to_disc(KEY_LIGHT_RAW), angle);
    raster_light(meshes, view, size as u32, inner_radius_mm, outer_radius_mm, key, false)
}

/// Software-rasterize `mesh` (in disc-frame mm) for `view` onto a `size × size` full-disc
/// target PNG matching `testscene::render_target`, using perspective projection from
/// `view.eye_pos()`, a perspective-correct `1/w` z-buffer, 2×2 supersampling, grayscale
/// Lambert + ambient shading, and `band_mask`.
pub fn render_target(
    mesh: &Mesh,
    view: &ViewSpec,
    size: u32,
    inner_radius_mm: f64,
    outer_radius_mm: f64,
) -> RgbaImage {
    render_meshes_inner(
        std::slice::from_ref(mesh),
        view,
        size,
        inner_radius_mm,
        outer_radius_mm,
        KEY_LIGHT_RAW,
        true,
    )
}

/// Software-rasterize multiple placed scene meshes (converted via `scene_to_disc`) in one
/// shared z-buffer, masked to the DVD data ring.
pub fn render_scene_target(
    placed_meshes: &[Mesh],
    view: &ViewSpec,
    size: u32,
    inner_radius_mm: f64,
    outer_radius_mm: f64,
) -> RgbaImage {
    render_meshes_inner(
        placed_meshes,
        view,
        size,
        inner_radius_mm,
        outer_radius_mm,
        scene_to_disc(KEY_LIGHT_RAW),
        false,
    )
}

/// Build horizontal head-sway `ViewSpec`s for a 3D scene, converting eye and lamp from scene
/// space (+y up, viewer at -y) into the disc picture frame (+y down, viewer at +y) via `scene_to_disc`.
pub fn scene_head_motion_views(opts: &SceneViewOptions) -> Vec<ViewSpec> {
    let n = opts.views.max(1);
    let a = opts.tilt_deg.to_radians();
    let ey_scene = -opts.distance_mm * a.sin();
    let ez_scene = opts.distance_mm * a.cos();
    let light_disc = scene_to_disc(opts.light_mm);
    (0..n)
        .map(|i| {
            let hx = if n == 1 {
                0.0
            } else {
                -opts.head_half_span_mm + 2.0 * opts.head_half_span_mm * i as f64 / (n - 1) as f64
            };
            let eye_disc = scene_to_disc([hx, ey_scene, ez_scene]);
            ViewSpec::lamp("", eye_disc, light_disc)
        })
        .collect()
}

/// View `k` averaged with its neighbours, in light rather than in sRGB values. The disc cannot
/// tell views this close apart, so a point far from its surface would show as a row of
/// separate copies; blended, it shows as one soft image.
fn blend_neighbours(views: &[RgbaImage], k: usize) -> RgbaImage {
    const W: [f64; 5] = [0.1, 0.23, 0.34, 0.23, 0.1];
    if views.len() < 16 {
        return views[k].clone();
    }
    let lin = |v: u8| (v as f64 / 255.0).powf(2.2);
    RgbaImage::from_fn(views[k].width(), views[k].height(), |x, y| {
        let (mut sum, mut w) = (0.0, 0.0);
        for (d, wd) in W.iter().enumerate() {
            let i = k as isize + d as isize - 2;
            if i >= 0 && (i as usize) < views.len() {
                sum += wd * lin(views[i as usize].get_pixel(x, y)[0]);
                w += wd;
            }
        }
        let g = ((sum / w).powf(1.0 / 2.2) * 255.0).round() as u8;
        Rgba([g, g, g, 255])
    })
}

/// Compose all visible objects in `scene` into placed disc-frame meshes and render `opts.views`
/// head-motion targets into `dir`, returning `ModelViewsOutput` ready for preview or burn.
pub fn write_scene_views(
    scene: &Scene,
    dir: &Path,
    preset: &DiscGeometryPreset,
    opts: &SceneViewOptions,
) -> Result<ModelViewsOutput, String> {
    if scene.version != SCENE_VERSION {
        return Err(format!("不支持的场景版本：{}", scene.version));
    }
    let mut cache: std::collections::HashMap<String, Arc<Mesh>> = std::collections::HashMap::new();
    let mut placed: Vec<Mesh> = Vec::new();
    let mut triangles: usize = 0;

    for obj in &scene.objects {
        if !obj.visible {
            continue;
        }
        let raw = if let Some(m) = cache.get(&obj.path) {
            m
        } else {
            let loaded = load_mesh_or_builtin(&obj.path)?;
            cache.insert(obj.path.clone(), loaded);
            cache.get(&obj.path).unwrap()
        };
        let mesh = place_scene_object(raw, obj)?;
        triangles += mesh.tris.len();
        placed.push(mesh);
    }

    if placed.is_empty() {
        return Err("场景中没有可见模型".into());
    }

    let views_spec = scene_head_motion_views(opts);
    let sharp: Vec<RgbaImage> = views_spec
        .par_iter()
        .map(|v| render_scene_target(&placed, v, opts.target_size, preset.inner_radius_mm, preset.outer_radius_mm))
        .collect();
    let rendered: Vec<(ViewSpec, RgbaImage)> = (0..sharp.len())
        .into_par_iter()
        .map(|k| blend_neighbours(&sharp, k))
        .collect::<Vec<_>>()
        .into_iter()
        .zip(views_spec)
        .map(|(img, v)| (v, img))
        .collect();

    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut views = Vec::with_capacity(rendered.len());
    let mut thumbnails = Vec::with_capacity(rendered.len());
    for (k, (mut view, img)) in rendered.into_iter().enumerate() {
        let path = dir.join(format!("view-{k}.png"));
        img.save(&path).map_err(|e| format!("无法写入场景视角图 {}：{e}", path.display()))?;
        thumbnails.push(thumbnail_data_url(&img, THUMBNAIL_SIZE)?);
        view.image_path = path.to_string_lossy().into_owned();
        views.push(view);
    }
    Ok(ModelViewsOutput { views, thumbnails, triangles })
}

fn thumbnail_data_url(img: &RgbaImage, thumb_size: u32) -> Result<String, String> {
    let small = image::imageops::resize(img, thumb_size, thumb_size, FilterType::Triangle);
    let mut buf = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(small)
        .write_to(&mut buf, ImageFormat::Png)
        .map_err(|e| format!("无法编码缩略图：{e}"))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(buf.into_inner());
    Ok(format!("data:image/png;base64,{b64}"))
}

/// Generate `(ViewSpec, RgbaImage)` pairs for `n` fixed-lamp horizontal head-motion views of `mesh`.
pub fn generate_model_views(
    mesh: &Mesh,
    params: &ModelParams,
    preset: &DiscGeometryPreset,
    n: usize,
    distance_mm: f64,
    tilt_deg: f64,
    half_span_mm: f64,
    light_mm: [f64; 3],
    size: u32,
) -> Result<Vec<(ViewSpec, RgbaImage)>, String> {
    let prepared = prepare_mesh(mesh, params, preset, distance_mm, tilt_deg)?;
    let views = ViewSpec::head_motion_views(n, distance_mm, tilt_deg, -90.0, half_span_mm, light_mm);
    let rendered: Vec<(ViewSpec, RgbaImage)> = views
        .into_par_iter()
        .map(|v| {
            let img = render_target(&prepared, &v, size, preset.inner_radius_mm, preset.outer_radius_mm);
            (v, img)
        })
        .collect();
    Ok(rendered)
}

/// Load a 3D model from `model_path` (OBJ / glTF / GLB), render the default 8 head-motion
/// views (fixed sideways lamp `[250, 0, 400]` mm, 20° tilt, 350 mm distance, ±80 mm head sway)
/// into `dir` as `view-<k>.png`, and return the matching `ViewSpec` list plus base64 thumbnails.
pub fn write_model_views(
    model_path: &Path,
    dir: &Path,
    params: &ModelParams,
    preset: &DiscGeometryPreset,
) -> Result<ModelViewsOutput, String> {
    let mesh = load_mesh(model_path)?;
    let triangles = mesh.tris.len();
    let rendered = generate_model_views(
        &mesh,
        params,
        preset,
        DEFAULT_VIEWS,
        DEFAULT_DISTANCE_MM,
        DEFAULT_TILT_DEG,
        DEFAULT_HEAD_HALF_SPAN_MM,
        DEFAULT_TEST3D_LAMP_MM,
        DEFAULT_TARGET_SIZE,
    )?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut views = Vec::with_capacity(rendered.len());
    let mut thumbnails = Vec::with_capacity(rendered.len());
    for (k, (mut view, img)) in rendered.into_iter().enumerate() {
        let path = dir.join(format!("view-{k}.png"));
        img.save(&path).map_err(|e| format!("无法写入模型视角图 {}：{e}", path.display()))?;
        thumbnails.push(thumbnail_data_url(&img, THUMBNAIL_SIZE)?);
        view.image_path = path.to_string_lossy().into_owned();
        views.push(view);
    }
    Ok(ModelViewsOutput { views, thumbnails, triangles })
}

// ---------------------------------------------------------------------------
// Pure-Rust OBJ & glTF / GLB loaders
// ---------------------------------------------------------------------------

/// Load an OBJ, glTF, or GLB mesh from disk.
pub fn load_mesh(path: &Path) -> Result<Mesh, String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "obj" => load_obj(path),
        "glb" => {
            let bytes = std::fs::read(path).map_err(|e| format!("无法读取模型 {}：{e}", path.display()))?;
            load_glb_bytes(&bytes)
        }
        "gltf" => load_gltf(path),
        _ => {
            let bytes = std::fs::read(path).map_err(|e| format!("无法读取模型 {}：{e}", path.display()))?;
            if bytes.starts_with(b"glTF") {
                load_glb_bytes(&bytes)
            } else {
                let text = String::from_utf8(bytes).map_err(|_| "不支持的 3D 模型格式，请使用 OBJ、glTF 或 GLB".to_string())?;
                if text.trim_start().starts_with('{') {
                    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("glTF JSON 解析失败：{e}"))?;
                    load_gltf_json(&json, path.parent(), None)
                } else {
                    load_obj_str(&text)
                }
            }
        }
    }
}

pub fn load_obj(path: &Path) -> Result<Mesh, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("无法读取 OBJ 文件 {}：{e}", path.display()))?;
    load_obj_str(&text)
}

fn resolve_obj_index(idx: isize, len: usize) -> Option<usize> {
    if idx > 0 {
        let i = (idx - 1) as usize;
        (i < len).then_some(i)
    } else if idx < 0 {
        let i = len as isize + idx;
        (i >= 0 && (i as usize) < len).then_some(i as usize)
    } else {
        None
    }
}

/// Parse Wavefront OBJ text (`v`, `vn`, `f` with positive/negative indices and n-gon fan triangulation).
pub fn load_obj_str(src: &str) -> Result<Mesh, String> {
    let mut positions: Vec<V3> = Vec::new();
    let mut normals: Vec<V3> = Vec::new();
    let mut tris: Vec<[Vertex; 3]> = Vec::new();

    for raw_line in src.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(tag) = parts.next() else { continue };
        match tag {
            "v" => {
                let (Some(x), Some(y), Some(z)) = (parts.next(), parts.next(), parts.next()) else {
                    continue;
                };
                if let (Ok(x), Ok(y), Ok(z)) = (x.parse::<f64>(), y.parse::<f64>(), z.parse::<f64>()) {
                    if x.is_finite() && y.is_finite() && z.is_finite() {
                        positions.push([x, y, z]);
                    }
                }
            }
            "vn" => {
                let (Some(x), Some(y), Some(z)) = (parts.next(), parts.next(), parts.next()) else {
                    continue;
                };
                if let (Ok(x), Ok(y), Ok(z)) = (x.parse::<f64>(), y.parse::<f64>(), z.parse::<f64>()) {
                    normals.push(unit3([x, y, z]).unwrap_or([0.0, 0.0, 1.0]));
                }
            }
            "f" => {
                let mut poly: Vec<(usize, Option<usize>)> = Vec::new();
                for tok in parts {
                    let mut segs = tok.split('/');
                    let Some(v_str) = segs.next() else { continue };
                    let Ok(v_raw) = v_str.parse::<isize>() else { continue };
                    let Some(vi) = resolve_obj_index(v_raw, positions.len()) else { continue };
                    let _vt = segs.next();
                    let ni = segs
                        .next()
                        .filter(|s| !s.is_empty())
                        .and_then(|s| s.parse::<isize>().ok())
                        .and_then(|n_raw| resolve_obj_index(n_raw, normals.len()));
                    poly.push((vi, ni));
                }
                if poly.len() < 3 {
                    continue;
                }
                for i in 1..(poly.len() - 1) {
                    let idx3 = [poly[0], poly[i], poly[i + 1]];
                    let p0 = positions[idx3[0].0];
                    let p1 = positions[idx3[1].0];
                    let p2 = positions[idx3[2].0];
                    let Some(flat_n) = unit3(cross3(sub3(p1, p0), sub3(p2, p0))) else {
                        continue;
                    };
                    let mut tri = [Vertex { pos: [0.0; 3], normal: flat_n, albedo: 1.0 }; 3];
                    for k in 0..3 {
                        tri[k] = Vertex {
                            pos: positions[idx3[k].0],
                            normal: idx3[k].1.map(|ni| normals[ni]).unwrap_or(flat_n),
                            albedo: 1.0,
                        };
                    }
                    tris.push(tri);
                }
            }
            _ => {}
        }
    }

    if tris.is_empty() {
        return Err("OBJ 文件中没有找到有效三角形面".into());
    }
    Ok(Mesh { tris })
}

// --- glTF 2.0 & GLB loader ---

type M4 = [f64; 16]; // column-major, matching glTF 2.0 spec

const M4_ID: M4 = [
    1.0, 0.0, 0.0, 0.0,
    0.0, 1.0, 0.0, 0.0,
    0.0, 0.0, 1.0, 0.0,
    0.0, 0.0, 0.0, 1.0,
];

fn m4_mul(a: &M4, b: &M4) -> M4 {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[col * 4 + k];
            }
            out[col * 4 + row] = sum;
        }
    }
    out
}

fn m4_point(m: &M4, p: V3) -> V3 {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

/// Transform a normal by the cofactor matrix of the upper-left 3×3 (proportional to inverse
/// transpose, with determinant sign applied so mirrored transforms keep outward orientation).
fn m4_normal(m: &M4, n: V3) -> Option<V3> {
    let (a00, a10, a20) = (m[0], m[1], m[2]);
    let (a01, a11, a21) = (m[4], m[5], m[6]);
    let (a02, a12, a22) = (m[8], m[9], m[10]);

    let c00 = a11 * a22 - a12 * a21;
    let c01 = a12 * a20 - a10 * a22;
    let c02 = a10 * a21 - a11 * a20;

    let c10 = a02 * a21 - a01 * a22;
    let c11 = a00 * a22 - a02 * a20;
    let c12 = a01 * a20 - a00 * a21;

    let c20 = a01 * a12 - a02 * a11;
    let c21 = a02 * a10 - a00 * a12;
    let c22 = a00 * a11 - a01 * a10;

    let det = a00 * c00 + a01 * c01 + a02 * c02;
    let sign = if det < 0.0 { -1.0 } else { 1.0 };
    unit3([
        sign * (c00 * n[0] + c10 * n[1] + c20 * n[2]),
        sign * (c01 * n[0] + c11 * n[1] + c21 * n[2]),
        sign * (c02 * n[0] + c12 * n[1] + c22 * n[2]),
    ])
}

fn node_local_matrix(node: &serde_json::Value) -> M4 {
    if let Some(arr) = node.get("matrix").and_then(|v| v.as_array()) {
        if arr.len() == 16 {
            let mut m = [0.0; 16];
            for (i, v) in arr.iter().enumerate() {
                m[i] = v.as_f64().unwrap_or(M4_ID[i]);
            }
            return m;
        }
    }
    let t = node
        .get("translation")
        .and_then(|v| v.as_array())
        .filter(|a| a.len() == 3)
        .map(|a| [a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0), a[2].as_f64().unwrap_or(0.0)])
        .unwrap_or([0.0, 0.0, 0.0]);
    let q = node
        .get("rotation")
        .and_then(|v| v.as_array())
        .filter(|a| a.len() == 4)
        .map(|a| [a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0), a[2].as_f64().unwrap_or(0.0), a[3].as_f64().unwrap_or(1.0)])
        .unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let s = node
        .get("scale")
        .and_then(|v| v.as_array())
        .filter(|a| a.len() == 3)
        .map(|a| [a[0].as_f64().unwrap_or(1.0), a[1].as_f64().unwrap_or(1.0), a[2].as_f64().unwrap_or(1.0)])
        .unwrap_or([1.0, 1.0, 1.0]);

    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, xz, yz) = (x * y, x * z, y * z);
    let (wx, wy, wz) = (w * x, w * y, w * z);

    [
        (1.0 - 2.0 * (yy + zz)) * s[0],
        (2.0 * (xy + wz)) * s[0],
        (2.0 * (xz - wy)) * s[0],
        0.0,
        (2.0 * (xy - wz)) * s[1],
        (1.0 - 2.0 * (xx + zz)) * s[1],
        (2.0 * (yz + wx)) * s[1],
        0.0,
        (2.0 * (xz + wy)) * s[2],
        (2.0 * (yz - wx)) * s[2],
        (1.0 - 2.0 * (xx + yy)) * s[2],
        0.0,
        t[0],
        t[1],
        t[2],
        1.0,
    ]
}

pub fn load_gltf(path: &Path) -> Result<Mesh, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("无法读取 glTF 文件 {}：{e}", path.display()))?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("glTF JSON 解析失败：{e}"))?;
    load_gltf_json(&json, path.parent(), None)
}

pub fn load_glb_bytes(bytes: &[u8]) -> Result<Mesh, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"glTF" {
        return Err("不是有效的 GLB 文件（缺少 glTF 文件头）".into());
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    if version != 2 {
        return Err(format!("不支持的 GLB 版本：{version}（仅支持 glTF 2.0）"));
    }
    let total_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let end = total_len.min(bytes.len());
    let mut off = 12usize;
    let mut json_bytes: Option<&[u8]> = None;
    let mut bin_bytes: Option<&[u8]> = None;
    while off + 8 <= end {
        let chunk_len = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        let chunk_type = u32::from_le_bytes(bytes[off + 4..off + 8].try_into().unwrap());
        off += 8;
        if off + chunk_len > end {
            return Err("GLB 数据块超出文件长度".into());
        }
        let data = &bytes[off..off + chunk_len];
        match chunk_type {
            0x4E4F_534A => json_bytes = Some(data), // "JSON"
            0x004E_4942 => bin_bytes = Some(data),  // "BIN\0"
            _ => {}
        }
        off += chunk_len;
    }
    let json_raw = json_bytes.ok_or_else(|| "GLB 文件缺少 JSON 块".to_string())?;
    let json: serde_json::Value = serde_json::from_slice(json_raw).map_err(|e| format!("GLB JSON 解析失败：{e}"))?;
    load_gltf_json(&json, None, bin_bytes)
}

/// The colour a glTF's materials give its surfaces, as lightness.
struct GltfTextures {
    /// Per material: its own lightness, and the image and texture coordinates that multiply it.
    materials: Vec<(f32, Option<(usize, u64)>)>,
    images: Vec<Option<image::GrayImage>>,
}

impl GltfTextures {
    fn load(json: &serde_json::Value, buffers: &[Vec<u8>], views: &[serde_json::Value], base_dir: Option<&Path>) -> Self {
        let list = |k: &str| json.get(k).and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let textures = list("textures");
        let mut wanted = std::collections::HashSet::new();
        let materials: Vec<(f32, Option<(usize, u64)>)> = list("materials")
            .iter()
            .map(|m| {
                let pbr = m.get("pbrMetallicRoughness");
                let factor = pbr.and_then(|p| p.get("baseColorFactor")).and_then(|v| v.as_array()).map(|a| {
                    let c = |i: usize| a.get(i).and_then(|v| v.as_f64()).unwrap_or(1.0);
                    (0.2126 * c(0) + 0.7152 * c(1) + 0.0722 * c(2)) as f32
                });
                let tex = pbr.and_then(|p| p.get("baseColorTexture")).and_then(|t| {
                    let source = textures.get(t.get("index")?.as_u64()? as usize)?.get("source")?.as_u64()? as usize;
                    wanted.insert(source);
                    Some((source, t.get("texCoord").and_then(|v| v.as_u64()).unwrap_or(0)))
                });
                (factor.unwrap_or(1.0), tex)
            })
            .collect();
        let images = list("images")
            .iter()
            .enumerate()
            .map(|(i, im)| {
                if !wanted.contains(&i) {
                    return None;
                }
                let bytes: Vec<u8> = if let Some(v) = im.get("bufferView").and_then(|v| v.as_u64()) {
                    let v = views.get(v as usize)?;
                    let num = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                    buffers.get(num("buffer"))?.get(num("byteOffset")..num("byteOffset") + num("byteLength"))?.to_vec()
                } else {
                    let uri = im.get("uri")?.as_str()?;
                    match uri.strip_prefix("data:").and_then(|r| r.split_once(',')) {
                        Some((_, b64)) => base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()?,
                        None => std::fs::read(base_dir?.join(uri)).ok()?,
                    }
                };
                // A picture that cannot be read leaves the surface plain.
                image::load_from_memory(&bytes).ok().map(|i| i.thumbnail(2048, 2048).to_luma8())
            })
            .collect();
        GltfTextures { materials, images }
    }

    fn texcoord(&self, material: Option<usize>) -> Option<u64> {
        self.materials.get(material?)?.1.map(|t| t.1)
    }

    fn albedo(&self, material: Option<usize>, uv: Option<[f32; 2]>) -> f32 {
        let Some((factor, tex)) = material.and_then(|m| self.materials.get(m)) else { return 1.0 };
        let texel = tex.zip(uv).and_then(|((i, _), uv)| {
            let im = self.images.get(i)?.as_ref()?;
            let at = |t: f32, n: u32| (((t - t.floor()) * n as f32) as u32).min(n - 1);
            let v = im.get_pixel(at(uv[0], im.width()), at(uv[1], im.height()))[0] as f32 / 255.0;
            Some(v.powf(2.2))
        });
        factor * texel.unwrap_or(1.0)
    }
}

fn load_gltf_json(
    json: &serde_json::Value,
    base_dir: Option<&Path>,
    bin_chunk: Option<&[u8]>,
) -> Result<Mesh, String> {
    // 1. Resolve buffers
    let mut buffers: Vec<Vec<u8>> = Vec::new();
    if let Some(arr) = json.get("buffers").and_then(|v| v.as_array()) {
        for (bi, b) in arr.iter().enumerate() {
            if let Some(uri) = b.get("uri").and_then(|v| v.as_str()) {
                if let Some(comma) = uri.strip_prefix("data:").and_then(|rest| rest.find(',').map(|i| &rest[i + 1..])) {
                    let decoded = base64::engine::general_purpose::STANDARD
                        .decode(comma.trim())
                        .map_err(|e| format!("glTF 内嵌 buffer[{bi}] base64 解码失败：{e}"))?;
                    buffers.push(decoded);
                } else {
                    let dir = base_dir.ok_or_else(|| format!("glTF 外部 buffer `{uri}` 缺少基准目录"))?;
                    let buf_path = dir.join(uri);
                    let data = std::fs::read(&buf_path).map_err(|e| format!("无法读取 glTF buffer {}：{e}", buf_path.display()))?;
                    buffers.push(data);
                }
            } else if bi == 0 {
                let bin = bin_chunk.ok_or_else(|| "glTF buffer[0] 缺少 URI 且没有 GLB BIN 数据块".to_string())?;
                buffers.push(bin.to_vec());
            } else {
                return Err(format!("glTF buffer[{bi}] 缺少 URI"));
            }
        }
    }

    let buffer_views = json.get("bufferViews").and_then(|v| v.as_array()).map(|v| v.as_slice()).unwrap_or(&[]);
    let accessors = json.get("accessors").and_then(|v| v.as_array()).map(|v| v.as_slice()).unwrap_or(&[]);
    let meshes = json.get("meshes").and_then(|v| v.as_array()).map(|v| v.as_slice()).unwrap_or(&[]);
    let nodes = json.get("nodes").and_then(|v| v.as_array()).map(|v| v.as_slice()).unwrap_or(&[]);

    let read_accessor_slice = |acc_idx: usize| -> Result<(&[u8], usize, usize, u64), String> {
        let acc = accessors.get(acc_idx).ok_or_else(|| format!("glTF accessor[{acc_idx}] 不存在"))?;
        let bv_idx = acc
            .get("bufferView")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| format!("glTF accessor[{acc_idx}] 缺少 bufferView"))? as usize;
        let bv = buffer_views.get(bv_idx).ok_or_else(|| format!("glTF bufferView[{bv_idx}] 不存在"))?;
        let buf_idx = bv.get("buffer").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let buf = buffers.get(buf_idx).ok_or_else(|| format!("glTF buffer[{buf_idx}] 不存在"))?;
        let bv_off = bv.get("byteOffset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let acc_off = acc.get("byteOffset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let start = bv_off + acc_off;
        if start > buf.len() {
            return Err(format!("glTF accessor[{acc_idx}] 偏移越界"));
        }
        let count = acc.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let stride = bv.get("byteStride").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let comp = acc.get("componentType").and_then(|v| v.as_u64()).unwrap_or(0);
        Ok((&buf[start..], count, stride, comp))
    };

    let read_vec3 = |acc_idx: usize| -> Result<Vec<V3>, String> {
        let (data, count, stride, comp) = read_accessor_slice(acc_idx)?;
        if comp != 5126 {
            return Err(format!("glTF VEC3 accessor[{acc_idx}] 仅支持 FLOAT (5126)，实际为 {comp}"));
        }
        let step = if stride == 0 { 12 } else { stride };
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let o = i * step;
            if o + 12 > data.len() {
                return Err(format!("glTF accessor[{acc_idx}] 数据截断"));
            }
            let x = f32::from_le_bytes(data[o..o + 4].try_into().unwrap()) as f64;
            let y = f32::from_le_bytes(data[o + 4..o + 8].try_into().unwrap()) as f64;
            let z = f32::from_le_bytes(data[o + 8..o + 12].try_into().unwrap()) as f64;
            out.push([x, y, z]);
        }
        Ok(out)
    };

    let read_indices = |acc_idx: usize| -> Result<Vec<usize>, String> {
        let (data, count, stride, comp) = read_accessor_slice(acc_idx)?;
        let elem = match comp {
            5121 => 1usize, // UNSIGNED_BYTE
            5123 => 2usize, // UNSIGNED_SHORT
            5125 => 4usize, // UNSIGNED_INT
            _ => return Err(format!("不支持的 glTF 索引类型：{comp}")),
        };
        let step = if stride == 0 { elem } else { stride };
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let o = i * step;
            if o + elem > data.len() {
                return Err(format!("glTF 索引 accessor[{acc_idx}] 数据截断"));
            }
            let val = match comp {
                5121 => data[o] as usize,
                5123 => u16::from_le_bytes(data[o..o + 2].try_into().unwrap()) as usize,
                5125 => u32::from_le_bytes(data[o..o + 4].try_into().unwrap()) as usize,
                _ => unreachable!(),
            };
            out.push(val);
        }
        Ok(out)
    };

    let textures = GltfTextures::load(json, &buffers, buffer_views, base_dir);
    let read_uv = |acc_idx: usize| -> Option<Vec<[f32; 2]>> {
        let (data, count, stride, comp) = read_accessor_slice(acc_idx).ok()?;
        let (size, scale) = match comp { 5126 => (4, 1.0), 5123 => (2, 1.0 / 65535.0), 5121 => (1, 1.0 / 255.0), _ => return None };
        let step = if stride == 0 { size * 2 } else { stride };
        (0..count)
            .map(|i| {
                let at = |c: usize| -> Option<f32> {
                    let b = data.get(i * step + c * size..i * step + (c + 1) * size)?;
                    Some(match comp {
                        5126 => f32::from_le_bytes(b.try_into().ok()?),
                        5123 => u16::from_le_bytes(b.try_into().ok()?) as f32 * scale,
                        _ => b[0] as f32 * scale,
                    })
                };
                Some([at(0)?, at(1)?])
            })
            .collect()
    };

    let mut tris: Vec<[Vertex; 3]> = Vec::new();
    let mut emit_mesh = |mesh_idx: usize, world: &M4| -> Result<(), String> {
        let Some(m) = meshes.get(mesh_idx) else { return Ok(()) };
        let Some(prims) = m.get("primitives").and_then(|v| v.as_array()) else { return Ok(()) };
        for prim in prims {
            let mode = prim.get("mode").and_then(|v| v.as_u64()).unwrap_or(4);
            if mode != 4 && mode != 5 && mode != 6 {
                continue;
            }
            let Some(pos_acc) = prim
                .get("attributes")
                .and_then(|a| a.get("POSITION"))
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
            else {
                continue;
            };
            let raw_pos = read_vec3(pos_acc)?;
            let raw_nrm = match prim
                .get("attributes")
                .and_then(|a| a.get("NORMAL"))
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
            {
                Some(ni) => Some(read_vec3(ni)?),
                None => None,
            };
            let material = prim.get("material").and_then(|v| v.as_u64()).map(|v| v as usize);
            let uv = textures.texcoord(material).and_then(|set| prim.get("attributes")?.get(format!("TEXCOORD_{set}"))?.as_u64()).and_then(|a| read_uv(a as usize));
            let shade = |i: usize| textures.albedo(material, uv.as_ref().and_then(|u| u.get(i)).copied());
            let indices: Vec<usize> = match prim.get("indices").and_then(|v| v.as_u64()).map(|v| v as usize) {
                Some(ii) => read_indices(ii)?,
                None => (0..raw_pos.len()).collect(),
            };

            let mut push_tri = |i0: usize, i1: usize, i2: usize| {
                if i0 >= raw_pos.len() || i1 >= raw_pos.len() || i2 >= raw_pos.len() {
                    return;
                }
                let p0 = m4_point(world, raw_pos[i0]);
                let p1 = m4_point(world, raw_pos[i1]);
                let p2 = m4_point(world, raw_pos[i2]);
                let Some(flat_n) = unit3(cross3(sub3(p1, p0), sub3(p2, p0))) else { return };
                let n_of = |idx: usize| -> V3 {
                    raw_nrm
                        .as_ref()
                        .and_then(|ns| ns.get(idx))
                        .and_then(|&n| m4_normal(world, n))
                        .unwrap_or(flat_n)
                };
                tris.push([
                    Vertex { pos: p0, normal: n_of(i0), albedo: shade(i0) },
                    Vertex { pos: p1, normal: n_of(i1), albedo: shade(i1) },
                    Vertex { pos: p2, normal: n_of(i2), albedo: shade(i2) },
                ]);
            };

            match mode {
                4 => {
                    for chunk in indices.chunks_exact(3) {
                        push_tri(chunk[0], chunk[1], chunk[2]);
                    }
                }
                5 => {
                    for i in 0..indices.len().saturating_sub(2) {
                        if i % 2 == 0 {
                            push_tri(indices[i], indices[i + 1], indices[i + 2]);
                        } else {
                            push_tri(indices[i + 1], indices[i], indices[i + 2]);
                        }
                    }
                }
                6 => {
                    for i in 1..indices.len().saturating_sub(1) {
                        push_tri(indices[0], indices[i], indices[i + 1]);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    };

    // Traverse scene graph if nodes exist; otherwise emit all meshes with identity transform.
    if nodes.is_empty() {
        for mi in 0..meshes.len() {
            emit_mesh(mi, &M4_ID)?;
        }
    } else {
        let roots: Vec<usize> = if let Some(scenes) = json.get("scenes").and_then(|v| v.as_array()) {
            let si = json.get("scene").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            scenes
                .get(si)
                .and_then(|s| s.get("nodes"))
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_u64().map(|x| x as usize)).collect())
                .unwrap_or_default()
        } else {
            let mut is_child = vec![false; nodes.len()];
            for n in nodes {
                if let Some(ch) = n.get("children").and_then(|v| v.as_array()) {
                    for c in ch {
                        if let Some(ci) = c.as_u64().map(|x| x as usize) {
                            if ci < is_child.len() {
                                is_child[ci] = true;
                            }
                        }
                    }
                }
            }
            (0..nodes.len()).filter(|&i| !is_child[i]).collect()
        };

        let mut stack: Vec<(usize, M4)> = roots.into_iter().map(|r| (r, M4_ID)).collect();
        let mut visited = 0usize;
        while let Some((ni, parent_m)) = stack.pop() {
            visited += 1;
            if visited > nodes.len().saturating_mul(16).max(64) {
                break;
            }
            let Some(node) = nodes.get(ni) else { continue };
            let world = m4_mul(&parent_m, &node_local_matrix(node));
            if let Some(mi) = node.get("mesh").and_then(|v| v.as_u64()).map(|x| x as usize) {
                emit_mesh(mi, &world)?;
            }
            if let Some(ch) = node.get("children").and_then(|v| v.as_array()) {
                for c in ch {
                    if let Some(ci) = c.as_u64().map(|x| x as usize) {
                        stack.push((ci, world));
                    }
                }
            }
        }
    }

    if tris.is_empty() {
        return Err("glTF/GLB 文件中没有找到有效三角形网格".into());
    }
    Ok(Mesh { tris })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holo::design::{self, DesignInput};
    use crate::preset::reference_preset;

    #[test]
    fn a_model_stretches_along_its_own_axes_and_old_sizes_still_read() {
        let norm = normalize_mesh(&load_mesh_or_builtin(BUILTIN_SAMPLE_PATH).unwrap()).unwrap();
        let extent = |m: &Mesh, axis: usize| {
            let v = m.tris.iter().flatten().map(|v| v.pos[axis]);
            v.clone().fold(f64::MIN, f64::max) - v.fold(f64::MAX, f64::min)
        };
        let obj: SceneObject = serde_json::from_value(serde_json::json!({
            "id": "o", "path": BUILTIN_SAMPLE_PATH, "name": "s", "positionMm": [0, 0, 0], "rotationDeg": [0, 0, 0], "scaleMm": 20
        }))
        .unwrap();
        assert_eq!(obj.scale_mm, [20.0; 3], "a single size is the same on all three axes");
        let even = place_normalized_with(&norm, &obj, None);
        let wide = place_normalized_with(&norm, &SceneObject { scale_mm: [40.0, 20.0, 20.0], ..obj.clone() }, None);
        assert!((extent(&wide, 0) / extent(&even, 0) - 2.0).abs() < 1e-9);
        assert!((extent(&wide, 1) - extent(&even, 1)).abs() < 1e-9);
        // Turned a quarter about z first, the stretch goes with the model: across the disc it is now tall.
        let turned = place_normalized_with(&norm, &SceneObject { scale_mm: [40.0, 20.0, 20.0], rotation_deg: [0.0, 0.0, 90.0], ..obj.clone() }, None);
        assert!((extent(&turned, 1) / extent(&even, 0) - 2.0).abs() < 1e-6, "{} {}", extent(&turned, 1), extent(&even, 0));
    }

    fn unit_cube_obj() -> &'static str {
        "\
        v -1 -1 -1\n\
        v  1 -1 -1\n\
        v  1  1 -1\n\
        v -1  1 -1\n\
        v -1 -1  1\n\
        v  1 -1  1\n\
        v  1  1  1\n\
        v -1  1  1\n\
        f 5 6 7 8\n\
        f 1 4 3 2\n\
        f 1 2 6 5\n\
        f 3 4 8 7\n\
        f 2 3 7 6\n\
        f 4 1 5 8\n"
    }

    /// Weighted x-centroid of rendered target brightness (in mm).
    fn brightness_centroid_x(img: &RgbaImage, outer_radius_mm: f64) -> f64 {
        let size = img.width();
        let (mut sum_w, mut sum_x) = (0.0, 0.0);
        for y in 0..size {
            for x in 0..size {
                let g = img.get_pixel(x, y)[0] as f64 / 255.0;
                if g > 0.0 {
                    let xm = ((x as f64 + 0.5) / size as f64 - 0.5) * 2.0 * outer_radius_mm;
                    sum_w += g;
                    sum_x += g * xm;
                }
            }
        }
        assert!(sum_w > 0.0, "rendered target must have non-zero brightness");
        sum_x / sum_w
    }

    #[test]
    fn cube_front_face_shifts_opposite_head_x_and_z0_stays_fixed() {
        let preset = reference_preset();
        let views = ViewSpec::head_motion_views(
            DEFAULT_VIEWS,
            DEFAULT_DISTANCE_MM,
            DEFAULT_TILT_DEG,
            -90.0,
            DEFAULT_HEAD_HALF_SPAN_MM,
            DEFAULT_TEST3D_LAMP_MM,
        );

        // 1. Analytic perspective projection checks: z=0 points never move; z>0 moves left as head moves right; z<0 moves right.
        let left_eye = views[0].eye_pos();
        let right_eye = views[7].eye_pos();
        assert!(left_eye[0] < 0.0 && right_eye[0] > 0.0);

        let p_z0 = [3.5, -41.0, 0.0];
        let (lx0, ly0, _) = project_point(p_z0, left_eye).unwrap();
        let (rx0, ry0, _) = project_point(p_z0, right_eye).unwrap();
        assert!((lx0 - p_z0[0]).abs() < 1e-12 && (ly0 - p_z0[1]).abs() < 1e-12);
        assert!((rx0 - p_z0[0]).abs() < 1e-12 && (ry0 - p_z0[1]).abs() < 1e-12);

        let p_front = [0.0, -41.0, 20.0];
        let (lxf, _, _) = project_point(p_front, left_eye).unwrap();
        let (rxf, _, _) = project_point(p_front, right_eye).unwrap();
        assert!(
            rxf < lxf - 8.0,
            "floating point (+20 mm) must shift left as head moves right: left_x={lxf:.2}, right_x={rxf:.2}"
        );

        let p_back = [0.0, -41.0, -20.0];
        let (lxb, _, _) = project_point(p_back, left_eye).unwrap();
        let (rxb, _, _) = project_point(p_back, right_eye).unwrap();
        assert!(
            rxb > lxb + 8.0,
            "sunk point (-20 mm) must shift right as head moves right: left_x={lxb:.2}, right_x={rxb:.2}"
        );

        // 2. Rasterized cube: front face at z > 0 shifts left when head_x increases; thin plate at z = 0 stays fixed.
        let cube = load_obj_str(unit_cube_obj()).unwrap();
        assert_eq!(cube.tris.len(), 12);

        let floating_cube = prepare_mesh(
            &cube,
            &ModelParams { scale_mm: 16.0, depth_mm: 15.0, ..Default::default() },
            &preset,
            DEFAULT_DISTANCE_MM,
            DEFAULT_TILT_DEG,
        )
        .unwrap();
        let img_left = render_target(&floating_cube, &views[0], 256, preset.inner_radius_mm, preset.outer_radius_mm);
        let img_right = render_target(&floating_cube, &views[7], 256, preset.inner_radius_mm, preset.outer_radius_mm);
        let cx_left = brightness_centroid_x(&img_left, preset.outer_radius_mm);
        let cx_right = brightness_centroid_x(&img_right, preset.outer_radius_mm);
        assert!(
            cx_right < cx_left - 5.0,
            "floating cube centroid must shift left as head moves right: left={cx_left:.2}, right={cx_right:.2}"
        );

        // Flat square pinned strictly to z = 0 in the near band must not move at all across head positions.
        let flat_z0 = Mesh {
            tris: vec![
                [
                    Vertex { pos: [-6.0, -47.0, 0.0], normal: [0.0, 0.0, 1.0], albedo: 1.0 },
                    Vertex { pos: [6.0, -47.0, 0.0], normal: [0.0, 0.0, 1.0], albedo: 1.0 },
                    Vertex { pos: [6.0, -35.0, 0.0], normal: [0.0, 0.0, 1.0], albedo: 1.0 },
                ],
                [
                    Vertex { pos: [-6.0, -47.0, 0.0], normal: [0.0, 0.0, 1.0], albedo: 1.0 },
                    Vertex { pos: [6.0, -35.0, 0.0], normal: [0.0, 0.0, 1.0], albedo: 1.0 },
                    Vertex { pos: [-6.0, -35.0, 0.0], normal: [0.0, 0.0, 1.0], albedo: 1.0 },
                ],
            ],
        };
        let z0_left = render_target(&flat_z0, &views[0], 256, preset.inner_radius_mm, preset.outer_radius_mm);
        let z0_right = render_target(&flat_z0, &views[7], 256, preset.inner_radius_mm, preset.outer_radius_mm);
        assert_eq!(z0_left.as_raw(), z0_right.as_raw(), "geometry strictly at z=0 must render identically across head positions");
    }

    #[test]
    fn band_mask_accepts_near_far_and_rejects_sides_and_hole() {
        let preset = reference_preset();
        let (ri, ro) = (preset.inner_radius_mm, preset.outer_radius_mm);
        // Near & far band centres: full weight
        assert!((band_mask(0.0, -41.0, ri, ro) - 1.0).abs() < 1e-12);
        assert!((band_mask(0.0, 41.0, ri, ro) - 1.0).abs() < 1e-12);
        // 3 & 9 o'clock dead zones: zero weight
        assert_eq!(band_mask(41.0, 0.0, ri, ro), 0.0);
        assert_eq!(band_mask(-41.0, 0.0, ri, ro), 0.0);
        // Inside centre hole & outside outer edge: zero weight
        assert_eq!(band_mask(0.0, -15.0, ri, ro), 0.0);
        assert_eq!(band_mask(0.0, -60.0, ri, ro), 0.0);
        // Smooth fade between 25° and 40°
        let mid_az = band_mask(41.0 * 32.5f64.to_radians().sin(), -41.0 * 32.5f64.to_radians().cos(), ri, ro);
        assert!(mid_az > 0.2 && mid_az < 0.8, "expected smooth transition at 32.5°, got {mid_az}");
    }

    #[test]
    fn obj_loader_handles_negative_indices_and_quads() {
        let obj = "\
            v 0 0 0\n\
            v 1 0 0\n\
            v 1 1 0\n\
            v 0 1 0\n\
            vn 0 0 1\n\
            f -4//-1 -3//-1 -2//-1 -1//-1\n";
        let m = load_obj_str(obj).unwrap();
        assert_eq!(m.tris.len(), 2);
        for tri in &m.tris {
            for v in tri {
                assert!((v.normal[2] - 1.0).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn glb_loader_parses_valid_binary_gltf_with_node_transform() {
        // Build a minimal valid GLB with one indexed triangle and a node translation.
        let mut bin = Vec::new();
        for f in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            bin.extend_from_slice(&f.to_le_bytes());
        }
        let idx_off = bin.len();
        for idx in [0u16, 1, 2] {
            bin.extend_from_slice(&idx.to_le_bytes());
        }
        while bin.len() % 4 != 0 {
            bin.push(0);
        }

        let json_str = format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0,"translation":[10.0,20.0,30.0]}}],"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"indices":1}}]}}],"buffers":[{{"byteLength":{}}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},{{"buffer":0,"byteOffset":{},"byteLength":6}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}},{{"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"}}]}}"#,
            bin.len(),
            idx_off
        );
        let mut json_bytes = json_str.into_bytes();
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }

        let total_len = 12 + 8 + json_bytes.len() + 8 + bin.len();
        let mut glb = Vec::with_capacity(total_len);
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&(total_len as u32).to_le_bytes());
        glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
        glb.extend_from_slice(&json_bytes);
        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(&0x004E_4942u32.to_le_bytes());
        glb.extend_from_slice(&bin);

        let mesh = load_glb_bytes(&glb).unwrap();
        assert_eq!(mesh.tris.len(), 1);
        assert!((mesh.tris[0][0].pos[0] - 10.0).abs() < 1e-6);
        assert!((mesh.tris[0][0].pos[1] - 20.0).abs() < 1e-6);
        assert!((mesh.tris[0][0].pos[2] - 30.0).abs() < 1e-6);
        assert!((mesh.tris[0][1].pos[0] - 11.0).abs() < 1e-6);
    }

    #[test]
    fn sample_star_obj_solves_and_renders_with_near_field_parallax() {
        let preset = reference_preset();
        let model_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/holo/models/star.obj");
        let dir = std::env::temp_dir().join(format!("holo-model3d-test-{}", std::process::id()));
        let out = write_model_views(&model_path, &dir, &ModelParams::default(), &preset).unwrap();
        assert_eq!(out.views.len(), DEFAULT_VIEWS);
        assert_eq!(out.thumbnails.len(), DEFAULT_VIEWS);
        assert_eq!(out.triangles, 20);
        assert!(out.thumbnails[0].starts_with("data:image/png;base64,"));

        let shared = design::shared_codebook(preset.channel_bit_length_nm * 1e-3);
        let (_, resp) = &*shared;
        let solved = design::solve(
            &DesignInput { views: out.views.clone(), chroma_weight: 0.0, exposure: 0.6, grid: 48 },
            resp,
            &preset,
        )
        .unwrap();
        let r0 = design::render(&solved, resp, &preset, &out.views[0], 96, 1.0);
        let r7 = design::render(&solved, resp, &preset, &out.views[7], 96, 1.0);
        assert_ne!(r0, r7, "leftmost and rightmost rendered hologram views must differ");

        // Verify 3 & 9 o'clock regions in target PNGs are strictly masked to black.
        let t0 = image::open(&out.views[0].image_path).unwrap().to_rgba8();
        let mid = t0.height() / 2;
        assert_eq!(t0.get_pixel(t0.width() / 8, mid)[0], 0);
        assert_eq!(t0.get_pixel(t0.width() * 7 / 8, mid)[0], 0);

        // Verify disc ROLL ±5° changes the model ROI far more than PITCH ±5° (horizontal-track physics).
        let rot_y = |p: [f64; 3], deg: f64| -> [f64; 3] {
            let a = deg.to_radians();
            [p[0] * a.cos() + p[2] * a.sin(), p[1], -p[0] * a.sin() + p[2] * a.cos()]
        };
        let rot_x = |p: [f64; 3], deg: f64| -> [f64; 3] {
            let a = deg.to_radians();
            [p[0], p[1] * a.cos() - p[2] * a.sin(), p[1] * a.sin() + p[2] * a.cos()]
        };
        let size = 96usize;
        let tgt_small = image::imageops::resize(&t0, size as u32, size as u32, FilterType::Triangle);
        let roi: Vec<usize> = (0..size * size)
            .filter(|&i| tgt_small.get_pixel((i % size) as u32, (i / size) as u32)[0] > 10)
            .collect();
        assert!(roi.len() > 20, "model ROI should cover enough pixels");
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
        let r_neg = luma_roi(&design::render(&solved, resp, &preset, &ViewSpec::lamp("", rot_y(eye0, -5.0), rot_y(lamp0, -5.0)), size, 0.0));
        let r_pos = luma_roi(&design::render(&solved, resp, &preset, &ViewSpec::lamp("", rot_y(eye0, 5.0), rot_y(lamp0, 5.0)), size, 0.0));
        let p_neg = luma_roi(&design::render(&solved, resp, &preset, &ViewSpec::lamp("", rot_x(eye0, -5.0), rot_x(lamp0, -5.0)), size, 0.0));
        let p_pos = luma_roi(&design::render(&solved, resp, &preset, &ViewSpec::lamp("", rot_x(eye0, 5.0), rot_x(lamp0, 5.0)), size, 0.0));
        let roll_diff = 1.0 - pearson(&r_neg, &r_pos);
        let pitch_diff = 1.0 - pearson(&p_neg, &p_pos);
        assert!(
            roll_diff > 2.0 * pitch_diff,
            "near-band 3D model ROLL ±5° must vary far more than PITCH ±5°: roll={roll_diff:.3}, pitch={pitch_diff:.3}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Generate the near-field simulated head-sway strip for `docs/holo/models/star.obj`:
    /// `MODEL_SWAY_OUT=model-sway.png cargo test -p disc-core --release holo::model3d::tests::render_model_sway_strip -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn render_model_sway_strip() {
        let preset = reference_preset();
        let model_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/holo/models/star.obj");
        let out_path = std::env::var("MODEL_SWAY_OUT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("model-sway.png"));

        let tmp = std::env::temp_dir().join(format!("holo-model-sway-{}", std::process::id()));
        let params = ModelParams::default();
        let generated = write_model_views(&model_path, &tmp, &params, &preset).unwrap();

        let shared = design::shared_codebook(preset.channel_bit_length_nm * 1e-3);
        let (_, resp) = &*shared;
        let solved = design::solve(
            &DesignInput { views: generated.views.clone(), chroma_weight: 0.0, exposure: 0.6, grid: 256 },
            resp,
            &preset,
        )
        .unwrap();

        // Compose a 2-row × 8-column strip (top: rasterized targets, bottom: near-field hologram renders).
        let tile = 256u32;
        let cols = generated.views.len() as u32;
        let mut strip = RgbaImage::from_pixel(tile * cols, tile * 2, Rgba([16, 16, 20, 255]));
        for (k, v) in generated.views.iter().enumerate() {
            let tgt = image::open(&v.image_path).unwrap().to_rgba8();
            let tgt_small = image::imageops::resize(&tgt, tile, tile, FilterType::Triangle);
            let ren_raw = design::render(&solved, resp, &preset, v, tile as usize, 1.0);
            let ren_img = RgbaImage::from_raw(tile, tile, ren_raw).unwrap();
            let x0 = k as u32 * tile;
            for y in 0..tile {
                for x in 0..tile {
                    let tp = *tgt_small.get_pixel(x, y);
                    strip.put_pixel(x0 + x, y, tp);
                    let rp = *ren_img.get_pixel(x, y);
                    if rp[3] > 0 {
                        strip.put_pixel(x0 + x, tile + y, rp);
                    }
                }
            }
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        strip.save(&out_path).unwrap();
        println!("Saved model sway strip to {} (solve mse={:.6})", out_path.display(), solved.error);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Build a chiral 3D letter "F" in scene space (+x right, +y up, +z toward viewer):
    /// - Vertical spine on the left:  x in [-0.5, -0.2], y in [-0.5, +0.5]
    /// - Top horizontal bar:          x in [-0.2, +0.5], y in [+0.25, +0.5]
    /// - Middle horizontal bar:       x in [-0.2, +0.25], y in [-0.1, +0.1]
    /// Bottom-right (x > 0, y < -0.15) is empty, uniquely distinguishing upright F from all
    /// mirrored/rotated reflections.
    fn chiral_letter_f_mesh() -> Mesh {
        let mut tris = Vec::new();
        let mut add_rect = |x0: f64, y0: f64, x1: f64, y1: f64, z: f64| {
            let n = [0.0, 0.0, 1.0];
            let v00 = Vertex { pos: [x0, y0, z], normal: n, albedo: 1.0 };
            let v10 = Vertex { pos: [x1, y0, z], normal: n, albedo: 1.0 };
            let v11 = Vertex { pos: [x1, y1, z], normal: n, albedo: 1.0 };
            let v01 = Vertex { pos: [x0, y1, z], normal: n, albedo: 1.0 };
            tris.push([v00, v10, v11]);
            tris.push([v00, v11, v01]);
        };
        add_rect(-0.5, -0.5, -0.2, 0.5, 0.0); // spine (left)
        add_rect(-0.2, 0.25, 0.5, 0.5, 0.0);  // top bar (top-right)
        add_rect(-0.2, -0.1, 0.25, 0.1, 0.0); // middle bar
        Mesh { tris }
    }

    #[test]
    fn chiral_letter_f_matches_upright_disc_picture_without_mirroring() {
        let preset = reference_preset();
        let letter_f = chiral_letter_f_mesh();
        let size = 256u32;
        let ro = preset.outer_radius_mm;

        // Place upright F in the top (12 o'clock, +y) half of the disc, on the surface (z = 0).
        let obj = SceneObject {
            id: "f".into(),
            path: "f.obj".into(),
            name: "F".into(),
            visible: true,
            position_mm: [0.0, 41.0, 0.0],
            rotation_deg: [0.0, 0.0, 0.0],
            scale_mm: [20.0; 3], position_id: String::new(), style: Default::default(), motion: Default::default(),
            mode: Default::default(),
            parallax: 2.0,
            tilt_deg: 0.0,
        };
        let placed = place_scene_object(&letter_f, &obj).unwrap();
        let views = scene_head_motion_views(&SceneViewOptions { views: 3, ..Default::default() });
        let center_view = &views[1];
        let img = render_scene_target(&[placed], center_view, size, preset.inner_radius_mm, ro);

        // Helper: sample PNG brightness at scene-space mm (x right, y UP -> row 0 is top of PNG).
        let sample_scene_mm = |xs: f64, ys: f64| -> u8 {
            let u = xs / (2.0 * ro) + 0.5;
            let v = -ys / (2.0 * ro) + 0.5; // top of PNG is +y in scene space
            let px = (u * size as f64).floor().clamp(0.0, (size - 1) as f64) as u32;
            let py = (v * size as f64).floor().clamp(0.0, (size - 1) as f64) as u32;
            img.get_pixel(px, py)[0]
        };

        // 1. All lit corners of the upright F must be bright, and bottom-right must be dark.
        let top_left = sample_scene_mm(-7.0, 48.5);     // spine top
        let top_right = sample_scene_mm(7.0, 48.5);     // top bar right end
        let mid_right = sample_scene_mm(2.0, 41.0);     // middle bar
        let bottom_left = sample_scene_mm(-7.0, 33.5);  // spine bottom
        let bottom_right = sample_scene_mm(7.0, 33.5);  // empty bottom-right quadrant

        assert!(top_left > 100, "top-left of F should be lit, got {top_left}");
        assert!(top_right > 100, "top-right bar of F should be lit, got {top_right}");
        assert!(mid_right > 100, "middle bar of F should be lit, got {mid_right}");
        assert!(bottom_left > 100, "bottom-left spine of F should be lit, got {bottom_left}");
        assert_eq!(bottom_right, 0, "bottom-right of F must be empty (would be lit if mirrored), got {bottom_right}");

        // 2. Verify entire lit region sits in the TOP half of the PNG (row < size/2), never bottom half.
        let mut bottom_half_lit = 0usize;
        for y in (size / 2)..size {
            for x in 0..size {
                if img.get_pixel(x, y)[0] > 0 {
                    bottom_half_lit += 1;
                }
            }
        }
        assert_eq!(bottom_half_lit, 0, "object placed at +y (12 o'clock) must render in top half of PNG");

        // 3. Floating F (+12 mm) shifts LEFT in the PNG as head moves RIGHT (+x).
        let floating = SceneObject { position_mm: [0.0, -41.0, 12.0], ..obj };
        let placed_float = place_scene_object(&letter_f, &floating).unwrap();
        let img_left = render_scene_target(&[placed_float.clone()], &views[0], size, preset.inner_radius_mm, ro);
        let img_right = render_scene_target(&[placed_float], &views[2], size, preset.inner_radius_mm, ro);
        let cx_left = brightness_centroid_x(&img_left, ro);
        let cx_right = brightness_centroid_x(&img_right, ro);
        assert!(
            cx_right < cx_left - 3.0,
            "floating F must shift left as head moves right: left={cx_left:.2}, right={cx_right:.2}"
        );
    }

    /// The editor's whole path: placed scene, targets, solve, what five eyes see.
    #[test]
    #[ignore]
    fn look_editor_scene() {
        let preset = reference_preset();
        let dir = std::env::temp_dir().join("holo-editor-look");
        let star = load_mesh_or_builtin(BUILTIN_SAMPLE_PATH).unwrap();
        let mk = |pos: [f64; 3], scale_mm: f64| SceneObject {
            id: "o".into(), path: "x".into(), name: "x".into(), visible: true,
            position_mm: pos, rotation_deg: [0.0; 3], scale_mm: [scale_mm; 3], position_id: String::new(), style: Default::default(), motion: Default::default(),
            mode: Default::default(),
            parallax: 2.0,
            tilt_deg: 0.0,
        };
        let placed = [
            place_scene_object(&star, &mk([0.0, -41.0, 8.0], 24.0)).unwrap(),
            place_scene_object(&chiral_letter_f_mesh(), &mk([0.0, 41.0, -6.0], 24.0)).unwrap(),
        ];
        let opts = SceneViewOptions::default();
        std::fs::create_dir_all(&dir).unwrap();
        let mut views = scene_head_motion_views(&opts);
        for (i, v) in views.iter_mut().enumerate() {
            let path = dir.join(format!("v{i}.png"));
            render_scene_target(&placed, v, 512, preset.inner_radius_mm, preset.outer_radius_mm).save(&path).unwrap();
            v.image_path = path.to_string_lossy().into_owned();
        }
        let shared = design::shared_codebook(preset.channel_bit_length_nm * 1e-3);
        let began = std::time::Instant::now();
        let input = DesignInput { views: views.clone(), chroma_weight: 0.3, exposure: 0.6, grid: 256 };
        let solved = design::solve(&input, &shared.1, &preset).unwrap();
        println!("solve {:?}", began.elapsed());
        let n = 300u32;
        let mut out = RgbaImage::new(n * 5, n * 2);
        for (k, i) in [0usize, 2, 4, 5, 7].into_iter().enumerate() {
            let t = image::open(&views[i].image_path).unwrap().resize_exact(n, n, FilterType::Triangle).to_rgba8();
            image::imageops::replace(&mut out, &t, k as i64 * n as i64, 0);
            let px = design::render(&solved, &shared.1, &preset, &views[i], n as usize, 1.0);
            let im = RgbaImage::from_raw(n, n, px).unwrap();
            image::imageops::replace(&mut out, &im, k as i64 * n as i64, n as i64);
        }
        out.save(std::env::var("HOLO_OUT").unwrap_or_else(|_| "look.png".into())).unwrap();
    }

    #[test]
    fn dense_meshes_are_simplified_and_keep_their_shape() {
        // A sphere of 320 000 triangles.
        let (nu, nv) = (400usize, 400usize);
        let at = |u: usize, v: usize| {
            let (a, b) = (u as f64 / nu as f64 * std::f64::consts::TAU, v as f64 / nv as f64 * std::f64::consts::PI);
            let p = [a.cos() * b.sin(), a.sin() * b.sin(), b.cos()];
            Vertex { pos: p, normal: p, albedo: 1.0 }
        };
        let mut tris = Vec::new();
        for u in 0..nu {
            for v in 0..nv {
                tris.push([at(u, v), at(u, v + 1), at(u + 1, v + 1)]);
                tris.push([at(u, v), at(u + 1, v + 1), at(u + 1, v)]);
            }
        }
        let out = simplify_mesh(Mesh { tris }, 50_000);
        assert!(out.tris.len() <= 50_000 && out.tris.len() > 15_000, "{}", out.tris.len());
        for v in out.tris.iter().flatten() {
            assert!((norm3(v.pos) - 1.0).abs() < 0.02);
            assert!(dot3(v.normal, unit3(v.pos).unwrap()) > 0.95);
        }
    }

    /// The editor's own path (views, solve, background) with MODEL and the sample, as the phone
    /// sees it: lamp to the right, then its own flash with the disc turned.
    #[test]
    #[ignore]
    fn look_editor_path() {
        let preset = reference_preset();
        let dir = std::env::temp_dir().join("holo-editor-path");
        let obj = |path: &str, pos: [f64; 3], rot: [f64; 3], scale_mm: f64| SceneObject {
            id: "o".into(), path: path.into(), name: "x".into(), visible: true, position_mm: pos, rotation_deg: rot, scale_mm: [scale_mm; 3], position_id: String::new(), style: Default::default(), motion: Default::default(),
            mode: Default::default(),
            parallax: 2.0,
            tilt_deg: 0.0,
        };
        let scene = Scene { version: SCENE_VERSION, lamp_mm: None, positions: vec![], images: vec![], objects: vec![
            obj(&std::env::var("MODEL").unwrap(), [0.0, 40.0, 0.0], [0.0; 3], 34.0),
            obj(BUILTIN_SAMPLE_PATH, [0.0, -41.0, 4.0], [0.0; 3], 26.0),
            obj(BUILTIN_SAMPLE_PATH, [41.0, 0.0, 0.0], [0.0; 3], 22.0),
        ] };
        let n = 360u32;
        let eye = |h: f64| [h, 350.0 * 20f64.to_radians().sin(), 350.0 * 20f64.to_radians().cos()];
        let flash = |r: f64, p: f64| { let (a, b) = (r.to_radians(), p.to_radians()); [350.0 * a.sin() * b.cos(), 350.0 * b.sin(), 350.0 * a.cos() * b.cos()] };
        let mut poses: Vec<([f64; 3], [f64; 3])> = [-40.0, 0.0, 40.0].map(|h| (eye(h), [150.0, 0.0, 400.0])).to_vec();
        for (r, p) in [(8.0, 0.0), (12.0, 5.0)] { let e = flash(r, p); poses.push((e, [e[0] + 12.0, e[1] + 10.0, e[2]])); }
        let mut out = RgbaImage::new(n * 5, n * 2);
        for (row, new) in [(0u32, false), (1, true)] {
            let o = if new { SceneViewOptions { light_mm: [150.0, 0.0, 400.0], head_half_span_mm: 45.0, views: 24, ..Default::default() } } else { SceneViewOptions::default() };
            let t0 = std::time::Instant::now();
            let v = write_scene_views(&scene, &dir, &preset, &o).unwrap().views;
            let t1 = t0.elapsed();
            let input = DesignInput { views: v, chroma_weight: 0.3, exposure: 0.6, grid: 256 };
            let shared = design::shared_codebook(preset.channel_bit_length_nm * 1e-3);
            let mut s = design::solve(&input, &shared.1, &preset).unwrap();
            if new { design::uniform_background(&mut s, &input, &shared.0).unwrap(); }
            println!("views {t1:?} solve {:?}", t0.elapsed() - t1);
            for (ci, (e, l)) in poses.iter().enumerate() {
                let px = crate::holo::lightfield::render_float(&s, &shared.1, &preset, *e, *l, n as usize, true);
                for (i, p) in px.iter().enumerate() {
                    let c = p.map(|v| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8);
                    out.put_pixel(ci as u32 * n + i as u32 % n, row * n + i as u32 / n, Rgba([c[0], c[1], c[2], 255]));
                }
            }
        }
        out.save(std::env::var("HOLO_OUT").unwrap()).unwrap();
    }

    /// Reads every model named in MODELS (colon separated), as the editor would.
    #[test]
    #[ignore]
    fn look_models() {
        for path in std::env::var("MODELS").unwrap().split(':') {
            let began = std::time::Instant::now();
            let m = load_mesh_or_builtin(path).unwrap();
            let bytes = mesh_to_display_bytes(&m).unwrap().len();
            println!("{} tris, {} bytes, {:?}", m.tris.len(), bytes, began.elapsed());
            let placed = place_scene_object(&m, &SceneObject {
                id: "o".into(), path: "x".into(), name: "x".into(), visible: true,
                position_mm: [0.0, -41.0, 0.0], rotation_deg: [0.0; 3], scale_mm: [30.0; 3], position_id: String::new(), style: Default::default(), motion: Default::default(),
                mode: Default::default(),
                parallax: 2.0,
                tilt_deg: 0.0,
            }).unwrap();
            let v = &scene_head_motion_views(&SceneViewOptions { views: 3, ..Default::default() })[1];
            let p = reference_preset();
            let img = render_scene_target(&[placed], v, 1024, p.inner_radius_mm, p.outer_radius_mm);
            let name = format!("{}-{}.png", std::env::var("HOLO_OUT").unwrap(), m.tris.len());
            image::imageops::crop_imm(&img, 340, 660, 344, 344).to_image().save(name).unwrap();
        }
    }

    #[test]
    fn multi_object_scene_shares_z_buffer_and_skips_hidden() {
        let preset = reference_preset();
        let ro = preset.outer_radius_mm;
        let size = 128u32;

        // Flat square facing +z (bright under key light) vs tilted square (dimmer under key light).
        let bright_plate = load_obj_str(
            "v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\nvn 0 0 1\nf 1//1 2//1 3//1 4//1\n",
        )
        .unwrap();
        let dark_plate = load_obj_str(
            "v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\nvn -0.9 0 0.4359\nf 1//1 2//1 3//1 4//1\n",
        )
        .unwrap();

        let front = place_scene_object(
            &bright_plate,
            &SceneObject {
                id: "front".into(),
                path: "front.obj".into(),
                name: "Front".into(),
                visible: true,
                position_mm: [0.0, -41.0, 10.0],
                rotation_deg: [0.0, 0.0, 0.0],
                scale_mm: [16.0; 3], position_id: String::new(), style: Default::default(), motion: Default::default(),
                mode: Default::default(),
                parallax: 2.0,
                tilt_deg: 0.0,
            },
        )
        .unwrap();
        let back = place_scene_object(
            &dark_plate,
            &SceneObject {
                id: "back".into(),
                path: "back.obj".into(),
                name: "Back".into(),
                visible: true,
                position_mm: [0.0, -41.0, -10.0],
                rotation_deg: [0.0, 0.0, 0.0],
                scale_mm: [24.0; 3], position_id: String::new(), style: Default::default(), motion: Default::default(),
                mode: Default::default(),
                parallax: 2.0,
                tilt_deg: 0.0,
            },
        )
        .unwrap();

        let view = &scene_head_motion_views(&SceneViewOptions { views: 1, ..Default::default() })[0];
        let img_fb = render_scene_target(&[front.clone(), back.clone()], view, size, preset.inner_radius_mm, ro);
        let img_bf = render_scene_target(&[back, front], view, size, preset.inner_radius_mm, ro);
        assert_eq!(img_fb.as_raw(), img_bf.as_raw(), "shared z-buffer must be order-independent");

        // Verify write_scene_views skips invisible objects and accepts builtin sample.
        let dir = std::env::temp_dir().join(format!("holo-scene-test-{}", std::process::id()));
        let scene = Scene {
            images: vec![],
            version: SCENE_VERSION, lamp_mm: None, positions: vec![],
            objects: vec![
                SceneObject {
                    id: "star".into(),
                    path: BUILTIN_SAMPLE_PATH.into(),
                    name: "Star".into(),
                    visible: true,
                    position_mm: [0.0, -41.0, 8.0],
                    rotation_deg: [0.0, 0.0, 0.0],
                    scale_mm: [22.0; 3], position_id: String::new(), style: Default::default(), motion: Default::default(),
                    mode: Default::default(),
                    parallax: 2.0,
                    tilt_deg: 0.0,
                },
                SceneObject {
                    id: "hidden".into(),
                    path: "/nonexistent/does-not-exist.obj".into(),
                    name: "Hidden".into(),
                    visible: false,
                    position_mm: [0.0, 41.0, 0.0],
                    rotation_deg: [0.0, 0.0, 0.0],
                    scale_mm: [20.0; 3], position_id: String::new(), style: Default::default(), motion: Default::default(),
                    mode: Default::default(),
                    parallax: 2.0,
                    tilt_deg: 0.0,
                },
            ],
        };
        let out = write_scene_views(
            &scene,
            &dir,
            &preset,
            &SceneViewOptions { views: 2, target_size: 128, ..Default::default() },
        )
        .unwrap();
        assert_eq!(out.views.len(), 2);
        assert_eq!(out.triangles, 20);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn display_mesh_binary_packing_and_builtin_sample() {
        let star = load_mesh_or_builtin(BUILTIN_SAMPLE_PATH).unwrap();
        let bytes = mesh_to_display_bytes(&star).unwrap();
        let v_count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let i_count = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        assert_eq!(i_count, star.tris.len() * 3);
        assert!(v_count > 0 && v_count <= i_count);
        assert_eq!(bytes.len(), 8 + v_count * 12 + v_count * 12 + i_count * 4);
        // All packed positions must lie within [-0.501, 0.501].
        for i in 0..(v_count * 3) {
            let off = 8 + i * 4;
            let f = f32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
            assert!(f.is_finite() && f.abs() <= 0.501, "position out of unit cube: {f}");
        }
    }

    #[test]
    fn motion_sample_interpolates_and_handles_edge_cases() {
        assert_eq!(Motion::default().sample(0.5), IDENTITY_MAT4);
        let tx10 = [
            1.0, 0.0, 0.0, 10.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ];
        let single = Motion { samples: vec![tx10], ..Default::default() };
        assert_eq!(single.sample(0.0), tx10);
        assert_eq!(single.sample(0.5), tx10);

        let motion = Motion { samples: vec![IDENTITY_MAT4, tx10], ..Default::default() };
        assert_eq!(motion.sample(0.0)[3], 0.0);
        assert!((motion.sample(0.5)[3] - 5.0).abs() < 1e-12);
        assert_eq!(motion.sample(1.0)[3], 10.0);
        // Clamped outside [0, 1].
        assert_eq!(motion.sample(-0.5)[3], 0.0);
        assert_eq!(motion.sample(1.5)[3], 10.0);
    }

    #[test]
    fn place_scene_object_with_applies_scene_transform_and_normal_inverse_transpose() {
        // Unit-size triangle with normal (1, 1, 0)/sqrt(2) before non-uniform scale diag(2, 1, 1).
        let n = unit3([1.0, 1.0, 0.0]).unwrap();
        let mesh = Mesh {
            tris: vec![[
                Vertex { pos: [-0.5, 0.0, 0.0], normal: n, albedo: 1.0 },
                Vertex { pos: [0.5, 0.0, 0.0], normal: n, albedo: 1.0 },
                Vertex { pos: [0.0, 1.0, 0.0], normal: n, albedo: 1.0 },
            ]],
        };
        let obj = SceneObject {
            id: "t".into(),
            path: "t.obj".into(),
            name: "T".into(),
            visible: true,
            position_mm: [2.0, 30.0, 4.0],
            rotation_deg: [0.0; 3],
            scale_mm: [10.0; 3],
            position_id: String::new(),
            style: Default::default(),
            motion: Default::default(),
            mode: Default::default(),
            parallax: 2.0,
            tilt_deg: 0.0,
        };
        let mat = [
            2.0, 0.0, 0.0, 5.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ];
        let placed = place_scene_object_with(&mesh, &obj, Some(&mat)).unwrap();
        // Vertex 1 normalized pos is (+0.5, -0.5, 0) -> scaled (5, -5, 0) -> L*(5,-5,0) = (10,-5,0)
        // -> + pos(2,30,4) + tr(5,0,0) = (17, 25, 4) -> disc frame (17, -25, 4).
        let v1 = placed.tris[0][1];
        assert!((v1.pos[0] - 17.0).abs() < 1e-9, "pos={:?}", v1.pos);
        assert!((v1.pos[1] - (-25.0)).abs() < 1e-9, "pos={:?}", v1.pos);
        assert!((v1.pos[2] - 4.0).abs() < 1e-9, "pos={:?}", v1.pos);
        // Inverse-transpose of diag(2, 1, 1) applied to (1, 1, 0) gives proportional to (0.5, 1, 0),
        // then scene_to_disc negates y -> (0.5, -1, 0) normalized.
        let expected_n = unit3([0.5, -1.0, 0.0]).unwrap();
        for c in 0..3 {
            assert!((v1.normal[c] - expected_n[c]).abs() < 1e-9, "normal={:?}, expected={:?}", v1.normal, expected_n);
        }
    }
}
